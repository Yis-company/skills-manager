# Validation — Agents Manager

All required local gates passed on 2026-09-27.

| Check | Result |
| --- | --- |
| `pnpm lint` | Passed |
| `pnpm build` | Passed; existing large-chunk advisory remains |
| `pnpm test` | 187 passed in 19 files |
| `pnpm e2e` | 55 passed |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Passed |
| `cargo test --offline --manifest-path src-tauri/Cargo.toml` | 684 passed across library, CLI and integration suites; 6 existing network tests ignored |
| `cargo clippy --offline --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Passed |
| `git diff --check` | Passed |

The final Rust run includes 662 library tests, 7 CLI unit tests, 5 existing CLI integration tests, 4 new resource CLI tests, and 6 serve-protocol tests. The final scoped Instructions suite has 9 tests and the MCP suite has 16 tests. A final cleanup removed unrelated formatting churn from ProjectDetail and the E2E fixture; TypeScript and scoped lint passed afterward, with behavior retained.

## Behavior evidence

- Real-process CLI tests exercise save → dry-run → preview → apply, stale target preservation, comment/unrelated-secret preservation, a read-only preview against the v11 database without migration, and command parity over the remote stdio protocol.
- Two-device Git fixtures cover whole-bundle conflicts, retained versions, reviewed resolution, convergence, exclusion of bundled `SKILL.md` documents from skills, and cleanup after a simulated Git/SQLite completion gap.
- Instructions tests cover whole bundles, native/reference discovery, symlink boundaries, stale/retargeted previews, overlapping and non-overlapping updates, interrupted apply/undeploy recovery, and preservation of local/pre-existing content during undeploy.
- MCP tests cover JSONC/TOML/YAML scoped edits, five native adapter mappings, exact selected-entry preconditions, overrides and typed reconciliation, renamed targets, import drafts, explicit detach, safe-entry recovery after an injected database failure, and credential sentinels excluded from persisted state and responses.
- Browser tests cover navigation/reload, reviewed apply, import without implicit save, explicit library detach, multi-agent project selection, nested root-relative editing, and a delayed remote response after host switching.
- The built CLI fetched 11 public filesystem catalog results and fetched details successfully. A pinned PyPI entry produced one supported draft. Entries with unsupported runtime arguments stayed manual. No package or server was executed.
- The installed previous CLI (`1.41.2`) was run against a disposable expanded backup. It accepted the new namespace but wrote only the old protocol trailer. New-client tests reject that incompatible writer marker; coordinated upgrades are documented.

## Verification boundaries

- No real user project, credential file, or native agent configuration was changed for testing. All backend deployment fixtures used isolated temporary paths.
- The new SSH command contract was exercised through real CLI stdio processes and mocked sessions. No managed remote host deployment was performed. The existing localhost SSH test can self-skip when localhost is unavailable and is not treated here as live remote evidence.
- T3's collaborative preview rendered the desktop resource forms and supported DOM inspection/navigation. Its screenshot API repeatedly failed; no screenshot or full native desktop runtime verification is claimed.
- CodeRabbit was signed out. Independent local reviews were used instead; see `implementation-review.md`.
- No remote CI run, commit, push, release, or production installation was performed.
- External editors do not take the manager lock. Exact-byte checks occur immediately before atomic replacement, but this is not transaction isolation from other software.

Local check logs are in `/tmp/agents-manager-{lint,build,vitest,e2e}-final.log`, `/tmp/agents-manager-rust-verified.log`, `/tmp/agents-manager-clippy-final.log`, and `/tmp/agents-manager-fmt-verified.log`.
