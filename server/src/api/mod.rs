use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::pdf::{
    annots, compare, compress, exportops, formbuild, formops, imageops, objects, ops, pageops,
    protect,
};
use crate::sidecar;
use crate::SharedState;
use crate::llm;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/documents", post(upload).get(list_docs))
        .route("/api/documents/{id}/info", get(doc_info))
        .route("/api/documents/{id}/pages/{page}/render", get(render_page))
        .route("/api/documents/{id}/pages/{page}/text", get(page_text))
        .route("/api/documents/{id}/search", get(search))
        .route("/api/documents/{id}/download", get(download))
        .route("/api/documents/{id}/export", post(export_document))
        .route("/api/documents/{id}/compress", post(compress_document))
        .route("/api/documents/{id}/protection", get(protection_status))
        .route("/api/documents/{id}/protect", post(protect_document))
        .route("/api/documents/{id}/unprotect", post(unprotect_document))
        .route("/api/documents/{id}/encrypt", post(encrypt_document))
        .route("/api/documents/{id}/decrypt", post(decrypt_document))
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
        .route("/api/documents/compare", post(compare_documents))
        .route("/api/documents/{id}/extract", post(extract_pages))
        .route(
            "/api/documents/{id}/pages/{page}/objects",
            get(list_text_objects),
        )
        .route(
            "/api/documents/{id}/pages/{page}/objects/{index}",
            axum::routing::patch(edit_text_object).delete(delete_page_object),
        )
        .route(
            "/api/documents/{id}/pages/{page}/images",
            get(list_page_images).post(insert_page_image),
        )
        .route(
            "/api/documents/{id}/pages/{page}/images/{index}",
            post(replace_page_image),
        )
        .route("/api/documents/{id}/form", get(list_form_fields))
        .route(
            "/api/documents/{id}/pages/{page}/form",
            post(create_form_field),
        )
        .route(
            "/api/documents/{id}/pages/{page}/form/{index}",
            post(set_form_field)
                .patch(update_form_field)
                .delete(delete_form_field),
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
        let msg = e.to_string();
        // `protect::assert_editable` — must be 400, not a server fault.
        if msg.starts_with("document is protected;") {
            return ApiError(StatusCode::BAD_REQUEST, msg);
        }
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, msg)
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
        let meta = state.storage.save(filename, &bytes, None)?;
        return Ok(Json(serde_json::to_value(&meta.for_client()).unwrap()));
    }
    Err(ApiError(
        StatusCode::BAD_REQUEST,
        "missing multipart field 'file'".into(),
    ))
}

async fn list_docs(State(state): State<SharedState>) -> impl IntoResponse {
    let docs: Vec<_> = state
        .storage
        .list()
        .into_iter()
        .map(|m| m.for_client())
        .collect();
    Json(docs)
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

async fn list_page_images(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let items = state
        .engine
        .run(move |pdfium, cache| imageops::list_images(cache.open(pdfium, &path)?, page))
        .await?;
    Ok(Json(items))
}

/// Pull one image file plus named numeric fields out of a multipart form.
async fn image_multipart(
    multipart: &mut Multipart,
) -> Result<(image::DynamicImage, std::collections::HashMap<String, f32>), ApiError> {
    let mut img = None;
    let mut fields = std::collections::HashMap::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
                img = Some(image::load_from_memory(&bytes).map_err(|e| {
                    ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("not an image: {e}"))
                })?);
            }
            Some(name) => {
                let name = name.to_string();
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
                let value = text.parse::<f32>().map_err(|_| {
                    ApiError(
                        StatusCode::BAD_REQUEST,
                        format!("field '{name}' is not a number"),
                    )
                })?;
                fields.insert(name, value);
            }
            None => {}
        }
    }
    let img = img.ok_or_else(|| {
        ApiError(StatusCode::BAD_REQUEST, "missing multipart field 'file'".into())
    })?;
    Ok((img, fields))
}

async fn insert_page_image(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let (img, fields) = image_multipart(&mut multipart).await?;
    let get = |name: &str| {
        fields.get(name).copied().ok_or_else(|| {
            ApiError(
                StatusCode::BAD_REQUEST,
                format!("missing multipart field '{name}'"),
            )
        })
    };
    let (x, y, w, h) = (get("x")?, get("y")?, get("w")?, get("h")?);
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            imageops::insert_image(pdfium, &path, page, &img, x, y, w, h)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn replace_page_image(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let (img, _fields) = image_multipart(&mut multipart).await?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            imageops::replace_image(pdfium, &path, page, index, &img)?;
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

/// Wraps `formbuild::FormBuildError` so it can cross `engine.run`'s
/// `anyhow::Result<T>` boundary (the closure signature is fixed to
/// `anyhow::Result`, unlike the `spawn_blocking` jobs the protect/encrypt
/// handlers use) and be downcast back into its typed User/Internal variant
/// afterwards, mirroring `map_protect_err`'s handling of `ProtectError`.
struct FormBuildErrWrap(formbuild::FormBuildError);

impl std::fmt::Debug for FormBuildErrWrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            formbuild::FormBuildError::User(msg) => write!(f, "{msg}"),
            formbuild::FormBuildError::Internal(err) => write!(f, "{err:?}"),
        }
    }
}

