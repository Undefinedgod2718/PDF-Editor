use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::pdf::{annots, formops, objects, ops, pageops};
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
        .route(
            "/api/documents/{id}/pages/{page}/rotate",
            post(rotate_page),
        )
        .route(
            "/api/documents/{id}/pages/{page}",
            axum::routing::delete(delete_page),
        )
        .route("/api/documents/{id}/pages", post(insert_page))
        .route("/api/documents/{id}/pages/crop", post(crop_pages))
        .route("/api/documents/{id}/pages/resize", post(resize_pages))
        .route(
            "/api/documents/{id}/pages/insert-from",
            post(insert_pages_from),
        )
        .route("/api/documents/{id}/pages/reorder", post(reorder_pages))
        .route("/api/documents/merge", post(merge_documents))
        .route("/api/documents/{id}/extract", post(extract_pages))
        .route(
            "/api/documents/{id}/pages/{page}/objects",
            get(list_text_objects),
        )
        .route(
            "/api/documents/{id}/pages/{page}/objects/{index}",
            axum::routing::patch(edit_text_object).delete(delete_page_object),
        )
        .route("/api/documents/{id}/form", get(list_form_fields))
        .route(
            "/api/documents/{id}/pages/{page}/form/{index}",
            post(set_form_field),
        )
        .route("/api/stamps", post(upload_stamp).get(list_stamps))
        .route(
            "/api/stamps/{id}",
            axum::routing::delete(delete_stamp),
        )
        .route("/api/stamps/{id}/image", get(stamp_image))
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
        .run(move |pdfium, cache| ops::doc_info(cache.open(pdfium, &path)?))
        .await?;
    Ok(Json(serde_json::json!({
        "id": meta.id,
        "filename": meta.filename,
        "size": meta.size,
        "revision": meta.revision,
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
    /// Document revision the client believes it is rendering. Unused for
    /// rendering itself; its presence makes the URL unique per content
    /// state, which is what allows the immutable cache policy below.
    v: Option<u64>,
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
        .run(move |pdfium, cache| ops::render_page(cache.open(pdfium, &path)?, page, scale))
        .await?;
    // Versioned URLs are unique per content state, so their responses can
    // be cached forever; unversioned requests must never be cached.
    let cache_control = if params.v.is_some() {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, cache_control),
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
        .run(move |pdfium, cache| ops::page_text(cache.open(pdfium, &path)?, page))
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
        .run(move |pdfium, cache| ops::search(cache.open(pdfium, &path)?, &params.q))
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
    // Stamp annotations reference a library image; decode it here so the
    // PDFium worker gets ready-to-embed pixels.
    let stamp_image = if let annots::NewAnnotation::Stamp { stamp_id, .. } = &ann {
        state
            .storage
            .get_stamp(*stamp_id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "stamp not found".into()))?;
        let bytes = tokio::fs::read(state.storage.stamp_path(*stamp_id))
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Some(
            image::load_from_memory(&bytes)
                .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?,
        )
    } else {
        None
    };
    let count = state
        .engine
        .run(move |pdfium, cache| {
            let count = annots::create(pdfium, &path, page, &ann, stamp_image)?;
            // pdfium can't write /NM; stamp stable ids in a lopdf pass.
            annots::ensure_annotation_names(&path)?;
            cache.invalidate(&path);
            Ok(count)
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "count": count, "revision": revision })))
}

async fn list_text_objects(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let items = state
        .engine
        .run(move |pdfium, cache| objects::list_text_objects(cache.open(pdfium, &path)?, page))
        .await?;
    Ok(Json(items))
}

#[derive(Deserialize)]
struct EditTextBody {
    text: String,
}

async fn edit_text_object(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    Json(body): Json<EditTextBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            objects::set_text(pdfium, &path, page, index, &body.text)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn delete_page_object(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            objects::delete_object(pdfium, &path, page, index)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn list_form_fields(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let fields = state
        .engine
        .run(move |pdfium, cache| {
            let doc = cache.open(pdfium, &path)?;
            formops::list_fields(doc, &path)
        })
        .await?;
    Ok(Json(fields))
}

async fn set_form_field(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    Json(body): Json<formops::SetFieldBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            formops::set_field(pdfium, &path, page, index, &body)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn upload_stamp(
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
        let filename = field.file_name().unwrap_or("stamp.png").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("not an image: {e}")))?;
        // Re-encode to PNG so the library holds one predictable format
        // (uploads may be PNG/WebP/etc.); RGBA keeps any alpha channel.
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let meta = state.storage.save_stamp(filename, w, h, &png.into_inner())?;
        return Ok(Json(serde_json::to_value(&meta).unwrap()));
    }
    Err(ApiError(
        StatusCode::BAD_REQUEST,
        "missing multipart field 'file'".into(),
    ))
}

