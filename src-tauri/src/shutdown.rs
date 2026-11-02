//! Desktop shutdown coordination; operation execution stays in the host service.

use crate::core::{error::AppError, host::HostCtx, operations, remote_session::RemoteSessions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Quit,
    Restart,
    InstallUpdate,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Drain,
    Cancel,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Pending {
    pub id: u64,
    pub target: Target,
    pub mode: Option<Mode>,
}
#[derive(Default)]
struct State {
    next: u64,
    pending: Option<Pending>,
    calls: usize,
    sealed: bool,
    committed: bool,
    prepared: bool,
}
impl State {
    fn ready_target(&self, id: u64) -> Result<Target, AppError> {
        let pending = self
            .pending
            .filter(|p| p.id == id && p.mode.is_some())
            .ok_or_else(|| AppError::cancelled("This quit request is no longer pending"))?;
        if !self.prepared || !self.sealed || self.calls != 0 {
            return Err(AppError::invalid_input("Work has not safely finished"));
        }
        Ok(pending.target)
    }
}
#[derive(Default)]
pub struct Shutdown {
    state: Mutex<State>,
    transition: tokio::sync::Mutex<()>,
}
#[derive(Serialize)]
pub struct Prepared {
    pub ready: bool,
    pub snapshots: Vec<Value>,
}
/// A desktop IPC holds admission before spawning/awaiting the host request.
/// It does not own or schedule any item execution.
pub struct Call(Arc<Shutdown>);
impl Drop for Call {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().calls -= 1;
    }
}
impl Shutdown {
    pub fn pending(&self) -> Option<Pending> {
        self.state.lock().unwrap().pending
    }
    pub fn committed(&self) -> bool {
        self.state.lock().unwrap().committed
    }
    pub fn request(&self, target: Target) -> Pending {
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending {
            return pending;
        }
        state.next += 1;
        let pending = Pending {
            id: state.next,
            target,
            mode: None,
        };
        state.pending = Some(pending);
        pending
    }
    pub fn choose(&self, id: u64, mode: Mode) -> Result<(), AppError> {
        let mut state = self.state.lock().unwrap();
        let pending = state
            .pending
            .as_mut()
            .filter(|p| p.id == id)
            .ok_or_else(|| AppError::cancelled("This quit request is no longer pending"))?;
        pending.mode = Some(mode);
        Ok(())
    }
    pub fn keep_open(&self, id: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.committed && state.pending.is_some_and(|p| p.id == id) {
            state.pending = None;
            state.prepared = false;
            return true;
        }
        false
    }
    pub fn enter(self: &Arc<Self>, mutation: bool) -> Result<Call, AppError> {
        let mut state = self.state.lock().unwrap();
        if state.sealed || (mutation && state.pending.is_some_and(|p| p.mode == Some(Mode::Cancel)))
        {
            return Err(AppError::invalid_input(
                "The app is closing; keep it open before starting more work",
            ));
        }
        state.calls += 1;
        Ok(Call(self.clone()))
    }
    pub async fn resume(
        &self,
        id: u64,
        ctx: &HostCtx,
        sessions: &RemoteSessions,
    ) -> Result<(), AppError> {
        let _transition = self.transition.lock().await;
        if self.state.lock().unwrap().next != id {
            return Ok(());
        }
        self.resume_hosts(ctx, sessions).await
    }
    async fn resume_hosts(&self, ctx: &HostCtx, sessions: &RemoteSessions) -> Result<(), AppError> {
        operations::dispatch(ctx, json!({"action":"shutdown_resume"}))?;
        let resumed = sessions.shutdown("shutdown_resume").await;
        let mut state = self.state.lock().unwrap();
        state.sealed = false;
        state.prepared = false;
        resumed.map(|_| ())
    }
    pub async fn prepare(
        &self,
        id: u64,
        ctx: &HostCtx,
        sessions: &RemoteSessions,
    ) -> Result<Prepared, AppError> {
        let _transition = self.transition.lock().await;
        let mode = {
            let mut state = self.state.lock().unwrap();
            let pending = state
                .pending
                .filter(|p| p.id == id)
                .ok_or_else(|| AppError::cancelled("This quit request is no longer pending"))?;
            let mode = pending.mode.ok_or_else(|| {
                AppError::invalid_input("Choose how to finish work before quitting")
            })?;
            if state.calls > 0 && mode == Mode::Drain {
                return Ok(Prepared {
                    ready: false,
                    snapshots: vec![],
                });
            }
            state.sealed = true;
            state.prepared = false;
            mode
        };
        let action = if mode == Mode::Cancel {
            "shutdown_cancel"
        } else {
            "shutdown_seal"
        };
        let local = operations::dispatch(ctx, json!({"action":action}));
        let remote = sessions.shutdown(action).await;
        let result = local.and_then(|local| {
            remote.map(|remote| {
                let ready = local["ready"] == true
                    && remote["ready"] == true
                    && self.state.lock().unwrap().calls == 0;
                let snapshots = [local, remote]
                    .iter()
                    .flat_map(|value| value["snapshots"].as_array().cloned().unwrap_or_default())
                    .collect();
                Prepared { ready, snapshots }
            })
        });
        let current = self.pending().is_some_and(|p| p.id == id);
        if !current || !result.as_ref().is_ok_and(|r| r.ready) {
            // Also release any hosts sealed before another host reported busy.
            self.resume_hosts(ctx, sessions).await?;
        }
        if !current {
            return Err(AppError::cancelled("The app will stay open"));
        }
        if result.as_ref().is_ok_and(|r| r.ready) {
            let mut state = self.state.lock().unwrap();
            if state.pending.is_some_and(|p| p.id == id) {
                state.prepared = true;
            }
        }
        result
    }
    /// Called only after the observer has awaited its serialized flush.
    /// A stale callback (including Keep open during a save) has no authority.
    pub fn finish(&self, id: u64) -> Result<Target, AppError> {
        let mut state = self.state.lock().unwrap();
        let target = state.ready_target(id)?;
        state.committed = true;
        Ok(target)
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    Read,
    Inspect,
    Resume { id: u64 },
    Choose { id: u64, mode: Mode },
    KeepOpen { id: u64 },
    Prepare { id: u64 },
    Finish { id: u64 },
}

#[tauri::command]
pub fn install_app_update<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    crate::request_shutdown(&app, Target::InstallUpdate);
}

