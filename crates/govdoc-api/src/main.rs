use govdoc_api::{router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = router(AppState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

