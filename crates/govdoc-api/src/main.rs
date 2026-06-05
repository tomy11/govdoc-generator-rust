use govdoc_api::{router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = router(AppState::default());
    let addr = std::env::var("GOVDOC_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("govdoc-api listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
