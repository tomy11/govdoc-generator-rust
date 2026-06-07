//! `MemoryRepository` backed by the shared SQLite store.
//!
//! Similarity search builds an in-memory cosine index from the embeddings
//! stored alongside each example, looks up the nearest ids, then returns their
//! `fields_json`. When no embeddings exist for the requested doc type it falls
//! back to the most recent examples so generation still gets context.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use async_trait::async_trait;
use govdoc_domain::DocRequest;
use govdoc_storage::{HnswIndex, InMemoryVectorIndex, SqliteStore};
use govdoc_usecases::MemoryRepository;
use serde_json::Value;

pub struct SqliteMemoryRepository {
    store: Arc<Mutex<SqliteStore>>,
}

impl SqliteMemoryRepository {
    pub fn new(store: Arc<Mutex<SqliteStore>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl MemoryRepository for SqliteMemoryRepository {
    async fn retrieve(&self, req: &DocRequest, limit: usize) -> anyhow::Result<Vec<Value>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        store.recent_memory_fields(Some(req.doc_type.as_thai()), limit)
    }

    async fn retrieve_by_similarity(
        &self,
        req: &DocRequest,
        embedding: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<Value>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let doc_type = req.doc_type.as_thai();

        let pairs = store.memory_embeddings(Some(doc_type))?;
        if pairs.is_empty() {
            return store.recent_memory_fields(Some(doc_type), limit);
        }

        let mut index = InMemoryVectorIndex::default();
        for (id, vector) in &pairs {
            index.add(*id, vector)?;
        }
        let ids: Vec<i64> = index
            .search(embedding, limit)?
            .into_iter()
            .map(|hit| hit.id)
            .collect();
        store.memory_fields_by_ids(&ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use govdoc_domain::DocType;
    use govdoc_storage::NewMemoryRecord;

    fn request(doc_type: DocType) -> DocRequest {
        DocRequest {
            doc_type,
            subject: String::new(),
            purpose: String::new(),
            recipient_name: String::new(),
            recipient_class: Default::default(),
            recipient_agency: String::new(),
            sender_name: String::new(),
            sender_position: String::new(),
            additional_context: String::new(),
            use_critic: None,
        }
    }

    #[tokio::test]
    async fn similarity_returns_nearest_example_for_doc_type() {
        let store = SqliteStore::open_memory().unwrap();
        store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "near",
                fields: &serde_json::json!({ "subject": "ใกล้" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: Some(&[1.0, 0.0]),
            })
            .unwrap();
        store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "far",
                fields: &serde_json::json!({ "subject": "ไกล" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: Some(&[0.0, 1.0]),
            })
            .unwrap();

        let repo = SqliteMemoryRepository::new(Arc::new(Mutex::new(store)));
        let hits = repo
            .retrieve_by_similarity(&request(DocType::External), &[0.9, 0.1], 1)
            .await
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["subject"], "ใกล้");
    }

    #[tokio::test]
    async fn similarity_falls_back_to_recent_when_no_embeddings() {
        let store = SqliteStore::open_memory().unwrap();
        store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "plain",
                fields: &serde_json::json!({ "subject": "ไม่มีเวกเตอร์" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: None,
            })
            .unwrap();

        let repo = SqliteMemoryRepository::new(Arc::new(Mutex::new(store)));
        let hits = repo
            .retrieve_by_similarity(&request(DocType::External), &[0.1, 0.2], 3)
            .await
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["subject"], "ไม่มีเวกเตอร์");
    }
}
