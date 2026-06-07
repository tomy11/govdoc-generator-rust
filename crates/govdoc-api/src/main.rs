use govdoc_api::{maybe_start_local_llm, router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env (if present) before any std::env::var reads. Real shell env
    // still wins over .env values.
    let _ = dotenvy::dotenv();
    maybe_start_local_llm().await?;
    let app = router(AppState::default());
    let addr = std::env::var("GOVDOC_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("govdoc-api listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
