//! Real LLM provider that talks to any OpenAI-compatible chat endpoint.
//!
//! The same `TyphoonProvider` serves two backends, chosen by `LLM_BACKEND`:
//! - `typhoon-local`: `mlx_lm.server` running a Typhoon MLX model on Apple
//!   Silicon (MLX cannot be driven from Rust directly, so it lives behind HTTP).
//! - `typhoon-cloud`: the hosted Typhoon API at `https://api.opentyphoon.ai/v1`,
//!   authenticated with an API key.
//!
//! Both speak the OpenAI `/v1/chat/completions` contract, so the only
//! differences are the base URL, the model id, and whether a key is required.

use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use govdoc_usecases::{EmbeddingProvider, LlmProvider};
use serde_json::{json, Value};

/// Connection settings for a Typhoon (or any OpenAI-compatible) chat endpoint.
#[derive(Clone, Debug)]
pub struct TyphoonConfig {
    /// Base URL including the version prefix, e.g. `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    /// Model id sent in each request. For `mlx_lm.server` this is usually the
    /// model path or repo it was launched with; for the cloud it is a hosted
    /// model name such as `typhoon-v2.5-30b-a3b-instruct`.
    pub model: String,
    /// Optional bearer token. A local MLX server ignores it; the Typhoon cloud
    /// API requires it.
    pub api_key: Option<String>,
    /// Sampling temperature. Keep it low for deterministic-leaning documents.
    pub temperature: f32,
    /// Optional nucleus sampling cutoff. Sent only when set.
    pub top_p: Option<f32>,
}

impl TyphoonConfig {
    /// Defaults for the local `mlx_lm.server` MLX model. The model id must match
    /// what the server reports at `/v1/models` (the converted model keeps the
    /// base repo name), otherwise mlx_lm.server tries to fetch it from HF.
    pub fn local() -> Self {
        Self::from_env_with_defaults("http://127.0.0.1:8080/v1", "typhoon-ai/typhoon2.5-qwen3-4b")
    }

    /// Defaults for the hosted Typhoon cloud API. Requires `LLM_API_KEY`.
    pub fn cloud() -> Self {
        Self::from_env_with_defaults(
            "https://api.opentyphoon.ai/v1",
            "typhoon-v2.5-30b-a3b-instruct",
        )
    }

    /// Build config from the `LLM_*` environment variables, falling back to the
    /// supplied backend defaults for anything not set.
    fn from_env_with_defaults(default_url: &str, default_model: &str) -> Self {
        Self {
            base_url: std::env::var("LLM_BASE_URL").unwrap_or_else(|_| default_url.to_string()),
            model: std::env::var("LLM_MODEL").unwrap_or_else(|_| default_model.to_string()),
            api_key: std::env::var("LLM_API_KEY").ok().filter(|k| !k.is_empty()),
            temperature: std::env::var("LLM_TEMPERATURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.2),
            top_p: std::env::var("LLM_TOP_P").ok().and_then(|v| v.parse().ok()),
        }
    }
}

/// LLM provider backed by an OpenAI-compatible chat completions endpoint.
pub struct TyphoonProvider {
    client: reqwest::Client,
    config: TyphoonConfig,
}

impl TyphoonProvider {
    pub fn new(config: TyphoonConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("failed to build HTTP client for Typhoon provider")?;
        Ok(Self { client, config })
    }

    async fn chat(&self, system: &str, user: &str, max_tokens: usize) -> anyhow::Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "max_tokens": max_tokens,
            "temperature": self.config.temperature,
            "stream": false,
        });
        if let Some(top_p) = self.config.top_p {
            body["top_p"] = json!(top_p);
        }

        let mut request = self.client.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;

        let status = response.status();
        let payload = response
            .text()
            .await
            .context("failed to read LLM response body")?;
        if !status.is_success() {
            return Err(anyhow!("LLM server returned {status}: {payload}"));
        }

        let parsed: Value =
            serde_json::from_str(&payload).context("LLM response was not valid JSON")?;
        parsed["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("LLM response missing choices[0].message.content: {payload}"))
    }
}

#[async_trait]
impl LlmProvider for TyphoonProvider {
    async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: usize,
    ) -> anyhow::Result<String> {
        self.chat(system, user, max_tokens).await
    }

    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _schema: Value,
        max_tokens: usize,
    ) -> anyhow::Result<Value> {
        // mlx_lm.server does not reliably enforce a JSON schema, so we instruct
        // the model to emit JSON only and then defensively extract it.
        let system = format!(
            "{system}\n\nสำคัญ: ตอบกลับเป็น JSON ที่ถูกต้องเท่านั้น ห้ามมีคำอธิบาย \
             ห้ามมี markdown code fence ใด ๆ"
        );
        let raw = self.chat(&system, user, max_tokens).await?;
        extract_json(&raw).with_context(|| format!("could not parse JSON from LLM output: {raw}"))
    }
}

