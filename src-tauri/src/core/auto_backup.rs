//! Automatic backup (§3.4): after central-repo changes settle for a couple of
//! minutes, commit and push in the background — no snapshot tag (tags are
//! reserved for user-visible backup points).
//!
//! With the object merge engine (merge-engine design §9 3d-γ) a round that
//! finds the remote ahead also merges it and pushes, so two devices converge
//! hands-free. Conflicts never pause a round (SID-108): a pending skill keeps
//! its local version while everything else keeps syncing. Only a remote
//! written in a newer Sync format stops the round until the app is updated
//! (SID-101).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::central_repo;
use super::git_backup;
use super::merge;
use super::repo_lock::RepoLock;
use super::skill_store::SkillStore;
use super::sync_metadata;

/// Off-switch setting. Anything except an explicit "off"-ish value means on.
pub const SETTING_ENABLED: &str = "backup_auto_enabled";
/// Last auto-backup failure, kept until a successful push (auto or manual)
/// clears it, so the Backup page failure card survives navigation/restart.
pub const SETTING_LAST_ERROR: &str = "backup_last_auto_error";
const EVENT_COMPLETED: &str = "backup-auto-completed";
const AUTO_COMMIT_MESSAGE: &str = "auto backup";

/// Trailing debounce after the last central-repo change (§3.4 分钟级防抖).
const DEBOUNCE: Duration = Duration::from_secs(120);
/// With nothing changed locally, a round still runs this often to pull other
/// hosts' changes (SID-103).
const PULL_INTERVAL: Duration = Duration::from_secs(300);
/// Scheduler wake cadence; also the retry latency after a busy repo lock.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Delay before the first round, which uploads anything committed at the
/// previous exit. Matches the auto-updater's startup courtesy delay.
const INITIAL_DELAY: Duration = Duration::from_secs(90);
/// Cap for the exponential retry backoff after failed rounds: 2^5 × 2min ≈ 1h.
const MAX_BACKOFF_SHIFT: u32 = 5;

static DIRTY: AtomicBool = AtomicBool::new(false);
static LAST_CHANGE_MS: AtomicI64 = AtomicI64::new(0);
static LAST_ROUND_MS: AtomicI64 = AtomicI64::new(0);
static CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);

/// Called by the file watcher whenever something outside `.git` changes in the
/// central repository. Re-arms the debounce window.
pub fn notify_central_change() {
    LAST_CHANGE_MS.store(chrono::Utc::now().timestamp_millis(), Ordering::Release);
    DIRTY.store(true, Ordering::Release);
}

/// Pure debounce/backoff decision, kept free of the statics for testability.
/// A round is due when a change is pending and the quiet period — stretched
/// exponentially by consecutive failures — has elapsed since the last change,
/// or, with nothing pending, when the last round is a pull interval old.
fn is_due(
    dirty: bool,
    last_change_ms: i64,
    last_round_ms: i64,
    now_ms: i64,
    failures: u32,
) -> bool {
    if !dirty {
        return now_ms - last_round_ms >= PULL_INTERVAL.as_millis() as i64;
    }
    let quiet = DEBOUNCE.as_millis() as i64;
    let backoff = quiet << failures.min(MAX_BACKOFF_SHIFT);
    now_ms - last_change_ms >= backoff
}

pub fn is_enabled(store: &SkillStore) -> bool {
    let value = store
        .get_setting(SETTING_ENABLED)
        .ok()
        .flatten()
        .map(|v| v.trim().to_ascii_lowercase());
    !matches!(value.as_deref(), Some("off" | "false" | "0" | "no"))
}

#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Nothing attempted: disabled, no repo/remote, busy lock, needs repair.
    Skipped(&'static str),
    /// Nothing to commit or push (the round may still have pulled).
    UpToDate,
    /// Committed and/or pushed successfully.
    BackedUp,
    /// Another device pushed inside this round's window. Transient: the
    /// re-armed next round merges and pushes.
    RemoteAhead,
    /// The remote uses a newer Sync format. Nothing was merged; retrying
    /// cannot help until the app is updated, so the round is not re-armed.
    UpdateRequired(String),
    Failed(String),
}

