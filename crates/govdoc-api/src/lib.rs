mod mock;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use govdoc_domain::DocRequest;
use govdoc_usecases::{generate_document_json, GenerationOptions, GenerationServices, TraceEvent};
use serde::Serialize;
use serde_json::Value;

use crate::mock::{EmptyMemoryRepository, FakeEmbeddingProvider, FakeLlmProvider};

#[derive(Clone, Debug)]
pub struct AppState {
    pub app_name: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            app_name: "govdoc-generator-rust".to_string(),
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
        .with_state(state)
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<HealthResponse> {
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
}
