use axum::{routing::get, Json, Router};
use serde::Serialize;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn builds_router() {
        let _router = router(AppState::default());
    }
}

