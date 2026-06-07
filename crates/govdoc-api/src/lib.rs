mod memory;
mod mock;
mod providers;

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::Context;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use govdoc_domain::{
    AnnouncementDoc, DocRequest, DocType, EditRequest, ExternalDoc, InternalDoc, OrderDoc,
    RenderRequest,
};
use govdoc_storage::{NewMemoryRecord, NewTemplateRecord, SqliteStore, TemplateRecord};
use govdoc_usecases::{
    edit_document_json, generate_document_json, EmbeddingProvider, GenerationOptions,
    GenerationServices, LlmProvider, TraceEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memory::SqliteMemoryRepository;
use crate::mock::{FakeEmbeddingProvider, FakeLlmProvider};
use crate::providers::{
    EmbeddingConfig, OcrConfig, TyphoonConfig, TyphoonEmbeddingProvider, TyphoonOcrProvider,
    TyphoonProvider,
};

/// Which LLM implementation `/generate` and `/edit` use, resolved once at
/// startup from the `LLM_BACKEND` environment variable.
#[derive(Clone, Debug)]
enum LlmBackend {
    /// Deterministic in-process stub. Default so tests and offline runs work.
    Fake,
    /// Local OpenAI-compatible server (e.g. `mlx_lm.server` with Typhoon MLX).
    TyphoonLocal(TyphoonConfig),
    /// Hosted Typhoon cloud API, authenticated with `LLM_API_KEY`.
    TyphoonCloud(TyphoonConfig),
}

impl LlmBackend {
    fn from_env() -> Self {
        match std::env::var("LLM_BACKEND").as_deref() {
            Ok("typhoon-cloud") | Ok("cloud") | Ok("key") => {
                LlmBackend::TyphoonCloud(TyphoonConfig::cloud())
            }
            Ok("typhoon-local") | Ok("typhoon") | Ok("local") => {
                LlmBackend::TyphoonLocal(TyphoonConfig::local())
            }
            _ => LlmBackend::Fake,
        }
    }
}

/// Which embedding implementation the retrieval path uses, resolved from
/// `EMBEDDING_BACKEND`.
#[derive(Clone, Debug)]
enum EmbeddingBackend {
    /// Zero-vector stub. Retrieval then falls back to recency-based lookup.
    Fake,
    /// OpenAI-compatible `/v1/embeddings` endpoint (Typhoon cloud, OpenAI, ...).
    Remote(EmbeddingConfig),
}

impl EmbeddingBackend {
    fn from_env() -> Self {
        match std::env::var("EMBEDDING_BACKEND").as_deref() {
            Ok("typhoon") | Ok("openai") | Ok("remote") | Ok("cloud") => {
                EmbeddingBackend::Remote(EmbeddingConfig::from_env())
            }
            _ => EmbeddingBackend::Fake,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub app_name: String,
    template_store: Arc<Mutex<SqliteStore>>,
    renderer_cmd: Option<String>,
    python_source: Option<String>,
    llm_backend: LlmBackend,
    embedding_backend: EmbeddingBackend,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            app_name: "govdoc-generator-rust".to_string(),
            template_store: Arc::new(Mutex::new(open_store())),
            renderer_cmd: std::env::var("GOVDOC_RENDERER_CMD").ok(),
            python_source: std::env::var("GOVDOC_PYTHON_SOURCE").ok(),
            llm_backend: LlmBackend::from_env(),
            embedding_backend: EmbeddingBackend::from_env(),
        }
    }
}

/// Open the persistent store at `SQLITE_PATH`, creating parent dirs as needed.
/// Falls back to an in-memory store when `SQLITE_PATH` is unset (e.g. tests),
/// in which case ingested data does not survive a restart.
fn open_store() -> SqliteStore {
    match std::env::var("SQLITE_PATH") {
        Ok(path) if !path.is_empty() => {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .unwrap_or_else(|e| panic!("failed to create dir for {path}: {e}"));
                }
            }
            SqliteStore::open(&path).unwrap_or_else(|e| panic!("failed to open SQLite {path}: {e}"))
        }
        _ => SqliteStore::open_memory().expect("in-memory SQLite store should open"),
    }
}

