# Anti-slop provenance

- Source: the `install-anti-slop` agent skill (`assets/anti-slop/`), copied with its `scripts/install.mjs` on 2026-10-01.
- Source commit: unknown. The skill was installed without a recorded repository or revision, so no pristine upstream snapshot can be identified.
- Installed paths: `tools/oxlint/anti-slop/index.ts` (generic plugin, registered in `.oxlintrc.json`). `effect/` is copied but not registered because the project does not depend on `effect`.
- Vendored third-party code: `vendor/eslint-stylistic/` (see its own `UPSTREAM.md` and `LICENSE`).
- Local deviations: none.
