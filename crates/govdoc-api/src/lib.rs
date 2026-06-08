mod memory;
mod mock;
mod providers;

use std::io::Write;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Context;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use govdoc_domain::{
    AnnouncementDoc, DocRequest, DocType, EditRequest, ExternalDoc, InternalDoc, OrderDoc,
    RenderRequest,
};
use govdoc_storage::{
    DocumentRecord, DocumentSummary, GeneralDocumentBlock, GeneralDocumentPage,
    GeneralDocumentSummary, NewGeneralDocument, NewGeneralDocumentBlock, NewMemoryRecord,
    NewTemplateRecord, PersistentVectorIndex, SqliteStore, TemplateRecord,
};
use govdoc_usecases::{
    edit_document_json, generate_document_json, structure_document_from_text, EmbeddingProvider,
    GenerationOptions, GenerationServices, LlmProvider, TraceEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::CorsLayer;

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
    vector_index: Arc<RwLock<PersistentVectorIndex>>,
    renderer_cmd: Option<String>,
    python_source: Option<String>,
    llm_backend: LlmBackend,
    embedding_backend: EmbeddingBackend,
}

impl Default for AppState {
    fn default() -> Self {
        let template_store = Arc::new(Mutex::new(open_store()));
        let vector_index = Arc::new(RwLock::new(open_index(&template_store)));
        Self {
            app_name: "govdoc-generator-rust".to_string(),
            template_store,
            vector_index,
            renderer_cmd: renderer_cmd_from_env(),
            python_source: optional_env_or_existing_path(
                "GOVDOC_PYTHON_SOURCE",
                ["../govdoc-generator"],
            ),
            llm_backend: LlmBackend::from_env(),
            embedding_backend: EmbeddingBackend::from_env(),
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn optional_env_or_existing_path<const N: usize>(
    name: &str,
    fallbacks: [&str; N],
) -> Option<String> {
    optional_env(name).or_else(|| {
        fallbacks
            .into_iter()
            .find_map(|path| resolve_existing_path(path).map(|path| path.display().to_string()))
    })
}

fn renderer_cmd_from_env() -> Option<String> {
    optional_env("GOVDOC_RENDERER_CMD").or_else(|| {
        resolve_existing_path("scripts/render_docx_sidecar.py")
            .map(|script| format!("python3 {}", shell_quote_path(&script)))
    })
}

fn resolve_existing_path(relative: &str) -> Option<PathBuf> {
    let path = FsPath::new(relative);
    if path.is_absolute() && path.exists() {
        return Some(path.to_path_buf());
    }

    let mut bases = vec![std::env::current_dir().ok()];
    if let Ok(exe) = std::env::current_exe() {
        let mut cursor = exe.parent();
        while let Some(dir) = cursor {
            bases.push(Some(dir.to_path_buf()));
            cursor = dir.parent();
        }
    }
    bases.push(Some(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    ));

    bases.into_iter().flatten().find_map(|base| {
        let candidate = base.join(relative);
        candidate.exists().then_some(candidate)
    })
}

fn shell_quote_path(path: &FsPath) -> String {
    let path = path.display().to_string();
    format!("'{}'", path.replace('\'', "'\\''"))
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

/// Load the vector index from `HNSW_INDEX_PATH`, rebuilding it from SQLite
/// (the source of truth) when the file is missing or empty.
fn open_index(store: &Arc<Mutex<SqliteStore>>) -> PersistentVectorIndex {
    let path = std::env::var("HNSW_INDEX_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let mut index = PersistentVectorIndex::load(path);
    if index.is_empty() {
        if let Ok(store) = store.lock() {
            if let Ok(rows) = store.memory_vectors() {
                if !rows.is_empty() {
                    let _ = index.rebuild(rows);
                }
            }
        }
    }
    index
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

    /// Memory repository over the shared SQLite store and vector index.
    fn memory_repo(&self) -> SqliteMemoryRepository {
        SqliteMemoryRepository::new(self.template_store.clone(), self.vector_index.clone())
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
        .route("/status", get(status))
        .route("/generate", post(generate))
        .route("/edit", post(edit))
        .route("/render", post(render))
        .route("/render/save", post(render_save))
        .route("/ingest", post(ingest))
        .route("/ingest/ocr", post(ingest_ocr))
        .route("/ingest/ocr/upload", post(ingest_ocr_upload))
        .route("/templates", get(list_templates).post(create_template))
        .route("/templates/upload", post(upload_template))
        .route("/templates/default", get(resolve_default_template))
        .route("/documents", get(list_documents).post(save_document))
        .route(
            "/documents/:id",
            get(get_document)
                .put(update_document)
                .delete(delete_document),
        )
        .route(
            "/general-documents",
            get(list_general_documents).post(upload_general_document),
        )
        .route("/general-documents/upload", post(upload_general_document))
        .route(
            "/general-documents/:id",
            get(get_general_document).delete(delete_general_document),
        )
        .route(
            "/general-documents/:id/delete",
            post(delete_general_document),
        )
        .route(
            "/general-documents/:id/pages/:page",
            get(get_general_document_page),
        )
        .route(
            "/general-documents/:id/pages/:page/blocks",
            get(list_general_page_blocks),
        )
        .route(
            "/general-documents/:id/pages/:page/image",
            get(get_general_page_image),
        )
        .route(
            "/general-documents/:id/search",
            post(search_general_document),
        )
        .route("/general-documents/:id/ocr", post(ocr_general_document))
        .route("/general-documents/:id/edit", post(edit_general_document))
        .route(
            "/general-documents/:id/export/docx",
            post(export_general_docx),
        )
        .route(
            "/general-documents/:id/export/pdf",
            post(export_general_pdf),
        )
        // Allow up to 25 MB uploads (scanned PDFs, .docx templates).
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
        // Permissive CORS: the API binds to localhost only and is consumed by
        // the Tauri webview / local tools, so any local origin is acceptable.
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        app: state.app_name,
    })
}

/// Report the active backends so a UI can show configuration and flag anything
/// that still needs setup (e.g. a cloud key). Backs the hybrid runtime: the
/// frontend can tell whether the LLM/embedding/OCR are fake, local, or cloud.
async fn status(State(state): State<AppState>) -> Json<Value> {
    let (llm_backend, llm_ready) = match &state.llm_backend {
        LlmBackend::Fake => ("fake", true),
        LlmBackend::TyphoonLocal(_) => ("typhoon-local", true),
        LlmBackend::TyphoonCloud(config) => ("typhoon-cloud", config.api_key.is_some()),
    };
    let embedding_backend = match &state.embedding_backend {
        EmbeddingBackend::Fake => "fake",
        EmbeddingBackend::Remote(_) => "remote",
    };
    let ocr_ready = OcrConfig::from_env().api_key.is_some();
    let persistent = std::env::var("SQLITE_PATH").is_ok_and(|p| !p.is_empty());

    Json(serde_json::json!({
        "app": state.app_name,
        "llm": { "backend": llm_backend, "ready": llm_ready },
        "embedding": { "backend": embedding_backend },
        "ocr": { "ready": ocr_ready },
        "renderer_configured": state.renderer_cmd.is_some(),
        "persistent": persistent,
    }))
}

/// Lightweight endpoint index served at `/` and `/docs`. The Axum port does not
/// ship a Swagger UI like the FastAPI original, so this lists the routes and
/// their request body types instead.
async fn api_index() -> Json<Value> {
    Json(serde_json::json!({
        "app": "govdoc-generator-rust",
        "endpoints": [
            { "method": "GET",  "path": "/health",            "desc": "Health check" },
            { "method": "GET",  "path": "/status",            "desc": "Active backends (llm/embedding/ocr) and readiness" },
            { "method": "POST", "path": "/generate",          "body": "DocRequest",    "desc": "Generate a Thai government document" },
            { "method": "POST", "path": "/edit",              "body": "EditRequest",   "desc": "Edit document fields" },
            { "method": "POST", "path": "/render",            "body": "RenderRequest", "desc": "Render a document to .docx via the sidecar" },
            { "method": "POST", "path": "/render/save",       "body": "RenderRequest", "desc": "Render a document and save it to disk" },
            { "method": "POST", "path": "/ingest",            "body": "IngestRequest", "desc": "Store a structured example in memory" },
            { "method": "POST", "path": "/ingest/ocr",        "body": "IngestOcrRequest", "desc": "OCR a local file into a memory example" },
            { "method": "POST", "path": "/ingest/ocr/upload", "body": "multipart (file, doc_type)", "desc": "Upload + OCR a scan into a memory example" },
            { "method": "POST", "path": "/documents",         "body": "SaveDocumentRequest", "desc": "Save a generated document" },
            { "method": "GET",  "path": "/documents",         "query": "doc_type", "desc": "List saved documents (newest first)" },
            { "method": "GET",  "path": "/documents/:id",     "desc": "Get one saved document" },
            { "method": "PUT",  "path": "/documents/:id",     "body": "SaveDocumentRequest", "desc": "Replace one saved document" },
            { "method": "DELETE","path": "/documents/:id",    "desc": "Delete a saved document" },
            { "method": "POST", "path": "/general-documents/upload", "body": "multipart file", "desc": "Upload a general PDF/image document" },
            { "method": "GET",  "path": "/general-documents", "desc": "List general documents" },
            { "method": "GET",  "path": "/general-documents/:id", "desc": "Get general document metadata and page summaries" },
            { "method": "DELETE", "path": "/general-documents/:id", "desc": "Delete a general document and its stored files" },
            { "method": "GET",  "path": "/general-documents/:id/pages/:page", "desc": "Get one OCR/edited page" },
            { "method": "GET",  "path": "/general-documents/:id/pages/:page/blocks", "desc": "List layout/text blocks for one page" },
            { "method": "GET",  "path": "/general-documents/:id/pages/:page/image", "desc": "Get rendered page image when available" },
            { "method": "POST", "path": "/general-documents/:id/search", "body": "GeneralSearchRequest", "desc": "Search block-level RAG index with page/type filters" },
            { "method": "POST", "path": "/general-documents/:id/ocr", "desc": "OCR general document pages" },
            { "method": "POST", "path": "/general-documents/:id/edit", "body": "GeneralEditRequest", "desc": "Edit/check OCR text by page range" },
            { "method": "POST", "path": "/general-documents/:id/export/docx", "desc": "Export general document to DOCX" },
            { "method": "POST", "path": "/general-documents/:id/export/pdf", "desc": "Export general document to PDF" },
            { "method": "GET",  "path": "/templates",         "query": "doc_type, agency", "desc": "List templates" },
            { "method": "POST", "path": "/templates",         "body": "TemplateCreateRequest", "desc": "Register a template by file path" },
            { "method": "POST", "path": "/templates/upload",  "body": "multipart (file, doc_type, name)", "desc": "Upload a .docx render template" },
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
    /// Run an LLM pass to parse the OCR text into the document schema. Defaults
    /// to true; set false to store the raw text instead.
    #[serde(default = "default_true")]
    structure: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct IngestResponse {
    id: i64,
    /// Whether a real embedding was stored (false on the fake backend).
    embedded: bool,
    /// Whether the stored fields were structured into the document schema
    /// (false means raw OCR text was stored as a fallback).
    structured: bool,
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
        structured: true,
    }))
}

/// OCR a local file, then store its text as a memory example. Unless disabled,
/// an LLM pass parses the text into the document schema first.
async fn ingest_ocr(
    State(state): State<AppState>,
    Json(req): Json<IngestOcrRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    let bytes = std::fs::read(&req.file_path).map_err(|err| ApiError {
        status: StatusCode::BAD_REQUEST,
        detail: format!("cannot read {}: {err}", req.file_path),
    })?;
    let filename = std::path::Path::new(&req.file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document")
        .to_string();

    let resp = run_ocr_ingest(
        &state,
        req.doc_type,
        &filename,
        &bytes,
        req.agency.as_deref(),
        req.recipient_class.as_deref(),
        req.structure,
    )
    .await?;
    Ok(Json(resp))
}

/// Upload a scanned document (image/PDF) and ingest it as a memory example via
/// OCR. Multipart fields: `file`, `doc_type`, optional `agency`,
/// `recipient_class`, `structure` (default true).
async fn ingest_ocr_upload(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<IngestResponse>, ApiError> {
    let upload = Upload::collect(multipart).await?;
    let doc_type = upload.doc_type()?;
    let (filename, bytes) = upload.file()?;
    let structure = upload
        .field("structure")
        .map(|v| v != "false")
        .unwrap_or(true);

    let resp = run_ocr_ingest(
        &state,
        doc_type,
        filename,
        bytes,
        upload.field("agency"),
        upload.field("recipient_class"),
        structure,
    )
    .await?;
    Ok(Json(resp))
}

/// Shared OCR-ingest pipeline used by both the path and upload variants: OCR the
/// bytes, optionally structure into the schema, embed, and store.
async fn run_ocr_ingest(
    state: &AppState,
    doc_type: DocType,
    filename: &str,
    bytes: &[u8],
    agency: Option<&str>,
    recipient_class: Option<&str>,
    structure: bool,
) -> Result<IngestResponse, ApiError> {
    let ocr = state.build_ocr()?;
    let text = ocr
        .extract_text(bytes, filename)
        .await
        .map_err(|err| ApiError {
            status: StatusCode::BAD_GATEWAY,
            detail: format!("OCR failed: {err}"),
        })?;

    // Best-effort: structure the OCR text into the document schema. On any
    // failure, fall back to storing the raw text as a content example.
    let mut structured_fields = None;
    if structure {
        let llm = state.build_llm()?;
        if let Ok(fields) = structure_document_from_text(&doc_type, &text, llm.as_ref()).await {
            structured_fields = Some(fields);
        }
    }

    let (fields, summary, structured) = match structured_fields {
        Some(fields) => {
            let summary = derive_summary(doc_type, &fields);
            (fields, summary, true)
        }
        None => {
            let fields = serde_json::json!({
                "doc_type": doc_type.as_thai(),
                "content": text,
                "source": filename,
            });
            (fields, truncate_chars(&text, 2000), false)
        }
    };

    let embedding = state.embed_for_storage(&summary).await?;
    let id = store_memory_record(
        state,
        doc_type,
        &summary,
        &fields,
        agency,
        recipient_class,
        Some(text.as_str()),
        embedding.as_deref(),
    )?;
    Ok(IngestResponse {
        id,
        embedded: embedding.is_some(),
        structured,
    })
}

/// Upload a `.docx` render template, save it under `GOVDOC_TEMPLATES_DIR`, and
/// register it. Multipart fields: `file`, `doc_type`, `name`, optional `agency`,
/// `is_default`.
async fn upload_template(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<TemplateResponse>, ApiError> {
    let upload = Upload::collect(multipart).await?;
    let doc_type = upload.required("doc_type")?.to_string();
    let name = upload.required("name")?.to_string();
    let (orig_name, bytes) = upload.file()?;
    let is_default = matches!(upload.field("is_default"), Some("true" | "on" | "1"));

    let dir =
        std::env::var("GOVDOC_TEMPLATES_DIR").unwrap_or_else(|_| "app-data/templates".to_string());
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = format!("{dir}/{millis}_{}", sanitize_filename(orig_name));
    std::fs::write(&path, bytes).map_err(io_error)?;

    let store = lock_store(&state)?;
    let id = store
        .create_template(NewTemplateRecord {
            doc_type: &doc_type,
            name: &name,
            file_path: &path,
            agency: upload.field("agency"),
            is_default,
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

/// A collected multipart form: text fields keyed by name plus one optional file.
struct Upload {
    fields: std::collections::HashMap<String, String>,
    filename: Option<String>,
    bytes: Option<Vec<u8>>,
}

impl Upload {
    async fn collect(mut multipart: Multipart) -> Result<Self, ApiError> {
        let mut fields = std::collections::HashMap::new();
        let mut filename = None;
        let mut bytes = None;
        while let Some(field) = multipart.next_field().await.map_err(bad_multipart)? {
            let name = field.name().unwrap_or("").to_string();
            if name == "file" {
                filename = field.file_name().map(ToOwned::to_owned);
                bytes = Some(field.bytes().await.map_err(bad_multipart)?.to_vec());
            } else {
                fields.insert(name, field.text().await.map_err(bad_multipart)?);
            }
        }
        Ok(Self {
            fields,
            filename,
            bytes,
        })
    }

    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    fn required(&self, name: &str) -> Result<&str, ApiError> {
        self.field(name).ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: format!("missing form field: {name}"),
        })
    }

    fn doc_type(&self) -> Result<DocType, ApiError> {
        let raw = self.required("doc_type")?;
        serde_json::from_value::<DocType>(Value::String(raw.to_string())).map_err(|_| ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: format!("invalid doc_type: {raw}"),
        })
    }

    fn file(&self) -> Result<(&str, &[u8]), ApiError> {
        let bytes = self.bytes.as_deref().ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "missing file field".to_string(),
        })?;
        Ok((self.filename.as_deref().unwrap_or("upload"), bytes))
    }
}

fn bad_multipart(err: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        detail: format!("invalid multipart form: {err}"),
    }
}

fn io_error(err: std::io::Error) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: err.to_string(),
    }
}

/// Keep just the base name and drop control/path characters from an uploaded
/// file name before writing it to disk.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    if cleaned.is_empty() {
        "template.docx".to_string()
    } else {
        cleaned
    }
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
    // Write to SQLite (source of truth), releasing the lock before touching the
    // index to avoid holding two locks at once.
    let id = {
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
            .map_err(internal_error)?
    };

    if let Some(vector) = embedding {
        let mut index = state.vector_index.write().map_err(|_| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: "vector index lock poisoned".to_string(),
        })?;
        index
            .add(id, doc_type.as_thai(), vector.to_vec())
            .map_err(internal_error)?;
    }

    Ok(id)
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

#[derive(Debug, Serialize)]
struct RenderSaveResponse {
    file_path: String,
    bytes: usize,
}

async fn render_save(
    State(state): State<AppState>,
    Json(req): Json<RenderRequest>,
) -> Result<Json<RenderSaveResponse>, ApiError> {
    validate_render_doc(&req)?;
    let template_path = resolve_template_path(&state, &req)?;
    let docx = render_with_sidecar(&state, &req, template_path.as_deref())?;
    let file_path = save_docx_to_disk(&docx)?;

    Ok(Json(RenderSaveResponse {
        file_path: file_path.display().to_string(),
        bytes: docx.len(),
    }))
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

#[derive(Debug, Deserialize)]
struct SaveDocumentRequest {
    doc_type: DocType,
    /// The generated document JSON (as returned by `/generate`).
    doc_data: Value,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DocumentQuery {
    doc_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct SavedResponse {
    id: i64,
}

#[derive(Debug, Serialize)]
struct DocumentSummaryResponse {
    id: i64,
    doc_type: String,
    title: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct DocumentResponse {
    id: i64,
    doc_type: String,
    title: Option<String>,
    doc_data: Value,
    created_at: String,
}

async fn save_document(
    State(state): State<AppState>,
    Json(req): Json<SaveDocumentRequest>,
) -> Result<Json<SavedResponse>, ApiError> {
    let store = lock_store(&state)?;
    let id = store
        .save_document(req.doc_type.as_thai(), req.title.as_deref(), &req.doc_data)
        .map_err(internal_error)?;
    Ok(Json(SavedResponse { id }))
}

async fn list_documents(
    State(state): State<AppState>,
    Query(query): Query<DocumentQuery>,
) -> Result<Json<Vec<DocumentSummaryResponse>>, ApiError> {
    let store = lock_store(&state)?;
    let documents = store
        .list_documents(query.doc_type.as_deref())
        .map_err(internal_error)?;
    Ok(Json(documents.into_iter().map(Into::into).collect()))
}

async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DocumentResponse>, ApiError> {
    let store = lock_store(&state)?;
    let document = store
        .get_document(id)
        .map_err(internal_error)?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            detail: format!("Document {id} not found"),
        })?;
    Ok(Json(document.into()))
}

async fn update_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<SaveDocumentRequest>,
) -> Result<Json<SavedResponse>, ApiError> {
    let store = lock_store(&state)?;
    let updated = store
        .update_document(
            id,
            req.doc_type.as_thai(),
            req.title.as_deref(),
            &req.doc_data,
        )
        .map_err(internal_error)?;
    if !updated {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            detail: format!("Document {id} not found"),
        });
    }
    Ok(Json(SavedResponse { id }))
}

async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let store = lock_store(&state)?;
    if store.delete_document(id).map_err(internal_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError {
            status: StatusCode::NOT_FOUND,
            detail: format!("Document {id} not found"),
        })
    }
}

