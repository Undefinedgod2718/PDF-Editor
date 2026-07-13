# PDF Editor

Web PDF editor — Rust (Axum) backend + React (Vite) frontend. Single-port deployment (default `8050`).

## Quick start

```powershell
cd web && npm ci && npm run build
cd server
$env:CARGO_TARGET_DIR="$PWD\target"
cargo run --release
```

Open http://localhost:8050

Requires `server/pdfium.dll` (Windows x64, included in this repo).

## Releases

Tagged milestones: [GitHub Releases](https://github.com/Undefinedgod2718/PDF-Editor/releases). Current release candidate: **v0.2.0** (Phase 3+4: page ops, content edit, forms, signatures). Stable tag until release: **v0.1.0**.
