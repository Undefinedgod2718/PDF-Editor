# Release checklist

Use this when a Phase is validated and ready to ship.

## Desktop MSI (Windows)

CI job `windows-msi` builds `desktop/target/release/bundle/msi/*.msi` on every
`main` push / PR. Tag pushes (`v*`) also attach the MSI to the GitHub Release.

Local build (Windows only):

```powershell
.\deploy\windows\build-msi.ps1
```

ICE57 rule: HKCU file associations must **not** share a Component with the
per-machine `Path` exe (`desktop/windows/main.wxs`).

## v0.3.1 — desktop shell + WiX MSI

1. Verification: `cargo build`, `cargo test`, `npm run build`, MSI smoke install.
2. Merge release PR to `main`.
3. Tag and release (CI uploads MSI after the tag build finishes):

```powershell
git tag -a v0.3.1 -m "Desktop shell + Windows WiX MSI"
git push origin v0.3.1
gh release create v0.3.1 --title "v0.3.1" --notes "## Windows desktop`n- Tauri MSI installer`n- Open With PDF registration + optional default-PDF checkbox`n`n## Fix`n- ICE57: file associations split from Path component"
```

If CI has already produced the MSI artifact, attach manually:

```powershell
gh release upload v0.3.1 ".\desktop\target\release\bundle\msi\PDF Editor_0.3.1_x64_zh-TW.msi" --clobber
```

## v0.2.0 — Phase 3 + Phase 4

Includes content editing / page operations (P3) and forms / signatures (P4).

1. Verification: `cargo build`, `cargo test`, `npm run build`, manual UI smoke test.
2. Bump versions to `0.2.0`.
3. Merge release PR to `main`.
4. Tag and release (see git history for notes template).

## Later milestones

| Milestone | Version |
|-----------|---------|
| Phase 5 — production deploy (192.168.17.56) | v0.3.0 or v1.0.0 |

Patch releases (`0.x.z`): bug fixes only, no new Phase scope.