/// Connection settings for an OpenAI-compatible embeddings endpoint.
#[derive(Clone, Debug)]
pub struct EmbeddingConfig {
    /// Base URL including the version prefix, e.g. `https://api.opentyphoon.ai/v1`.
    pub base_url: String,
    /// Embedding model id.
    pub model: String,
    /// Optional bearer token (required by hosted APIs).
    pub api_key: Option<String>,
    /// Reported vector width. Informational only; the real width comes from the
    /// server response.
    pub dimensions: usize,
}

impl EmbeddingConfig {
    /// Build config from `EMBEDDING_*` env vars, reusing `LLM_BASE_URL` /
    /// `LLM_API_KEY` as fallbacks so a single cloud key covers both providers.
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("EMBEDDING_BASE_URL")
                .or_else(|_| std::env::var("LLM_BASE_URL"))
                .unwrap_or_else(|_| "https://api.opentyphoon.ai/v1".to_string()),
            model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string()),
            api_key: std::env::var("LLM_API_KEY").ok().filter(|k| !k.is_empty()),
            dimensions: std::env::var("EMBEDDING_DIM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024),
        }
    }
}

/// Embedding provider backed by an OpenAI-compatible `/v1/embeddings` endpoint.
pub struct TyphoonEmbeddingProvider {
    client: reqwest::Client,
    config: EmbeddingConfig,
}

impl TyphoonEmbeddingProvider {
    pub fn new(config: EmbeddingConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build HTTP client for embedding provider")?;
        Ok(Self { client, config })
    }
}

#[async_trait]
impl EmbeddingProvider for TyphoonEmbeddingProvider {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/embeddings", self.config.base_url.trim_end_matches('/'));
        let body = json!({ "model": self.config.model, "input": text });

        let mut request = self.client.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let status = response.status();
        let payload = response
            .text()
            .await
            .context("failed to read embedding response body")?;
        if !status.is_success() {
            return Err(anyhow!("embedding server returned {status}: {payload}"));
        }

        let parsed: Value =
            serde_json::from_str(&payload).context("embedding response was not valid JSON")?;
        let array = parsed["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| anyhow!("embedding response missing data[0].embedding: {payload}"))?;
        array
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| anyhow!("embedding vector contained a non-number"))
            })
            .collect()
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }
}

/// Connection settings for the Typhoon OCR endpoint.
#[derive(Clone, Debug)]
pub struct OcrConfig {
    /// Base URL including the version prefix.
    pub base_url: String,
    /// OCR model id (e.g. `typhoon-ocr`).
    pub model: String,
    /// Bearer token. Required: OCR is cloud-only.
    pub api_key: Option<String>,
}

impl OcrConfig {
    /// Build config from `OCR_*` env vars, reusing `LLM_API_KEY` as the key.
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("OCR_BASE_URL")
                .unwrap_or_else(|_| "https://api.opentyphoon.ai/v1".to_string()),
            model: std::env::var("OCR_MODEL").unwrap_or_else(|_| "typhoon-ocr".to_string()),
            api_key: std::env::var("LLM_API_KEY").ok().filter(|k| !k.is_empty()),
        }
    }
}

/// Document OCR backed by the Typhoon `/v1/ocr` endpoint.
pub struct TyphoonOcrProvider {
    client: reqwest::Client,
    config: OcrConfig,
}

#[derive(Clone, Debug)]
pub struct OcrPageOutput {
    pub text: String,
    pub raw_json: Value,
}

impl TyphoonOcrProvider {
    pub fn new(config: OcrConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("failed to build HTTP client for OCR provider")?;
        Ok(Self { client, config })
    }

    /// Extract text/markdown from an image or PDF. Returns the natural text of
    /// every successfully processed page, joined by blank lines.
    pub async fn extract_text(&self, file: &[u8], filename: &str) -> anyhow::Result<String> {
        self.extract_text_page(file, filename, None).await
    }

    pub async fn extract_text_page(
        &self,
        file: &[u8],
        filename: &str,
        page_num: Option<usize>,
    ) -> anyhow::Result<String> {
        self.extract_page(file, filename, page_num)
            .await
            .map(|output| output.text)
    }

    pub async fn extract_page(
        &self,
        file: &[u8],
        filename: &str,
        page_num: Option<usize>,
    ) -> anyhow::Result<OcrPageOutput> {
        let key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow!("OCR requires LLM_API_KEY"))?;
        let url = format!("{}/ocr", self.config.base_url.trim_end_matches('/'));

