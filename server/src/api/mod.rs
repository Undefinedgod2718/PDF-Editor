use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::actions;
use crate::pdf::{
    annots, compare, compress, exportops, fingerprint, formbuild, formops, imageops, objects, ocr,
    ops, pageops, protect, redact, textedit,
};
use crate::sidecar;
use crate::storage;
use crate::SharedState;
use crate::llm;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/documents", post(upload).get(list_docs))
        .route("/api/documents/{id}/info", get(doc_info))
        .route("/api/documents/{id}/pages/{page}/render", get(render_page))
        .route("/api/documents/{id}/pages/{page}/text", get(page_text))
        .route("/api/documents/{id}/search", get(search))
        .route("/api/documents/{id}/download", get(download))
        .route("/api/documents/{id}/export", post(export_document))
        .route("/api/documents/{id}/compress", post(compress_document))
        .route("/api/documents/{id}/ocr", post(ocr_document))
        .route("/api/documents/{id}/ocr/jobs/{job_id}", get(ocr_job_status))
        .route("/api/ocr/languages", get(ocr_languages))
        .route("/api/documents/{id}/redact", post(redact_document))
        .route("/api/documents/{id}/protection", get(protection_status))
        .route("/api/documents/{id}/protect", post(protect_document))
        .route("/api/documents/{id}/unprotect", post(unprotect_document))
        .route("/api/documents/{id}/encrypt", post(encrypt_document))
        .route("/api/documents/{id}/decrypt", post(decrypt_document))
        .route("/api/documents/{id}/fingerprint", get(fingerprint_document))
        .route("/api/actions", post(create_action).get(list_actions))
        .route(
            "/api/actions/{id}",
            get(get_action).delete(delete_action),
        )
        .route("/api/actions/{id}/run", post(run_action))
        .route("/api/actions/runs/{run_id}", get(action_run_status))
        .route(
            "/api/actions/runs/{run_id}/files/{index}",
            get(action_run_file),
        )
        .route(
            "/api/actions/runs/{run_id}/download",
            get(action_run_download),
        )
        .route(
            "/api/documents/{id}/pages/{page}/annotations",
            post(create_annotation).get(list_annotations),
        )
        .route(
            "/api/documents/{id}/pages/{page}/annotations/{index}",
            axum::routing::delete(delete_annotation).patch(update_annotation),
        )
        .route(
            "/api/documents/{id}/pages/{page}/annotations/{index}/replies",
            post(reply_to_annotation),
        )
        .route(
            "/api/documents/{id}/pages/{page}/rotate",
            post(rotate_page),
        )
        .route("/api/documents/{id}/pages/rotate-all", post(rotate_all_pages))
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
        .route(
            "/api/documents/{id}/pages/{page}/lines",
            get(list_text_lines).post(insert_text_line),
        )
        .route(
            "/api/documents/{id}/pages/{page}/lines/{index}",
            axum::routing::patch(edit_text_line),
        )
        .route(
            "/api/documents/{id}/pages/{page}/lines/{index}/shift",
            post(shift_text_line),
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
        // Catch-all for any /api/* path this router doesn't otherwise match
        // (e.g. desktop-only /api/local/* hit against the plain web binary).
        // Without this, the unmatched request falls through to main.rs's
        // static-file fallback_service, which serves index.html with 200 —
        // that previously made the frontend's detectMode() (GET
        // /api/local/ping) misreport 'local' mode even for a plain web
        // deploy, since matchit always prefers a literal/param route over
        // this catch-all, registration order doesn't matter for precedence.
        .route("/api/{*rest}", axum::routing::any(unmatched_api_route))
}

async fn unmatched_api_route() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "no such API route".into())
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
        if is_client_error(&msg) {
            return ApiError(StatusCode::BAD_REQUEST, msg);
        }
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
}