impl AppState {
    /// Construct the configured LLM provider for a single request.
    fn build_llm(&self) -> Result<Box<dyn LlmProvider>, ApiError> {
        match &self.llm_backend {
            LlmBackend::Fake => Ok(Box::new(FakeLlmProvider)),
            LlmBackend::TyphoonLocal(config) => build_typhoon(config),
            LlmBackend::TyphoonCloud(config) => {
                if config.api_key.is_none() {
                    return Err(ApiError {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        detail: "LLM_BACKEND=typhoon-cloud requires LLM_API_KEY".to_string(),
                    });
                }
                build_typhoon(config)
            }
        }
    }

    /// Construct the configured embedding provider for a single request.
    fn build_embedding(&self) -> Result<Box<dyn EmbeddingProvider>, ApiError> {
        match &self.embedding_backend {
            EmbeddingBackend::Fake => Ok(Box::new(FakeEmbeddingProvider)),
            EmbeddingBackend::Remote(config) => TyphoonEmbeddingProvider::new(config.clone())
                .map(|provider| Box::new(provider) as Box<dyn EmbeddingProvider>)
                .map_err(internal_error),
        }
    }

    /// Memory repository over the shared SQLite store.
    fn memory_repo(&self) -> SqliteMemoryRepository {
        SqliteMemoryRepository::new(self.template_store.clone())
    }

    /// Embed text for storage. Returns `None` for the fake backend so retrieval
    /// falls back to recency instead of indexing meaningless zero vectors.
    async fn embed_for_storage(&self, text: &str) -> Result<Option<Vec<f32>>, ApiError> {
        match &self.embedding_backend {
            EmbeddingBackend::Fake => Ok(None),
            EmbeddingBackend::Remote(config) => {
                let provider =
                    TyphoonEmbeddingProvider::new(config.clone()).map_err(internal_error)?;
                let vector = provider.embed(text).await.map_err(|err| ApiError {
                    status: StatusCode::BAD_GATEWAY,
                    detail: format!("embedding failed: {err}"),
                })?;
                Ok(Some(vector))
            }
        }
    }

    /// Construct the Typhoon OCR provider. OCR is cloud-only and needs a key.
    fn build_ocr(&self) -> Result<TyphoonOcrProvider, ApiError> {
        let config = OcrConfig::from_env();
        if config.api_key.is_none() {
            return Err(ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                detail: "OCR ingestion requires LLM_API_KEY".to_string(),
            });
        }
        TyphoonOcrProvider::new(config).map_err(internal_error)
    }
}

fn build_typhoon(config: &TyphoonConfig) -> Result<Box<dyn LlmProvider>, ApiError> {
    TyphoonProvider::new(config.clone())
        .map(|provider| Box::new(provider) as Box<dyn LlmProvider>)
        .map_err(internal_error)
}

/// Optionally start the local LLM sidecar before the API begins serving.
///
/// No-op unless `LLM_AUTO_SERVE` is truthy *and* `LLM_BACKEND` selects the local
/// Typhoon server. When enabled it spawns `scripts/serve_llm.sh` (override with
/// `LLM_SERVE_CMD`) and blocks until the server answers, so the first
/// `/generate` request does not race model loading. The sidecar keeps running
/// in the background after this returns.
pub async fn maybe_start_local_llm() -> anyhow::Result<()> {
    if !env_flag("LLM_AUTO_SERVE") {
        return Ok(());
    }
    if !matches!(LlmBackend::from_env(), LlmBackend::TyphoonLocal(_)) {
        return Ok(());
    }

    let config = TyphoonConfig::local();
    let script =
        std::env::var("LLM_SERVE_CMD").unwrap_or_else(|_| "scripts/serve_llm.sh".to_string());
    println!("starting local LLM sidecar: {script}");
    Command::new("/bin/sh")
        .arg(&script)
        .spawn()
        .with_context(|| format!("failed to start LLM sidecar via {script}"))?;

    let timeout = std::time::Duration::from_secs(
        std::env::var("LLM_SERVE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(180),
    );
    providers::wait_until_ready(&config.base_url, timeout).await?;
    println!("local LLM server is ready at {}", config.base_url);
    Ok(())
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    app: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(api_index))
        .route("/docs", get(api_index))
        .route("/health", get(health))
        .route("/generate", post(generate))
        .route("/edit", post(edit))
        .route("/render", post(render))
        .route("/ingest", post(ingest))
        .route("/ingest/ocr", post(ingest_ocr))
        .route("/templates", get(list_templates).post(create_template))
        .route("/templates/default", get(resolve_default_template))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        app: state.app_name,
    })
}