const GENERAL_DOC_MAX_PAGES: usize = 20;
const GENERAL_DOCS_DIR: &str = "app-data/general-documents";

#[derive(Debug, Serialize)]
struct GeneralDocumentListResponse {
    id: i64,
    filename: String,
    page_count: i64,
    status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct GeneralPageResponse {
    page_number: i64,
    status: String,
    ocr_text: Option<String>,
    edited_text: Option<String>,
    error: Option<String>,
    page_image_path: Option<String>,
    page_width: Option<i64>,
    page_height: Option<i64>,
    layout_warning: Option<String>,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct GeneralBlockResponse {
    id: i64,
    page_number: i64,
    block_index: i64,
    block_type: String,
    text: Option<String>,
    bbox: Option<Value>,
    style: Option<Value>,
    image_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeneralSearchRequest {
    query: String,
    page_start: Option<i64>,
    page_end: Option<i64>,
    block_type: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct GeneralSearchHitResponse {
    score: f32,
    block: GeneralBlockResponse,
}

#[derive(Debug, Serialize)]
struct GeneralDocumentResponse {
    id: i64,
    filename: String,
    page_count: i64,
    status: String,
    created_at: String,
    updated_at: String,
    pages: Vec<GeneralPageResponse>,
}

#[derive(Debug, Deserialize)]
struct GeneralEditRequest {
    instruction: String,
    page: Option<i64>,
    block_index: Option<i64>,
    all_pages: Option<bool>,
}

#[derive(Debug, Serialize)]
struct GeneralActionResponse {
    id: i64,
    status: String,
}

async fn upload_general_document(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<GeneralActionResponse>, ApiError> {
    let upload = Upload::collect(multipart).await?;
    let (filename, bytes) = upload.file()?;
    validate_general_file(filename, bytes)?;
    let page_count = estimate_page_count(filename, bytes)?;
    if page_count > GENERAL_DOC_MAX_PAGES {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: format!("รองรับสูงสุด {GENERAL_DOC_MAX_PAGES} หน้า (ไฟล์นี้ประมาณ {page_count} หน้า)"),
        });
    }

    let root =
        optional_env("GOVDOC_GENERAL_DOCS_DIR").unwrap_or_else(|| GENERAL_DOCS_DIR.to_string());
    std::fs::create_dir_all(&root).map_err(io_error)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let staging_dir = PathBuf::from(&root).join(format!("upload-{millis}"));
    let source_dir = staging_dir.join("source");
    std::fs::create_dir_all(&source_dir).map_err(io_error)?;
    let source_path = source_dir.join(sanitize_filename(filename));
    std::fs::write(&source_path, bytes).map_err(io_error)?;

    let store = lock_store(&state)?;
    let id = store
        .create_general_document(NewGeneralDocument {
            filename,
            file_path: &source_path.display().to_string(),
            page_count: page_count as i64,
        })
        .map_err(internal_error)?;
    drop(store);

    let doc_dir = PathBuf::from(&root).join(id.to_string());
    if doc_dir.exists() {
        std::fs::remove_dir_all(&doc_dir).map_err(io_error)?;
    }
    std::fs::rename(&staging_dir, &doc_dir).map_err(io_error)?;
    let final_source_path = doc_dir.join("source").join(sanitize_filename(filename));
    {
        let store = lock_store(&state)?;
        store
            .update_general_document_file_path(id, &final_source_path.display().to_string())
            .map_err(internal_error)?;
        let assets =
            prepare_general_page_images(&doc_dir, filename, &final_source_path, page_count)?;
        for asset in assets {
            store
                .update_general_page_asset(
                    id,
                    asset.page_number,
                    asset.path.as_deref(),
                    asset.width,
                    asset.height,
                    asset.warning.as_deref(),
                )
                .map_err(internal_error)?;
        }
    }
    Ok(Json(GeneralActionResponse {
        id,
        status: "pending".to_string(),
    }))
}

async fn list_general_documents(
    State(state): State<AppState>,
) -> Result<Json<Vec<GeneralDocumentListResponse>>, ApiError> {
    let store = lock_store(&state)?;
    let docs = store.list_general_documents().map_err(internal_error)?;
    Ok(Json(docs.into_iter().map(Into::into).collect()))
}

async fn get_general_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<GeneralDocumentResponse>, ApiError> {
    let store = lock_store(&state)?;
    let doc = store
        .get_general_document(id)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("general document", id))?;
    let pages = store
        .list_general_document_pages(id)
        .map_err(internal_error)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(GeneralDocumentResponse::from_parts(doc, pages)))
}

async fn delete_general_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let doc = {
        let store = lock_store(&state)?;
        store
            .get_general_document(id)
            .map_err(internal_error)?
            .ok_or_else(|| not_found("general document", id))?
    };
    {
        let store = lock_store(&state)?;
        if !store.delete_general_document(id).map_err(internal_error)? {
            return Err(not_found("general document", id));
        }
    }
    if let Some(dir) = general_document_dir(&doc) {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(io_error)?;
        }
    } else {
        let path = FsPath::new(&doc.file_path);
        if path.exists() {
            std::fs::remove_file(path).map_err(io_error)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_general_document_page(
    State(state): State<AppState>,
    Path((id, page)): Path<(i64, i64)>,
) -> Result<Json<GeneralPageResponse>, ApiError> {
    let store = lock_store(&state)?;
    let page = store
        .get_general_document_page(id, page)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("general document page", id))?;
    Ok(Json(page.into()))
}

async fn list_general_page_blocks(
    State(state): State<AppState>,
    Path((id, page)): Path<(i64, i64)>,
) -> Result<Json<Vec<GeneralBlockResponse>>, ApiError> {
    let store = lock_store(&state)?;
    ensure_general_document(&store, id)?;
    let blocks = store
        .list_general_document_blocks(id, Some(page))
        .map_err(internal_error)?;
    Ok(Json(blocks.into_iter().map(Into::into).collect()))
}

async fn search_general_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<GeneralSearchRequest>,
) -> Result<Json<Vec<GeneralSearchHitResponse>>, ApiError> {
    let query = req.query.trim();
    if query.is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "query is required".to_string(),
        });
    }
    let limit = req.limit.unwrap_or(10).clamp(1, 50);
    let query_embedding = state.embed_for_storage(query).await?;
    let store = lock_store(&state)?;
    ensure_general_document(&store, id)?;
    let blocks = store
        .list_general_document_blocks(id, None)
        .map_err(internal_error)?;
    let mut hits = rank_general_blocks(
        blocks,
        query,
        query_embedding.as_deref(),
        req.page_start,
        req.page_end,
        req.block_type.as_deref(),
    );
    hits.truncate(limit);
    Ok(Json(
        hits.into_iter()
            .map(|(score, block)| GeneralSearchHitResponse {
                score,
                block: block.into(),
            })
            .collect(),
    ))
}