/// User/input faults → 400. Real server faults stay 500.
/// Match on stable message fragments from pdf-core (pageops/imageops/compress/…).
///
/// Keep every marker specific enough that it cannot appear in an internal
/// failure. A bare "too small" used to live here and swallowed
/// `font.rs`'s "font too small" — a corrupt bundled font reported to the
/// caller as their mistake. "crop rect" already covers the page-geometry case.
fn is_client_error(msg: &str) -> bool {
    const MARKERS: &[&str] = &[
        "document is protected;",
        "document is encrypted",
        "encrypted documents are not supported",
        "out of range",
        "must be positive",
        "cannot delete the only remaining page",
        "order is not a permutation",
        "order length",
        "unsupported rotation",
        "crop rect",
        "crop needs",
        "resize needs",
        "insert needs",
        "merge needs",
        "extract needs",
        "lies outside the page",
        "is not an image object",
        // annots::set_contents — 對文字框/印章編輯內容是送錯對象，不是伺服器故障。
        "cannot edit contents of",
        // annots::set_contents / add_reply — 指定了不存在的註解 id。語意上更接近
        // 404，但這套 marker 只能映射到 400；至少不要算成伺服器故障。
        "no annotation with id",
        // annots::set_color — 對外觀流是「畫出來」的類型換色，目前不支援。
        "cannot recolour",
        // annots::set_rect — 標記類的位置綁在它覆蓋的文字上，不能自由拖拉。
        "cannot move this annotation type",
    ];
    MARKERS.iter().any(|m| msg.contains(m))
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "document not found".into())
}

/// Liveness + dependency probe. `sidecar.ok=false` means docx/xlsx export is
/// broken (missing Python interpreter or convert.py) — poll this from log/
/// monitoring scripts instead of waiting for a user-facing 500.
async fn health() -> impl IntoResponse {
    let sidecar_status = match sidecar::health() {
        Ok((python, script)) => serde_json::json!({
            "ok": true,
            "python": python.display().to_string(),
            "script": script.display().to_string(),
        }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    };
    Json(serde_json::json!({ "status": "ok", "sidecar": sidecar_status }))
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

async fn list_text_lines(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let items = state
        .engine
        .run(move |pdfium, cache| textedit::list_lines(cache.open(pdfium, &path)?, page))
        .await?;
    Ok(Json(items))
}

async fn insert_text_line(
    State(state): State<SharedState>,
    Path((id, page)): Path<(Uuid, u16)>,
    Json(body): Json<textedit::InsertLine>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            textedit::insert_line(pdfium, &path, page, &body)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn edit_text_line(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    Json(body): Json<EditTextBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            textedit::edit_line(pdfium, &path, page, index, &body.text)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

async fn shift_text_line(
    State(state): State<SharedState>,
    Path((id, page, index)): Path<(Uuid, u16, usize)>,
    Json(body): Json<textedit::ShiftLine>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            textedit::shift_line(pdfium, &path, page, index, &body)?;
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
        .run(move |pdfium, cache| {
            let doc = cache.open(pdfium, &path)?;
            annots::list(doc, &path, page)
        })
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
            // 先清掉以它為父節點的回覆，再刪本體。反過來的話回覆的 /IRT 會指向
            // 一個不存在的物件，清單上留下一則沒有歸屬、也無法再回覆的孤兒。
            // 只在該頁真的有回覆時才會重寫檔案（見 remove_replies_to）。
            annots::remove_replies_to(&path, page, &annot_id)?;
            annots::delete(pdfium, &path, page, &annot_id)?;
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct UpdateAnnotationBody {
    #[serde(default)]
    contents: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    color: Option<annots::InColor>,
    #[serde(default)]
    rect: Option<annots::InRect>,
}

async fn update_annotation(
    State(state): State<SharedState>,
    Path((id, page, annot_id)): Path<(Uuid, u16, String)>,
    Json(body): Json<UpdateAnnotationBody>,
) -> Result<impl IntoResponse, ApiError> {
    if body.contents.is_none() && body.author.is_none() && body.color.is_none() && body.rect.is_none()
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "nothing to update; supply contents, author, color and/or rect".into(),
        ));
    }
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            if body.contents.is_some() || body.author.is_some() {
                annots::set_contents(
                    &path,
                    page,
                    &annot_id,
                    body.contents.as_deref(),
                    body.author.as_deref(),
                )?;
            }
            if let Some(color) = body.color {
                annots::set_color(&path, page, &annot_id, color)?;
            }
            if let Some(rect) = body.rect {
                annots::set_rect(pdfium, &path, page, &annot_id, rect)?;
            }
            cache.invalidate(&path);
            Ok(())
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "revision": revision })))
}