/// Lightweight endpoint index served at `/` and `/docs`. The Axum port does not
/// ship a Swagger UI like the FastAPI original, so this lists the routes and
/// their request body types instead.
async fn api_index() -> Json<Value> {
    Json(serde_json::json!({
        "app": "govdoc-generator-rust",
        "endpoints": [
            { "method": "GET",  "path": "/health",            "desc": "Health check" },
            { "method": "POST", "path": "/generate",          "body": "DocRequest",    "desc": "Generate a Thai government document" },
            { "method": "POST", "path": "/edit",              "body": "EditRequest",   "desc": "Edit document fields" },
            { "method": "POST", "path": "/render",            "body": "RenderRequest", "desc": "Render a document to .docx via the sidecar" },
            { "method": "POST", "path": "/ingest",            "body": "IngestRequest", "desc": "Store a structured example in memory" },
            { "method": "POST", "path": "/ingest/ocr",        "body": "IngestOcrRequest", "desc": "OCR a local file into a memory example" },
            { "method": "GET",  "path": "/templates",         "query": "doc_type, agency", "desc": "List templates" },
            { "method": "POST", "path": "/templates",         "body": "TemplateCreateRequest", "desc": "Create a template" },
            { "method": "GET",  "path": "/templates/default", "query": "doc_type, agency", "desc": "Resolve the default template" }
        ]
    }))
}

#[derive(Debug, Serialize)]
struct GenerateResponse {
    doc: Value,
    trace: Vec<TraceEvent>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    detail: String,
}

async fn generate(
    State(state): State<AppState>,
    Json(req): Json<DocRequest>,
) -> Result<Json<GenerateResponse>, ApiError> {
    let llm = state.build_llm()?;
    let embedding_provider = state.build_embedding()?;
    let memory_repo = state.memory_repo();
    let mut trace = Vec::new();

    let doc = generate_document_json(
        &req,
        GenerationServices {
            generator: llm.as_ref(),
            critic: llm.as_ref(),
            memory_repo: &memory_repo,
            embedding_provider: embedding_provider.as_ref(),
        },
        GenerationOptions {
            max_rounds: 3,
            use_critic: req.use_critic.unwrap_or(true),
        },
        &mut trace,
    )
    .await
    .map_err(|err| ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: err.to_string(),
    })?;

    Ok(Json(GenerateResponse { doc, trace }))
}

async fn edit(
    State(state): State<AppState>,
    Json(req): Json<EditRequest>,
) -> Result<Json<Value>, ApiError> {
    let editor = state.build_llm()?;
    let edited = edit_document_json(
        req.doc_data,
        &req.edit_instructions,
        editor.as_ref(),
        &req.target_fields,
    )
    .await
    .map_err(|err| ApiError {
        status: StatusCode::BAD_REQUEST,
        detail: err.to_string(),
    })?;

    Ok(Json(edited))
}

