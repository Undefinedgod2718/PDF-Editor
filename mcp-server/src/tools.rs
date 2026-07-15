//! MCP tool definitions: one `#[tool]` method per wrapped PDF Editor HTTP
//! endpoint (see wiki/ADR-001-MCP.md §5 for the v1 list). No PDF engine
//! logic lives here or is linked in — every tool is a thin HTTP forward to
//! `base_url` (default `http://127.0.0.1:8050`, overridable via the
//! `PDF_EDITOR_URL` env var read in `main.rs`).
//!
//! Conventions applied uniformly below (ADR-001 §2-4):
//! - Page indices are 0-based everywhere, matching the HTTP API. Every tool
//!   description that takes a page index says so.
//! - Large payloads never travel as base64: `upload_pdf` takes a local file
//!   path; `render_page` / `save_pdf` / `export_pages` / `convert_to_office`
//!   write their result to a caller-supplied `output_path` and return a
//!   small JSON receipt (`{"path": ..., "bytes": ...}`).
//! - Small JSON payloads (info/text/search, and write-op responses) are
//!   returned to the caller pretty-printed, verbatim from the backend, so
//!   new ids / revision bumps are visible.
//! - HTTP 4xx -> tool error whose message is the backend's response body
//!   text as-is. HTTP 5xx / connection failure -> tool error prefixed
//!   `backend error:`; connection refused additionally calls out that the
//!   PDF Editor server may not be running (default port 8050).
//! - No tool ever panics; all failure paths return `Result<_, McpError>`.

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde_json::Value;

/// Timeout for ordinary read/write calls (everything except office export).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for raster/pptx export: generous headroom over typical render
/// time for a many-page document.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(90);
/// Timeout for docx/xlsx conversion, which runs through the Python sidecar.
/// The sidecar itself caps a single conversion at 300s (see
/// `server/src/sidecar.rs`), so this must stay comfortably above that.
const OFFICE_TIMEOUT: Duration = Duration::from_secs(330);