#[tauri::command]
pub async fn shutdown_request(
    app: tauri::AppHandle,
    boundary: tauri::State<'_, Arc<Shutdown>>,
    ctx: tauri::State<'_, HostCtx>,
    sessions: tauri::State<'_, RemoteSessions>,
    request: Request,
) -> Result<Value, AppError> {
    match request {
        Request::Resume { id } => {
            if boundary.pending().is_some_and(|p| p.id == id) {
                boundary.resume(id, &ctx, &sessions).await?;
            }
            Ok(Value::Null)
        }
        Request::Inspect => {
            let local = operations::dispatch(&ctx, json!({"action":"shutdown_status"}))?;
            Ok(
                json!({"unfinished":local["ready"] != true || sessions.has_unfinished() || boundary.state.lock().unwrap().calls > 0}),
            )
        }
        Request::Read => Ok(json!(boundary.pending())),
        Request::Choose { id, mode } => {
            boundary.choose(id, mode)?;
            Ok(json!(boundary.pending()))
        }
        Request::KeepOpen { id } => {
            if boundary.keep_open(id) {
                boundary.resume(id, &ctx, &sessions).await?;
            }
            Ok(Value::Null)
        }
        Request::Prepare { id } => Ok(json!(boundary.prepare(id, &ctx, &sessions).await?)),
        Request::Finish { id } => {
            let target = boundary.state.lock().unwrap().ready_target(id)?;
            let result = if target == Target::InstallUpdate {
                install_update(&app, &boundary, id).await
            } else {
                boundary.finish(id)?;
                let app_for_main = app.clone();
                app.run_on_main_thread(move || {
                    crate::teardown_before_exit(&app_for_main);
                    match target {
                        Target::Quit => app_for_main.exit(0),
                        Target::Restart => app_for_main.request_restart(),
                        Target::InstallUpdate => unreachable!(),
                    }
                })
                .map_err(|_| AppError::internal("Could not schedule app shutdown"))
            };
            if result.is_err() {
                let same_request = {
                    let mut state = boundary.state.lock().unwrap();
                    if state.pending.is_some_and(|p| p.id == id) {
                        state.committed = false;
                        true
                    } else {
                        false
                    }
                };
                if same_request {
                    boundary.resume(id, &ctx, &sessions).await?;
                }
            }
            result.map(|_| Value::Null)
        }
    }
}

