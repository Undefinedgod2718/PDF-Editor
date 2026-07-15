//! stdio entry point for the PDF Editor MCP server.
//!
//! This process speaks MCP over stdin/stdout to whatever launched it (e.g.
//! Claude Code via `claude mcp add`). All logging therefore goes to stderr —
//! anything written to stdout that isn't a JSON-RPC message would corrupt the
//! protocol stream.
//!
//! No PDF logic lives here or anywhere in this crate: every tool call is
//! forwarded over HTTP to the PDF Editor backend (see `tools.rs`). The
//! backend must already be running; see wiki/MCP.md and
//! wiki/ADR-001-MCP.md for the design.

mod tools;

use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

use tools::PdfEditorTools;

/// Default backend base URL, matching `server`'s default `PDF_EDITOR_PORT`.
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8050";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let base_url = std::env::var("PDF_EDITOR_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    tracing::info!(base_url, "starting pdf-editor MCP server (stdio)");

    let service = PdfEditorTools::new(base_url)
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {e:?}");
        })?;

    service.waiting().await?;
    Ok(())
}
