//! `sync`: this host's Sync with the shared remote (SID-110). The same core
//! functions answer the app over `serve --stdio`.

use std::sync::Arc;

use app_lib::core::{
    central_repo, git_backup, host::HostCtx, skill_store::SkillStore, sync_service,
};

use crate::args::{SyncArgs, SyncCommand};
use crate::output::{map_app_err, print_json};

pub(crate) fn run_sync(args: SyncArgs, store: &Arc<SkillStore>, json: bool) -> anyhow::Result<()> {
    let ctx = HostCtx::headless(store.clone());
    match args.command {
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