impl std::fmt::Display for FormBuildErrWrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            formbuild::FormBuildError::User(msg) => write!(f, "{msg}"),
            formbuild::FormBuildError::Internal(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FormBuildErrWrap {}

fn map_formbuild_err(e: anyhow::Error) -> ApiError {
    match e.downcast::<FormBuildErrWrap>() {
        Ok(FormBuildErrWrap(formbuild::FormBuildError::User(msg))) => {
            ApiError(StatusCode::BAD_REQUEST, msg)
        }
        Ok(FormBuildErrWrap(formbuild::FormBuildError::Internal(err))) => {
            tracing::error!("form build failed: {err:#}");
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "form field operation failed".into(),
            )
        }
        Err(e) => ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn create_form_field(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    Json(body): Json<formbuild::NewField>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |_pdfium, cache| {
            formbuild::create_field(&path, page, &body)
                .map_err(|e| anyhow::Error::new(FormBuildErrWrap(e)))?;
            cache.invalidate(&path);
            Ok(())
        })
        .await
        .map_err(map_formbuild_err)?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn update_form_field(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    Json(body): Json<formbuild::FieldUpdate>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |_pdfium, cache| {
            formbuild::update_field(&path, page, index, &body)
                .map_err(|e| anyhow::Error::new(FormBuildErrWrap(e)))?;
            cache.invalidate(&path);
            Ok(())
        })
        .await
        .map_err(map_formbuild_err)?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn delete_form_field(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |_pdfium, cache| {
            formbuild::delete_field(&path, page, index)
                .map_err(|e| anyhow::Error::new(FormBuildErrWrap(e)))?;
            cache.invalidate(&path);
            Ok(())
        })
        .await
        .map_err(map_formbuild_err)?;
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
    let meta = state.storage.save(filename, &bytes, None)?;
    Ok(Json(serde_json::to_value(&meta.for_client()).unwrap()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompareBody {
    old_id: Uuid,
    new_id: Uuid,
    filename: Option<String>,
    #[serde(default = "default_true")]
    visual_diff: bool,
    #[serde(default = "default_true")]
    llm_summary: bool,
}

fn default_true() -> bool {
    true
}

async fn compare_documents(
    State(state): State<SharedState>,
    Json(body): Json<CompareBody>,
) -> Result<impl IntoResponse, ApiError> {
    let old_meta = state.storage.get(body.old_id).ok_or_else(not_found)?;
    let new_meta = state.storage.get(body.new_id).ok_or_else(not_found)?;
    let old_path = state.storage.pdf_path(body.old_id);
    let new_path = state.storage.pdf_path(body.new_id);
    let opts = compare::CompareOptions {
        visual_diff: body.visual_diff,
    };

    let (mut report, bytes) = state
        .engine
        .run(move |pdfium, _cache| compare::compare(pdfium, &old_path, &new_path, &opts))
        .await?;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("compare_{}_vs_{}", old_meta.filename, new_meta.filename));
    let out_meta = state.storage.save(filename, &bytes, None)?;
    // pdfium can't write /NM; stamp stable ids in a lopdf pass, same as
    // create_annotation — the annotations burned in by compare::compare
    // still need it for the per-page annotation UI to address them later.
    annots::ensure_annotation_names(&state.storage.pdf_path(out_meta.id))?;

    if body.llm_summary {
        report.summary = llm::summarize_diff(&report).await;
    }

    Ok(Json(serde_json::json!({
        "document": out_meta.for_client(),
        "report": report,
    })))
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
    let new_meta = state.storage.save(filename, &bytes, None)?;
    Ok(Json(serde_json::to_value(&new_meta.for_client()).unwrap()))
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

/// Wire format for `POST .../export`. Office variants are handled by the
/// Python sidecar; raster/PPTX go to [`exportops`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    Png,
    Jpg,
    Tiff,
    Pptx,
    Docx,
    Xlsx,
}

impl ExportFormat {
    fn as_office(self) -> Option<sidecar::OfficeFormat> {
        match self {
            ExportFormat::Docx => Some(sidecar::OfficeFormat::Docx),
            ExportFormat::Xlsx => Some(sidecar::OfficeFormat::Xlsx),
            _ => None,
        }
    }

    fn as_raster(self) -> Option<exportops::ExportFormat> {
        match self {
            ExportFormat::Png => Some(exportops::ExportFormat::Png),
            ExportFormat::Jpg => Some(exportops::ExportFormat::Jpg),
            ExportFormat::Tiff => Some(exportops::ExportFormat::Tiff),
            ExportFormat::Pptx => Some(exportops::ExportFormat::Pptx),
            ExportFormat::Docx | ExportFormat::Xlsx => None,
        }
    }
}

#[derive(Deserialize)]
struct ExportBody {
    format: ExportFormat,
    /// 0-based page indices; omitted or empty means "all pages".
    #[serde(default)]
    pages: Vec<u16>,
    dpi: Option<u32>,
    /// JPEG quality only; ignored for other formats.
    quality: Option<u8>,
}

async fn export_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ExportBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    let dpi = body.dpi.unwrap_or(150).clamp(72, 600);
    let quality = body.quality.unwrap_or(85).clamp(10, 100);
    let scale = dpi as f32 / 72.0;
    let format = body.format;

    let (bytes, content_type, ext) = if let Some(office) = format.as_office() {
        // Office conversion runs in the Python sidecar; dpi/quality don't apply.
        // Skip the PDFium page-count pre-check: the sidecar validates page
        // range/duplicates and encryption itself with clearer messages (an
        // encrypted document would fail the PDFium open with a 500 here).
        let page_arg = if body.pages.is_empty() {
            None
        } else {
            Some(body.pages.as_slice())
        };
        let bytes = sidecar::convert(&path, office, page_arg)
            .await
            .map_err(|e| match e {
                sidecar::SidecarError::User(msg) => ApiError(StatusCode::BAD_REQUEST, msg),
                sidecar::SidecarError::Internal(err) => {
                    tracing::error!("sidecar conversion failed: {err:#}");
                    ApiError(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "conversion failed".into(),
                    )
                }
            })?;
        (bytes, office.content_type(), office.ext())
    } else {
        // as_office / as_raster partition ExportFormat; office already returned.
        let Some(raster) = format.as_raster() else {
            unreachable!("ExportFormat must be office or raster");
        };
        // Resolve the page count first so out-of-range indices get a 400
        // instead of surfacing as a 500 from deep inside the renderer.
        let count_path = path.clone();
        let page_count: u16 = state
            .engine
            .run(move |pdfium, cache| Ok(cache.open(pdfium, &count_path)?.pages().len()))
            .await?;

        let pages: Vec<u16> = if body.pages.is_empty() {
            (0..page_count).collect()
        } else {
            body.pages
        };
        // ZIP members are named by page index; duplicates would collide / overwrite.
        let mut seen = std::collections::HashSet::with_capacity(pages.len());
        for &p in &pages {
            if p >= page_count {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "page index out of range".into(),
                ));
            }
            if !seen.insert(p) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "duplicate page index".into(),
                ));
            }
        }

        let export_path = path.clone();
        let result = state
            .engine
            .run(move |pdfium, cache| {
                let doc = cache.open(pdfium, &export_path)?;
                exportops::export(doc, raster, &pages, scale, quality)
            })
            .await?;
        (result.bytes, result.content_type, result.ext)
    };

    let stem = std::path::Path::new(&meta.filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let filename = format!("{stem}.{ext}");
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        urlencoding::encode(&filename)
    );
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum CompressPreset {
    Screen,
    Ebook,
    Printer,
    Custom,
}