#[derive(Debug, Deserialize)]
struct IngestRequest {
    doc_type: DocType,
    /// The example document JSON to store and surface during retrieval.
    fields: Value,
    /// Text used to compute the retrieval embedding. Derived from `fields` when
    /// omitted.
    summary: Option<String>,
    agency: Option<String>,
    recipient_class: Option<String>,
    raw_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IngestOcrRequest {
    /// Local path to an image or PDF to OCR into a memory example.
    file_path: String,
    doc_type: DocType,
    agency: Option<String>,
    recipient_class: Option<String>,
}

#[derive(Debug, Serialize)]
struct IngestResponse {
    id: i64,
    /// Whether a real embedding was stored (false on the fake backend).
    embedded: bool,
}

/// Store a structured example document in memory for retrieval.
async fn ingest(
    State(state): State<AppState>,
    Json(req): Json<IngestRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    let summary = req
        .summary
        .clone()
        .unwrap_or_else(|| derive_summary(req.doc_type, &req.fields));
    let embedding = state.embed_for_storage(&summary).await?;
    let id = store_memory_record(
        &state,
        req.doc_type,
        &summary,
        &req.fields,
        req.agency.as_deref(),
        req.recipient_class.as_deref(),
        req.raw_text.as_deref(),
        embedding.as_deref(),
    )?;
    Ok(Json(IngestResponse {
        id,
        embedded: embedding.is_some(),
    }))
}

/// OCR a local file, then store its text as a memory example.
async fn ingest_ocr(
    State(state): State<AppState>,
    Json(req): Json<IngestOcrRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    let ocr = state.build_ocr()?;
    let bytes = std::fs::read(&req.file_path).map_err(|err| ApiError {
        status: StatusCode::BAD_REQUEST,
        detail: format!("cannot read {}: {err}", req.file_path),
    })?;
    let filename = std::path::Path::new(&req.file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document")
        .to_string();

    let text = ocr
        .extract_text(&bytes, &filename)
        .await
        .map_err(|err| ApiError {
            status: StatusCode::BAD_GATEWAY,
            detail: format!("OCR failed: {err}"),
        })?;

    let summary = truncate_chars(&text, 2000);
    let embedding = state.embed_for_storage(&summary).await?;
    let fields = serde_json::json!({
        "doc_type": req.doc_type.as_thai(),
        "content": text,
        "source": filename,
    });
    let id = store_memory_record(
        &state,
        req.doc_type,
        &summary,
        &fields,
        req.agency.as_deref(),
        req.recipient_class.as_deref(),
        Some(text.as_str()),
        embedding.as_deref(),
    )?;
    Ok(Json(IngestResponse {
        id,
        embedded: embedding.is_some(),
    }))
}

#[allow(clippy::too_many_arguments)]
fn store_memory_record(
    state: &AppState,
    doc_type: DocType,
    summary: &str,
    fields: &Value,
    agency: Option<&str>,
    recipient_class: Option<&str>,
    raw_text: Option<&str>,
    embedding: Option<&[f32]>,
) -> Result<i64, ApiError> {
    let store = state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "memory store lock poisoned".to_string(),
    })?;
    store
        .store_memory(NewMemoryRecord {
            doc_type: doc_type.as_thai(),
            summary_text: summary,
            fields,
            recipient_class,
            agency,
            template_id: None,
            raw_text,
            embedding,
        })
        .map_err(internal_error)
}

/// Build a retrieval summary from a structured document when none was supplied.
fn derive_summary(doc_type: DocType, fields: &Value) -> String {
    let mut parts = vec![format!("ประเภท: {}", doc_type.as_thai())];
    for key in ["subject", "title"] {
        if let Some(value) = fields.get(key).and_then(Value::as_str) {
            if !value.is_empty() {
                parts.push(format!("เรื่อง: {value}"));
                break;
            }
        }
    }
    parts.join(" | ")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

async fn render(
    State(state): State<AppState>,
    Json(req): Json<RenderRequest>,
) -> Result<Response, ApiError> {
    validate_render_doc(&req)?;
    let template_path = resolve_template_path(&state, &req)?;
    let docx = render_with_sidecar(&state, &req, template_path.as_deref())?;

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"govdoc.docx\"",
        )
        .body(Body::from(docx))
        .map_err(|err| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: err.to_string(),
        })
}

#[derive(Debug, Deserialize)]
struct TemplateCreateRequest {
    doc_type: String,
    name: String,
    file_path: String,
    agency: Option<String>,
    #[serde(default)]
    is_default: bool,
}

#[derive(Debug, Deserialize)]
struct TemplateQuery {
    doc_type: Option<String>,
    agency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DefaultTemplateQuery {
    doc_type: String,
    agency: Option<String>,
}

#[derive(Debug, Serialize)]
struct TemplateResponse {
    id: i64,
    doc_type: String,
    agency: Option<String>,
    name: String,
    file_path: String,
    is_default: bool,
}

async fn create_template(
    State(state): State<AppState>,
    Json(req): Json<TemplateCreateRequest>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let store = state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "template store lock poisoned".to_string(),
    })?;

    let id = store
        .create_template(NewTemplateRecord {
            doc_type: &req.doc_type,
            name: &req.name,
            file_path: &req.file_path,
            agency: req.agency.as_deref(),
            is_default: req.is_default,
        })
        .map_err(internal_error)?;

    let template = store
        .get_template(id)
        .map_err(internal_error)?
        .ok_or_else(|| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: "created template could not be read back".to_string(),
        })?;

    Ok(Json(template.into()))
}

