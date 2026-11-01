# Validation: selected upstream fixes

Plannotator approved this plan on 2026-11-01 without notes. Implementation targets fork base `241a807`, adapting selected upstream fixes through `fccb4e8`.

## Local checks

- `pnpm format:check`: passed.
- `pnpm lint`: passed.
- `pnpm test`: 216 passed across 24 files.
- `pnpm build`: passed.
- `pnpm e2e`: 105 passed, including removal confirmation, Escape, fresh scan and navigation cancellation, and relocation error messaging.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --locked`: 783 passed, 7 existing tests ignored, no failures. Covers library, CLI, CLI Sync, resources and stdio host integration.

## Regression evidence

Before changing Markdown rendering, the 14 new behavior tests produced 7 failures and 7 passes. All 14 pass after the fix. Rust regressions cover URL normalization; refused saves preserving configuration; save/startup agreement for relocation targets, including symlinks; changed and unchanged monorepo siblings; ancestor .gitattributes; unavailable installed revisions; comparison failures; and a shared remote resolution for siblings installed at different revisions.

Old WebKit is simulated by rejecting lookbehind during module initialization; no old macOS runtime was available. Browser tests use the existing fake Tauri backend. Windows and Linux execution remains for PR CI; local validation ran on macOS.

The full Rust run spent time hashing in the existing agent-workspace backfill scanner and then completed. No test exclusions or timeout changes were added.

## Upstream source attribution

- [95d7d6d0](https://github.com/xingkongliang/skills-manager/commit/95d7d6d08a3b3105d4cb83e74419660daeb46cd7): fix(projects): confirm before removing a skill from a workspace via the library chip (#542)
- [e5f9683d](https://github.com/xingkongliang/skills-manager/commit/e5f9683dff5a1946536e4804092edd2206789d2b): fix(projects): keep Escape on the remove confirmation from closing the skill sheet
- [811a1f56](https://github.com/xingkongliang/skills-manager/commit/811a1f56c20d8bd8a41a45d0d5f6382d19566033): fix(project): confirm before deleting workspace copies that are not in sync
- [1a814f3d](https://github.com/xingkongliang/skills-manager/commit/1a814f3d5df67aeaf842d2aaf9235d50e860031b): fix(project): re-read sync status before deleting and settle stale confirmations
- [3c6995de](https://github.com/xingkongliang/skills-manager/commit/3c6995de5bc07c6b72d5c804befc9ad035f2eeb7): fix(settings): refuse a central repo path the next launch could not move to (#596)
- [53dc88c5](https://github.com/xingkongliang/skills-manager/commit/53dc88c53e657f143c92225537f4c1ff42e94360): fix(update-check): stop flagging monorepo siblings on unrelated commits (#561)
- [a14e3211](https://github.com/xingkongliang/skills-manager/commit/a14e321146afdf065975147bca036654e4b81ea2): fix(update-check): count ancestor .gitattributes in the subtree comparison (#561)
- [fccb4e8a](https://github.com/xingkongliang/skills-manager/commit/fccb4e8a488c1c687bd8d82abb70edd1c2367baf): fix(update-check): treat failed subtree comparisons as failed checks (#583)
- [077eed84](https://github.com/xingkongliang/skills-manager/commit/077eed84571ccedee4c31bdfd70a401096a8831a): fix(git-fetcher): normalize owner/repo.git and ssh:// URLs correctly (#517)
- [282f0fd6](https://github.com/xingkongliang/skills-manager/commit/282f0fd66216c3fe4df2c184229c64b1769adb54): fix(preview): handle CRLF and empty skill frontmatter (#620)
- [478035a7](https://github.com/xingkongliang/skills-manager/commit/478035a79ccbe85a9640e054aebbebf669cc41f3): fix(preview): render plain Markdown where regex lookbehind is unsupported (#613) (#627)

No upstream merge, hash cache, schema migration, locale additions, adapter additions, dependency/lockfile changes or release workflow changes are included.
