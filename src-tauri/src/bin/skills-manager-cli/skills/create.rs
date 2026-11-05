//! `skills create`: the CLI's front end to the app's `create_skill` command.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context};
use app_lib::core::skill_store::SkillStore;
use serde_json::{json, Value};

use crate::resources::call_on_host;
use crate::skills::install::expand_path;

pub(crate) struct CreateArgs {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) body_file: Option<String>,
    pub(crate) from: Option<std::path::PathBuf>,
    pub(crate) agents: Vec<String>,
    pub(crate) preset: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) dry_run: bool,
}

pub(crate) fn run_create(store: &Arc<SkillStore>, args: CreateArgs) -> anyhow::Result<Value> {
    let host = args.host.as_deref();
    let content = match (&args.from, &args.description, &args.body_file) {
        (Some(from), _, _) => json!({"kind": "folder", "path": local_path(from)?}),
        (None, Some(description), Some(body_file)) => json!({
            "kind": "written",
            "description": description,
            "instructions": read_body(body_file)?,
        }),
        _ => bail!("pass --description with --body-file, or --from"),
    };
    let project = args
        .project
        .as_deref()
        .map(|reference| project_ref(reference, host.is_some()))
        .transpose()?;
    let request = json!({
        "name": args.name,
        "content": content,
        "agents": args.agents,
        "preset": args.preset,
        "project": project,
        "dry_run": args.dry_run,
    });
    call_on_host(store.clone(), host, "create_skill", request)
}

fn local_path(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    expand_path(&path.to_string_lossy())
}

/// Read here even with `--host`: the text travels in the request, so the file
/// only has to exist on this machine.
fn read_body(body_file: &str) -> anyhow::Result<String> {
    if body_file == "-" {
        let mut body = String::new();
        std::io::stdin()
            .read_to_string(&mut body)
            .context("failed to read the skill body from stdin")?;
        return Ok(body);
    }
    let path = expand_path(body_file)?;
    std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read the skill body from {}", path.display()))
}

/// A relative path means this machine's filesystem, so it can only be resolved
/// here, and means nothing on another host. Anything else is matched by the
/// command, on the host that holds the projects.
fn project_ref(reference: &str, on_host: bool) -> anyhow::Result<String> {
    let relative = matches!(reference, "." | ".." | "~")
        || ["./", "../", "~/"]
            .iter()
            .any(|prefix| reference.starts_with(prefix));
    if !relative {
        return Ok(reference.to_string());
    }
    if on_host {
        bail!("--project with --host needs an id, a name or an absolute path, not '{reference}'");
    }
    let path = expand_path(reference)?;
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("project path does not exist: {}", path.display()))?;
    Ok(path.to_string_lossy().to_string())
}
