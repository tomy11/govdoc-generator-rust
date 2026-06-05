mod mock;

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use govdoc_domain::{DocRequest, EditRequest};
use govdoc_storage::{NewTemplateRecord, SqliteStore, TemplateRecord};
use govdoc_usecases::{
    edit_document_json, generate_document_json, GenerationOptions, GenerationServices, TraceEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mock::{EmptyMemoryRepository, FakeEmbeddingProvider, FakeLlmProvider};

#[derive(Clone)]
pub struct AppState {
    pub app_name: String,
    template_store: Arc<Mutex<SqliteStore>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            app_name: "govdoc-generator-rust".to_string(),
            template_store: Arc::new(Mutex::new(
                SqliteStore::open_memory().expect("in-memory SQLite store should open"),
            )),
        }
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    app: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/generate", post(generate))
        .route("/edit", post(edit))
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

#[derive(Debug, Serialize)]
struct GenerateResponse {
    doc: Value,
    trace: Vec<TraceEvent>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    detail: String,
}

async fn generate(Json(req): Json<DocRequest>) -> Result<Json<GenerateResponse>, ApiError> {
    let generator = FakeLlmProvider;
    let critic = FakeLlmProvider;
    let memory_repo = EmptyMemoryRepository;
    let embedding_provider = FakeEmbeddingProvider;
    let mut trace = Vec::new();

    let doc = generate_document_json(
        &req,
        GenerationServices {
            generator: &generator,
            critic: &critic,
            memory_repo: &memory_repo,
            embedding_provider: &embedding_provider,
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

async fn edit(Json(req): Json<EditRequest>) -> Result<Json<Value>, ApiError> {
    let editor = FakeLlmProvider;
    let edited = edit_document_json(
        req.doc_data,
        &req.edit_instructions,
        &editor,
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
}