async fn list_templates(
    State(state): State<AppState>,
    Query(query): Query<TemplateQuery>,
) -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    let store = state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "template store lock poisoned".to_string(),
    })?;
    let templates = store
        .list_templates(query.doc_type.as_deref(), query.agency.as_deref())
        .map_err(internal_error)?;

    Ok(Json(templates.into_iter().map(Into::into).collect()))
}

async fn resolve_default_template(
    State(state): State<AppState>,
    Query(query): Query<DefaultTemplateQuery>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let store = state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "template store lock poisoned".to_string(),
    })?;
    let template = store
        .resolve_default(&query.doc_type, query.agency.as_deref())
        .map_err(internal_error)?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            detail: "template not found".to_string(),
        })?;

    Ok(Json(template.into()))
}

struct ApiError {
    status: StatusCode,
    detail: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                detail: self.detail,
            }),
        )
            .into_response()
    }
}

impl From<TemplateRecord> for TemplateResponse {
    fn from(template: TemplateRecord) -> Self {
        Self {
            id: template.id,
            doc_type: template.doc_type,
            agency: template.agency,
            name: template.name,
            file_path: template.file_path,
            is_default: template.is_default,
        }
    }
}

fn internal_error(err: anyhow::Error) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: err.to_string(),
    }
}

fn validate_render_doc(req: &RenderRequest) -> Result<(), ApiError> {
    match req.doc_type {
        DocType::External => serde_json::from_value::<ExternalDoc>(req.doc_data.clone())
            .map(|_| ())
            .map_err(bad_render_data),
        DocType::Internal => serde_json::from_value::<InternalDoc>(req.doc_data.clone())
            .map(|_| ())
            .map_err(bad_render_data),
        DocType::Order => serde_json::from_value::<OrderDoc>(req.doc_data.clone())
            .map(|_| ())
            .map_err(bad_render_data),
        DocType::Announcement => serde_json::from_value::<AnnouncementDoc>(req.doc_data.clone())
            .map(|_| ())
            .map_err(bad_render_data),
    }
}

fn bad_render_data(err: serde_json::Error) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        detail: format!("Invalid document data: {err}"),
    }
}

fn resolve_template_path(
    state: &AppState,
    req: &RenderRequest,
) -> Result<Option<String>, ApiError> {
    let Some(template_id) = req.template_id else {
        return Ok(None);
    };
    let store = state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "template store lock poisoned".to_string(),
    })?;
    let template = store
        .get_template(template_id)
        .map_err(internal_error)?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            detail: format!("Template {template_id} not found"),
        })?;
    if template.doc_type != req.doc_type.as_thai() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: format!(
                "Template {template_id} is for {}, not {}",
                template.doc_type,
                req.doc_type.as_thai()
            ),
        });
    }
    Ok(Some(template.file_path))
}

