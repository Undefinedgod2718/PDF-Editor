//! 單人服務桌面殼（ADR-002/003 Phase 1）。
//!
//! 內嵌 `server` lib 的 axum router，bind `127.0.0.1:0`（隨機埠），
//! 每次啟動產生一次性 token。雙軌認證：WebView initialization script 攔截
//! `fetch`，自動加 `Authorization: Bearer <token>`；同時伺服器對非 `/api`
//! 回應（首頁、靜態檔）補一個 `HttpOnly` cookie，讓 `<img src>`／
//! `<a download>` 這類不經過 `window.fetch` 的請求也能帶認證——`HttpOnly`
//! 讓 XSS 讀不到 cookie 值，token 仍不會透過 `document.cookie` 外洩。
//! `/api/*` 兩者都缺一律 401。終態（Phase 2）遷 Tauri IPC 後關閉整個 port。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tower_http::services::{ServeDir, ServeFile};

use pdf_editor_server::{actions, api, pdf, sidecar, storage, AppState, SharedState};

mod local_api;
mod print;
mod recent;

const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
const TOKEN_COOKIE: &str = "pdfed_token";
const API_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// 一次性 session token：兩個 v4 UUID 串接（2×122 bit CSPRNG）。
/// 只存在記憶體與 WebView init-script 閉包，不落磁碟、不進 log。
/// `PDF_EDITOR_TOKEN` 可覆蓋 — 僅供煙霧測試/CI；必須為非空 hex，
/// 避免寫入 JS 字面值時注入。正常桌面啟動不得設定。
fn generate_token() -> anyhow::Result<String> {
    if let Ok(t) = std::env::var("PDF_EDITOR_TOKEN") {
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("PDF_EDITOR_TOKEN must be non-empty hexadecimal");
        }
        tracing::warn!("PDF_EDITOR_TOKEN override in effect — test/CI use only");
        return Ok(t);
    }
    Ok(format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    ))
}

/// `/api/*` 需帶 token（Bearer 優先；cookie 亦可）；靜態前端檔放行，但順便在
/// 回應上補一個 `HttpOnly` cookie（見下方 `set_auth_cookie`）——`<img src>`／
/// `<a download>` 這類不經過 `window.fetch` 的請求靠這個 cookie 自動帶認證，
/// `HttpOnly` 讓注入的 XSS 讀不到，不會重新打開 auth_init_script 原本要避免
/// 的「token 進 document.cookie 可被 JS 讀出」風險。
async fn require_token(
    token: Arc<String>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !req.uri().path().starts_with("/api") {
        let mut res = next.run(req).await;
        set_auth_cookie(&mut res, &token);
        return Ok(res);
    }
    let cookie_ok = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|cookies| {
            cookies
                .split(';')
                .any(|kv| kv.trim() == format!("{TOKEN_COOKIE}={token}"))
        })
        .unwrap_or(false);
    let bearer_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|h| h == format!("Bearer {token}"))
        .unwrap_or(false);
    if cookie_ok || bearer_ok {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// `token` 全 ascii hex（見 `generate_token`），直接格式化進 header value 不會
/// 產生非法字元/換行注入。沒有 `Secure`：整個 app 只跑 `http://127.0.0.1`，
/// 加了 `Secure` 瀏覽器會整條丟掉這個 cookie。
fn set_auth_cookie(res: &mut Response, token: &str) {
    if let Ok(value) =
        header::HeaderValue::from_str(&format!("{TOKEN_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/"))
    {
        res.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// 桌面版資料目錄：`PDF_EDITOR_DATA` 覆蓋，否則平台慣例路徑。
fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PDF_EDITOR_DATA") {
        return dir.into();
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        format!(
            "{}/.local/share",
            std::env::var("HOME").unwrap_or_else(|_| ".".into())
        )
    });
    std::path::PathBuf::from(base).join("pdf-editor")
}