#[derive(Deserialize)]
struct CompressBody {
    preset: CompressPreset,
    /// Custom preset only; ignored otherwise.
    dpi: Option<f32>,
    quality: Option<u8>,
    filename: Option<String>,
}

async fn compress_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CompressBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    let (dpi, quality) = match body.preset {
        CompressPreset::Screen => (72.0, 60),
        CompressPreset::Ebook => (150.0, 75),
        CompressPreset::Printer => (300.0, 85),
        CompressPreset::Custom => (
            body.dpi.unwrap_or(150.0).clamp(36.0, 600.0),
            body.quality.unwrap_or(75).clamp(10, 100),
        ),
    };
    let opts = compress::CompressOptions {
        target_dpi: dpi,
        jpeg_quality: quality,
    };

    let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    // Pure lopdf work — no PDFium involved, so run it on the blocking pool
    // instead of tying up the single PDFium worker thread.
    let job_path = path.clone();
    let (bytes, stats) = tokio::task::spawn_blocking(move || compress::compress(&job_path, &opts))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    let after = bytes.len() as u64;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("compressed_{}", meta.filename));
    let new_meta = state.storage.save(filename, &bytes, None)?;
    Ok(Json(serde_json::json!({
        "document": new_meta.for_client(),
        "before": before,
        "after": after,
        "stats": stats,
    })))
}