#[derive(Deserialize)]
struct ReplyBody {
    contents: String,
    #[serde(default)]
    author: Option<String>,
}

async fn reply_to_annotation(
    State(state): State<SharedState>,
    Path((id, page, annot_id)): Path<(Uuid, u16, String)>,
    Json(body): Json<ReplyBody>,
) -> Result<impl IntoResponse, ApiError> {
    if body.contents.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "reply contents must not be empty".into(),
        ));
    }
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let nm = state
        .engine
        .run(move |_pdfium, cache| {
            let nm = annots::add_reply(&path, page, &annot_id, &body.contents, body.author.as_deref())?;
            cache.invalidate(&path);
            Ok(nm)
        })
        .await?;
    let revision = state.storage.bump_revision(id)?;
    Ok(Json(serde_json::json!({ "ok": true, "nm": nm, "revision": revision })))
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

#[derive(Deserialize)]
struct RotateAllBody {
    /// Multiple of 90; negative = counter-clockwise.
    delta: i32,
}

async fn rotate_all_pages(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RotateAllBody>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    state
        .engine
        .run(move |pdfium, cache| {
            pageops::rotate_all(pdfium, &path, body.delta)?;
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
    // pdfium can't write /NM; stamp on a temp file *before* library save so a
    // failed lopdf pass never leaves a half-registered document (unlike
    // create_annotation which mutates an already-owned path in place).
    let tmp = std::env::temp_dir().join(format!("pdf-editor-compare-{}.pdf", Uuid::new_v4()));
    std::fs::write(&tmp, &bytes).map_err(|e| {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let named_bytes = match annots::ensure_annotation_names(&tmp).and_then(|_| {
        std::fs::read(&tmp).map_err(anyhow::Error::from)
    }) {
        Ok(b) => {
            let _ = std::fs::remove_file(&tmp);
            b
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
    };
    let out_meta = state.storage.save(filename, &named_bytes, None)?;

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
/// `pub` (rather than the module-private default other `*Body` wire types
/// use): [`crate::actions`] embeds this in `Step::Export` so a saved
/// action's export step matches the single-shot endpoint exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Png,
    Jpg,
    Tiff,
    Pptx,
    Docx,
    Xlsx,
    Markdown,
}

impl ExportFormat {
    pub fn as_office(self) -> Option<sidecar::OfficeFormat> {
        match self {
            ExportFormat::Docx => Some(sidecar::OfficeFormat::Docx),
            ExportFormat::Xlsx => Some(sidecar::OfficeFormat::Xlsx),
            ExportFormat::Markdown => Some(sidecar::OfficeFormat::Markdown),
            _ => None,
        }
    }

    pub fn as_raster(self) -> Option<exportops::ExportFormat> {
        match self {
            ExportFormat::Png => Some(exportops::ExportFormat::Png),
            ExportFormat::Jpg => Some(exportops::ExportFormat::Jpg),
            ExportFormat::Tiff => Some(exportops::ExportFormat::Tiff),
            ExportFormat::Pptx => Some(exportops::ExportFormat::Pptx),
            ExportFormat::Docx | ExportFormat::Xlsx | ExportFormat::Markdown => None,
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

    if format == ExportFormat::Markdown && !body.pages.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Markdown 匯出不支援頁面篩選，請匯出整份文件".into(),
        ));
    }

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

/// `pub`: [`crate::actions`] embeds this in `Step::Compress` so a saved
/// action's compress step matches the single-shot endpoint exactly.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressPreset {
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

#[derive(Deserialize)]
struct OcrBody {
    /// Tesseract language spec, e.g. "eng+chi_tra". Defaults server-side.
    langs: Option<String>,
    /// Render/recognition DPI (36..=600). Defaults to 300.
    dpi: Option<f32>,
    /// Drop recognized words below this confidence (0-100). Defaults to 60.
    min_confidence: Option<f32>,
    /// OCR pages even if they already have extractable text.
    #[serde(default)]
    force: bool,
    /// Tesseract page-segmentation mode (3 auto / 4 single column /
    /// 6 single block / 11 sparse). Anything else falls back to the default.
    psm: Option<u8>,
    /// 影像前處理：none / gray / otsu / contrast / sharpen。無法辨識的值忽略。
    preprocess: Option<String>,
    filename: Option<String>,
}

/// State of a background OCR job, keyed by job id in `AppState::ocr_jobs`.
/// `Running` is read-mostly (the polling handler just snapshots the atomic
/// counters); `Done`/`Error` are terminal and removed from the map the
/// first time a poll observes them.
pub enum OcrJobState {
    Running(Arc<ocr::OcrProgress>),
    Done { document: storage::DocMeta, stats: ocr::OcrStats },
    Error(String),
}

/// Kick off OCR as a background job and return its id immediately — OCR on
/// a multi-page scan can take a while, and the caller polls
/// `GET .../ocr/jobs/{job_id}` for progress instead of blocking one HTTP
/// request for the whole run.
async fn ocr_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<OcrBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    let mut opts = ocr::OcrOptions::default();
    if let Some(langs) = body.langs {
        opts.langs = langs;
    }
    if let Some(dpi) = body.dpi {
        opts.dpi = dpi.clamp(36.0, 600.0);
    }
    if let Some(mc) = body.min_confidence {
        opts.min_confidence = mc.clamp(0.0, 100.0);
    }
    opts.force = body.force;
    if let Some(psm) = body.psm {
        opts.psm = psm;
    }
    if let Some(pp) = body.preprocess.as_deref().and_then(ocr::Preprocess::parse) {
        opts.preprocess = pp;
    }

    let progress = Arc::new(ocr::OcrProgress::default());
    let job_id = Uuid::new_v4();
    state
        .ocr_jobs
        .lock()
        .unwrap()
        .insert(job_id, OcrJobState::Running(progress.clone()));

    let bg_state = state.clone();
    tokio::spawn(async move {
        let result = bg_state
            .engine
            .run(move |pdfium, _cache| ocr::ocr_document(pdfium, &path, &opts, &progress))
            .await;

        let outcome = match result {
            Ok((bytes, stats)) => {
                let filename = body.filename.unwrap_or_else(|| format!("ocr_{}", meta.filename));
                match bg_state.storage.save(filename, &bytes, None) {
                    Ok(new_meta) => OcrJobState::Done { document: new_meta.for_client(), stats },
                    Err(e) => OcrJobState::Error(format!("failed to save OCR result: {e:#}")),
                }
            }
            Err(e) => OcrJobState::Error(format!("{e:#}")),
        };
        bg_state.ocr_jobs.lock().unwrap().insert(job_id, outcome);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": job_id }))))
}

/// Poll a background OCR job. Terminal states (`done`/`error`) are removed
/// from the map on the read that observes them — a one-shot result, not a
/// cache the client can re-fetch.
async fn ocr_job_status(
    State(state): State<SharedState>,
    Path((_id, job_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let mut jobs = state.ocr_jobs.lock().unwrap();
    match jobs.get(&job_id) {
        None => Err(ApiError(StatusCode::NOT_FOUND, "no such OCR job".into())),
        Some(OcrJobState::Running(progress)) => {
            let (done, total) = progress.snapshot();
            Ok(Json(serde_json::json!({
                "status": "running",
                "pages_done": done,
                "pages_total": total,
            })))
        }
        Some(OcrJobState::Done { .. }) => {
            let Some(OcrJobState::Done { document, stats }) = jobs.remove(&job_id) else {
                unreachable!("just matched Done above");
            };
            Ok(Json(serde_json::json!({
                "status": "done",
                "document": document,
                "stats": stats,
            })))
        }
        Some(OcrJobState::Error(_)) => {
            let Some(OcrJobState::Error(message)) = jobs.remove(&job_id) else {
                unreachable!("just matched Error above");
            };
            Ok(Json(serde_json::json!({ "status": "error", "message": message })))
        }
    }
}

/// Installed Tesseract language models, for the OCR dialog's language
/// picker — reads whatever `.traineddata` files are actually present so a
/// new language just needs a file dropped into the tessdata dir, no code
/// change or redeploy.
async fn ocr_languages() -> Result<impl IntoResponse, ApiError> {
    let langs = ocr::available_languages().map_err(|e| {
        tracing::error!("ocr language list failed: {e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "tessdata unavailable".into())
    })?;
    Ok(Json(langs))
}

#[derive(Deserialize)]
struct RedactBoxBody {
    page: u16,
    /// Points, top-left origin — same convention as annotation rects.
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Deserialize)]
struct RedactBody {
    boxes: Vec<RedactBoxBody>,
    /// Rasterization DPI (36..=600). Defaults to 300.
    dpi: Option<f32>,
    /// JPEG quality (10..=100) for the flattened page image. Defaults to 90.
    jpeg_quality: Option<u8>,
    filename: Option<String>,
}

/// Redact permanently rasterizes the pages it touches, so keeping the
/// pre-redaction original around under its own id would defeat the whole
/// point — once the new (redacted) document is saved, the original working
/// copy is deleted from storage. A delete failure is logged but doesn't
/// fail the request: the caller already has their redacted document.
async fn redact_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RedactBody>,
) -> Result<impl IntoResponse, ApiError> {
    let meta = state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);

    if body.boxes.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "redact needs at least one box".into()));
    }
    let boxes: Vec<redact::RedactBox> = body
        .boxes
        .into_iter()
        .map(|b| redact::RedactBox { page: b.page, x: b.x, y: b.y, w: b.w, h: b.h })
        .collect();

    let mut opts = redact::RedactOptions::default();
    if let Some(dpi) = body.dpi {
        opts.dpi = dpi.clamp(36.0, 600.0);
    }
    if let Some(q) = body.jpeg_quality {
        opts.jpeg_quality = q.clamp(10, 100);
    }

    let (bytes, stats) = state
        .engine
        .run(move |pdfium, _cache| redact::redact_document(pdfium, &path, &boxes, &opts))
        .await?;

    let filename = body.filename.unwrap_or_else(|| format!("redacted_{}", meta.filename));
    let new_meta = state.storage.save(filename, &bytes, None)?;
    if let Err(e) = state.storage.delete(id) {
        tracing::warn!("failed to delete pre-redaction original {id}: {e}");
    }
    Ok(Json(serde_json::json!({
        "document": new_meta.for_client(),
        "stats": stats,
    })))
}

/// P11 指紋：整份檔案 SHA-256（完整性）＋每頁 average-hash（內容指紋）。
/// 兩層都現算現回（見 `fingerprint` module 註解，不快取）。SHA-256 是純
/// bytes 讀取丟 blocking pool，不管有無加密都算得出來。phash 要渲染頁面走
/// PDFium——P12 開檔密碼文件 PDFium 沒密碼打不開，先用
/// `protect::needs_open_password` 篩掉，回 `pages: []` + `locked: true`，
/// 不要讓 PDFium 打不開的錯誤流出去變成 500（P10 sidecar 同款坑重演）。
async fn fingerprint_document(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state.storage.get(id).ok_or_else(not_found)?;
    let path = state.storage.pdf_path(id);
    let hash_path = path.clone();
    let sha256 = tokio::task::spawn_blocking(move || fingerprint::sha256_file(&hash_path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;

    let check_path = path.clone();
    let locked = tokio::task::spawn_blocking(move || protect::needs_open_password(&check_path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;

    let pages = if locked {
        Vec::new()
    } else {
        state
            .engine
            .run(move |pdfium, cache| fingerprint::all_page_phashes(cache.open(pdfium, &path)?))
            .await?
    };

    Ok(Json(
        serde_json::json!({ "sha256": sha256, "pages": pages, "locked": locked }),
    ))
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

// ---------------------------------------------------------------------
// P17 動作精靈（Action Wizard）— saved pipelines + batch runs.
// Definition/execution logic lives in `crate::actions`; this section is
// just the HTTP layer over it.
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateActionBody {
    name: String,
    steps: Vec<actions::Step>,
}

async fn create_action(
    State(state): State<SharedState>,
    Json(body): Json<CreateActionBody>,
) -> Result<impl IntoResponse, ApiError> {
    let def = state
        .actions
        .create(body.name, body.steps)
        .map_err(|msg| ApiError(StatusCode::BAD_REQUEST, msg))?;
    Ok((StatusCode::CREATED, Json(def_json(&def))))
}

async fn list_actions(State(state): State<SharedState>) -> impl IntoResponse {
    let defs: Vec<_> = state.actions.list().iter().map(def_json).collect();
    Json(defs)
}

async fn get_action(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let def = state
        .actions
        .get(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "action not found".into()))?;
    Ok(Json(def_json(&def)))
}

async fn delete_action(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .actions
        .delete(id)
        .map_err(|msg| ApiError(StatusCode::NOT_FOUND, msg))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn def_json(def: &actions::ActionDef) -> serde_json::Value {
    serde_json::json!({ "id": def.id, "name": def.name, "steps": def.steps })
}

#[derive(Deserialize)]
struct RunActionBody {
    document_ids: Vec<Uuid>,
    /// Run-time-only secrets for `Step::Protect`/`Step::Encrypt`, keyed by
    /// step index. Never persisted — see `crate::actions` module docs.
    #[serde(default)]
    step_secrets: HashMap<usize, actions::StepSecrets>,
}

async fn run_action(
    State(state): State<SharedState>,
    Path(action_id): Path<Uuid>,
    Json(body): Json<RunActionBody>,
) -> Result<impl IntoResponse, ApiError> {
    let action = state
        .actions
        .get(action_id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "action not found".into()))?;
    if body.document_ids.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "run needs at least one document_id".into()));
    }
    actions::validate_secrets(&action.steps, &body.step_secrets)
        .map_err(|msg| ApiError(StatusCode::BAD_REQUEST, msg))?;
    // Steps mutate their document in place as often as they produce a new
    // one — the same id twice would re-run later steps against whatever
    // the first pass already left behind, not two independent copies.
    let mut seen = std::collections::HashSet::with_capacity(body.document_ids.len());
    for &id in &body.document_ids {
        if state.storage.get(id).is_none() {
            return Err(ApiError(StatusCode::NOT_FOUND, format!("document {id} not found")));
        }
        if !seen.insert(id) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("document {id} appears more than once in document_ids"),
            ));
        }
    }

    let run_id = Uuid::new_v4();
    let progress = Arc::new(actions::ActionRunProgress::new(body.document_ids.len(), action.steps.len()));
    state.action_runs.lock().unwrap().insert_running(run_id, progress.clone());

    let bg_state = state.clone();
    tokio::spawn(async move {
        let results = actions::run_batch(&bg_state, &action, &body.document_ids, &body.step_secrets, &progress).await;
        bg_state.action_runs.lock().unwrap().insert_done(run_id, results);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "run_id": run_id }))))
}