        let part = reqwest::multipart::Part::bytes(file.to_vec()).file_name(filename.to_string());
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.config.model.clone())
            .text("task_type", "default")
            .text("max_tokens", "16384")
            .text("temperature", "0.1")
            .text("top_p", "0.6")
            .text("repetition_penalty", "1.2");
        let form = if let Some(page_num) = page_num {
            form.text("page_num", page_num.to_string())
        } else {
            form
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(key)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let status = response.status();
        let payload = response
            .text()
            .await
            .context("failed to read OCR response body")?;
        if !status.is_success() {
            return Err(anyhow!("OCR server returned {status}: {payload}"));
        }

        let parsed: Value =
            serde_json::from_str(&payload).context("OCR response was not valid JSON")?;
        let text = parse_ocr_response(&parsed)?;
        Ok(OcrPageOutput {
            text,
            raw_json: parsed,
        })
    }
}

/// Pull the natural text out of a Typhoon OCR response, concatenating pages.
///
/// Each page's `message.choices[0].message.content` is either plain text or a
/// JSON string carrying a `natural_text` field; both are handled.
fn parse_ocr_response(payload: &Value) -> anyhow::Result<String> {
    let results = payload["results"]
        .as_array()
        .ok_or_else(|| anyhow!("OCR response missing results array: {payload}"))?;

    let mut texts = Vec::new();
    for page in results {
        if page["success"].as_bool() != Some(true) {
            let detail = page["error"].as_str().unwrap_or("unknown error");
            let name = page["filename"].as_str().unwrap_or("page");
            return Err(anyhow!("OCR failed for {name}: {detail}"));
        }
        let Some(content) = page["message"]["choices"][0]["message"]["content"].as_str() else {
            continue;
        };
        texts.push(natural_text(content));
    }

    if texts.is_empty() {
        return Err(anyhow!("OCR returned no text"));
    }
    Ok(texts.join("\n\n"))
}

/// Structured OCR output is a JSON string with a `natural_text` field; plain
/// output is used as-is.
fn natural_text(content: &str) -> String {
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value["natural_text"].as_str().map(str::to_owned))
        .unwrap_or_else(|| content.to_string())
}

/// Pull a JSON value out of a model response that may be wrapped in prose or a
/// ```json fenced block.
fn extract_json(text: &str) -> anyhow::Result<Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    // Strip a leading ```json / ``` fence if present.
    let without_fence = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(value) = serde_json::from_str::<Value>(without_fence) {
        return Ok(value);
    }

    // Last resort: grab the outermost { ... } span.
    let start = without_fence.find('{');
    let end = without_fence.rfind('}');
    if let (Some(start), Some(end)) = (start, end) {
        if end > start {
            return serde_json::from_str::<Value>(&without_fence[start..=end])
                .context("substring between first '{' and last '}' was not valid JSON");
        }
    }

    Err(anyhow!("no JSON object found in LLM output"))
}

/// Poll the local server's `/models` endpoint until it answers or the timeout
/// elapses. Used when auto-spawning the sidecar so the first request does not
/// race model loading.
pub async fn wait_until_ready(base_url: &str, timeout: Duration) -> anyhow::Result<()> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "local LLM server at {url} did not become ready within {timeout:?}"
            ));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_handles_plain_object() {
        let value = extract_json(r#"{"passed": true}"#).unwrap();
        assert_eq!(value["passed"], true);
    }

    #[test]
    fn extract_json_strips_code_fence() {
        let value = extract_json("```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn extract_json_recovers_from_surrounding_prose() {
        let value = extract_json("นี่คือผลลัพธ์:\n{\"passed\": false, \"issues\": []}\nจบ").unwrap();
        assert_eq!(value["passed"], false);
    }

    #[test]
    fn extract_json_errors_without_object() {
        assert!(extract_json("ไม่มี JSON เลย").is_err());
    }

    #[test]
    fn ocr_parses_natural_text_and_plain_pages() {
        let payload = serde_json::json!({
            "results": [
                {
                    "success": true,
                    "message": {
                        "choices": [
                            { "message": { "content": "{\"natural_text\": \"หน้า 1\"}" } }
                        ]
                    }
                },
                {
                    "success": true,
                    "message": { "choices": [ { "message": { "content": "หน้า 2" } } ] }
                }
            ]
        });
        assert_eq!(parse_ocr_response(&payload).unwrap(), "หน้า 1\n\nหน้า 2");
    }

    #[test]
    fn ocr_surfaces_page_errors() {
        let payload = serde_json::json!({
            "results": [
                { "success": false, "filename": "scan.pdf", "error": "bad page" }
            ]
        });
        let err = parse_ocr_response(&payload).unwrap_err().to_string();
        assert!(err.contains("scan.pdf"));
        assert!(err.contains("bad page"));
    }
}