async fn protection_status(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let status = tokio::task::spawn_blocking(move || protect::inspect(&path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtectBody {
    owner_password: String,
    permissions: protect::PermissionFlags,
    filename: Option<String>,
}

async fn protect_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ProtectBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    let job_path = path.clone();
    let owner_password = body.owner_password;
    let flags = body.permissions;
    // Argon2id hashing is memory-hard (deliberately slow); run it and the PDF
    // encryption together on the blocking pool, off the async executor.
    let (bytes, hash) = tokio::task::spawn_blocking(move || {
        let hash = protect::hash_password(&owner_password)?;
        let bytes = protect::protect(&job_path, &owner_password, flags)?;
        Ok::<_, protect::ProtectError>((bytes, hash))
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| match e {
        protect::ProtectError::User(msg) => ApiError(StatusCode::BAD_REQUEST, msg),
        protect::ProtectError::Internal(err) => {
            tracing::error!("protect failed: {err:#}");
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "protection failed".into(),
            )
        }
    })?;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("protected_{}", meta.filename));
    let new_meta = state.storage.save(filename, &bytes, Some(hash))?;
    Ok(Json(serde_json::json!({ "document": new_meta.for_client() })))
}

#[derive(Deserialize)]
struct UnprotectBody {
    password: String,
    filename: Option<String>,
}

async fn unprotect_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UnprotectBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    // An empty-user-password PDF auto-decrypts on load in any reader
    // (including our own lopdf), so `protect::unprotect` cannot itself
    // re-verify the owner password for documents protected via `/protect`.
    // Verify against the hash recorded at protect-time and pass
    // `owner_verified`; without a hash (re-upload / foreign tool), the
    // empty-user-password path refuses rather than becoming a free unlock.
    let owner_verified = if let Some(stored_hash) = &meta.protection_hash {
        if !protect::verify_password(&body.password, stored_hash) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "incorrect password".into(),
            ));
        }
        true
    } else {
        false
    };

    let job_path = path.clone();
    let password = body.password;
    let bytes =
        tokio::task::spawn_blocking(move || protect::unprotect(&job_path, &password, owner_verified))
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| match e {
                protect::ProtectError::User(msg) => ApiError(StatusCode::BAD_REQUEST, msg),
                protect::ProtectError::Internal(err) => {
                    tracing::error!("unprotect failed: {err:#}");
                    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unprotect failed".into())
                }
            })?;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("unprotected_{}", meta.filename));
    let new_meta = state.storage.save(filename, &bytes, None)?;
    Ok(Json(serde_json::json!({ "document": new_meta.for_client() })))
}

/// Build an `attachment` PDF download response with a UTF-8 filename.
fn pdf_download(filename: &str, bytes: Vec<u8>) -> impl IntoResponse {
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        urlencoding::encode(filename)
    );
    (
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
}

fn map_protect_err(what: &'static str) -> impl Fn(protect::ProtectError) -> ApiError {
    move |e| match e {
        protect::ProtectError::User(msg) => ApiError(StatusCode::BAD_REQUEST, msg),
        protect::ProtectError::Internal(err) => {
            tracing::error!("{what} failed: {err:#}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{what} failed"))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptBody {
    /// Open password: required to open/render the output in any reader.
    user_password: String,
    /// Permission-change password. Defaults to the user password.
    owner_password: Option<String>,
    /// Per-action limits; defaults to all-allowed.
    permissions: Option<protect::PermissionFlags>,
    filename: Option<String>,
}

/// P12: encrypt with a real open password. Returns the encrypted PDF as a
/// download — it is *not* stored in the library, since without the password
/// our own PDFium viewer could not render it. The source document is
/// untouched and stays viewable.
async fn encrypt_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<EncryptBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    let user_password = body.user_password;
    let owner_password = body.owner_password.unwrap_or_else(|| user_password.clone());
    let flags = body
        .permissions
        .unwrap_or_else(protect::PermissionFlags::all_allowed);

    let job_path = path.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        protect::encrypt(&job_path, &user_password, &owner_password, flags)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_protect_err("encryption"))?;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("encrypted_{}", meta.filename));
    Ok(pdf_download(&filename, bytes))
}

#[derive(Deserialize)]
struct DecryptBody {
    password: String,
    filename: Option<String>,
}

/// P12: remove an open password given the password. Returns the decrypted
/// PDF as a download; not stored in the library.
async fn decrypt_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DecryptBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    // Library-side P11 protection is tracked by this hash; those files have an
    // empty user password and must use `/unprotect`, never `/decrypt`.
    if meta.protection_hash.is_some() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "document has no open password; use unprotect instead".into(),
        ));
    }
    let path = state.storage.pdf_path(id);

    let password = body.password;
    let job_path = path.clone();
    let bytes = tokio::task::spawn_blocking(move || protect::decrypt(&job_path, &password))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_protect_err("decryption"))?;

    let filename = body
        .filename
        .unwrap_or_else(|| format!("decrypted_{}", meta.filename));
    Ok(pdf_download(&filename, bytes))
}
