# Release checklist

Use this when a Phase is validated and ready to ship.

## Phase 3 → v0.2.0

1. Run verification (see local `wiki/Verification.md`): `cargo build`, `cargo test`, `npm run build`, manual UI smoke test.
2. Commit Phase 3 work on `main` with clear `feat:` messages.
3. Bump version in `server/Cargo.toml` and `web/package.json` to `0.2.0`.
4. Push `main`.
5. Tag and release:

```powershell
git tag -a v0.2.0 -m "Phase 3: content editing and page operations"
git push origin v0.2.0
gh release create v0.2.0 --title "v0.2.0" --notes "Phase 3: text edit, page rotate/delete/insert/reorder, merge/extract."
```

## Later milestones

| Phase | Version |
|-------|---------|
| 4 — forms & signatures | v0.3.0 |
| 5 — production deploy | v0.4.0 or v1.0.0 |

Patch releases (`0.x.z`): bug fixes only, no new Phase scope.