/// Poll a batch run. Terminal (`done`) isn't a one-shot read — unlike OCR
/// jobs, a run's results (esp. `Exported` bytes) may be fetched more than
/// once via the file/zip download routes — but it isn't kept forever
/// either; `ActionRuns` evicts the oldest finished run past a cap so
/// long-lived server memory doesn't grow without bound.
async fn action_run_status(
    State(state): State<SharedState>,
    Path(run_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let runs = state.action_runs.lock().unwrap();
    match runs.get(run_id) {
        None => Err(ApiError(StatusCode::NOT_FOUND, "no such action run".into())),
        Some(actions::ActionRunState::Running(progress)) => {
            let (current_file, total_files, current_step, total_steps) = progress.snapshot();
            Ok(Json(serde_json::json!({
                "status": "running",
                "current_file": current_file,
                "total_files": total_files,
                "current_step": current_step,
                "total_steps": total_steps,
            })))
        }
        Some(actions::ActionRunState::Done(results)) => {
            let results: Vec<_> = results
                .iter()
                .enumerate()
                .map(|(index, r)| match &r.outcome {
                    actions::FileRunOutcome::Document(meta) => serde_json::json!({
                        "source_document_id": r.source_document_id,
                        "outcome": "document",
                        "document": meta.clone().for_client(),
                    }),
                    actions::FileRunOutcome::Exported { filename, content_type, bytes } => serde_json::json!({
                        "source_document_id": r.source_document_id,
                        "outcome": "exported",
                        "index": index,
                        "filename": filename,
                        "content_type": content_type,
                        "size": bytes.len(),
                    }),
                    actions::FileRunOutcome::Failed { step_index, message } => serde_json::json!({
                        "source_document_id": r.source_document_id,
                        "outcome": "failed",
                        "step_index": step_index,
                        "message": message,
                    }),
                })
                .collect();
            Ok(Json(serde_json::json!({ "status": "done", "results": results })))
        }
    }
}

/// Download one `Exported` result by its position in the run's results
/// list (the `index` field `action_run_status` reports for that entry).
/// `Document`/`Failed` entries have no file here — download a `Document`
/// outcome via its own `/api/documents/{id}/download`.
async fn action_run_file(
    State(state): State<SharedState>,
    Path((run_id, index)): Path<(Uuid, usize)>,
) -> Result<impl IntoResponse, ApiError> {
    // Clone the one result out and drop the lock immediately — holding a
    // process-wide mutex across the response body being built (however
    // cheap here) would block every other run's status poll/insert for no
    // reason.
    let (filename, content_type, bytes) = {
        let runs = state.action_runs.lock().unwrap();
        let Some(actions::ActionRunState::Done(results)) = runs.get(run_id) else {
            return Err(ApiError(StatusCode::NOT_FOUND, "run not done (or doesn't exist)".into()));
        };
        let Some(result) = results.get(index) else {
            return Err(ApiError(StatusCode::NOT_FOUND, "no such result index".into()));
        };
        match &result.outcome {
            actions::FileRunOutcome::Exported { filename, content_type, bytes } => {
                (filename.clone(), content_type.clone(), bytes.clone())
            }
            _ => {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "result at this index isn't an exported file".into(),
                ))
            }
        }
    };

    let disposition = format!("attachment; filename*=UTF-8''{}", urlencoding::encode(&filename));
    Ok((
        [(header::CONTENT_TYPE, content_type), (header::CONTENT_DISPOSITION, disposition)],
        bytes,
    ))
}