async fn list_stamps(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.storage.list_stamps())
}

async fn stamp_image(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .storage
        .get_stamp(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "stamp not found".into()))?;
    let bytes = tokio::fs::read(state.storage.stamp_path(id))
        .await
        .map_err(|_| not_found())?;
    Ok(([(header::CONTENT_TYPE, "image/png")], bytes))
}

async fn delete_stamp(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.delete_stamp(id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_annotations(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let items = state
        .engine
        .run(move |pdfium, cache| annots::list(cache.open(pdfium, &path)?, page))
        .await?;
    Ok(Json(items))
}

async fn delete_annotation(
    State(state): State<SharedState>,
    Path((id, page, annot_id)): Path<(Uuid, u16, String)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            annots::delete(pdfium, &path, page, &annot_id)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct RotateBody {
    degrees: u16,
}

async fn rotate_page(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    Json(body): Json<RotateBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::rotate(pdfium, &path, page, body.degrees)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn delete_page(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::delete_page(pdfium, &path, page)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct InsertBody {
    at: u16,
    width: Option<f32>,
    height: Option<f32>,
}

async fn insert_page(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<InsertBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::insert_blank(pdfium, &path, body.at, body.width, body.height)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct CropBody {
    /// 0-based page indices to crop.
    pages: Vec<u16>,
    /// View-space rect in points (origin top-left of the rendered page).
    /// `null`/absent resets the crop to the full page.
    rect: Option<pageops::CropRect>,
}

async fn crop_pages(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CropBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |_pdfium, cache| {
            pageops::crop(&path, &body.pages, body.rect)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct ResizeBody {
    /// 0-based page indices to resize.
    pages: Vec<u16>,
    /// Target size in points, in display orientation.
    width: f32,
    height: f32,
    mode: pageops::ResizeMode,
}

async fn resize_pages(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ResizeBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |_pdfium, cache| {
            pageops::resize(&path, &body.pages, body.width, body.height, body.mode)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct ReorderBody {
    order: Vec<u16>,
}

async fn reorder_pages(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReorderBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::reorder(pdfium, &path, &body.order)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InsertFromBody {
    /// Document to copy pages from (may equal the destination to duplicate).
    source_id: Uuid,
    /// 0-based page indices in the source, inserted in this order.
    pages: Vec<u16>,
    /// 0-based insert position in the destination; page count = append.
    at: u16,
}

async fn insert_pages_from(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<InsertFromBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    state
        .storage
        .get(body.source_id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "source document not found".into()))?;
    let path = state.storage.pdf_path(id);
    let src_path = state.storage.pdf_path(body.source_id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::insert_from(pdfium, &path, &src_path, &body.pages, body.at)?;
            // Copied pages may carry annotations without /NM; keep ids stable.
            annots::ensure_annotation_names(&path)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct MergeBody {
    ids: Vec<Uuid>,
    filename: Option<String>,
}

async fn merge_documents(
    State(state): State<SharedState>,
    Json(body): Json<MergeBody>,
) -> Result<impl IntoResponse, ApiError> {
    let mut paths = Vec::new();
    for id in &body.ids {
        state.storage.get(*id).ok_or_else(not_found)?;
        paths.push(state.storage.pdf_path(*id));
    }
    let bytes = state
        .engine
        .run(move |pdfium, _cache| pageops::merge(pdfium, &paths))
        .await?;
    let filename = body.filename.unwrap_or_else(|| "merged.pdf".into());
    let meta = state.storage.save(filename, &bytes)?;
    Ok(Json(serde_json::to_value(&meta).unwrap()))
}

#[derive(Deserialize)]
struct ExtractBody {
    pages: Vec<u16>,
    filename: Option<String>,
}

async fn extract_pages(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ExtractBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let pages = body.pages.clone();
    let bytes = state
        .engine
        .run(move |pdfium, _cache| pageops::extract(pdfium, &path, &pages))
        .await?;
    let filename = body
        .filename
        .unwrap_or_else(|| format!("extract_{}", meta.filename));
    let new_meta = state.storage.save(filename, &bytes)?;
    Ok(Json(serde_json::to_value(&new_meta).unwrap()))
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