async fn get_general_page_image(
    State(state): State<AppState>,
    Path((id, page)): Path<(i64, i64)>,
) -> Result<Response, ApiError> {
    let page = {
        let store = lock_store(&state)?;
        store
            .get_general_document_page(id, page)
            .map_err(internal_error)?
            .ok_or_else(|| not_found("general document page", id))?
    };
    let path = page.page_image_path.ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        detail: page
            .layout_warning
            .unwrap_or_else(|| "page image is not available".to_string()),
    })?;
    let bytes = std::fs::read(&path).map_err(io_error)?;
    let content_type = image_content_type(&path);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|err| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: err.to_string(),
        })
}

async fn ocr_general_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<GeneralActionResponse>, ApiError> {
    let doc = {
        let store = lock_store(&state)?;
        store
            .get_general_document(id)
            .map_err(internal_error)?
            .ok_or_else(|| not_found("general document", id))?
    };
    let bytes = std::fs::read(&doc.file_path).map_err(io_error)?;
    let repaired_page_count = estimate_page_count(&doc.filename, &bytes)? as i64;
    if repaired_page_count > doc.page_count {
        let doc_dir = FsPath::new(&doc.file_path)
            .parent()
            .and_then(FsPath::parent)
            .map(FsPath::to_path_buf);
        let store = lock_store(&state)?;
        store
            .ensure_general_document_pages(id, repaired_page_count)
            .map_err(internal_error)?;
        if let Some(doc_dir) = doc_dir {
            let assets = prepare_general_page_images(
                &doc_dir,
                &doc.filename,
                FsPath::new(&doc.file_path),
                repaired_page_count as usize,
            )?;
            for asset in assets {
                store
                    .update_general_page_asset(
                        id,
                        asset.page_number,
                        asset.path.as_deref(),
                        asset.width,
                        asset.height,
                        asset.warning.as_deref(),
                    )
                    .map_err(internal_error)?;
            }
        }
    }
    let doc = if repaired_page_count > doc.page_count {
        lock_store(&state)?
            .get_general_document(id)
            .map_err(internal_error)?
            .ok_or_else(|| not_found("general document", id))?
    } else {
        doc
    };
    ensure_general_page_images_for_ocr(&state, id, &doc)?;
    let ocr = state.build_ocr()?;

    for page in 1..=doc.page_count {
        {
            let store = lock_store(&state)?;
            store
                .update_general_page_ocr(id, page, "running", None, None, None)
                .map_err(internal_error)?;
        }

        let page_image_path = lock_store(&state)?
            .get_general_document_page(id, page)
            .map_err(internal_error)?
            .and_then(|page| page.page_image_path);
        let page_payload = page_image_path
            .as_deref()
            .and_then(|path| {
                std::fs::read(path)
                    .ok()
                    .map(|page_bytes| (path, page_bytes))
            })
            .map(|(path, page_bytes)| {
                let filename = FsPath::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("page.png")
                    .to_string();
                (page_bytes, filename, None)
            })
            .unwrap_or_else(|| (bytes.clone(), doc.filename.clone(), Some(page as usize)));

        let result = match ocr
            .extract_page(&page_payload.0, &page_payload.1, page_payload.2)
            .await
        {
            Ok(output) => {
                let mut blocks = extract_general_blocks(id, page, &output.text, &output.raw_json);
                embed_general_blocks(&state, &mut blocks).await?;
                Ok((output, blocks))
            }
            Err(err) => Err(err),
        };
        {
            let mut store = lock_store(&state)?;
            match result {
                Ok((output, blocks)) => {
                    store
                        .update_general_page_ocr(
                            id,
                            page,
                            "succeeded",
                            Some(&output.text),
                            Some(&output.raw_json),
                            None,
                        )
                        .map_err(internal_error)?;
                    let records: Vec<_> = blocks.iter().map(new_general_block_record).collect();
                    store
                        .replace_general_page_blocks(id, page, &records)
                        .map_err(internal_error)?;
                }
                Err(err) => store
                    .update_general_page_ocr(id, page, "failed", None, None, Some(&err.to_string()))
                    .map_err(internal_error)?,
            }
        }
        if page < doc.page_count {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }

    let status = lock_store(&state)?
        .get_general_document(id)
        .map_err(internal_error)?
        .map(|doc| doc.status)
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Json(GeneralActionResponse { id, status }))
}

