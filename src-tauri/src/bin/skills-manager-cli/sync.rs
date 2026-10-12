//! `sync`: this host's Sync with the shared remote (SID-110). The same core
//! functions answer the app over `serve --stdio`.

use std::sync::Arc;

use std::io::Write;

use app_lib::core::{
    central_repo, git_backup, host::HostCtx, skill_store::SkillStore, sync_join, sync_service,
    sync_timer,
};

use crate::args::{SyncArgs, SyncCommand};
use crate::output::{map_app_err, print_json};

pub(crate) fn run_sync(args: SyncArgs, store: &Arc<SkillStore>, json: bool) -> anyhow::Result<()> {
    let ctx = HostCtx::headless(store.clone());
    match args.command {
        SyncCommand::Join { url, yes } => {
            if !yes {
                print_json(&sync_join::preview(store, &url).map_err(map_app_err)?, json);
                if json || !confirm("Join this Sync? [y/N] ")? {
                    return Ok(());
                }
            }
            let preview = join(&ctx, &url)?;
            let timer = sync_timer::install();
            if let Some(warning) = timer.warning.as_ref().filter(|_| !json) {
                eprintln!("note: {warning}");
            }
            print_json(
                &serde_json::json!({ "preview": preview, "timer": timer }),
                json,
            );
        }
        SyncCommand::UndoJoin { copy } => {
            let restored = sync_join::undo_join(&ctx, copy.as_deref()).map_err(map_app_err)?;
            print_json(
                &serde_json::json!({ "ok": true, "restored": restored }),
                json,
            );
        }
        SyncCommand::Leave => {
            sync_join::leave(store).map_err(map_app_err)?;
            sync_timer::remove();
            print_json(&sync_service::sync_status(store), json);
        }
        SyncCommand::Now => print_json(
            &sync_service::sync_now(&ctx, sync_service::SYNC_COMMIT_MESSAGE)
                .map_err(map_app_err)?,
            json,
        ),
        SyncCommand::Status => print_json(&sync_service::sync_status(store), json),
        SyncCommand::History { limit } => {
            print_json(&sync_service::history(limit).map_err(map_app_err)?, json)
        }
        SyncCommand::Restore { point } => {
            sync_service::restore(&ctx, &point).map_err(map_app_err)?;
            print_json(&serde_json::json!({ "ok": true, "restored": point }), json);
        }
        SyncCommand::Pause | SyncCommand::Resume => {
            let paused = matches!(args.command, SyncCommand::Pause);
            sync_service::set_paused(store, paused).map_err(map_app_err)?;
            print_json(&sync_service::sync_status(store), json);
        }
        SyncCommand::Conflicts => print_json(&sync_service::conflicts(store), json),
        SyncCommand::Resolve { skill, keep, both } => {
            let (skill_id, action) =
                sync_service::resolve_choice(store, &skill, keep.as_deref(), both)
                    .map_err(map_app_err)?;
            sync_service::resolve(&ctx, &skill_id, action).map_err(map_app_err)?;
            print_json(
                &serde_json::json!({ "ok": true, "skill_id": skill_id }),
                json,
            );
        }
        SyncCommand::PruneRefs => {
            let removed = git_backup::prune_hidden_refs_on_remote(&central_repo::skills_dir())?;
            print_json(&serde_json::json!({ "removed": removed }), json);
        }
    }
    Ok(())
}

/// Join without asking: `sync join --yes`, and the old `git clone` and
/// `git set-remote`, which now go through join.
pub(crate) fn join(ctx: &HostCtx, url: &str) -> anyhow::Result<sync_join::JoinPreview> {
    sync_join::join(ctx, url).map_err(map_app_err)
}

fn confirm(prompt: &str) -> anyhow::Result<bool> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}