/// 前端靜態檔：`PDF_EDITOR_WEB` 覆蓋；打包後預設 `<exe>/web/dist`。
fn resolve_web_dist() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("PDF_EDITOR_WEB") {
        let dir = PathBuf::from(dir);
        if dir.join("index.html").is_file() {
            return Ok(dir);
        }
        anyhow::bail!("PDF_EDITOR_WEB has no index.html: {}", dir.display());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled = exe_dir.join("web/dist");
            if bundled.join("index.html").is_file() {
                return Ok(bundled);
            }
        }
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    if development.join("index.html").is_file() {
        return Ok(development);
    }
    anyhow::bail!("web frontend not found beside executable or in development tree")
}

fn is_pdf_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// 檔案關聯／CLI：`pdf-editor-desktop.exe path\to\file.pdf`
fn startup_pdf_from_argv() -> Option<PathBuf> {
    for arg in std::env::args_os().skip(1) {
        if arg.to_str().is_some_and(|value| value.starts_with('-')) {
            continue;
        }
        let path = PathBuf::from(arg);
        if is_pdf_path(&path) {
            return Some(path);
        }
    }
    None
}

/// 拖放進視窗：Tauri 攔截 OS 層 drag-drop（`dragDropEnabled` 預設 true），拿到真實路徑，
/// 不像瀏覽器 File API 只給 blob。多檔只挑第一個 .pdf。
fn first_pdf_path(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths.iter().find(|p| is_pdf_path(p))
}

/// 拖放成功：eval 一段 JS，在前端 dispatch 自訂事件帶 doc id，讓 App.tsx 走既有 openDocById。
/// 不用 `@tauri-apps/api`（ADR-003 精神：前端只 fetch，不碰 Tauri JS API），純字串注入同 auth_init_script。
fn dispatch_open_doc_js(id: &uuid::Uuid) -> String {
    let id_js = serde_json::to_string(&id.to_string()).expect("uuid");
    format!(
        r#"window.dispatchEvent(new CustomEvent("pdf-editor:open-doc", {{ detail: {{ id: {id_js} }} }}));"#
    )
}

fn dispatch_open_error_js(message: &str) -> String {
    let message_js = serde_json::to_string(message).expect("error is valid string");
    format!(
        r#"window.dispatchEvent(new CustomEvent("pdf-editor:open-error", {{ detail: {{ message: {message_js} }} }}));"#
    )
}

/// Init script：用 JSON 字面值帶入 token，攔截 fetch 自動加 Bearer。
/// 不寫 `document.cookie`，避免 XSS 直接讀出 token 字串。
fn auth_init_script(token: &str) -> String {
    let token_js = serde_json::to_string(token).expect("token is plain hex");
    format!(
        r#"(function () {{
  var token = {token_js};
  var bearer = "Bearer " + token;
  var orig = window.fetch.bind(window);
  window.fetch = function (input, init) {{
    init = init || {{}};
    var headers = new Headers(init.headers || {{}});
    if (!headers.has("Authorization")) {{
      headers.set("Authorization", bearer);
    }}
    init.headers = headers;
    return orig(input, init);
  }};
}})();"#
    )
}

