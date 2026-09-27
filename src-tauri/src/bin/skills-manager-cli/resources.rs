//! Thin resource CLI: exactly the same requests and preconditions as desktop/SSH.
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use app_lib::core::{
    host::{HostCtx, NoopEvents},
    host_dispatch,
    install_cancel::InstallCancelRegistry,
    remote_session::RemoteSessions,
    skill_store::SkillStore,
};
use clap::{Args, ValueEnum};
use serde_json::{json, Value};

#[derive(Clone, Debug, ValueEnum)]
pub enum Action {
    List,
    Get,
    Save,
    Remove,
    Scan,
    Read,
    Write,
    Preview,
    Apply,
    Deployments,
    Undeploy,
    Recover,
    Capabilities,
    Inspect,
    Import,
    Catalog,
    Conflicts,
    Resolve,
}

#[derive(Debug, Args)]
pub struct ResourceArgs {
    #[arg(value_enum)]
    pub action: Action,
    /// JSON request fields from a file, or '-' for stdin. No credentials.
    #[arg(long)]
    pub input: Option<PathBuf>,
    /// Resource identifier for get/remove, or preview identifier for apply.
    #[arg(long)]
    pub id: Option<String>,
    /// Target agent key (e.g. codex or claude_code).
    #[arg(long)]
    pub agent: Option<String>,
    /// Existing project identifier; omitted means global scope.
    #[arg(long)]
    pub project: Option<String>,
    /// Registered SSH host identifier; defaults to this machine.
    #[arg(long)]
    pub host: Option<String>,
    /// Compute a preview without writing library, target or preview state.
    #[arg(long)]
    pub dry_run: bool,
}

fn request(kind: &str, args: &ResourceArgs) -> Result<Value> {
    let mut value = if let Some(path) = &args.input {
        let mut input = Vec::new();
        if path.as_os_str() == "-" {
            std::io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_end(&mut input)?;
        } else {
            std::fs::File::open(path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut input)?;
        }
        if input.len() > 1024 * 1024 {
            bail!("Request exceeds 1 MiB");
        }
        // Do not echo source JSON in parse errors: it might contain a pasted credential.
        serde_json::from_slice::<Value>(&input)
            .map_err(|_| anyhow::anyhow!("Invalid JSON request"))?
    } else {
        json!({})
    };
    let object = value
        .as_object_mut()
        .context("Request must be a JSON object")?;
    let action = args
        .action
        .to_possible_value()
        .unwrap()
        .get_name()
        .replace('-', "_");
    object.insert("action".into(), json!(action));
    if let Some(id) = &args.id {
        let key = if matches!(args.action, Action::Apply) {
            if kind == "instructions" {
                "preview_id"
            } else {
                "previewId"
            }
        } else if matches!(args.action, Action::Recover) && kind == "mcps" {
            "recoveryId"
        } else {
            "id"
        };
        object.insert(key.into(), json!(id));
    }
    if args.project.is_some() && args.agent.is_none() {
        bail!("--project requires --agent");
    }
    if let Some(agent) = &args.agent {
        object.insert(
            "target".into(),
            if kind == "instructions" {
                json!({"agent_key":agent,"project_id":args.project})
            } else {
                json!({"agentKey":agent,"projectId":args.project})
            },
        );
    }
    if args.dry_run {
        // These are the explicit validation-only entry points. Refuse rather
        // than silently execute an action whose dry-run contract is unknown.
        if !matches!(args.action, Action::Preview) {
            bail!("--dry-run is supported on preview; use preview --input <request.json> before apply");
        }
        object.insert(
            if kind == "instructions" {
                "dry_run"
            } else {
                "dryRun"
            }
            .into(),
            json!(true),
        );
    }
    Ok(value)
}

pub fn run(
    kind: &str,
    args: ResourceArgs,
    store: Arc<SkillStore>,
    json_output: bool,
) -> Result<()> {
    let mut request = request(kind, &args)?;
    let command = if matches!(args.action, Action::Conflicts | Action::Resolve) {
        if matches!(args.action, Action::Conflicts) {
            request["action"] = json!("list");
        }
        "resource_sync_request".to_owned()
    } else {
        format!("{kind}_request")
    };
    let result = if let Some(host) = args.host {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let sessions = RemoteSessions::new(store, Arc::new(NoopEvents));
            let result = match sessions.session_for(&host).await {
                Ok(session) => session.call(&command, json!({"request":request})).await,
                Err(error) => Err(error),
            };
            sessions.disconnect().await;
            result
        })
    } else {
        let ctx = HostCtx {
            store,
            cancel: Arc::new(InstallCancelRegistry::new()),
            events: Arc::new(NoopEvents),
        };
        host_dispatch::dispatch(&ctx, &command, &json!({"request":request}))
    }
    .map_err(crate::output::map_app_err)?;
    crate::output::print_json(&result, json_output);
    if result.get("partial").and_then(Value::as_bool) == Some(true) {
        bail!("Resource operation partially failed; inspect the per-file result before retrying");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_uses_native_contract_and_is_explicitly_read_only() {
        let args = ResourceArgs {
            action: Action::Preview,
            input: None,
            id: None,
            agent: Some("codex".into()),
            project: Some("project".into()),
            host: None,
            dry_run: true,
        };
        let value = request("instructions", &args).unwrap();
        assert_eq!(value["target"]["project_id"], "project");
        assert_eq!(value["dry_run"], true);
        let value = request("mcps", &args).unwrap();
        assert_eq!(value["target"]["projectId"], "project");
        assert_eq!(value["dryRun"], true);
    }
    #[test]
    fn dry_run_cannot_accidentally_apply() {
        let args = ResourceArgs {
            action: Action::Apply,
            input: None,
            id: Some("x".into()),
            agent: None,
            project: None,
            host: None,
            dry_run: true,
        };
        assert!(request("mcps", &args).is_err());
    }
}
