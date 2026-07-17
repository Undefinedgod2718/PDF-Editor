//! 單人服務桌面殼（ADR-002/003 Phase 1）。
//!
//! 內嵌 `server` lib 的 axum router，bind `127.0.0.1:0`（隨機埠），
//! 每次啟動產生一次性 token；WebView 以 initialization script 種
//! cookie，`/api/*` 之外（靜態前端檔）不驗 token，`/api/*` 缺 token
//! 一律 401 — 防同機其他程序打 loopback API。終態（Phase 2）遷
//! Tauri IPC 後關閉整個 port。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tower_http::services::{ServeDir, ServeFile};

use pdf_editor_server::{api, pdf, sidecar, storage, AppState, SharedState};

const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
const TOKEN_COOKIE: &str = "pdfed_token";

/// 一次性 session token：兩個 v4 UUID 串接（2×122 bit CSPRNG）。
/// 只存在記憶體與 WebView cookie，不落磁碟、不進 log。
/// `PDF_EDITOR_TOKEN` 可覆蓋 — 僅供煙霧測試/CI 對 API 打已知 token，
/// 正常桌面啟動不得設定。
fn generate_token() -> String {
    if let Ok(t) = std::env::var("PDF_EDITOR_TOKEN") {
        tracing::warn!("PDF_EDITOR_TOKEN override in effect — test/CI use only");
        return t;
    }
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// `/api/*` 需帶 token（cookie 或 Bearer header）；靜態前端檔放行。
async fn require_token(
    token: Arc<String>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !req.uri().path().starts_with("/api") {
        return Ok(next.run(req).await);
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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let storage = storage::Storage::new(data_dir())?;
    let engine = pdf::engine::PdfEngine::spawn()?;
    let state: SharedState = Arc::new(AppState { storage, engine });

    match sidecar::health() {
        Ok(_) => tracing::info!("office sidecar ready"),
        // 桌面版缺 sidecar 屬預期（ADR-005）：docx/xlsx 匯出 UI 應偵測停用
        Err(e) => tracing::warn!("office sidecar unavailable — docx/xlsx export disabled: {e}"),
    }

    let token = Arc::new(generate_token());

    let web_dist = std::env::var("PDF_EDITOR_WEB").unwrap_or_else(|_| "../web/dist".into());
    let index = format!("{web_dist}/index.html");
    let static_files = ServeDir::new(&web_dist).fallback(ServeFile::new(&index));

    let guard_token = token.clone();
    let app = api::router()
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
                    axum::serve(listener, app).await.expect("axum serve");
                });
        })?;

    // token 先種 cookie 再載頁：init script 在每次 navigation 的
    // document start 執行，頁面 JS 的所有 same-origin fetch 自動帶上。
    let init_script = format!("document.cookie = \"{TOKEN_COOKIE}={token}; path=/\";");
    let url: tauri::Url = format!("http://127.0.0.1:{port}/").parse()?;

    tauri::Builder::default()
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url.clone()),
            )
            .title("PDF Editor")
            .inner_size(1440.0, 900.0)
            .initialization_script(&init_script)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri run");
    Ok(())
}
