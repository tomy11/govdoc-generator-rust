use async_trait::async_trait;
use govdoc_domain::DocRequest;
use serde_json::Value;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user: &str, max_tokens: usize) -> anyhow::Result<String>;

    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema: Value,
        max_tokens: usize,
    ) -> anyhow::Result<Value>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;

    fn dimensions(&self) -> usize;
}

#[async_trait]
pub trait MemoryRepository: Send + Sync {
    async fn retrieve(&self, req: &DocRequest, limit: usize) -> anyhow::Result<Vec<Value>>;

    async fn retrieve_by_similarity(
        &self,
        req: &DocRequest,
        embedding: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<Value>>;
}

