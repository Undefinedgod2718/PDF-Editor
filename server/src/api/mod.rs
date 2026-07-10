use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::pdf::{annots, ops};
use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/documents", post(upload).get(list_docs))
        .route("/api/documents/{id}/info", get(doc_info))
        .route("/api/documents/{id}/pages/{page}/render", get(render_page))
        .route("/api/documents/{id}/pages/{page}/text", get(page_text))
        .route("/api/documents/{id}/search", get(search))
        .route("/api/documents/{id}/download", get(download))
        .route(
            "/api/documents/{id}/pages/{page}/annotations",
            post(create_annotation).get(list_annotations),
        )
        .route(
            "/api/documents/{id}/pages/{page}/annotations/{index}",
            axum::routing::delete(delete_annotation),
        )
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "document not found".into())
}

async fn upload(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("document.pdf").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
        if !bytes.starts_with(b"%PDF") {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "not a PDF file".into(),
            ));
        }
        let meta = state.storage.save(filename, &bytes)?;
        return Ok(Json(serde_json::to_value(&meta).unwrap()));
    }
    Err(ApiError(
        StatusCode::BAD_REQUEST,
        "missing multipart field 'file'".into(),
    ))
}

async fn list_docs(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.storage.list())
}

async fn doc_info(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let info = state
        .engine
        .run(move |pdfium| ops::doc_info(pdfium, &path))
        .await?;
    Ok(Json(serde_json::json!({
        "id": meta.id,
        "filename": meta.filename,
        "size": meta.size,
        "pageCount": info.page_count,
        "title": info.title,
        "pages": info.pages,
    })))
}

#[derive(Deserialize)]
struct RenderParams {
    /// Pixels per PDF point; 1.0 = 72 dpi.
    #[serde(default = "default_scale")]
    scale: f32,
}

fn default_scale() -> f32 {
    1.5
}

async fn render_page(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    Query(params): Query<RenderParams>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let scale = params.scale.clamp(0.1, 8.0);
    let png = state
        .engine
        .run(move |pdfium| ops::render_page(pdfium, &path, page, scale))
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        png,
    ))
}

async fn page_text(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let text = state
        .engine
        .run(move |pdfium| ops::page_text(pdfium, &path, page))
        .await?;
    Ok(Json(text))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

async fn search(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let hits = state
        .engine
        .run(move |pdfium| ops::search(pdfium, &path, &params.q))
        .await?;
    Ok(Json(hits))
}

async fn create_annotation(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    Json(ann): Json<annots::NewAnnotation>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let count = state
        .engine
        .run(move |pdfium| annots::create(pdfium, &path, page, &ann))
        .await?;
    Ok(Json(serde_json::json!({ "count": count })))
}

async fn list_annotations(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let items = state
        .engine
        .run(move |pdfium| annots::list(pdfium, &path, page))
        .await?;
    Ok(Json(items))
}

async fn delete_annotation(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium| annots::delete(pdfium, &path, page, index))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn download(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let file = tokio::fs::File::open(state.storage.pdf_path(id))
        .await
        .map_err(|_| not_found())?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        urlencoding::encode(&meta.filename)
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from_stream(stream),
    ))
}
