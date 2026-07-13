# Release checklist

Use this when a Phase is validated and ready to ship.

## v0.2.0 — Phase 3 + Phase 4 (this release)

Includes content editing / page operations (P3) and forms / signatures (P4).

1. Verification: `cargo build`, `cargo test`, `npm run build`, manual UI smoke test.
2. Bump `server/Cargo.toml` and `web/package.json` to `0.2.0`.
3. Merge release PR to `main`.
4. Tag and release:

```powershell
git tag -a v0.2.0 -m "Phase 3+4: page ops, content edit, forms, signatures"
git push origin v0.2.0
gh release create v0.2.0 --title "v0.2.0" --notes-file - <<'EOF'
## Phase 3 — content & pages
- Page rotate / delete / insert / reorder / merge / extract
- Crop / resize / insert-from
- Text object edit, font subsetting, stamps, Excalidraw drawing

## Phase 4 — forms & signatures
- AcroForm list / fill
- Signature pad UI
EOF
```

## Later milestones

| Milestone | Version |
|-----------|---------|
| Phase 5 — production deploy (192.168.17.56) | v0.3.0 or v1.0.0 |

Patch releases (`0.x.z`): bug fixes only, no new Phase scope.