/// 等內嵌 axum 真正開始 accept，避免 WebView 首屏連線失敗。
fn wait_until_accepting(port: u16, timeout: Duration) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let start = Instant::now();
    while start.elapsed() < timeout {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!("embedded API on 127.0.0.1:{port} did not become ready in {timeout:?}")
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let storage = storage::Storage::new(data_dir())?;
    let engine = pdf::engine::PdfEngine::spawn()?;
    let actions_store = actions::ActionStore::new(data_dir().join("actions"))?;

    let (startup_doc, startup_error) = match startup_pdf_from_argv() {
        Some(path) => match storage.open_path(&path) {
            Ok(meta) => {
                tracing::info!("opened argv PDF: {}", path.display());
                (Some(meta), None)
            }
            Err(e) => {
                let message = format!("無法開啟 {}：{e}", path.display());
                tracing::warn!("{message}");
                (None, Some(message))
            }
        },
        None => (None, None),
    };
    let state: SharedState = Arc::new(AppState {
        storage,
        engine,
        ocr_jobs: Default::default(),
        actions: actions_store,
        action_runs: Default::default(),
    });
    let dnd_state = state.clone();

    match sidecar::health() {
        Ok(_) => tracing::info!("office sidecar ready"),
        // 桌面版缺 sidecar 屬預期（ADR-005）：docx/xlsx 匯出 UI 應偵測停用
        Err(e) => tracing::warn!("office sidecar unavailable — docx/xlsx export disabled: {e}"),
    }

    let token = Arc::new(generate_token()?);

    let web_dist = resolve_web_dist()?;
    let index = web_dist.join("index.html");
    let static_files = ServeDir::new(&web_dist).fallback(ServeFile::new(index));

    let guard_token = token.clone();
    let app = api::router()
        .merge(local_api::router())
        .with_state(state)
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(move |req, next| {
            let token = guard_token.clone();
            require_token(token, req, next)
        }));

    // 先用 std listener 同步拿到隨機埠，再交給 tokio。
    let listener = std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    tracing::info!("embedded API listening on 127.0.0.1:{port}");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    std::thread::Builder::new()
        .name("embedded-axum".into())
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)
                        .expect("tokio listener from std");
                    let shutdown = async {
                        let _ = shutdown_rx.await;
                    };
                    if let Err(e) = axum::serve(listener, app)
                        .with_graceful_shutdown(shutdown)
                        .await
                    {
                        tracing::error!("axum serve ended with error: {e}");
                    }
                });
        })?;

    wait_until_accepting(port, API_READY_TIMEOUT)?;

    let mut init_script = auth_init_script(token.as_str());
    if let Some(meta) = startup_doc {
        let id_js = serde_json::to_string(&meta.id.to_string()).expect("uuid");
        init_script.push_str(&format!(
            r#"
(function () {{
  var id = {id_js};
  window.__PDF_EDITOR_STARTUP_DOC__ = id;
}})();"#
        ));
    }
    if let Some(message) = startup_error {
        let message_js = serde_json::to_string(&message).expect("error is valid string");
        init_script.push_str(&format!(
            r#"
(function () {{
  window.__PDF_EDITOR_STARTUP_ERROR__ = {message_js};
}})();"#
        ));
    }
    let url: tauri::Url = format!("http://127.0.0.1:{port}/").parse()?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let _ = local_api::APP_HANDLE.set(app.handle().clone());
            let window =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url.clone()))
                    .title("PDF Editor")
                    .inner_size(1440.0, 900.0)
                    .initialization_script(&init_script)
                    .build()?;

            let evt_window = window.clone();
            window.on_window_event(move |event| match event {
                tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                    let Some(path) = first_pdf_path(paths) else {
                        return;
                    };
                    let js = match dnd_state.storage.open_path(path) {
                        Ok(meta) => {
                            tracing::info!("opened dropped PDF: {}", path.display());
                            dispatch_open_doc_js(&meta.id)
                        }
                        Err(e) => {
                            let message = format!("無法開啟 {}：{e}", path.display());
                            tracing::warn!("{message}");
                            dispatch_open_error_js(&message)
                        }
                    };
                    if let Err(e) = evt_window.eval(js) {
                        tracing::warn!("drag-drop eval failed: {e}");
                    }
                }
                // 關窗一律先攔截，前端才有機會問「未存修改」；確認後前端呼叫
                // `/api/local/close`，該端點用 `destroy()` 真正關窗（不再走這個攔截）。
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    if let Err(e) = evt_window.eval(
                        r#"window.dispatchEvent(new CustomEvent("pdf-editor:close-requested"));"#,
                    ) {
                        // 前端收不到事件就永遠不會呼叫 /api/local/close，視窗會卡死關不掉
                        // （使用者只能工作管理員砍）。eval 失敗代表 webview 已經壞了，正常
                        // 詢問流程走不通，直接強制關窗比卡住合理。
                        tracing::warn!("close-requested eval failed: {e}; forcing close");
                        let _ = evt_window.destroy();
                    }
                }
                _ => {}
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri build")
        .run(move |_app, event| {
            if matches!(
                event,
                tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
            ) {
                if let Ok(mut guard) = shutdown_tx.lock() {
                    if let Some(tx) = guard.take() {
                        let _ = tx.send(());
                    }
                }
            }
        });
    Ok(())
}