// The Windows updater installs via process::exit, bypassing RunEvent. Never
// invoke that installer until the same admission/worker/persistence gate passed.
#[cfg(windows)]
async fn install_update(
    app: &tauri::AppHandle,
    boundary: &Shutdown,
    id: u64,
) -> Result<(), AppError> {
    use tauri::Manager;
    use tauri_plugin_updater::UpdaterExt;
    let store = app.state::<Arc<crate::core::skill_store::SkillStore>>();
    let mut builder = app.updater_builder();
    if let Some(proxy) = store
        .get_setting("proxy_url")
        .map_err(AppError::db)?
        .filter(|p| !p.is_empty())
    {
        builder = builder.proxy(
            proxy
                .parse()
                .map_err(|_| AppError::invalid_input("Invalid proxy URL"))?,
        );
    }
    let exiting = app.clone();
    let update = builder
        .on_before_exit(move || {
            let main = exiting.clone();
            let (done, wait) = std::sync::mpsc::channel();
            if let Err(error) = exiting.run_on_main_thread(move || {
                crate::teardown_before_exit(&main);
                main.cleanup_before_exit();
                let _ = done.send(());
            }) {
                log::error!("Could not schedule updater teardown: {error}");
                crate::teardown_before_exit(&exiting);
                exiting.cleanup_before_exit();
            } else if let Err(error) = wait.recv() {
                log::error!("Updater teardown did not complete: {error}");
            }
        })
        .build()
        .map_err(AppError::internal)?
        .check()
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::invalid_input("The update is no longer available; check again"))?;
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(AppError::internal)?;
    // Downloading is reversible. Keep open during that await invalidates id;
    // only the final installer is allowed to commit an irreversible exit.
    boundary.finish(id)?;
    update.install(bytes).map_err(AppError::internal)
}
#[cfg(not(windows))]
async fn install_update(
    _app: &tauri::AppHandle,
    _boundary: &Shutdown,
    _id: u64,
) -> Result<(), AppError> {
    Err(AppError::invalid_input(
        "This installer is only used on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{host::HostCtx, remote_session::RemoteSessions, skill_store::SkillStore};

    #[tokio::test]
    async fn pending_rpc_and_keep_open_invalidate_exit_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = HostCtx::headless(Arc::new(
            SkillStore::new(&tmp.path().join("test.db")).unwrap(),
        ));
        let sessions = RemoteSessions::new(ctx.store.clone(), ctx.events.clone());
        let boundary = Arc::new(Shutdown::default());
        let request = boundary.request(Target::Restart);
        boundary.choose(request.id, Mode::Drain).unwrap();
        let rpc = boundary.enter(true).unwrap();
        assert!(
            !boundary
                .prepare(request.id, &ctx, &sessions)
                .await
                .unwrap()
                .ready
        );
        assert!(boundary.finish(request.id).is_err());
        drop(rpc);
        assert!(
            boundary
                .prepare(request.id, &ctx, &sessions)
                .await
                .unwrap()
                .ready
        );
        assert!(boundary.enter(true).is_err());
        boundary.keep_open(request.id);
        boundary.resume(request.id, &ctx, &sessions).await.unwrap();
        assert!(boundary.finish(request.id).is_err());
        assert!(boundary.enter(true).is_ok());
        let next = boundary.request(Target::Quit);
        boundary.choose(next.id, Mode::Drain).unwrap();
        assert!(
            boundary
                .prepare(next.id, &ctx, &sessions)
                .await
                .unwrap()
                .ready
        );
        boundary.resume(request.id, &ctx, &sessions).await.unwrap();
        assert!(
            boundary.enter(true).is_err(),
            "an older request cannot resume a newer seal"
        );
        assert_eq!(boundary.finish(next.id).unwrap(), Target::Quit);
        assert!(
            !boundary.keep_open(next.id),
            "a committed exit is no longer a pending request"
        );
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use tauri::Manager;

    #[tokio::test]
    async fn close_to_tray_keeps_the_app_open_and_disabled_tray_requests_guarded_quit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::core::skill_store::SkillStore::new(&tmp.path().join("store.db")).unwrap(),
        );
        store.set_setting("show_tray_icon", "true").unwrap();
        let app = tauri::test::mock_builder()
            .manage(store.clone())
            .manage(Arc::new(Shutdown::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        crate::commands::settings::hide_to_tray(app.handle().clone(), window.clone(), app.state())
            .await
            .unwrap();
        assert!(app.state::<Arc<Shutdown>>().pending().is_none());
        assert!(app.get_webview_window("main").is_some());
        store.set_setting("show_tray_icon", "false").unwrap();
        crate::commands::settings::hide_to_tray(app.handle().clone(), window, app.state())
            .await
            .unwrap();
        assert_eq!(
            app.state::<Arc<Shutdown>>().pending().unwrap().target,
            Target::Quit
        );
        assert!(app.get_webview_window("main").is_some());
    }

    #[tokio::test]
    async fn native_tray_app_and_restart_adapters_request_safety_before_destroying_the_window() {
        for (entry, target) in [
            ("native", Target::Quit),
            ("tray", Target::Quit),
            ("app", Target::Quit),
            ("restart", Target::Restart),
            ("update", Target::InstallUpdate),
        ] {
            let app = tauri::test::mock_builder()
                .manage(Arc::new(Shutdown::default()))
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .unwrap();
            let _window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap();
            match entry {
                "native" => {
                    let prevented = std::cell::Cell::new(false);
                    crate::handle_native_exit(app.handle(), Some(0), || prevented.set(true));
                    assert!(prevented.get());
                }
                "tray" => crate::request_quit(app.handle()),
                "app" => crate::commands::settings::app_exit(app.handle().clone()).await,
                "restart" => crate::commands::settings::restart_app(app.handle().clone()).await,
                "update" => install_app_update(app.handle().clone()),
                _ => unreachable!(),
            }
            let boundary = app.state::<Arc<Shutdown>>();
            assert_eq!(boundary.pending().unwrap().target, target, "{entry}");
            assert!(!boundary.committed());
            assert!(
                app.get_webview_window("main").is_some(),
                "{entry} destroyed the window before safety/persistence"
            );
        }
    }
}
