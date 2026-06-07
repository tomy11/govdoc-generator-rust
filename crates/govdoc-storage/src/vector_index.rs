//! Persistent vector index for semantic retrieval.
//!
//! SQLite stays the source of truth for embeddings; this index is a derived,
//! on-disk cache so similarity search does not reload and rebuild from the
//! database on every query. It is loaded at startup (or rebuilt from SQLite),
//! updated incrementally on ingest, and saved to `HNSW_INDEX_PATH`.
//!
//! The search itself is a brute-force cosine scan — fine for the desktop-scale
//! corpora here. The name mirrors the aspirational `HnswIndex` interface; a true
//! HNSW graph can replace the internals later without changing callers.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::hnsw::cosine_similarity;

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: i64,
    doc_type: String,
    vector: Vec<f32>,
}

#[derive(Default)]
pub struct PersistentVectorIndex {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
}

impl PersistentVectorIndex {
    /// Load the index from `path`. A missing, unreadable, or corrupt file yields
    /// an empty index that still points at `path`, so the next write self-heals
    /// it. A `None` path yields an in-memory-only index (nothing is persisted).
    pub fn load(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_ref()
            .filter(|file| file.exists())
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|data| serde_json::from_slice::<Vec<Entry>>(&data).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Replace all entries (used to rebuild from SQLite) and persist.
    pub fn rebuild(&mut self, rows: Vec<(i64, String, Vec<f32>)>) -> Result<()> {
        self.entries = rows
            .into_iter()
            .map(|(id, doc_type, vector)| Entry {
                id,
                doc_type,
                vector,
            })
            .collect();
        self.save()
    }

    /// Add one vector and persist.
    pub fn add(&mut self, id: i64, doc_type: &str, vector: Vec<f32>) -> Result<()> {
        self.entries.push(Entry {
            id,
            doc_type: doc_type.to_string(),
            vector,
        });
        self.save()
    }

    /// Ids of the nearest vectors within a doc type, most similar first.
    pub fn search(&self, doc_type: &str, query: &[f32], limit: usize) -> Vec<i64> {
        let mut hits: Vec<(i64, f32)> = self
            .entries
            .iter()
            .filter(|entry| entry.doc_type == doc_type)
            .map(|entry| (entry.id, cosine_similarity(query, &entry.vector)))
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(limit);
        hits.into_iter().map(|(id, _)| id).collect()
    }

    fn save(&self) -> Result<()> {
        let Some(file) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(file, serde_json::to_vec(&self.entries)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_within_doc_type_only() {
        let mut index = PersistentVectorIndex::default();
        index.add(1, "ภายนอก", vec![1.0, 0.0]).unwrap();
        index.add(2, "ภายนอก", vec![0.0, 1.0]).unwrap();
        index.add(3, "คำสั่ง", vec![1.0, 0.0]).unwrap();

        let hits = index.search("ภายนอก", &[0.9, 0.1], 5);
        assert_eq!(hits, vec![1, 2]); // doc 3 excluded, nearest first
    }

    #[test]
    fn persists_and_reloads_from_disk() {
        let path = std::env::temp_dir().join("govdoc-index-test-7b1c2.json");
        let _ = std::fs::remove_file(&path);

        {
            let mut index = PersistentVectorIndex::load(Some(path.clone()));
            index.add(10, "ภายนอก", vec![0.2, 0.8]).unwrap();
        }

        let index = PersistentVectorIndex::load(Some(path.clone()));
        assert_eq!(index.len(), 1);
        assert_eq!(index.search("ภายนอก", &[0.2, 0.8], 1), vec![10]);

        std::fs::remove_file(&path).ok();
    }
}
