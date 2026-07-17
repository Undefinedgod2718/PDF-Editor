//! 桌面版專屬路由 `/api/local/*`（ADR-004）。
//!
//! 只掛在 desktop 內嵌 axum 上，多人 server 永不暴露（open-by-path 對
//! 網路服務是任意檔案讀取漏洞；對本機 app 是它存在的意義）。與其他
//! `/api/*` 一樣吃 token guard。native dialog 由 Rust 端 tauri-plugin-dialog
//! 開啟 — 前端只需 fetch，不碰 Tauri JS API。

use std::sync::OnceLock;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use pdf_editor_server::SharedState;

/// setup 時填入；dialog 端點用。axum 執行緒與 Tauri 主迴圈分離，
/// OnceLock 是最小同步機制。
pub static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/local/ping", get(ping))
        .route("/api/local/open", post(open_by_path))
        .route("/api/local/open-dialog", post(open_dialog))
        .route("/api/local/documents/{id}/save", post(save))
        .route("/api/local/documents/{id}/save-as", post(save_as_path))
        .route(
            "/api/local/documents/{id}/save-as-dialog",
            post(save_as_dialog),
        )
}

fn err(status: StatusCode, e: impl std::fmt::Display) -> Response {
    (status, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
}

/// 前端 mode 偵測：local build 下有此端點（200），web 版 404。
async fn ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "mode": "local" }))
}

#[derive(Deserialize)]
struct OpenReq {
    path: String,
}

/// 檔案關聯/CLI/測試入口：已知路徑直接開。
async fn open_by_path(
    State(state): State<SharedState>,
    Json(req): Json<OpenReq>,
) -> Response {
    match state.storage.open_path(&req.path) {
        Ok(meta) => Json(meta.for_client()).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

/// 人操作入口：native 開檔對話框。使用者取消 → 204。
async fn open_dialog(State(state): State<SharedState>) -> Response {
    let Some(app) = APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    let picked = pick_file(app.clone()).await;
    match picked {
        Some(path) => match state.storage.open_path(&path) {
            Ok(meta) => Json(meta.for_client()).into_response(),
            Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
        },
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

#[derive(Deserialize, Default)]
struct SaveReq {
    #[serde(default)]
    force: bool,
}

/// Ctrl+S：工作副本原子寫回來源檔。409 = 外部改檔，前端問過使用者
/// 後帶 `force: true` 重送。
async fn save(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    body: Option<Json<SaveReq>>,
) -> Response {
    let force = body.map(|Json(b)| b.force).unwrap_or(false);
    match state.storage.save_to_origin(id, force) {
        Ok(meta) => Json(meta.for_client()).into_response(),
        Err(e) if e.to_string().contains("changed on disk") => err(StatusCode::CONFLICT, e),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

#[derive(Deserialize)]
struct SaveAsReq {
    path: String,
}

/// 測試/自動化入口：另存到已知路徑。
async fn save_as_path(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SaveAsReq>,
) -> Response {
    match state.storage.save_as(id, &req.path) {
        Ok(meta) => Json(meta.for_client()).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

/// 人操作入口：native 另存對話框。取消 → 204。
async fn save_as_dialog(State(state): State<SharedState>, Path(id): Path<Uuid>) -> Response {
    let Some(app) = APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    let default_name = state
        .storage
        .get(id)
        .map(|m| m.filename)
        .unwrap_or_else(|| "document.pdf".into());
    let picked = pick_save_file(app.clone(), default_name).await;
    match picked {
        Some(path) => match state.storage.save_as(id, &path) {
            Ok(meta) => Json(meta.for_client()).into_response(),
            Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
        },
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// dialog callback 轉 async：plugin 的 pick_file 走主迴圈回呼，
/// oneshot 接回 axum 執行緒。
async fn pick_file(app: AppHandle) -> Option<std::path::PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("PDF", &["pdf"])
        .pick_file(move |file| {
            let _ = tx.send(file.and_then(|f| f.into_path().ok()));
        });
    rx.await.ok().flatten()
}

async fn pick_save_file(app: AppHandle, default_name: String) -> Option<std::path::PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("PDF", &["pdf"])
        .set_file_name(&default_name)
        .save_file(move |file| {
            let _ = tx.send(file.and_then(|f| f.into_path().ok()));
        });
    rx.await.ok().flatten()
}
