//! The app's side of an SSH host in Sync (SID-103, SID-111, SID-108). The
//! host makes its own deploy key; the app registers it with GitHub using
//! the token it holds (or shows it to paste), gives the host GitHub's host
//! keys, picks port 22 or 443, and joins the host over a one-shot session.
//! Leaving deletes the key on GitHub at once, which stops even an offline
//! host; its timer goes the next time the app reaches it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::error::{AppError, ErrorKind};
use super::host::NoopEvents;
use super::remote_session::RemoteSession;
use super::skill_store::SkillStore;
use super::{central_repo, git_backup, git_credentials, github_api, remote_host, sync_access};

/// What the app keeps about a host in Sync, in the `host_sync:<host id>`
/// setting.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    key_id: Option<u64>,
    key_title: String,
    /// The SSH URL the host reaches Sync with (port 22 or 443).
    url: Option<String>,
    /// Left while offline: remove its timer the next time it answers.
    leave_pending: bool,
}

fn setting(host_id: &str) -> String {
    format!("host_sync:{host_id}")
}

fn load(store: &SkillStore, host_id: &str) -> Option<Record> {
    let raw = store.get_setting(&setting(host_id)).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

fn save(store: &SkillStore, host_id: &str, record: &Record) -> Result<(), AppError> {
    let raw = serde_json::to_string(record).map_err(AppError::internal)?;
    store
        .set_setting(&setting(host_id), &raw)
        .map_err(AppError::db)
}

fn forget(store: &SkillStore, host_id: &str) {
    let _ = store.set_setting(&setting(host_id), "");
}

/// Run one command on a host over a session of its own, so the host the
/// user is looking at keeps its session.
async fn call(
    store: &Arc<SkillStore>,
    host_id: &str,
    command: &str,
    args: Value,
) -> Result<Value, AppError> {
    let host = store
        .get_remote_host(host_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Host not found"))?;
    let session = blocking(move || {
        RemoteSession::spawn(&remote_host::cli_command, &host, Arc::new(NoopEvents))
    })
    .await?;
    let session = Arc::new(session);
    let result = session.call(command, args).await;
    let closing = session.clone();
    let _ = blocking(move || {
        closing.close();
        Ok(())
    })
    .await;
    result
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(AppError::internal)?
}

/// `owner/name` of this app's Sync remote on GitHub.
fn sync_repo() -> Result<String, AppError> {
    git_backup::raw_remote_url(&central_repo::skills_dir())
        .as_deref()
        .and_then(github_api::repo_full_name)
        .ok_or_else(|| AppError::invalid_input("Hosts can join only a Sync remote on GitHub"))
}

fn github_token() -> Result<String, AppError> {
    git_credentials::load_credential("github.com")
        .map_err(AppError::internal)?
        .map(|credential| credential.password)
        .ok_or_else(|| AppError::not_found("No GitHub sign-in on this computer"))
}

/// A deploy key GitHub didn't take from the app: the user adds it under the
/// repository's Settings > Deploy keys (with write access), then prepares
/// again.
#[derive(Debug, Serialize)]
pub struct PasteKey {
    pub title: String,
    pub key: String,
    pub settings_url: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct Prepared {
    pub paste: Option<PasteKey>,
    /// The SSH URL the host reaches Sync with, once access works.
    pub url: Option<String>,
}

/// Give a host its own access to Sync. `pasted` skips registering the key
/// after the user added it by hand.
pub async fn prepare(
    store: &Arc<SkillStore>,
    host_id: &str,
    pasted: bool,
) -> Result<Prepared, AppError> {
    let full_name = sync_repo()?;
    let host = store
        .get_remote_host(host_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Host not found"))?;
    let mut record = load(store, host_id).unwrap_or_default();
    record.key_title = format!("Agents Manager · {}", host.name);
    record.leave_pending = false;
    let key: String = serde_json::from_value(
        call(
            store,
            host_id,
            "sync_access_key",
            json!({ "comment": record.key_title }),
        )
        .await?,
    )
    .map_err(AppError::internal)?;

    let proxy = store.proxy_url();
    if record.key_id.is_none() && !pasted {
        let (repo, title, public_key, proxy) = (
            full_name.clone(),
            record.key_title.clone(),
            key.clone(),
            proxy.clone(),
        );
        let registered = blocking(move || {
            let token = github_token()?;
            github_api::create_deploy_key(&token, &repo, &title, &public_key, proxy.as_deref())
                .map_err(AppError::network)
        })
        .await;
        match registered {
            Ok(id) => record.key_id = Some(id),
            Err(e) => {
                save(store, host_id, &record)?;
                return Ok(Prepared {
                    paste: Some(PasteKey {
                        title: record.key_title,
                        key,
                        settings_url: format!("https://github.com/{full_name}/settings/keys/new"),
                        reason: e.message,
                    }),
                    url: None,
                });
            }
        }
    }
    save(store, host_id, &record)?;

    let host_keys =
        blocking(move || github_api::ssh_host_keys(proxy.as_deref()).map_err(AppError::network))
            .await?;
    call(
        store,
        host_id,
        "sync_access_setup",
        json!({ "hostKeys": host_keys }),
    )
    .await?;
    // Port 22 first; ssh.github.com:443 where 22 is blocked.
    let mut blocked = None;
    for url in sync_access::ssh_urls(&full_name) {
        match call(store, host_id, "sync_access_check", json!({ "url": url })).await {
            Ok(_) => {
                record.url = Some(url.clone());
                save(store, host_id, &record)?;
                return Ok(Prepared {
                    paste: None,
                    url: Some(url),
                });
            }
            Err(e) if matches!(e.kind, ErrorKind::Network) => blocked = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(blocked.unwrap_or_else(|| AppError::network("GitHub is unreachable from this host")))
}

fn prepared_url(store: &SkillStore, host_id: &str) -> Result<String, AppError> {
    load(store, host_id)
        .and_then(|r| r.url)
        .ok_or_else(|| AppError::invalid_input("Set up the host's access to Sync first"))
}

/// The host's review list before it joins.
pub async fn preview(store: &Arc<SkillStore>, host_id: &str) -> Result<Value, AppError> {
    let url = prepared_url(store, host_id)?;
    call(store, host_id, "sync_join_preview", json!({ "url": url })).await
}

/// Join the host to Sync. Returns its review list and timer state.
pub async fn join(store: &Arc<SkillStore>, host_id: &str) -> Result<Value, AppError> {
    let url = prepared_url(store, host_id)?;
    call(store, host_id, "sync_join", json!({ "url": url })).await
}

#[derive(Debug, Serialize)]
pub struct Left {
    /// GitHub refused to delete the key: the user deletes this one by hand
    /// under Settings > Deploy keys.
    pub delete_key_by_hand: Option<String>,
    /// The host answered and stopped syncing; otherwise its timer goes the
    /// next time the app reaches it.
    pub host_reached: bool,
}

/// Take a host out of Sync. It keeps its library and agents (SID-111).
pub async fn leave(store: &Arc<SkillStore>, host_id: &str) -> Result<Left, AppError> {
    let Some(mut record) = load(store, host_id) else {
        return Ok(Left {
            delete_key_by_hand: None,
            host_reached: false,
        });
    };
    let mut delete_key_by_hand = None;
    if let Some(key_id) = record.key_id.take() {
        let proxy = store.proxy_url();
        let deleted = blocking(move || {
            let token = github_token()?;
            github_api::delete_deploy_key(&token, &sync_repo()?, key_id, proxy.as_deref())
                .map_err(AppError::network)
        })
        .await;
        if deleted.is_err() {
            delete_key_by_hand = Some(record.key_title.clone());
        }
    }
    let host_reached = call(store, host_id, "sync_leave", json!({})).await.is_ok();
    if host_reached {
        forget(store, host_id);
    } else {
        record.leave_pending = true;
        save(store, host_id, &record)?;
    }
    Ok(Left {
        delete_key_by_hand,
        host_reached,
    })
}

#[derive(Debug, Serialize)]
pub struct HostSyncState {
    pub host_id: String,
    pub name: String,
    /// The host's `sync status`, when it answered.
    pub status: Option<Value>,
    pub error: Option<String>,
}

/// One status call per host in Sync (SID-108), at startup and on the Hosts
/// screen. A host that left while offline gets its timer removed now.
pub async fn status_all(store: &Arc<SkillStore>) -> Result<Vec<HostSyncState>, AppError> {
    let mut states = Vec::new();
    for host in store.get_all_remote_hosts().map_err(AppError::db)? {
        let Some(record) = load(store, &host.id) else {
            continue;
        };
        if record.leave_pending {
            if call(store, &host.id, "sync_leave", json!({})).await.is_ok() {
                forget(store, &host.id);
            }
            continue;
        }
        let answer = call(store, &host.id, "sync_status", json!({})).await;
        states.push(HostSyncState {
            host_id: host.id,
            name: host.name,
            error: answer.as_ref().err().map(|e| e.message.clone()),
            status: answer.ok(),
        });
    }
    Ok(states)
}