pub fn start<R: Runtime>(app: AppHandle<R>, store: Arc<SkillStore>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;
        // Startup round: push whatever the exit-time commit captured. Backdate
        // the change stamp so the round is due on the first tick.
        LAST_CHANGE_MS.store(
            chrono::Utc::now().timestamp_millis() - DEBOUNCE.as_millis() as i64,
            Ordering::Release,
        );
        DIRTY.store(true, Ordering::Release);

        loop {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if is_due(
                DIRTY.load(Ordering::Acquire),
                LAST_CHANGE_MS.load(Ordering::Acquire),
                LAST_ROUND_MS.load(Ordering::Acquire),
                now_ms,
                CONSECUTIVE_FAILURES.load(Ordering::Acquire),
            ) {
                DIRTY.store(false, Ordering::Release);
                LAST_ROUND_MS.store(now_ms, Ordering::Release);
                let store_for_round = store.clone();
                let (outcome, summary) = tauri::async_runtime::spawn_blocking(move || {
                    run_round_blocking(&store_for_round)
                })
                .await
                .unwrap_or_else(|e| (Outcome::Failed(format!("join error: {e}")), None));
                let merged = summary.is_some();
                handle_outcome(&app, &store, outcome, summary);
                // Deployed instructions and MCPs follow the merge (SID-99),
                // after the round released the repo lock.
                if let Some(ctx) = app.try_state::<super::host::HostCtx>().filter(|_| merged) {
                    let ctx = ctx.inner().clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        super::sync_apply::reapply_resources(&ctx)
                    })
                    .await;
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[derive(Serialize, Clone)]
struct AutoBackupPayload {
    ok: bool,
    /// True when the remote moved ahead and a manual sync is needed.
    pending: bool,
    error: Option<String>,
    /// What the round's merge changed locally, if it merged (SID-99).
    summary: Option<merge::MergeSummary>,
}

fn handle_outcome<R: Runtime>(
    app: &AppHandle<R>,
    store: &SkillStore,
    outcome: Outcome,
    summary: Option<merge::MergeSummary>,
) {
    let merged = summary.is_some();
    let emit = |ok: bool, pending: bool, error: Option<String>| {
        let payload = AutoBackupPayload {
            ok,
            pending,
            error,
            summary: summary.clone(),
        };
        if let Err(e) = app.emit(EVENT_COMPLETED, payload) {
            log::debug!("auto backup: emit failed: {e}");
        }
    };
    let clear_error = |store: &SkillStore| {
        let had_error = matches!(
            store.get_setting(SETTING_LAST_ERROR).ok().flatten().as_deref(),
            Some(v) if !v.trim().is_empty()
        );
        if had_error {
            let _ = store.set_setting(SETTING_LAST_ERROR, "");
        }
        had_error
    };

    match outcome {
        Outcome::Skipped(reason) => {
            if reason == "repo busy" {
                // A foreground operation held the lock; retry on the next tick
                // (the change stamp is untouched, so the round is due again).
                DIRTY.store(true, Ordering::Release);
            }
            log::debug!("auto backup: skipped ({reason})");
        }
        Outcome::UpToDate => {
            CONSECUTIVE_FAILURES.store(0, Ordering::Release);
            super::sync_service::stamp_last_sync(store);
            // Only worth telling the UI if the round pulled something or a
            // stale failure card should clear.
            if clear_error(store) || merged {
                emit(true, false, None);
            }
        }
        Outcome::BackedUp => {
            CONSECUTIVE_FAILURES.store(0, Ordering::Release);
            super::sync_service::stamp_last_sync(store);
            clear_error(store);
            log::info!("auto backup: committed/pushed");
            emit(true, false, None);
        }
        Outcome::RemoteAhead => {
            CONSECUTIVE_FAILURES.store(0, Ordering::Release);
            log::info!("auto backup: remote is ahead, waiting for the next round or manual sync");
            emit(false, true, None);
        }
        Outcome::UpdateRequired(msg) => {
            CONSECUTIVE_FAILURES.store(0, Ordering::Release);
            log::warn!("auto backup: stopped, update required: {msg}");
            if let Err(e) = store.set_setting(SETTING_LAST_ERROR, &msg) {
                log::warn!("auto backup: failed to persist error: {e:#}");
            }
            emit(false, false, Some(msg));
        }
        Outcome::Failed(msg) => {
            CONSECUTIVE_FAILURES.fetch_add(1, Ordering::AcqRel);
            log::warn!("auto backup: failed: {msg}");
            if let Err(e) = store.set_setting(SETTING_LAST_ERROR, &msg) {
                log::warn!("auto backup: failed to persist error: {e:#}");
            }
            // Re-arm so the round retries after the (backed-off) quiet period.
            notify_central_change();
            emit(false, false, Some(msg));
        }
    }
}

/// One round, plus what its merge changed locally (kept even when the push
/// after it fails: the merge already landed).
pub(crate) fn run_round_blocking(store: &SkillStore) -> (Outcome, Option<merge::MergeSummary>) {
    let mut summary = None;
    let outcome = round(store, &mut summary);
    (outcome, summary)
}

fn round(store: &SkillStore, summary: &mut Option<merge::MergeSummary>) -> Outcome {
    if !is_enabled(store) {
        return Outcome::Skipped("disabled");
    }
    let skills_dir = central_repo::skills_dir();
    if !skills_dir.join(".git").exists() {
        return Outcome::Skipped("no repo");
    }
    if git_backup::raw_remote_url(&skills_dir).is_none() {
        return Outcome::Skipped("no remote");
    }
    crate::core::git_backup_store::sync_engine_pref(store);

    // Fail fast instead of queueing behind a user-initiated operation.
    let Ok(_lock) = RepoLock::acquire("auto backup") else {
        return Outcome::Skipped("repo busy");
    };
    // Before the check below, which reports any error as a quiet skip: an
    // unreadable repo format must reach the backup card, not stall silently.
    if let Err(e) = git_backup::ensure_files_ref_format(&skills_dir) {
        return Outcome::Failed(format!("{e:#}"));
    }
    if git_backup::ensure_no_interrupted_git_operation(&skills_dir).is_err() {
        return Outcome::Skipped("interrupted git operation");
    }

    // See the true remote state (3d-γ): rounds are debounced, so one quiet
    // fetch per round is cheap; offline it just leaves the refs stale.
    if let Err(e) = git_backup::fetch_remote(&skills_dir) {
        log::debug!("auto backup: fetch failed (continuing offline): {e:#}");
    }

    let status = match git_backup::get_status(&skills_dir) {
        Ok(status) => status,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    if status.upstream_health == "unrelated_histories" || status.upstream_health == "detached" {
        // The Backup page routes these to the recovery flow; retrying in the
        // background would only thrash.
        return Outcome::Skipped("needs manual repair");
    }

    crate::core::git_backup_store::apply_device_identity(store, &skills_dir);
    if let Err(e) = sync_metadata::write_all_from_db_unlocked(store) {
        return Outcome::Failed(format!("{e:#}"));
    }
    // Before the dirty check, so a shrunk previously-excluded skill re-enters
    // the backup via the resulting .gitignore change (§3.6).
    if let Err(e) =
        git_backup::apply_oversized_exclusions(&skills_dir, git_backup::SKILL_SIZE_LIMIT_BYTES)
    {
        log::debug!("auto backup: exclusion scan failed (continuing): {e:#}");
    }

    let mut committed = false;
    match git_backup::has_uncommitted_changes(&skills_dir) {
        Ok(true) => {
            if let Err(e) = git_backup::commit_all_unlocked(&skills_dir, AUTO_COMMIT_MESSAGE) {
                return Outcome::Failed(format!("{e:#}"));
            }
            committed = true;
        }
        Ok(false) => {}
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    }

    if status.behind > 0 {
        // SID-108: conflicts never pause the round. A pending skill keeps its
        // local version and the merge moves its conflict to the newest remote
        // version.
        match merge::object_merge_pull_unlocked(store, &skills_dir) {
            Ok(merged) => {
                *summary = Some(merged).filter(|m| !m.up_to_date);
                if let Err(e) =
                    crate::core::git_backup_store::reconcile_skills_index_unlocked(store)
                {
                    return Outcome::Failed(format!("{e:#}"));
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains(merge::protocol::UPDATE_REQUIRED) {
                    return Outcome::UpdateRequired(msg);
                }
                return Outcome::Failed(msg);
            }
        }
    }

    // Re-read after a possible merge: a merge commit shows up as ahead > 0,
    // a pure fast-forward leaves nothing to push.
    let status = match git_backup::get_status(&skills_dir) {
        Ok(status) => status,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    let needs_push = committed || status.ahead > 0 || status.upstream_health == "no_upstream";
    if !needs_push {
        return Outcome::UpToDate;
    }
    match git_backup::push_unlocked(&skills_dir) {
        Ok(()) => Outcome::BackedUp,
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("non-fast-forward")
                || msg.contains("fetch first")
                || msg.contains("[rejected]")
                || msg.contains("failed to push some refs")
            {
                // Another device pushed inside our window. Re-arm so the next
                // round (post-debounce) fetches, merges and retries.
                notify_central_change();
                Outcome::RemoteAhead
            } else {
                Outcome::Failed(msg)
            }
        }
    }
}

/// Best-effort "退出前" save (§3.4): commit outstanding changes locally so
/// nothing is lost between sessions. Never touches the network — the next
/// startup round pushes — and never blocks quitting (fail-fast lock).
pub fn commit_on_exit(store: &SkillStore) {
    if !is_enabled(store) {
        return;
    }
    let skills_dir = central_repo::skills_dir();
    if !skills_dir.join(".git").exists() {
        return;
    }
    let Ok(_lock) = RepoLock::acquire("auto backup on exit") else {
        return;
    };
    if git_backup::ensure_no_interrupted_git_operation(&skills_dir).is_err() {
        return;
    }
    crate::core::git_backup_store::apply_device_identity(store, &skills_dir);
    if let Err(e) = sync_metadata::write_all_from_db_unlocked(store) {
        log::warn!("auto backup on exit: metadata write failed: {e:#}");
    }
    match git_backup::has_uncommitted_changes(&skills_dir) {
        Ok(true) => {
            if let Err(e) = git_backup::commit_all_unlocked(&skills_dir, AUTO_COMMIT_MESSAGE) {
                log::warn!("auto backup on exit: commit failed: {e:#}");
            } else {
                log::info!("auto backup on exit: committed pending changes");
            }
        }
        Ok(false) => {}
        Err(e) => log::warn!("auto backup on exit: status check failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ── is_due (pure debounce/backoff decision) ──

    const MIN2: i64 = 120_000;

    #[test]
    fn idle_rounds_pull_every_five_minutes() {
        let last_round = 1_000_000;
        assert!(!is_due(
            false,
            0,
            last_round,
            last_round + 5 * 60_000 - 1,
            0
        ));
        assert!(is_due(false, 0, last_round, last_round + 5 * 60_000, 0));
    }

    #[test]
    fn due_after_quiet_period() {
        assert!(!is_due(true, 1_000_000, 0, 1_000_000 + MIN2 - 1, 0));
        assert!(is_due(true, 1_000_000, 0, 1_000_000 + MIN2, 0));
    }

    #[test]
    fn failures_stretch_the_quiet_period_with_a_cap() {
        let base = 1_000_000;
        assert!(!is_due(true, base, 0, base + MIN2, 1));
        assert!(is_due(true, base, 0, base + 2 * MIN2, 1));
        // Backoff is capped at 2^5 — an hour-ish, never unbounded.
        assert!(is_due(true, base, 0, base + 32 * MIN2, 40));
        assert!(!is_due(true, base, 0, base + 31 * MIN2, 40));
    }

    // ── round behaviour against a local bare remote ──

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        store: SkillStore,
        skills_dir: std::path::PathBuf,
        remote: std::path::PathBuf,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            central_repo::set_test_base_dir_override(None);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Skills repo (one commit, upstream tracking) + local bare remote.
    fn test_env() -> TestEnv {
        let lock = central_repo::test_base_dir_lock();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        central_repo::set_test_base_dir_override(Some(base.clone()));
        let skills_dir = central_repo::skills_dir();
        std::fs::create_dir_all(&skills_dir).unwrap();
        let store = SkillStore::new(&base.join("test.db")).unwrap();

        let remote = tmp.path().join("remote.git");
        let out = std::process::Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(out.status.success());

        git_backup::init_repo_unlocked(&skills_dir, "Test Device").unwrap();
        git_backup::set_remote_unlocked(&skills_dir, remote.to_str().unwrap()).unwrap();
        git_backup::push_unlocked(&skills_dir).unwrap();

        TestEnv {
            _lock: lock,
            _tmp: tmp,
            store,
            skills_dir,
            remote,
        }
    }

    #[test]
    fn round_commits_and_pushes_without_snapshot_tag() {
        let env = test_env();
        std::fs::create_dir_all(env.skills_dir.join("skill-a")).unwrap();
        std::fs::write(env.skills_dir.join("skill-a/SKILL.md"), "content").unwrap();

        let outcome = run_round_blocking(&env.store).0;
        assert_eq!(outcome, Outcome::BackedUp);

        // The change reached the remote…
        let remote_head = git_out(&env.remote, &["rev-parse", "main"]);
        let local_head = git_out(&env.skills_dir, &["rev-parse", "HEAD"]);
        assert_eq!(remote_head, local_head);
        assert_eq!(
            git_out(&env.skills_dir, &["log", "-1", "--format=%s"]),
            AUTO_COMMIT_MESSAGE
        );
        // …and §3.4 holds: automatic backups never mint a snapshot tag.
        assert_eq!(git_out(&env.skills_dir, &["tag", "--list", "sm-v-*"]), "");

        // A second round with nothing new is a no-op.
        assert_eq!(run_round_blocking(&env.store).0, Outcome::UpToDate);
    }

    #[test]
    fn a_reftable_repo_fails_the_round_visibly_instead_of_skipping() {
        let env = test_env();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&env.skills_dir)
            .args(["refs", "migrate", "--ref-format=reftable"])
            .output()
            .unwrap();
        if !out.status.success() {
            eprintln!("skipping: this git cannot migrate to reftable");
            return;
        }
        match run_round_blocking(&env.store).0 {
            Outcome::Failed(msg) => assert!(msg.contains("reftable"), "{msg}"),
            other => panic!("expected a visible failure, got {other:?}"),
        }
    }

    /// Clone the test remote as a second "device" with its own identity.
    fn clone_other_device(env: &TestEnv) -> std::path::PathBuf {
        let other = env._tmp.path().join("other");
        let out = std::process::Command::new("git")
            .arg("clone")
            .arg(&env.remote)
            .arg(&other)
            .output()
            .unwrap();
        assert!(out.status.success());
        git_backup::configure_device_identity(&other, "Device B").unwrap();
        other
    }

    /// A managed skill: content folder plus its Sync metadata file.
    fn write_skill(dir: &Path, id: &str, path: &str, content: &str) {
        std::fs::create_dir_all(dir.join(path)).unwrap();
        std::fs::write(dir.join(path).join("SKILL.md"), content).unwrap();
        let meta_dir = dir.join(".skills-manager/skills");
        std::fs::create_dir_all(&meta_dir).unwrap();
        let meta = crate::core::sync_metadata::SkillMetaFile {
            schema_version: 1,
            skill_id: id.to_string(),
            path: path.to_string(),
            path_key: path.to_string(),
            enabled: true,
            tags: vec![],
            source: crate::core::sync_metadata::SourceMeta {
                source_type: "import".to_string(),
                ref_: None,
                subpath: None,
                branch: None,
            },
        };
        std::fs::write(
            meta_dir.join(format!("{id}.json")),
            crate::core::sync_metadata::canonical_json_bytes(&meta).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join(".skills-manager/schema.json"),
            b"{\n  \"schema_version\": 1,\n  \"app_min_version\": \"2.0.0\",\n  \"created_by\": \"skills-manager\"\n}\n",
        )
        .unwrap();
    }

    #[test]
    fn idle_round_pulls_and_reports_what_changed() {
        // SID-103, SID-99: with nothing changed locally, a round still pulls,
        // and it reports what the pull changed.
        let env = test_env();
        let other = clone_other_device(&env);
        write_skill(&other, "skill-1", "alpha", "from B");
        git_backup::commit_all_unlocked(&other, "add skill on B").unwrap();
        git(&other, &["push", "origin", "main"]);

        let (_, summary) = run_round_blocking(&env.store);
        let summary = summary.expect("the pull reports what it changed");
        let changes: Vec<_> = summary
            .updated
            .iter()
            .map(|s| (s.skill_id.as_str(), s.kind, s.from_device.as_str()))
            .collect();
        assert_eq!(
            changes,
            vec![("skill-1", merge::apply::ChangeKind::Added, "Device B")]
        );
        assert!(env.skills_dir.join("alpha/SKILL.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn round_applies_pulled_preset_changes_to_agents() {
        // SID-99: after a pull the active preset is applied again. A skill
        // another device added to it is deployed; one it deleted is removed.
        use crate::core::{skill_store::ScenarioRecord, sync_apply, sync_metadata};
        let env = test_env();
        let agent_dir = env._tmp.path().join("agent-skills");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let custom = serde_json::json!([{
            "key": "test_agent",
            "display_name": "Test Agent",
            "skills_dir": agent_dir,
        }]);
        let builtin: Vec<String> = crate::core::tool_adapters::default_tool_adapters()
            .into_iter()
            .map(|a| a.key)
            .collect();
        env.store
            .set_setting("custom_tools", &custom.to_string())
            .unwrap();
        env.store
            .set_setting("disabled_tools", &serde_json::to_string(&builtin).unwrap())
            .unwrap();

        write_skill(&env.skills_dir, "skill-1", "alpha", "a");
        sync_metadata::reindex_from_metadata_unlocked(&env.store).unwrap();
        env.store
            .insert_scenario(&ScenarioRecord {
                id: "web".to_string(),
                name: "Web".to_string(),
                description: None,
                icon: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        env.store.add_skill_to_scenario("web", "skill-1").unwrap();
        env.store.set_active_scenario("web").unwrap();
        sync_apply::reapply_active_preset(&env.store, &sync_apply::capture(&env.store));
        assert!(agent_dir.join("alpha").is_symlink());
        sync_metadata::write_all_from_db_unlocked(&env.store).unwrap();
        git_backup::commit_all_unlocked(&env.skills_dir, "seed").unwrap();
        git_backup::push_unlocked(&env.skills_dir).unwrap();

        // The other device deletes alpha and adds beta to the preset.
        let other = clone_other_device(&env);
        std::fs::remove_dir_all(other.join("alpha")).unwrap();
        std::fs::remove_file(other.join(".skills-manager/skills/skill-1.json")).unwrap();
        std::fs::remove_file(other.join(".skills-manager/scenario-skills/web/skill-1.json"))
            .unwrap();
        write_skill(&other, "skill-2", "beta", "b");
        let member = sync_metadata::ScenarioSkillMetaFile {
            schema_version: 1,
            scenario_id: "web".to_string(),
            skill_id: "skill-2".to_string(),
            sort_order: 0,
            tools: Default::default(),
        };
        std::fs::write(
            other.join(".skills-manager/scenario-skills/web/skill-2.json"),
            sync_metadata::canonical_json_bytes(&member).unwrap(),
        )
        .unwrap();
        git_backup::commit_all_unlocked(&other, "swap alpha for beta").unwrap();
        git(&other, &["push", "origin", "main"]);

        run_round_blocking(&env.store);
        assert!(!agent_dir.join("alpha").is_symlink());
        assert_eq!(
            std::fs::read_link(agent_dir.join("beta")).unwrap(),
            env.skills_dir.join("beta")
        );
    }

    #[test]
    fn round_stops_with_update_required_when_the_remote_format_is_newer() {
        // SID-101: a remote written by a newer version is never merged with
        // this build's rules. The stop is visible, and the round is not
        // re-armed into a retry loop.
        let env = test_env();
        let other = clone_other_device(&env);
        std::fs::write(
            other.join(merge::protocol::PROTOCOL_FILE_REL),
            r#"{"schema_version":1,"merge_protocol":99}"#,
        )
        .unwrap();
        std::fs::write(other.join("from-newer.md"), "b").unwrap();
        git(&other, &["add", "-A"]);
        git(
            &other,
            &["commit", "-m", "newer write\n\nSkills-Manager-Protocol: 99"],
        );
        git(&other, &["push", "origin", "main"]);

        match run_round_blocking(&env.store).0 {
            Outcome::UpdateRequired(msg) => {
                assert!(msg.contains(merge::protocol::UPDATE_REQUIRED), "{msg}")
            }
            other => panic!("expected UpdateRequired, got {other:?}"),
        }
        assert!(!env.skills_dir.join("from-newer.md").exists());
    }

    #[test]
    fn round_merges_remote_changes_and_pushes_back() {
        // 3d-γ: with the (default) object engine, a round that finds the
        // remote ahead merges it and pushes the result — hands-free two-way
        // sync, including the round's own fetch.
        let env = test_env();
        let other = clone_other_device(&env);
        std::fs::write(other.join("from-b.md"), "b").unwrap();
        git_backup::commit_all_unlocked(&other, "from B").unwrap();
        git(&other, &["push", "origin", "main"]);

        // No manual fetch here — the round discovers the remote change itself.
        std::fs::write(env.skills_dir.join("local.md"), "a").unwrap();
        let outcome = run_round_blocking(&env.store).0;
        assert_eq!(outcome, Outcome::BackedUp);

        // Both sides' changes are present locally and on the remote.
        assert!(env.skills_dir.join("from-b.md").exists());
        assert!(env.skills_dir.join("local.md").exists());
        let remote_head = git_out(&env.remote, &["rev-parse", "main"]);
        let local_head = git_out(&env.skills_dir, &["rev-parse", "HEAD"]);
        assert_eq!(remote_head, local_head);
        assert_eq!(env.store.get_setting(SETTING_LAST_ERROR).unwrap(), None);
    }

    #[test]
    fn round_keeps_merging_when_remote_touches_a_pending_conflict() {
        // SID-108: a remote change to a skill that awaits a conflict decision
        // no longer pauses the round. The skill keeps the local version, its
        // conflict moves to the newest remote version, and everything else
        // keeps syncing.
        let env = test_env();
        let other = clone_other_device(&env);

        let skill_md = |dir: &Path| std::fs::read_to_string(dir.join("alpha/SKILL.md")).unwrap();
        let write_skill_md = |dir: &Path, content: &str| {
            std::fs::write(dir.join("alpha/SKILL.md"), content).unwrap();
        };

        // A managed skill exists on both sides (seeded through the remote).
        write_skill(&env.skills_dir, "skill-1", "alpha", "base");
        crate::core::sync_metadata::reindex_from_metadata_unlocked(&env.store).unwrap();
        git_backup::commit_all_unlocked(&env.skills_dir, "seed skill").unwrap();
        git_backup::push_unlocked(&env.skills_dir).unwrap();
        git(&other, &["pull", "origin", "main"]);

        // Both devices edit the skill: the round flags a conflict and keeps
        // the local version.
        write_skill_md(&other, "changed on B");
        git_backup::commit_all_unlocked(&other, "edit on B").unwrap();
        git(&other, &["push", "origin", "main"]);
        write_skill_md(&env.skills_dir, "changed here");
        assert_eq!(run_round_blocking(&env.store).0, Outcome::BackedUp);
        assert_eq!(env.store.list_pending_conflicts().unwrap().len(), 1);

        // The other device changes the pending skill again and adds a file.
        git(&other, &["pull", "origin", "main"]);
        write_skill_md(&other, "changed on B again");
        std::fs::write(other.join("from-b.md"), "b").unwrap();
        git_backup::commit_all_unlocked(&other, "touch pending + unrelated").unwrap();
        git(&other, &["push", "origin", "main"]);

        std::fs::write(env.skills_dir.join("local.md"), "a").unwrap();
        assert_eq!(run_round_blocking(&env.store).0, Outcome::BackedUp);
        assert_eq!(skill_md(&env.skills_dir), "changed here");
        assert!(env.skills_dir.join("from-b.md").exists());
        let pending = env.store.list_pending_conflicts().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            git_out(
                &env.skills_dir,
                &[
                    "show",
                    &format!("{}:alpha/SKILL.md", pending[0].theirs_commit)
                ]
            ),
            "changed on B again"
        );
        assert_eq!(
            git_out(&env.remote, &["rev-parse", "main"]),
            git_out(&env.skills_dir, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn round_skips_when_disabled() {
        let env = test_env();
        env.store.set_setting(SETTING_ENABLED, "off").unwrap();
        std::fs::write(env.skills_dir.join("x.md"), "x").unwrap();
        assert_eq!(
            run_round_blocking(&env.store).0,
            Outcome::Skipped("disabled")
        );
        assert!(git_backup::has_uncommitted_changes(&env.skills_dir).unwrap());
    }

    #[test]
    fn commit_on_exit_commits_locally_without_pushing() {
        let env = test_env();
        let remote_before = git_out(&env.remote, &["rev-parse", "main"]);
        std::fs::write(env.skills_dir.join("late-edit.md"), "bye").unwrap();

        commit_on_exit(&env.store);

        assert!(!git_backup::has_uncommitted_changes(&env.skills_dir).unwrap());
        assert_eq!(
            git_out(&env.skills_dir, &["log", "-1", "--format=%s"]),
            AUTO_COMMIT_MESSAGE
        );
        // Exit-time save is local only; the push belongs to the next launch.
        assert_eq!(git_out(&env.remote, &["rev-parse", "main"]), remote_before);
    }
}
