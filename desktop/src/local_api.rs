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
use tauri::{AppHandle, Manager};
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
        .route("/api/local/close", post(close_window))
        .route("/api/local/print", post(print_document))
        .route("/api/local/documents/{id}/save", post(save))
        .route("/api/local/documents/{id}/save-as", post(save_as_path))
        .route(
            "/api/local/documents/{id}/save-as-dialog",
            post(save_as_dialog),
        )
        .route("/api/local/documents/{id}/dirty", get(dirty))
        .route("/api/local/fullscreen", post(set_fullscreen))
        .merge(crate::recent::router())
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
        Ok(meta) => {
            // 只有真的開檔成功才記「最近使用」；記錄本身是 best-effort，
            // 壞掉也不影響這支端點回應（見 recent.rs 模組註解）。
            crate::recent::record_open(&state, std::path::Path::new(&req.path), meta.id).await;
            Json(meta.for_client()).into_response()
        }
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

/// 關窗確認流程（ADR-004 dirty 追蹤）最後一步：前端問完使用者、確定要關，
/// 呼叫這支再真的關窗。`destroy()` 不再觸發 `CloseRequested`，不會被
/// `main.rs` 的攔截器再擋一次，避免無限迴圈。
async fn close_window() -> Response {
    let Some(app) = APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.destroy();
            StatusCode::NO_CONTENT.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "window not found"),
    }
}

#[derive(Deserialize)]
struct FullscreenReq {
    fullscreen: bool,
}

/// F11／工具列切換全螢幕（沉浸閱讀）。只管「main」視窗本身進出 OS 全螢幕；tab bar／
/// toolbar 的隱藏是前端 CSS 的事（見 web/src/app.css `data-fullscreen`），這裡不碰
/// UI chrome。前端無論走哪個觸發點（F11／按鈕／Escape）一律呼叫同一支函式再打這支
/// 端點，兩邊狀態才不會分岔（見 App.tsx 的 enterFullscreen／exitFullscreen 註解）。
async fn set_fullscreen(Json(req): Json<FullscreenReq>) -> Response {
    let Some(app) = APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    match app.get_webview_window("main") {
        Some(window) => match window.set_fullscreen(req.fullscreen) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        None => err(StatusCode::NOT_FOUND, "window not found"),
    }
}

/// dirty = revision != saved_revision（見 pdf-core storage.rs 註解）。直接讀記憶體中的
/// meta，不必經 PDFium engine，前端可以隨便問（每次編輯後）也不貴。
async fn dirty(State(state): State<SharedState>, Path(id): Path<Uuid>) -> Response {
    match state.storage.get(id) {
        Some(meta) => Json(serde_json::json!({ "dirty": meta.revision != meta.saved_revision }))
            .into_response(),
        None => err(StatusCode::NOT_FOUND, "document not found"),
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
            Ok(meta) => {
                crate::recent::record_open(&state, &path, meta.id).await;
                Json(meta.for_client()).into_response()
            }
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrintReq {
    doc_id: Uuid,
    /// 0-based 頁碼；省略或空陣列＝整份。
    #[serde(default)]
    pages: Vec<u16>,
    /// 預設連註解一起印（同 Acrobat 的「文件和標記」）。
    #[serde(default = "print_annotations_default")]
    annotations: bool,
    /// 指定印表機＝不跳對話框。給自動化與「直接印到某台」用；一般流程留空。
    #[serde(default)]
    printer: Option<String>,
    /// 搭配 printer 使用：輸出檔路徑（例如印到「Microsoft Print to PDF」時的存檔位置），
    /// 給了就不會再跳存檔對話框。
    #[serde(default)]
    output: Option<String>,
}

fn print_annotations_default() -> bool {
    true
}

/// 系統列印（③）。網頁版沒有這支——瀏覽器碰不到印表機 DC，那邊走 window.print()。
///
/// 整段（含使用者在對話框前面猶豫的時間）跑在 PDFium worker 上，因為 render 一定
/// 要在那條執行緒。單人桌面版可以接受：這期間本來也不會有別的 PDF 操作在跑。
async fn print_document(State(state): State<SharedState>, Json(req): Json<PrintReq>) -> Response {
    let Some(app) = APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    if state.storage.get(req.doc_id).is_none() {
        return err(StatusCode::NOT_FOUND, "document not found");
    }
    let path = state.storage.pdf_path(req.doc_id);
    // 對話框要有 owner 才會正確置中並擋住主視窗；拿不到就傳 null，對話框仍會開。
    // HWND 內含裸指標、不是 Send，不能直接搬進 worker closure；傳整數過去再重建。
    let hwnd_raw = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0);

    let result = state
        .engine
        .run(move |pdfium, _cache| {
            let job = crate::print::PrintJob {
                pages: &req.pages,
                annotations: req.annotations,
                printer: req.printer.as_deref(),
                output: req.output.as_deref(),
            };
            let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
            crate::print::print_document(pdfium, &path, &job, hwnd)
        })
        .await;

    match result {
        // 0 頁＝使用者在對話框按了取消。那不是錯誤，但前端要能分辨「印了」與「沒印」。
        Ok(pages) => Json(serde_json::json!({ "printed": pages })).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}