async fn edit_general_document(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<GeneralEditRequest>,
) -> Result<Json<GeneralActionResponse>, ApiError> {
    if req.instruction.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "instruction is required".to_string(),
        });
    }
    let (pages, all_blocks) = {
        let store = lock_store(&state)?;
        ensure_general_document(&store, id)?;
        (
            store
                .list_general_document_pages(id)
                .map_err(internal_error)?,
            store
                .list_general_document_blocks(id, None)
                .map_err(internal_error)?,
        )
    };
    if let Some(block_index) = req.block_index {
        let page_number = req.page.unwrap_or(1);
        let block = all_blocks
            .iter()
            .find(|block| block.page_number == page_number && block.block_index == block_index)
            .ok_or_else(|| ApiError {
                status: StatusCode::BAD_REQUEST,
                detail: format!("block {block_index} on page {page_number} not found"),
            })?;
        let source = block.text.as_deref().unwrap_or("");
        let context = general_context_for_selected_block(block);
        let editor = state.build_llm()?;
        let edited = edit_general_text(editor.as_ref(), source, &req.instruction, &context).await?;
        let page_text = {
            let mut page_blocks: Vec<_> = all_blocks
                .iter()
                .filter(|block| block.page_number == page_number)
                .cloned()
                .collect();
            for block in &mut page_blocks {
                if block.block_index == block_index {
                    block.text = Some(edited.clone());
                }
            }
            page_text_from_blocks(&page_blocks).unwrap_or_else(|| edited.clone())
        };
        let store = lock_store(&state)?;
        if !store
            .update_general_block_text(id, page_number, block_index, &edited)
            .map_err(internal_error)?
        {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                detail: format!("block {block_index} on page {page_number} not found"),
            });
        }
        store
            .save_general_revision(id, page_number, &req.instruction, &page_text)
            .map_err(internal_error)?;
        let status = store
            .get_general_document(id)
            .map_err(internal_error)?
            .map(|doc| doc.status)
            .unwrap_or_else(|| "unknown".to_string());
        return Ok(Json(GeneralActionResponse { id, status }));
    }
    let target_pages: Vec<_> = if req.all_pages.unwrap_or(false) {
        pages
    } else {
        let page = req.page.unwrap_or(1);
        pages
            .into_iter()
            .filter(|item| item.page_number == page)
            .collect()
    };
    if target_pages.is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "no pages selected".to_string(),
        });
    }

    let query_embedding = state.embed_for_storage(&req.instruction).await?;
    let ranked_context = rank_general_blocks(
        all_blocks,
        &req.instruction,
        query_embedding.as_deref(),
        None,
        None,
        None,
    );
    let editor = state.build_llm()?;
    for page in target_pages {
        let source = page
            .edited_text
            .as_deref()
            .or(page.ocr_text.as_deref())
            .unwrap_or("");
        let context = general_context_for_page(&ranked_context, page.page_number);
        let edited = edit_general_text(editor.as_ref(), source, &req.instruction, &context).await?;
        let store = lock_store(&state)?;
        store
            .save_general_revision(id, page.page_number, &req.instruction, &edited)
            .map_err(internal_error)?;
    }

    let status = lock_store(&state)?
        .get_general_document(id)
        .map_err(internal_error)?
        .map(|doc| doc.status)
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Json(GeneralActionResponse { id, status }))
}

async fn export_general_docx(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<RenderSaveResponse>, ApiError> {
    let (doc, pages, blocks) = general_export_payload(&state, id)?;
    let bytes = build_simple_docx(&doc.filename, &pages, &blocks)?;
    let file_path = save_export_bytes(&bytes, "docx")?;
    Ok(Json(RenderSaveResponse {
        file_path: file_path.display().to_string(),
        bytes: bytes.len(),
    }))
}

async fn export_general_pdf(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<RenderSaveResponse>, ApiError> {
    let (doc, pages, blocks) = general_export_payload(&state, id)?;
    let bytes = build_simple_pdf(&doc.filename, &pages, &blocks)?;
    let file_path = save_export_bytes(&bytes, "pdf")?;
    Ok(Json(RenderSaveResponse {
        file_path: file_path.display().to_string(),
        bytes: bytes.len(),
    }))
}

fn lock_store(state: &AppState) -> Result<std::sync::MutexGuard<'_, SqliteStore>, ApiError> {
    state.template_store.lock().map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "store lock poisoned".to_string(),
    })
}

