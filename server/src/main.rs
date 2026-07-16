mod api;
mod llm;
mod pdf;
mod sidecar;
mod storage;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

/// PDF / stamp / image uploads — Axum's default 2 MiB limit rejects typical PDFs
/// with `Error parsing multipart/form-data request`.
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;

pub struct AppState {
    pub storage: storage::Storage,
    pub engine: pdf::engine::PdfEngine,
}

pub type SharedState = Arc<AppState>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let data_dir = std::env::var("PDF_EDITOR_DATA").unwrap_or_else(|_| "data".into());
    let storage = storage::Storage::new(&data_dir)?;
    let engine = pdf::engine::PdfEngine::spawn()?;

    let state: SharedState = Arc::new(AppState { storage, engine });

    // Boot-time detection: a missing sidecar otherwise only surfaces as a 500
    // on the first docx/xlsx export. ERROR level so log watchers catch it.
    match sidecar::health() {
        Ok((python, script)) => tracing::info!(
            "office sidecar ready: {} {}",
            python.display(),
            script.display()
        ),
        Err(e) => tracing::error!("office sidecar UNAVAILABLE — docx/xlsx export will fail: {e}"),
    }

    let web_dist = std::env::var("PDF_EDITOR_WEB").unwrap_or_else(|_| "../web/dist".into());
    let index = format!("{web_dist}/index.html");
    let static_files = ServeDir::new(&web_dist).fallback(ServeFile::new(&index));

    let app = api::router()
        .with_state(state)
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(CorsLayer::permissive());

    let port: u16 = std::env::var("PDF_EDITOR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8050);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("PDF Editor server listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