#[derive(Clone)]
pub struct PdfEditorTools {
    client: reqwest::Client,
    base_url: String,
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

// ---------------------------------------------------------------------
// Tool argument structs. Doc comments become JSON Schema `description`
// fields shown to the calling LLM.
// ---------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UploadPdfArgs {
    /// Local filesystem path to a PDF file (MCP server and backend share a
    /// filesystem, so the file is read from disk here, not base64-encoded).
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DocumentIdArgs {
    /// Document id, as returned by upload_pdf / merge_documents / extract_pages.
    pub id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PageTextArgs {
    /// Document id.
    pub id: String,
    /// 0-based page index.
    pub page: u16,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchTextArgs {
    /// Document id.
    pub id: String,
    /// Case-insensitive substring to search for across all pages.
    pub q: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RenderPageArgs {
    /// Document id.
    pub id: String,
    /// 0-based page index.
    pub page: u16,
    /// Absolute path to write the rendered PNG to; parent directory must
    /// already exist.
    pub output_path: String,
    /// Pixels per PDF point (1.0 = 72 dpi). Server default 1.5, clamped
    /// server-side to 0.1..=8.0.
    #[serde(default)]
    pub scale: Option<f32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SavePdfArgs {
    /// Document id.
    pub id: String,
    /// Absolute path to write the document's current PDF bytes to.
    pub output_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RotatePageArgs {
    /// Document id.
    pub id: String,
    /// 0-based page index.
    pub page: u16,
    /// Absolute rotation in degrees: one of 0, 90, 180, 270.
    pub degrees: u16,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeletePageArgs {
    /// Document id.
    pub id: String,
    /// 0-based page index.
    pub page: u16,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReorderPagesArgs {
    /// Document id.
    pub id: String,
    /// New page order: a permutation of 0..page_count, i.e. `order[i]` is
    /// the 0-based source page index that should end up at position `i`.
    pub order: Vec<u16>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct CropRectArg {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CropPagesArgs {
    /// Document id.
    pub id: String,
    /// 0-based page indices to crop.
    pub pages: Vec<u16>,
    /// View-space crop rectangle in points (origin top-left of the rendered
    /// page, as the user currently sees it). Omit or pass null to reset the
    /// crop back to the full page.
    #[serde(default)]
    pub rect: Option<CropRectArg>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ResizePagesArgs {
    /// Document id.
    pub id: String,
    /// 0-based page indices to resize.
    pub pages: Vec<u16>,
    /// Target width in points, in display orientation.
    pub width: f32,
    /// Target height in points, in display orientation.
    pub height: f32,
    /// "scale" (scale page content to fit the new size, uniform, centered)
    /// or "canvas" (change only the page box; content keeps its size,
    /// centered on the new canvas).
    pub mode: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MergeDocumentsArgs {
    /// Document ids to merge, in order (at least two).
    pub ids: Vec<String>,
    /// Filename for the resulting merged document. Defaults to "merged.pdf".
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtractPagesArgs {
    /// Document id to extract pages from.
    pub id: String,
    /// 0-based page indices to extract, in the given order (may repeat).
    /// Calling this once per desired output also implements "split".
    pub pages: Vec<u16>,
    /// Filename for the resulting document. Defaults to "extract_<original filename>".
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct InsertPagesFromArgs {
    /// Destination document id (this one gets mutated).
    pub id: String,
    /// Source document id to copy pages from; may equal `id` to duplicate
    /// pages within the same document.
    pub source_id: String,
    /// 0-based page indices in the source document, inserted in this order.
    pub pages: Vec<u16>,
    /// 0-based insert position in the destination document; equal to the
    /// destination's current page count to append at the end.
    pub at: u16,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CompressPdfArgs {
    /// Document id to compress.
    pub id: String,
    /// One of "screen" (72dpi/60), "ebook" (150dpi/75), "printer" (300dpi/85),
    /// or "custom" (use `dpi`/`quality`).
    pub preset: String,
    /// Custom preset only: target image DPI, clamped to 36..=600.
    #[serde(default)]
    pub dpi: Option<f32>,
    /// Custom preset only: JPEG quality, clamped to 10..=100.
    #[serde(default)]
    pub quality: Option<u8>,
    /// Filename for the resulting document. Defaults to "compressed_<original filename>".
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExportPagesArgs {
    /// Document id to export pages from.
    pub id: String,
    /// One of "png", "jpg", "tiff", "pptx".
    pub format: String,
    /// Absolute path to write the exported file to; parent directory must
    /// already exist.
    pub output_path: String,
    /// 0-based page indices to export. Omit or leave empty for all pages.
    #[serde(default)]
    pub pages: Vec<u16>,
    /// Raster DPI, clamped to 72..=600. Server default 150.
    #[serde(default)]
    pub dpi: Option<u32>,
    /// JPEG quality, clamped to 10..=100 (jpg format only). Server default 85.
    #[serde(default)]
    pub quality: Option<u8>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConvertToOfficeArgs {
    /// Document id to convert.
    pub id: String,
    /// One of "docx", "xlsx".
    pub format: String,
    /// Absolute path to write the converted file to; parent directory must
    /// already exist.
    pub output_path: String,
    /// 0-based page indices to include. Omit or leave empty for all pages.
    #[serde(default)]
    pub pages: Vec<u16>,
}

// ---------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------

impl PdfEditorTools {
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Turn a transport-level failure (connection refused, timeout, DNS,
    /// TLS, etc.) into a tool error per ADR-001 §4.
    fn map_transport_err(&self, e: reqwest::Error) -> McpError {
        if e.is_connect() {
            McpError::internal_error(
                format!(
                    "backend error: connection refused to {} — PDF Editor server not running (default port 8050)",
                    self.base_url
                ),
                None,
            )
        } else if e.is_timeout() {
            McpError::internal_error(
                format!("backend error: request to {} timed out", self.base_url),
                None,
            )
        } else {
            McpError::internal_error(format!("backend error: {e}"), None)
        }
    }

    /// Send a request and classify the HTTP-level outcome per ADR-001 §4:
    /// 4xx becomes a tool error with the backend body verbatim; 5xx becomes
    /// a tool error prefixed `backend error:`. On success the still-unread
    /// `Response` is handed back so the caller can consume JSON or bytes.
    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, McpError> {
        let resp = req.send().await.map_err(|e| self.map_transport_err(e))?;
        let status = resp.status();
        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(McpError::invalid_params(body, None));
        }
        if status.is_server_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(McpError::internal_error(format!("backend error: {body}"), None));
        }
        Ok(resp)
    }

    async fn get_json(&self, path: &str) -> Result<Value, McpError> {
        let req = self.client.get(self.url(path)).timeout(DEFAULT_TIMEOUT);
        let resp = self.send(req).await?;
        resp.json::<Value>().await.map_err(|e| self.map_transport_err(e))
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value, McpError> {
        let req = self
            .client
            .post(self.url(path))
            .timeout(DEFAULT_TIMEOUT)
            .json(body);
        let resp = self.send(req).await?;
        resp.json::<Value>().await.map_err(|e| self.map_transport_err(e))
    }

    async fn delete_json(&self, path: &str) -> Result<Value, McpError> {
        let req = self.client.delete(self.url(path)).timeout(DEFAULT_TIMEOUT);
        let resp = self.send(req).await?;
        resp.json::<Value>().await.map_err(|e| self.map_transport_err(e))
    }

    /// POST a `file` multipart field read from a local path, returning the
    /// backend's JSON response.
    async fn upload_multipart(&self, path: &str, url_path: &str) -> Result<Value, McpError> {
        let file_path = std::path::Path::new(path);
        let filename = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        let bytes = tokio::fs::read(file_path)
            .await
            .map_err(|e| McpError::invalid_params(format!("cannot read '{path}': {e}"), None))?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str("application/pdf")
            .map_err(|e| McpError::internal_error(format!("backend error: {e}"), None))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let req = self
            .client
            .post(self.url(url_path))
            .timeout(DEFAULT_TIMEOUT)
            .multipart(form);
        let resp = self.send(req).await?;
        resp.json::<Value>().await.map_err(|e| self.map_transport_err(e))
    }

    /// Run a GET/POST request whose successful response body is the raw
    /// file bytes; write them to `output_path` and report path + size.
    async fn download_to_file(
        &self,
        req: reqwest::RequestBuilder,
        output_path: &str,
    ) -> Result<String, McpError> {
        let resp = self.send(req).await?;
        let bytes = resp.bytes().await.map_err(|e| self.map_transport_err(e))?;
        tokio::fs::write(output_path, &bytes)
            .await
            .map_err(|e| McpError::internal_error(format!("cannot write '{output_path}': {e}"), None))?;
        Ok(pretty(&serde_json::json!({
            "path": output_path,
            "bytes": bytes.len(),
        })))
    }
}

// ---------------------------------------------------------------------
// Tools (ADR-001 §5, v1 core set — 18 tools)
// ---------------------------------------------------------------------

#[tool_router]
impl PdfEditorTools {
    #[tool(
        description = "Upload a PDF file from the local filesystem into the PDF Editor backend. Creates a new document with a fresh id (returned in the response JSON); all other tools reference documents by this id. Page indices used by other tools are 0-based."
    )]
    async fn upload_pdf(
        &self,
        Parameters(UploadPdfArgs { path }): Parameters<UploadPdfArgs>,
    ) -> Result<String, McpError> {
        let v = self.upload_multipart(&path, "/api/documents").await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "List every document currently held by the PDF Editor backend (id, filename, size, revision). Read-only."
    )]
    async fn list_documents(&self) -> Result<String, McpError> {
        let v = self.get_json("/api/documents").await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Get a document's metadata plus per-page info (0-based page index, width/height in PDF points, rotation in degrees). Read-only."
    )]
    async fn document_info(
        &self,
        Parameters(DocumentIdArgs { id }): Parameters<DocumentIdArgs>,
    ) -> Result<String, McpError> {
        let v = self.get_json(&format!("/api/documents/{id}/info")).await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Extract the text and per-character bounding boxes of one page (0-based page index) of a document. Read-only."
    )]
    async fn page_text(
        &self,
        Parameters(PageTextArgs { id, page }): Parameters<PageTextArgs>,
    ) -> Result<String, McpError> {
        let v = self
            .get_json(&format!("/api/documents/{id}/pages/{page}/text"))
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Case-insensitive substring search across every page of a document. Returns hits with 0-based page index, merged bounding rects, and a short excerpt. Read-only."
    )]
    async fn search_text(
        &self,
        Parameters(SearchTextArgs { id, q }): Parameters<SearchTextArgs>,
    ) -> Result<String, McpError> {
        let req = self
            .client
            .get(self.url(&format!("/api/documents/{id}/search")))
            .timeout(DEFAULT_TIMEOUT)
            .query(&[("q", q.as_str())]);
        let resp = self.send(req).await?;
        let v: Value = resp.json().await.map_err(|e| self.map_transport_err(e))?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Render one page (0-based page index) of a document to a PNG file at output_path (its parent directory must already exist). Read-only; never modifies the document. Returns JSON {path, bytes} for the written file."
    )]
    async fn render_page(
        &self,
        Parameters(RenderPageArgs { id, page, output_path, scale }): Parameters<RenderPageArgs>,
    ) -> Result<String, McpError> {
        let mut req = self
            .client
            .get(self.url(&format!("/api/documents/{id}/pages/{page}/render")))
            .timeout(DEFAULT_TIMEOUT);
        if let Some(s) = scale {
            req = req.query(&[("scale", s.to_string())]);
        }
        self.download_to_file(req, &output_path).await
    }

    #[tool(
        description = "Download a document's current PDF bytes (reflecting every edit applied so far) to output_path. Read-only. Returns JSON {path, bytes} for the written file."
    )]
    async fn save_pdf(
        &self,
        Parameters(SavePdfArgs { id, output_path }): Parameters<SavePdfArgs>,
    ) -> Result<String, McpError> {
        let req = self
            .client
            .get(self.url(&format!("/api/documents/{id}/download")))
            .timeout(DEFAULT_TIMEOUT);
        self.download_to_file(req, &output_path).await
    }

    #[tool(
        description = "Set the absolute rotation (0/90/180/270 degrees) of one page (0-based page index). Mutates the document in place and bumps its revision; the document id does not change."
    )]
    async fn rotate_page(
        &self,
        Parameters(RotatePageArgs { id, page, degrees }): Parameters<RotatePageArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "degrees": degrees });
        let v = self
            .post_json(&format!("/api/documents/{id}/pages/{page}/rotate"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Delete one page (0-based page index) from a document. Fails if it is the document's only remaining page. Mutates the document in place and bumps its revision; the document id does not change."
    )]
    async fn delete_page(
        &self,
        Parameters(DeletePageArgs { id, page }): Parameters<DeletePageArgs>,
    ) -> Result<String, McpError> {
        let v = self
            .delete_json(&format!("/api/documents/{id}/pages/{page}"))
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Reorder every page of a document. `order` must be a permutation of 0..page_count (0-based source page indices in their new order). Mutates the document in place and bumps its revision; the document id does not change."
    )]
    async fn reorder_pages(
        &self,
        Parameters(ReorderPagesArgs { id, order }): Parameters<ReorderPagesArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "order": order });
        let v = self
            .post_json(&format!("/api/documents/{id}/pages/reorder"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Set the crop box on the given pages (0-based page indices) of a document. `rect` is a view-space rectangle in points (origin top-left of the rendered page as currently displayed); omit it (or pass null) to reset the crop back to the full page. Mutates the document in place and bumps its revision; the document id does not change."
    )]
    async fn crop_pages(
        &self,
        Parameters(CropPagesArgs { id, pages, rect }): Parameters<CropPagesArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "pages": pages, "rect": rect });
        let v = self
            .post_json(&format!("/api/documents/{id}/pages/crop"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Resize the given pages (0-based page indices) of a document to width x height PDF points, in display orientation. mode=\"scale\" scales page content to fit the new size (uniform, centered); mode=\"canvas\" changes only the page box, keeping content size and centering it on the new canvas. Mutates the document in place and bumps its revision; the document id does not change."
    )]
    async fn resize_pages(
        &self,
        Parameters(ResizePagesArgs { id, pages, width, height, mode }): Parameters<ResizePagesArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({
            "pages": pages,
            "width": width,
            "height": height,
            "mode": mode,
        });
        let v = self
            .post_json(&format!("/api/documents/{id}/pages/resize"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Merge two or more existing documents (by id, in the given order) into one brand-new document. Source documents are left untouched. Returns the new document's metadata, including its new id."
    )]
    async fn merge_documents(
        &self,
        Parameters(MergeDocumentsArgs { ids, filename }): Parameters<MergeDocumentsArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "ids": ids, "filename": filename });
        let v = self.post_json("/api/documents/merge", &body).await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Extract the given pages (0-based page indices, in the given order; indices may repeat) of a document into a brand-new document. Calling this once per desired page range also implements \"split\". The source document is left untouched. Returns the new document's metadata, including its new id."
    )]
    async fn extract_pages(
        &self,
        Parameters(ExtractPagesArgs { id, pages, filename }): Parameters<ExtractPagesArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "pages": pages, "filename": filename });
        let v = self
            .post_json(&format!("/api/documents/{id}/extract"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Copy pages (0-based page indices in source_id, inserted in the given order) into document id at 0-based position `at` (pass id's current page count to append at the end). source_id may equal id to duplicate pages within the same document. Mutates id in place and bumps its revision (id does not change); source_id is left untouched."
    )]
    async fn insert_pages_from(
        &self,
        Parameters(InsertPagesFromArgs { id, source_id, pages, at }): Parameters<InsertPagesFromArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "sourceId": source_id, "pages": pages, "at": at });
        let v = self
            .post_json(&format!("/api/documents/{id}/pages/insert-from"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Compress a document's embedded images. preset is one of \"screen\" (72dpi/quality 60), \"ebook\" (150dpi/75), \"printer\" (300dpi/85), or \"custom\" (use dpi 36..=600 and quality 10..=100). Encrypted documents are rejected. Produces a brand-new document (new id) plus before/after byte sizes and per-run stats; the original document is left untouched."
    )]
    async fn compress_pdf(
        &self,
        Parameters(CompressPdfArgs { id, preset, dpi, quality, filename }): Parameters<CompressPdfArgs>,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({
            "preset": preset,
            "dpi": dpi,
            "quality": quality,
            "filename": filename,
        });
        let v = self
            .post_json(&format!("/api/documents/{id}/compress"), &body)
            .await?;
        Ok(pretty(&v))
    }

    #[tool(
        description = "Export pages (0-based page indices; omit or leave empty for all pages) of a document as an image/presentation file written to output_path (parent directory must already exist). format is one of \"png\", \"jpg\", \"tiff\", \"pptx\". A single exported raster page writes the raw image; multiple raster pages are zipped by the backend; tiff/pptx are always one multi-page/slide file. dpi (72..=600, default 150) and quality (10..=100, jpg only, default 85) are optional. Read-only: does not modify the document or create a stored document id. Returns JSON {path, bytes} for the written file."
    )]
    async fn export_pages(
        &self,
        Parameters(ExportPagesArgs { id, format, output_path, pages, dpi, quality }): Parameters<
            ExportPagesArgs,
        >,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({
            "format": format,
            "pages": pages,
            "dpi": dpi,
            "quality": quality,
        });
        let req = self
            .client
            .post(self.url(&format!("/api/documents/{id}/export")))
            .timeout(EXPORT_TIMEOUT)
            .json(&body);
        self.download_to_file(req, &output_path).await
    }

    #[tool(
        description = "Convert a document to Microsoft Office format (\"docx\" or \"xlsx\") via the backend's Python sidecar, writing the result to output_path (parent directory must already exist). pages (0-based page indices; omit or leave empty for all pages) selects which pages to include. Can take up to a few minutes for large documents — the backend sidecar itself times out a single conversion at 300s. Read-only: does not modify the document or create a stored document id. Returns JSON {path, bytes} for the written file."
    )]
    async fn convert_to_office(
        &self,
        Parameters(ConvertToOfficeArgs { id, format, output_path, pages }): Parameters<
            ConvertToOfficeArgs,
        >,
    ) -> Result<String, McpError> {
        let body = serde_json::json!({ "format": format, "pages": pages });
        let req = self
            .client
            .post(self.url(&format!("/api/documents/{id}/export")))
            .timeout(OFFICE_TIMEOUT)
            .json(&body);
        self.download_to_file(req, &output_path).await
    }
}

#[tool_handler(
    name = "pdf-editor",
    instructions = "Wraps the PDF Editor HTTP backend (default http://127.0.0.1:8050, override with PDF_EDITOR_URL) as MCP tools. The backend must already be running. All page indices are 0-based. File-based tools (upload_pdf, render_page, save_pdf, export_pages, convert_to_office) exchange data through local filesystem paths, never base64 — the MCP server and backend run on the same machine. Write-op tools return the backend's JSON response as-is so you can see new document ids and revision bumps."
)]
impl ServerHandler for PdfEditorTools {}