#[derive(Debug)]
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

impl From<DocumentSummary> for DocumentSummaryResponse {
    fn from(doc: DocumentSummary) -> Self {
        Self {
            id: doc.id,
            doc_type: doc.doc_type,
            title: doc.title,
            created_at: doc.created_at,
        }
    }
}

impl From<DocumentRecord> for DocumentResponse {
    fn from(doc: DocumentRecord) -> Self {
        Self {
            id: doc.id,
            doc_type: doc.doc_type,
            title: doc.title,
            doc_data: doc.doc_json,
            created_at: doc.created_at,
        }
    }
}

impl From<GeneralDocumentSummary> for GeneralDocumentListResponse {
    fn from(doc: GeneralDocumentSummary) -> Self {
        Self {
            id: doc.id,
            filename: doc.filename,
            page_count: doc.page_count,
            status: doc.status,
            created_at: doc.created_at,
            updated_at: doc.updated_at,
        }
    }
}

impl GeneralDocumentResponse {
    fn from_parts(doc: GeneralDocumentSummary, pages: Vec<GeneralPageResponse>) -> Self {
        Self {
            id: doc.id,
            filename: doc.filename,
            page_count: doc.page_count,
            status: doc.status,
            created_at: doc.created_at,
            updated_at: doc.updated_at,
            pages,
        }
    }
}

impl From<GeneralDocumentPage> for GeneralPageResponse {
    fn from(page: GeneralDocumentPage) -> Self {
        Self {
            page_number: page.page_number,
            status: page.status,
            ocr_text: page.ocr_text,
            edited_text: page.edited_text,
            error: page.error,
            page_image_path: page.page_image_path,
            page_width: page.page_width,
            page_height: page.page_height,
            layout_warning: page.layout_warning,
            updated_at: page.updated_at,
        }
    }
}

impl From<GeneralDocumentBlock> for GeneralBlockResponse {
    fn from(block: GeneralDocumentBlock) -> Self {
        Self {
            id: block.id,
            page_number: block.page_number,
            block_index: block.block_index,
            block_type: block.block_type,
            text: block.text,
            bbox: parse_json_opt(block.bbox_json.as_deref()),
            style: parse_json_opt(block.style_json.as_deref()),
            image_path: block.image_path,
        }
    }
}

fn parse_json_opt(raw: Option<&str>) -> Option<Value> {
    raw.and_then(|text| serde_json::from_str(text).ok())
}

fn internal_error(err: anyhow::Error) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: err.to_string(),
    }
}

fn not_found(kind: &str, id: i64) -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        detail: format!("{kind} {id} not found"),
    }
}

fn ensure_general_document(store: &SqliteStore, id: i64) -> Result<(), ApiError> {
    store
        .get_general_document(id)
        .map_err(internal_error)?
        .map(|_| ())
        .ok_or_else(|| not_found("general document", id))
}

fn general_document_dir(doc: &GeneralDocumentSummary) -> Option<PathBuf> {
    let path = FsPath::new(&doc.file_path);
    let id = doc.id.to_string();
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("source") {
        return parent.parent().map(FsPath::to_path_buf);
    }
    (parent.file_name().and_then(|name| name.to_str()) == Some(id.as_str()))
        .then(|| parent.to_path_buf())
}

fn ensure_general_page_images_for_ocr(
    state: &AppState,
    id: i64,
    doc: &GeneralDocumentSummary,
) -> Result<(), ApiError> {
    if !doc.filename.to_lowercase().ends_with(".pdf") {
        return Ok(());
    }
    let pages = lock_store(state)?
        .list_general_document_pages(id)
        .map_err(internal_error)?;
    if pages.iter().all(|page| {
        page.page_image_path
            .as_deref()
            .is_some_and(|path| FsPath::new(path).exists())
    }) {
        return Ok(());
    }
    let Some(doc_dir) = FsPath::new(&doc.file_path)
        .parent()
        .and_then(FsPath::parent)
        .map(FsPath::to_path_buf)
    else {
        return Ok(());
    };
    let assets = prepare_general_page_images(
        &doc_dir,
        &doc.filename,
        FsPath::new(&doc.file_path),
        doc.page_count as usize,
    )?;
    let store = lock_store(state)?;
    for asset in assets {
        store
            .update_general_page_asset(
                id,
                asset.page_number,
                asset.path.as_deref(),
                asset.width,
                asset.height,
                asset.warning.as_deref(),
            )
            .map_err(internal_error)?;
    }
    Ok(())
}

fn rank_general_blocks(
    blocks: Vec<GeneralDocumentBlock>,
    query: &str,
    query_embedding: Option<&[f32]>,
    page_start: Option<i64>,
    page_end: Option<i64>,
    block_type: Option<&str>,
) -> Vec<(f32, GeneralDocumentBlock)> {
    let query_lower = query.to_lowercase();
    let mut hits: Vec<_> = blocks
        .into_iter()
        .filter(|block| {
            page_start.is_none_or(|start| block.page_number >= start)
                && page_end.is_none_or(|end| block.page_number <= end)
                && block_type.is_none_or(|kind| block.block_type == kind)
        })
        .filter_map(|block| {
            let semantic = query_embedding
                .zip(block.embedding.as_deref())
                .and_then(|(query, embedding)| cosine_similarity(query, embedding));
            let lexical = block
                .text
                .as_deref()
                .map(|text| lexical_score(&query_lower, text))
                .unwrap_or(0.0);
            let score = semantic.unwrap_or(0.0).max(lexical);
            (score > 0.0).then_some((score, block))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.page_number.cmp(&b.1.page_number))
            .then_with(|| a.1.block_index.cmp(&b.1.block_index))
    });
    hits
}

fn lexical_score(query_lower: &str, text: &str) -> f32 {
    let text_lower = text.to_lowercase();
    if text_lower.contains(query_lower) {
        return 1.0;
    }
    let mut matched = 0;
    let mut total = 0;
    for term in query_lower.split_whitespace() {
        total += 1;
        if text_lower.contains(term) {
            matched += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        matched as f32 / total as f32
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        None
    } else {
        Some(dot / (norm_a.sqrt() * norm_b.sqrt()))
    }
}

fn image_content_type(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

fn validate_general_file(filename: &str, bytes: &[u8]) -> Result<(), ApiError> {
    let lower = filename.to_lowercase();
    let supported = lower.ends_with(".pdf")
        || lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg");
    if !supported {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "รองรับเฉพาะ PDF, PNG, JPG, JPEG".to_string(),
        });
    }
    if bytes.is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: "uploaded file is empty".to_string(),
        });
    }
    Ok(())
}

fn estimate_page_count(filename: &str, bytes: &[u8]) -> Result<usize, ApiError> {
    if !filename.to_lowercase().ends_with(".pdf") {
        return Ok(1);
    }
    let text = String::from_utf8_lossy(bytes);
    let direct_pages = text
        .matches("/Type /Page")
        .count()
        .saturating_sub(text.matches("/Type /Pages").count());
    let page_tree_count = max_pdf_count_value(&text);
    Ok(direct_pages.max(page_tree_count).max(1))
}

fn max_pdf_count_value(text: &str) -> usize {
    let mut max_count = 0;
    let mut rest = text;
    while let Some(index) = rest.find("/Count") {
        rest = &rest[index + "/Count".len()..];
        let trimmed = rest.trim_start();
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(count) = digits.parse::<usize>() {
            max_count = max_count.max(count);
        }
    }
    max_count
}

struct PageImageAsset {
    page_number: i64,
    path: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    warning: Option<String>,
}

#[derive(Clone, Debug)]
struct DraftGeneralBlock {
    document_id: i64,
    page_number: i64,
    block_index: i64,
    block_type: String,
    text: Option<String>,
    bbox_json: Option<String>,
    style_json: Option<String>,
    image_path: Option<String>,
    embedding: Option<Vec<f32>>,
}

fn new_general_block_record(block: &DraftGeneralBlock) -> NewGeneralDocumentBlock<'_> {
    NewGeneralDocumentBlock {
        document_id: block.document_id,
        page_number: block.page_number,
        block_index: block.block_index,
        block_type: &block.block_type,
        text: block.text.as_deref(),
        bbox_json: block.bbox_json.as_deref(),
        style_json: block.style_json.as_deref(),
        image_path: block.image_path.as_deref(),
        embedding: block.embedding.as_deref(),
    }
}

