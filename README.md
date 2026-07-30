# PDF Editor

**A PDF editor you can run locally — and hand to your agent.**

---

PDF Editor is an open source PDF workspace: view, annotate, edit pages, fill forms, and protect documents. Use it as a single-port web app, or as a Windows desktop shell. Connect Claude / Cursor via MCP to drive the same backend from your agent.

### Local web + desktop

One Rust (Axum) backend and one React UI. Run the web server on `localhost`, or install the Windows MSI (Tauri) and work offline with the system WebView.

### Annotate, edit, export

Highlight, draw, stamps, signatures, page ops, crop/resize, compress, redact, OCR, Office conversion, and more — built for day-to-day PDF work, not demos.

### Built for agents (MCP)

A stdio MCP server forwards tools to the local HTTP backend. Keep the editor running, then let Claude Code or Cursor call upload / render / save / export on your machine.

## Quick start

```powershell
cd web && npm ci && npm run build
cd ../server
$env:CARGO_TARGET_DIR="$PWD\target"
cargo run --release
```

Open http://localhost:8050

Requires `server/pdfium.dll` (Windows x64, included in this repo).

### Desktop MSI (Windows)

```powershell
.\scripts\build-msi.ps1
```

Installers and tagged builds: [GitHub Releases](https://github.com/Undefinedgod2718/PDF-Editor/releases).

## MCP server

With the PDF Editor backend already running (default `http://127.0.0.1:8050`):

```powershell
cd mcp-server
$env:CARGO_TARGET_DIR="$PWD\..\target-mcp-server"
cargo run --release
```

Point your client at the `pdf-editor-mcp-server` binary over stdio. Override the backend with `PDF_EDITOR_URL` if needed.

**Claude Code**

```bash
claude mcp add --transport stdio pdf-editor -- /path/to/pdf-editor-mcp-server
```

The Windows MSI can register MCP for Claude / Cursor at install time when those tools are present.

## FAQ

**What platforms does it support?**

- Web backend: Windows / Linux (pdfium + fonts as documented in-tree).
- Desktop MSI: Windows (WiX / Tauri).

**Is this a hosted cloud service?**

No. This repository is for local and self-hosted use. There is no cloud deploy surface in the public tree.

**Where are releases?**

[GitHub Releases](https://github.com/Undefinedgod2718/PDF-Editor/releases). Current desktop line: **v0.3.x**.

## Development

```powershell
# Frontend
cd web && npm ci && npm run build

# Backend tests
cd server
$env:CARGO_TARGET_DIR="$PWD\target"
cargo test

# Desktop MSI
.\scripts\build-msi.ps1
```

See [`.github/RELEASE_CHECKLIST.md`](.github/RELEASE_CHECKLIST.md) for ship steps.

## License

Source in this repository is provided for use and contribution under the terms stated by the project maintainers. If a `LICENSE` file is added later, that file is authoritative.