/// Zip every `Exported` result together. 400 if the action has no `Export`
/// step (nothing to zip — download the resulting documents individually
/// via their ids instead).
async fn action_run_download(
    State(state): State<SharedState>,
    Path(run_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // Same reasoning as `action_run_file`: clone what's needed out of the
    // map and drop the lock before doing the (potentially not-cheap, for a
    // big batch) zip-write — building the zip has nothing to do with the
    // shared run map and shouldn't block other runs' polls/inserts while
    // it happens.
    let exported: Vec<(usize, String, Vec<u8>)> = {
        let runs = state.action_runs.lock().unwrap();
        let Some(actions::ActionRunState::Done(results)) = runs.get(run_id) else {
            return Err(ApiError(StatusCode::NOT_FOUND, "run not done (or doesn't exist)".into()));
        };
        results
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match &r.outcome {
                actions::FileRunOutcome::Exported { filename, bytes, .. } => {
                    Some((i, filename.clone(), bytes.clone()))
                }
                _ => None,
            })
            .collect()
    };
    if exported.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "this run has no exported files to zip (no Export step, or all files failed)".into(),
        ));
    }

    let mut seen_names = std::collections::HashSet::new();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (index, filename, bytes) in exported {
        let name = if seen_names.insert(filename.clone()) {
            filename.clone()
        } else {
            format!("{index}-{filename}")
        };
        zip.start_file(name, options).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        std::io::Write::write_all(&mut zip, &bytes)
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let cursor = zip
        .finish()
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename*=UTF-8''run-{run_id}.zip"),
            ),
        ],
        cursor.into_inner(),
    ))
}

#[cfg(test)]
mod client_error_tests {
    use super::is_client_error;

    #[test]
    fn classifies_user_faults() {
        assert!(is_client_error("page index 9 out of range (0..3)"));
        assert!(is_client_error("page size 20 pt out of range (36..=14400)"));
        assert!(is_client_error("image width/height must be positive"));
        assert!(is_client_error("encrypted documents are not supported"));
        assert!(is_client_error("document is protected; owner password required"));
        assert!(is_client_error("crop rect lies outside the page"));
    }

    #[test]
    fn crop_rect_minimum_is_still_a_client_fault() {
        // pageops.rs bails with this when the user drags too small a rect.
        assert!(is_client_error("crop rect too small: minimum is 8x8 pt"));
    }

    #[test]
    fn does_not_classify_server_faults() {
        assert!(!is_client_error("failed to spawn sidecar: Access denied"));
        assert!(!is_client_error("broken page /Contents reference"));
        assert!(!is_client_error("tessdata unavailable"));
        // font.rs: a truncated/corrupt bundled font is ours to fix, not the
        // caller's. A bare "too small" marker used to report this as 400.
        assert!(!is_client_error("font too small"));
    }
}