async fn embed_general_blocks(
    state: &AppState,
    blocks: &mut [DraftGeneralBlock],
) -> Result<(), ApiError> {
    for block in blocks {
        let Some(text) = block
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        block.embedding = state.embed_for_storage(text).await?;
    }
    Ok(())
}

fn extract_general_blocks(
    document_id: i64,
    page_number: i64,
    text: &str,
    raw_json: &Value,
) -> Vec<DraftGeneralBlock> {
    let mut blocks = Vec::new();
    collect_layout_blocks(raw_json, &mut blocks);
    if blocks.is_empty() {
        blocks = fallback_text_blocks(text);
    }
    blocks
        .into_iter()
        .enumerate()
        .map(|(index, mut block)| {
            block.document_id = document_id;
            block.page_number = page_number;
            block.block_index = index as i64;
            block
        })
        .collect()
}

fn collect_layout_blocks(value: &Value, out: &mut Vec<DraftGeneralBlock>) {
    match value {
        Value::Array(items) => {
            if looks_like_block_array(items) {
                for item in items {
                    if let Some(block) = block_from_json(item) {
                        out.push(block);
                    }
                }
            } else {
                for item in items {
                    collect_layout_blocks(item, out);
                }
            }
        }
        Value::Object(map) => {
            for key in ["blocks", "layout", "elements", "cells", "lines"] {
                if let Some(candidate) = map.get(key) {
                    collect_layout_blocks(candidate, out);
                }
            }
        }
        _ => {}
    }
}

fn looks_like_block_array(items: &[Value]) -> bool {
    items.iter().any(|item| {
        item.get("bbox").is_some()
            || item.get("bounding_box").is_some()
            || item.get("block_type").is_some()
            || item.get("type").is_some()
    })
}

fn block_from_json(value: &Value) -> Option<DraftGeneralBlock> {
    let text = first_string(
        value,
        &["text", "natural_text", "content", "markdown", "caption"],
    );
    let image_path =
        first_string(value, &["image_path", "image"]).filter(|s| !s.starts_with("data:"));
    if text.as_deref().unwrap_or("").trim().is_empty() && image_path.is_none() {
        return None;
    }
    let block_type = first_string(value, &["block_type", "type", "kind"]).unwrap_or_else(|| {
        if image_path.is_some() {
            "image"
        } else {
            "paragraph"
        }
        .to_string()
    });
    let bbox_json = value
        .get("bbox")
        .or_else(|| value.get("bounding_box"))
        .map(Value::to_string);
    let style_json = value
        .get("style")
        .or_else(|| value.get("styles"))
        .map(Value::to_string);
    Some(DraftGeneralBlock {
        document_id: 0,
        page_number: 0,
        block_index: 0,
        block_type,
        text,
        bbox_json,
        style_json,
        image_path,
        embedding: None,
    })
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn fallback_text_blocks(text: &str) -> Vec<DraftGeneralBlock> {
    text.split("\n\n")
        .flat_map(|chunk| {
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else if trimmed.len() > 1200 {
                trimmed
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            } else {
                vec![trimmed.to_string()]
            }
        })
        .enumerate()
        .map(|(index, text)| DraftGeneralBlock {
            document_id: 0,
            page_number: 0,
            block_index: index as i64,
            block_type: infer_block_type(&text),
            text: Some(text),
            bbox_json: None,
            style_json: None,
            image_path: None,
            embedding: None,
        })
        .collect()
}

fn infer_block_type(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with('|') {
        "table".to_string()
    } else if trimmed.len() < 120 && !trimmed.contains('.') && !trimmed.contains('\n') {
        "heading".to_string()
    } else {
        "paragraph".to_string()
    }
}

fn prepare_general_page_images(
    doc_dir: &FsPath,
    filename: &str,
    source_path: &FsPath,
    page_count: usize,
) -> Result<Vec<PageImageAsset>, ApiError> {
    let pages_dir = doc_dir.join("pages");
    std::fs::create_dir_all(&pages_dir).map_err(io_error)?;
    let lower = filename.to_lowercase();
    if !lower.ends_with(".pdf") {
        let extension = if lower.ends_with(".png") {
            "png"
        } else {
            "jpg"
        };
        let page_path = pages_dir.join(format!("page-001.{extension}"));
        std::fs::copy(source_path, &page_path).map_err(io_error)?;
        return Ok(vec![PageImageAsset {
            page_number: 1,
            path: Some(page_path.display().to_string()),
            width: None,
            height: None,
            warning: None,
        }]);
    }

    if python_fitz_available() && render_pdf_pages_with_fitz(source_path, &pages_dir, page_count)? {
        return Ok((1..=page_count)
            .map(|page| PageImageAsset {
                page_number: page as i64,
                path: Some(
                    pages_dir
                        .join(format!("page-{page:03}.png"))
                        .display()
                        .to_string(),
                ),
                width: None,
                height: None,
                warning: None,
            })
            .collect());
    }

    if command_exists("pdftoppm") {
        let prefix = pages_dir.join("rendered-page");
        let output = Command::new("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg("150")
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg(page_count.to_string())
            .arg(source_path)
            .arg(&prefix)
            .output()
            .map_err(io_error)?;
        if output.status.success() {
            let mut assets = Vec::new();
            for page in 1..=page_count {
                let rendered = pages_dir.join(format!("rendered-page-{page}.png"));
                let target = pages_dir.join(format!("page-{page:03}.png"));
                if rendered.exists() {
                    std::fs::rename(&rendered, &target).map_err(io_error)?;
                    assets.push(PageImageAsset {
                        page_number: page as i64,
                        path: Some(target.display().to_string()),
                        width: None,
                        height: None,
                        warning: None,
                    });
                } else {
                    assets.push(PageImageAsset {
                        page_number: page as i64,
                        path: None,
                        width: None,
                        height: None,
                        warning: Some(
                            "PDF page image render did not produce this page".to_string(),
                        ),
                    });
                }
            }
            return Ok(assets);
        }
    }

    if command_exists("qlmanage") {
        if let Some(first_page) = render_quicklook_preview(source_path, &pages_dir)? {
            let mut assets = vec![PageImageAsset {
                page_number: 1,
                path: Some(first_page.display().to_string()),
                width: None,
                height: None,
                warning: if page_count > 1 {
                    Some(
                        "QuickLook rendered only the first page; install pdftoppm for all page previews"
                            .to_string(),
                    )
                } else {
                    None
                },
            }];
            assets.extend((2..=page_count).map(|page| PageImageAsset {
                page_number: page as i64,
                path: None,
                width: None,
                height: None,
                warning: Some(
                    "PDF page image rendering for this page requires pdftoppm".to_string(),
                ),
            }));
            return Ok(assets);
        }
    }

    Ok((1..=page_count)
        .map(|page| PageImageAsset {
            page_number: page as i64,
            path: None,
            width: None,
            height: None,
            warning: Some(
                "PDF page image rendering requires pdftoppm; using text fallback".to_string(),
            ),
        })
        .collect())
}

fn render_quicklook_preview(
    source_path: &FsPath,
    pages_dir: &FsPath,
) -> Result<Option<PathBuf>, ApiError> {
    let before = list_png_files(pages_dir)?;
    let output = Command::new("qlmanage")
        .arg("-t")
        .arg("-s")
        .arg("1200")
        .arg("-o")
        .arg(pages_dir)
        .arg(source_path)
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Ok(None);
    }
    let after = list_png_files(pages_dir)?;
    let generated = after.into_iter().find(|path| !before.contains(path));
    let Some(generated) = generated else {
        return Ok(None);
    };
    let target = pages_dir.join("page-001.png");
    if generated != target {
        std::fs::rename(&generated, &target).map_err(io_error)?;
    }
    Ok(Some(target))
}

fn python_fitz_available() -> bool {
    Command::new("python3")
        .arg("-c")
        .arg("import fitz")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn render_pdf_pages_with_fitz(
    source_path: &FsPath,
    pages_dir: &FsPath,
    page_count: usize,
) -> Result<bool, ApiError> {
    let script = r#"
import pathlib
import sys
import fitz

source = sys.argv[1]
out_dir = pathlib.Path(sys.argv[2])
max_pages = int(sys.argv[3])
doc = fitz.open(source)
pages = min(len(doc), max_pages)
for index in range(pages):
    page = doc.load_page(index)
    pix = page.get_pixmap(matrix=fitz.Matrix(2, 2), alpha=False)
    pix.save(out_dir / f"page-{index + 1:03}.png")
print(pages)
"#;
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(source_path)
        .arg(pages_dir)
        .arg(page_count.to_string())
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Ok(false);
    }
    let rendered = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .unwrap_or(0);
    Ok(rendered >= page_count)
}

fn list_png_files(dir: &FsPath) -> Result<std::collections::HashSet<PathBuf>, ApiError> {
    let mut files = std::collections::HashSet::new();
    for entry in std::fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            files.insert(path);
        }
    }
    Ok(files)
}

fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn edit_general_text(
    editor: &dyn LlmProvider,
    text: &str,
    instruction: &str,
    rag_context: &str,
) -> Result<String, ApiError> {
    let prompt = format!(
        "คำสั่ง: {instruction}\n\nบริบทจาก block-level RAG (ใช้เพื่อเข้าใจตำแหน่ง/ตาราง/ย่อหน้า ห้ามคัด metadata ลงผลลัพธ์):\n{rag_context}\n\nข้อความต้นฉบับของหน้าที่ต้องแก้:\n{text}\n\nส่งกลับเฉพาะข้อความที่แก้แล้ว รักษาโครงย่อหน้า หัวข้อ ตาราง markdown และลำดับบรรทัดเดิมให้มากที่สุด"
    );
    editor
        .complete(
            "คุณคือผู้ช่วยตรวจคำผิดและจัดรูปแบบเอกสารทั่วไป รักษา layout เชิงข้อความให้มากที่สุด",
            &prompt,
            4096,
        )
        .await
        .map_err(|err| ApiError {
            status: StatusCode::BAD_GATEWAY,
            detail: format!("general document edit failed: {err}"),
        })
}

fn general_context_for_page(hits: &[(f32, GeneralDocumentBlock)], page_number: i64) -> String {
    let mut lines = Vec::new();
    for (score, block) in hits
        .iter()
        .filter(|(_, block)| block.page_number == page_number)
        .take(8)
    {
        let snippet = block
            .text
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(500)
            .collect::<String>();
        let bbox = block.bbox_json.as_deref().unwrap_or("-");
        lines.push(format!(
            "page={} block={} type={} score={score:.3} bbox={} text={}",
            block.page_number, block.block_index, block.block_type, bbox, snippet
        ));
    }
    if lines.is_empty() {
        "(ไม่มี block context ที่ match โดยตรง ใช้ข้อความหน้าปัจจุบันเป็นหลัก)".to_string()
    } else {
        lines.join("\n")
    }
}

fn general_context_for_selected_block(block: &GeneralDocumentBlock) -> String {
    format!(
        "page={} block={} type={} bbox={} text={}",
        block.page_number,
        block.block_index,
        block.block_type,
        block.bbox_json.as_deref().unwrap_or("-"),
        block.text.as_deref().unwrap_or("")
    )
}

fn general_export_payload(
    state: &AppState,
    id: i64,
) -> Result<
    (
        GeneralDocumentSummary,
        Vec<GeneralDocumentPage>,
        Vec<GeneralDocumentBlock>,
    ),
    ApiError,
> {
    let store = lock_store(state)?;
    let doc = store
        .get_general_document(id)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("general document", id))?;
    let pages = store
        .list_general_document_pages(id)
        .map_err(internal_error)?;
    let blocks = store
        .list_general_document_blocks(id, None)
        .map_err(internal_error)?;
    Ok((doc, pages, blocks))
}

fn page_text(page: &GeneralDocumentPage) -> String {
    page.edited_text
        .clone()
        .or_else(|| page.ocr_text.clone())
        .unwrap_or_else(|| format!("[หน้า {} ยังไม่มีข้อความ OCR]", page.page_number))
}

fn page_text_from_blocks(blocks: &[GeneralDocumentBlock]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let mut ordered = blocks.to_vec();
    ordered.sort_by_key(|block| block.block_index);
    let text = ordered
        .into_iter()
        .filter_map(|block| block.text)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn save_export_bytes(bytes: &[u8], extension: &str) -> Result<PathBuf, ApiError> {
    let dir = render_exports_dir();
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut path = dir.join(format!("general-document-{millis}.{extension}"));
    let mut counter = 2;
    while path.exists() {
        path = dir.join(format!("general-document-{millis}-{counter}.{extension}"));
        counter += 1;
    }
    std::fs::write(&path, bytes).map_err(io_error)?;
    Ok(path)
}

fn build_simple_docx(
    filename: &str,
    pages: &[GeneralDocumentPage],
    blocks: &[GeneralDocumentBlock],
) -> Result<Vec<u8>, ApiError> {
    let base = std::env::temp_dir().join(format!(
        "govdoc-docx-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let word_dir = base.join("word");
    let rels_dir = base.join("_rels");
    std::fs::create_dir_all(&word_dir).map_err(io_error)?;
    std::fs::create_dir_all(&rels_dir).map_err(io_error)?;
    std::fs::write(
        base.join("[Content_Types].xml"),
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
    )
    .map_err(io_error)?;
    std::fs::write(
        rels_dir.join(".rels"),
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
    )
    .map_err(io_error)?;

    let mut body = format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(filename));
    for page in pages {
        body.push_str(&format!(
            r#"<w:p><w:r><w:br w:type="page"/><w:t>หน้า {}</w:t></w:r></w:p>"#,
            page.page_number
        ));
        let page_blocks = blocks_for_page(blocks, page.page_number);
        if page_blocks.is_empty() {
            for line in page_text(page).lines() {
                body.push_str(&docx_paragraph(line, false));
            }
        } else {
            for block in page_blocks {
                let text = block.text.as_deref().unwrap_or("").trim();
                if text.is_empty() {
                    continue;
                }
                match block.block_type.as_str() {
                    "heading" => body.push_str(&docx_paragraph(text, true)),
                    "table" | "table_cell" => body.push_str(&docx_table_or_paragraph(text)),
                    _ => {
                        for line in text.lines() {
                            body.push_str(&docx_paragraph(line, false));
                        }
                    }
                }
            }
        }
    }
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    std::fs::write(word_dir.join("document.xml"), document_xml).map_err(io_error)?;

    let output = Command::new("/usr/bin/zip")
        .arg("-qr")
        .arg("out.docx")
        .arg("[Content_Types].xml")
        .arg("_rels")
        .arg("word")
        .current_dir(&base)
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: "failed to package DOCX".to_string(),
        });
    }
    let bytes = std::fs::read(base.join("out.docx")).map_err(io_error)?;
    let _ = std::fs::remove_dir_all(base);
    Ok(bytes)
}

fn blocks_for_page(blocks: &[GeneralDocumentBlock], page_number: i64) -> Vec<GeneralDocumentBlock> {
    let mut page_blocks: Vec<_> = blocks
        .iter()
        .filter(|block| block.page_number == page_number)
        .cloned()
        .collect();
    page_blocks.sort_by_key(|block| block.block_index);
    page_blocks
}

fn docx_paragraph(text: &str, heading: bool) -> String {
    if heading {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:b/><w:t>{}</w:t></w:r></w:p>"#,
            xml_escape(text)
        )
    } else {
        format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(text))
    }
}

fn docx_table_or_paragraph(text: &str) -> String {
    let rows: Vec<Vec<String>> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|') && line.ends_with('|'))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|cells| {
            !cells.is_empty()
                && !cells.iter().all(|cell| {
                    cell.chars()
                        .all(|c| c == '-' || c == ':' || c.is_whitespace())
                })
        })
        .collect();
    if rows.is_empty() {
        return docx_paragraph(text, false);
    }
    let mut xml = String::from("<w:tbl>");
    for row in rows {
        xml.push_str("<w:tr>");
        for cell in row {
            xml.push_str(&format!(
                "<w:tc><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc>",
                xml_escape(&cell)
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl>");
    xml
}

fn build_simple_pdf(
    filename: &str,
    pages: &[GeneralDocumentPage],
    blocks: &[GeneralDocumentBlock],
) -> Result<Vec<u8>, ApiError> {
    if let Some(bytes) = build_image_background_pdf(pages, blocks)? {
        return Ok(bytes);
    }
    let font_obj = 3 + (pages.len() * 2);
    let kids = pages
        .iter()
        .enumerate()
        .map(|(index, _)| format!("{} 0 R", 3 + (index * 2)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()),
    ];
    for page in pages {
        let content = pdf_page_content(filename, page, &blocks_for_page(blocks, page.page_number));
        let content_obj = objects.len() + 2;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 {font_obj} 0 R >> >> /Contents {content_obj} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}endstream",
            content.len(),
            content
        ));
    }
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let xref_at = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    Ok(pdf.into_bytes())
}