fn render_with_sidecar(
    state: &AppState,
    req: &RenderRequest,
    template_path: Option<&str>,
) -> Result<Vec<u8>, ApiError> {
    let Some(command) = state.renderer_cmd.as_deref() else {
        return Err(ApiError {
            status: StatusCode::NOT_IMPLEMENTED,
            detail: "Renderer sidecar is not configured; set GOVDOC_RENDERER_CMD".to_string(),
        });
    };

    let payload = serde_json::json!({
        "doc_type": req.doc_type.as_thai(),
        "doc_data": req.doc_data.clone(),
        "template_path": template_path,
        "python_source": state.python_source.as_deref(),
    });
    let input = serde_json::to_vec(&payload).map_err(|err| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: err.to_string(),
    })?;

    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: format!("Failed to start renderer sidecar: {err}"),
        })?;

    let mut stdin = child.stdin.take().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "Renderer sidecar stdin is unavailable".to_string(),
    })?;
    stdin.write_all(&input).map_err(|err| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: format!("Failed to write renderer input: {err}"),
    })?;
    drop(stdin);

    let output = child.wait_with_output().map_err(|err| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: format!("Failed to read renderer output: {err}"),
    })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ApiError {
            status: StatusCode::BAD_GATEWAY,
            detail: if detail.is_empty() {
                "Renderer sidecar failed".to_string()
            } else {
                detail
            },
        });
    }

    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn builds_router() {
        let _router = router(AppState::default());
    }

    #[tokio::test]
    async fn docs_index_lists_endpoints() {
        let app = router(AppState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["endpoints"].as_array().is_some_and(|e| !e.is_empty()));
    }

    #[tokio::test]
    async fn generate_returns_document_json_and_trace() {
        let app = router(AppState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/generate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "subject": "ขอเชิญร่วมงานวันเด็ก",
                            "purpose": "แจ้งกำหนดการและขอความร่วมมือ",
                            "recipient_name": "ผู้ปกครองนักเรียน",
                            "recipient_class": "executive",
                            "sender_name": "นายสมชาย รักเด็ก",
                            "sender_position": "ผู้อำนวยการโรงเรียน"
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["doc"]["doc_type"], "ภายนอก");
        assert_eq!(json["doc"]["subject"], "ขอเชิญร่วมงานวันเด็ก");
        assert_eq!(json["doc"]["salutation"], "กราบเรียน");
        assert_eq!(json["trace"][0]["step"], "retrieval");
    }

    #[tokio::test]
    async fn edit_updates_targeted_body_field() {
        let app = router(AppState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/edit")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "doc_data": {
                                "doc_type": "ภายนอก",
                                "subject": "เรื่องเดิม",
                                "body": ["ย่อหน้าเดิม"]
                            },
                            "edit_instructions": "เพิ่มความสุภาพ",
                            "target_fields": ["body"]
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["body"][0], "ย่อหน้าเดิม (แก้ไข: เพิ่มความสุภาพ)");
        assert_eq!(json["subject"], "เรื่องเดิม");
    }

    #[tokio::test]
    async fn template_store_creates_lists_and_resolves_default() {
        let app = router(AppState::default());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "name": "กลาง",
                            "file_path": "templates/external.docx",
                            "is_default": true
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/templates?doc_type=%E0%B8%A0%E0%B8%B2%E0%B8%A2%E0%B8%99%E0%B8%AD%E0%B8%81")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let default_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/templates/default?doc_type=%E0%B8%A0%E0%B8%B2%E0%B8%A2%E0%B8%99%E0%B8%AD%E0%B8%81")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(default_response.status(), StatusCode::OK);
        let body = to_bytes(default_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["name"], "กลาง");
        assert_eq!(json["is_default"], true);
    }

    #[tokio::test]
    async fn ingested_example_is_used_during_generate() {
        let app = router(AppState::default());

        let ingest = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ingest")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "fields": {
                                "doc_type": "ภายนอก",
                                "subject": "ตัวอย่างหนังสือเชิญประชุม",
                                "body": ["ย่อหน้าตัวอย่าง"]
                            }
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ingest.status(), StatusCode::OK);
        let body = to_bytes(ingest.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["embedded"], false);

        let generate = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/generate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "subject": "ขอเชิญประชุมประจำเดือน",
                            "recipient_class": "executive"
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(generate.status(), StatusCode::OK);
        let body = to_bytes(generate.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["trace"][0]["step"], "retrieval");
        assert_eq!(json["trace"][0]["detail"]["examples"], 1);
    }

    #[tokio::test]
    async fn render_requires_configured_sidecar() {
        let app = router(AppState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/render")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "doc_type": "ภายนอก",
                            "doc_data": {
                                "doc_type": "ภายนอก",
                                "number": "ศธ 0000/0001",
                                "agency": "หน่วยงานตัวอย่าง",
                                "date": "1 มกราคม 2569",
                                "subject": "ขอเชิญร่วมงาน",
                                "recipient": "ผู้ปกครอง",
                                "salutation": "เรียน",
                                "reference": [],
                                "enclosure": [],
                                "body": ["ย่อหน้าทดสอบ"],
                                "closing": "ขอแสดงความนับถือ",
                                "signer_name": "นายสมชาย",
                                "signer_position": "ผู้อำนวยการ"
                            }
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