fn build_image_background_pdf(
    pages: &[GeneralDocumentPage],
    blocks: &[GeneralDocumentBlock],
) -> Result<Option<Vec<u8>>, ApiError> {
    if !command_exists("magick") || pages.iter().any(|page| page.page_image_path.is_none()) {
        return Ok(None);
    }
    let base = std::env::temp_dir().join(format!(
        "govdoc-pdf-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).map_err(io_error)?;
    let mut rendered_pages = Vec::new();
    for page in pages {
        let output_path = base.join(format!("page-{:03}.png", page.page_number));
        let Some(image_path) = page.page_image_path.as_deref() else {
            return Ok(None);
        };
        let mut command = Command::new("magick");
        command
            .arg(image_path)
            .arg("-resize")
            .arg("595x842!")
            .arg("-font")
            .arg("Helvetica")
            .arg("-pointsize")
            .arg("10")
            .arg("-fill")
            .arg("black");
        for block in blocks_for_page(blocks, page.page_number) {
            let Some(text) = block
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            let Some((x, y, _, _)) = parse_bbox(block.bbox_json.as_deref()) else {
                continue;
            };
            let page_width = page.page_width.unwrap_or(595).max(1) as f32;
            let page_height = page.page_height.unwrap_or(842).max(1) as f32;
            let draw_x = (595.0 * (x / page_width)).clamp(0.0, 570.0);
            let draw_y = (842.0 * (y / page_height)).clamp(0.0, 820.0);
            command
                .arg("-annotate")
                .arg(format!("+{draw_x:.0}+{draw_y:.0}"))
                .arg(
                    text.lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(180)
                        .collect::<String>(),
                );
        }
        let status = command.arg(&output_path).status().map_err(io_error)?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&base);
            return Ok(None);
        }
        rendered_pages.push(output_path);
    }
    let output_pdf = base.join("out.pdf");
    let status = Command::new("magick")
        .args(&rendered_pages)
        .arg(&output_pdf)
        .status()
        .map_err(io_error)?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&base);
        return Ok(None);
    }
    let bytes = std::fs::read(&output_pdf).map_err(io_error)?;
    let _ = std::fs::remove_dir_all(&base);
    Ok(Some(bytes))
}

fn pdf_page_content(
    filename: &str,
    page: &GeneralDocumentPage,
    blocks: &[GeneralDocumentBlock],
) -> String {
    let mut content = format!(
        "BT /F1 12 Tf 50 805 Td ({}) Tj ET\nBT /F1 9 Tf 50 790 Td (Page {}) Tj ET\n",
        pdf_escape(filename),
        page.page_number
    );
    let page_width = page.page_width.unwrap_or(595).max(1) as f32;
    let page_height = page.page_height.unwrap_or(842).max(1) as f32;
    let mut used_bbox = false;
    for block in blocks {
        let Some(text) = block
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        let Some((x, y, _, _)) = parse_bbox(block.bbox_json.as_deref()) else {
            continue;
        };
        used_bbox = true;
        let pdf_x = 595.0 * (x / page_width);
        let pdf_y = 842.0 - (842.0 * (y / page_height));
        for (line_index, line) in text.lines().take(6).enumerate() {
            let line_y = (pdf_y - (line_index as f32 * 12.0)).clamp(40.0, 810.0);
            content.push_str(&format!(
                "BT /F1 9 Tf {:.1} {:.1} Td ({}) Tj ET\n",
                pdf_x.clamp(20.0, 560.0),
                line_y,
                pdf_escape(line)
            ));
        }
    }
    if !used_bbox {
        let mut y = 760;
        for line in page_text(page).lines().take(42) {
            content.push_str(&format!(
                "BT /F1 10 Tf 50 {y} Td ({}) Tj ET\n",
                pdf_escape(line)
            ));
            y -= 16;
            if y < 50 {
                break;
            }
        }
    }
    content
}

fn parse_bbox(raw: Option<&str>) -> Option<(f32, f32, f32, f32)> {
    let value = serde_json::from_str::<Value>(raw?).ok()?;
    if let Some(items) = value.as_array() {
        if items.len() >= 4 {
            let x = items[0].as_f64()? as f32;
            let y = items[1].as_f64()? as f32;
            let third = items[2].as_f64()? as f32;
            let fourth = items[3].as_f64()? as f32;
            let width = if third > x { third - x } else { third };
            let height = if fourth > y { fourth - y } else { fourth };
            return Some((x, y, width, height));
        }
    }
    let object = value.as_object()?;
    let x = object
        .get("x")
        .or_else(|| object.get("left"))
        .or_else(|| object.get("x1"))?
        .as_f64()? as f32;
    let y = object
        .get("y")
        .or_else(|| object.get("top"))
        .or_else(|| object.get("y1"))?
        .as_f64()? as f32;
    let width = object
        .get("width")
        .and_then(Value::as_f64)
        .map(|v| v as f32);
    let height = object
        .get("height")
        .and_then(Value::as_f64)
        .map(|v| v as f32);
    match (width, height) {
        (Some(width), Some(height)) => Some((x, y, width, height)),
        _ => {
            let right = object.get("right").or_else(|| object.get("x2"))?.as_f64()? as f32;
            let bottom = object
                .get("bottom")
                .or_else(|| object.get("y2"))?
                .as_f64()? as f32;
            Some((x, y, right - x, bottom - y))
        }
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn pdf_escape(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
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

fn save_docx_to_disk(docx: &[u8]) -> Result<PathBuf, ApiError> {
    let dir = render_exports_dir();
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let path = unique_export_path(&dir);
    std::fs::write(&path, docx).map_err(io_error)?;
    Ok(path)
}

fn render_exports_dir() -> PathBuf {
    if let Some(path) = optional_env("GOVDOC_EXPORTS_DIR") {
        return PathBuf::from(path);
    }
    if let Some(home) = optional_env("HOME") {
        let downloads = PathBuf::from(home).join("Downloads");
        if downloads.exists() {
            return downloads;
        }
    }
    PathBuf::from("app-data/exports")
}

fn unique_export_path(dir: &FsPath) -> PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let base = format!("govdoc-{millis}");
    let mut path = dir.join(format!("{base}.docx"));
    let mut counter = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{counter}.docx"));
        counter += 1;
    }
    path
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

    #[test]
    fn sanitize_filename_keeps_base_name_only() {
        assert_eq!(
            sanitize_filename("/etc/../หนังสือ ภายนอก.docx"),
            "หนังสือ_ภายนอก.docx"
        );
        assert_eq!(sanitize_filename("a\\b\\c.docx"), "c.docx");
        assert_eq!(sanitize_filename(""), "template.docx");
    }

    #[test]
    fn estimates_general_pdf_page_count() {
        let mut pdf = b"%PDF-1.4 /Type /Pages".to_vec();
        for _ in 0..20 {
            pdf.extend_from_slice(b" /Type /Page ");
        }
        assert_eq!(estimate_page_count("manual.pdf", &pdf).unwrap(), 20);

        pdf.extend_from_slice(b" /Type /Page ");
        assert!(estimate_page_count("manual.pdf", &pdf).unwrap() > GENERAL_DOC_MAX_PAGES);
        assert_eq!(estimate_page_count("scan.png", b"image").unwrap(), 1);
        assert_eq!(
            estimate_page_count("compressed.pdf", b"%PDF /Type /Pages /Count 7").unwrap(),
            7
        );
    }

    #[test]
    fn general_export_builders_return_files() {
        let pages = vec![GeneralDocumentPage {
            id: 1,
            document_id: 1,
            page_number: 1,
            status: "succeeded".to_string(),
            ocr_text: Some("Heading\nBody text".to_string()),
            edited_text: None,
            error: None,
            page_image_path: None,
            ocr_raw_json: None,
            page_width: None,
            page_height: None,
            layout_warning: None,
            updated_at: "now".to_string(),
        }];
        let blocks = vec![GeneralDocumentBlock {
            id: 1,
            document_id: 1,
            page_number: 1,
            block_index: 0,
            block_type: "heading".to_string(),
            text: Some("Heading".to_string()),
            bbox_json: Some("[50,50,200,80]".to_string()),
            style_json: None,
            image_path: None,
            embedding: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }];
        let docx = build_simple_docx("manual.pdf", &pages, &blocks).unwrap();
        let pdf = build_simple_pdf("manual.pdf", &pages, &blocks).unwrap();
        assert!(docx.starts_with(b"PK"));
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[tokio::test]
    async fn status_reports_default_backends() {
        let app = router(AppState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["llm"]["backend"], "fake");
        assert_eq!(json["embedding"]["backend"], "fake");
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
    async fn documents_save_list_get_delete() {
        let app = router(AppState::default());

        // save
        let save = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/documents")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"doc_type":"ภายนอก","title":"ขอเชิญประชุม","doc_data":{"subject":"ขอเชิญประชุม"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);
        let body = to_bytes(save.into_body(), usize::MAX).await.unwrap();
        let id = serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_i64()
            .unwrap();

        // list
        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/documents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(list.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json[0]["title"], "ขอเชิญประชุม");

        // get
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/documents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(get.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["doc_data"]["subject"], "ขอเชิญประชุม");

        // update
        let update = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/documents/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"doc_type":"ภายนอก","title":"ขอเชิญประชุมฉบับแก้ไข","doc_data":{"subject":"ขอเชิญประชุมฉบับแก้ไข"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);

        let get_updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/documents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(get_updated.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["title"], "ขอเชิญประชุมฉบับแก้ไข");
        assert_eq!(json["doc_data"]["subject"], "ขอเชิญประชุมฉบับแก้ไข");

        // delete
        let del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/documents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::NO_CONTENT);

        // gone
        let missing = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/documents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn render_requires_configured_sidecar() {
        let state = AppState {
            renderer_cmd: None,
            ..AppState::default()
        };
        let app = router(state);
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
