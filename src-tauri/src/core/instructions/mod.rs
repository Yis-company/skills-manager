//! Whole-file instruction library and guarded deployment operations.
//!
//! Instructions are kept as complete files. This intentionally does not parse
//! Markdown into sections: authors keep their native document and metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::core::error::AppError;
use crate::core::{central_repo, host::HostCtx, repo_lock::RepoLock};

const LIBRARY_DIR: &str = ".agents-manager/instructions";
const MAX_SCAN_FILES: usize = 5000;
const MAX_SCAN_DEPTH: usize = 24;
const MAX_REFERENCES: usize = 10_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request {
    List,
    Get {
        id: String,
    },
    Save {
        id: Option<String>,
        name: String,
        #[serde(default)]
        description: Option<String>,
        files: BTreeMap<String, String>,
        #[serde(default)]
        expected_revision: Option<String>,
    },
    Remove {
        id: String,
        #[serde(default)]
        detach: bool,
    },
    Scan {
        target: Target,
        #[serde(default)]
        include_dirs: Vec<String>,
    },
    Read {
        target: Target,
        path: String,
    },
    Write {
        target: Target,
        path: String,
        content: String,
        expected_revision: String,
    },
    Preview {
        target: Target,
        instruction_id: String,
        #[serde(default)]
        dry_run: bool,
    },
    Apply {
        preview_id: String,
        #[serde(default)]
        resolutions: BTreeMap<String, String>,
        #[serde(default)]
        dry_run: bool,
    },
    Deployments,
    Undeploy {
        deployment_id: String,
    },
    Recover,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Target {
    agent_key: String,
    project_id: Option<String>,
    relative_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Definition {
    id: String,
    name: String,
    description: Option<String>,
    revision: String,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Change {
    path: String,
    status: String,
    content: Option<String>,
    conflict: Option<String>,
    previous: Option<String>,
    #[serde(default)]
    baseline: Option<String>,
    #[serde(default)]
    incoming: Option<String>,
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    let req: Request =
        serde_json::from_value(request).map_err(|e| AppError::invalid_input(e.to_string()))?;
    match req {
        Request::List => list().map(|items| json!({"items":items})),
        Request::Get { id } => get(&id).map(|item| json!({"item":item})),
        Request::Save {
            id,
            name,
            description,
            files,
            expected_revision,
        } => save(ctx, id, name, description, files, expected_revision)
            .map(|item| json!({"item":item})),
        Request::Remove { id, detach } => {
            remove(ctx, &id, detach).map(|detached| json!({"removed":true,"detached":detached}))
        }
        Request::Scan {
            target,
            include_dirs,
        } => scan(ctx, &target, &include_dirs),
        Request::Read { target, path } => read_file(ctx, &target, &path),
        Request::Write {
            target,
            path,
            content,
            expected_revision,
        } => write_file(ctx, &target, &path, &content, &expected_revision),
        Request::Preview {
            target,
            instruction_id,
            dry_run,
        } => preview(ctx, target, &instruction_id, dry_run),
        Request::Apply {
            preview_id,
            resolutions,
            dry_run,
        } => apply(ctx, &preview_id, resolutions, dry_run),
        Request::Deployments => deployments(ctx).map(|items| json!({"items":items})),
        Request::Undeploy { deployment_id } => undeploy(ctx, &deployment_id),
        Request::Recover => recover(ctx),
    }
}

fn err(e: impl std::fmt::Display) -> AppError {
    AppError::invalid_input(e.to_string())
}
fn db<T>(r: anyhow::Result<T>) -> Result<T, AppError> {
    r.map_err(AppError::db)
}
fn revision() -> String {
    Uuid::new_v4().to_string()
}
fn lib_root() -> PathBuf {
    central_repo::skills_dir().join(LIBRARY_DIR)
}
fn definition_path(id: &str) -> Result<PathBuf, AppError> {
    crate::core::resource_store::validate_id(id).map_err(err)?;
    Ok(lib_root().join(id).join("definition.json"))
}

fn load_definition(id: &str) -> Result<(Definition, BTreeMap<String, String>), AppError> {
    let path = definition_path(id)?;
    validate_library_schema()?;
    crate::core::resource_store::reject_symlinks(&path).map_err(AppError::io)?;
    let dir = path.parent().unwrap();
    for checked in [dir, &path] {
        let metadata = fs::symlink_metadata(checked).map_err(AppError::io)?;
        if metadata.file_type().is_symlink() {
            return Err(err("instruction bundle cannot contain symlinks"));
        }
    }
    let raw = crate::core::resource_store::read_limited(&path).map_err(AppError::io)?;
    let def: Definition = serde_json::from_str(&raw).map_err(AppError::internal)?;
    if def.id != id
        || crate::core::resource_store::validate_id(&def.revision).is_err()
        || def.name.trim().is_empty()
    {
        return Err(err("instruction bundle metadata is invalid"));
    }
    let mut files = BTreeMap::new();
    let root = path.parent().unwrap().join("files");
    let mut total = 0usize;
    for entry in walkdir::WalkDir::new(&root)
        .max_depth(24)
        .follow_links(false)
    {
        let entry = entry.map_err(AppError::io)?;
        if entry.file_type().is_symlink() {
            return Err(err("instruction bundle cannot contain symlinks"));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&root)
            .map_err(AppError::io)?
            .to_string_lossy()
            .replace('\\', "/");
        safe_file_path(&rel)?;
        let content =
            crate::core::resource_store::read_limited(entry.path()).map_err(AppError::io)?;
        total = total.saturating_add(content.len());
        if total > 4 * 1024 * 1024 || files.len() >= MAX_SCAN_FILES {
            return Err(err("instruction bundle exceeds supported size"));
        }
        files.insert(rel, content);
    }
    if files.is_empty() {
        return Err(err("instruction bundle has no files"));
    }
    Ok((def, files))
}

fn validate_library_schema() -> Result<(), AppError> {
    crate::core::resource_store::check_schema().map_err(AppError::io)
}

fn get(id: &str) -> Result<Value, AppError> {
    let (d, files) = load_definition(id)?;
    let mut v = serde_json::to_value(d).map_err(AppError::internal)?;
    v["files"] = json!(files);
    Ok(v)
}

fn list() -> Result<Vec<Value>, AppError> {
    let root = lib_root();
    crate::core::resource_store::reject_symlinks(&root).map_err(AppError::io)?;
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(root).map_err(AppError::io)? {
        let entry = entry.map_err(AppError::io)?;
        if entry
            .file_type()
            .map(|t| t.is_symlink() || !t.is_dir())
            .unwrap_or(true)
        {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if crate::core::resource_store::validate_id(&id).is_err() {
            continue;
        }
        if let Ok((def, contents)) = load_definition(&id) {
            let files: Vec<Value> = contents
                .iter()
                .map(|(path, content)| json!({"path":path,"size":content.len()}))
                .collect();
            out.push(json!({"id":def.id,"name":def.name,"description":def.description,"revision":def.revision,"updated_at":def.updated_at,"files":files}));
        }
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(out)
}

fn save(
    ctx: &HostCtx,
    id: Option<String>,
    name: String,
    description: Option<String>,
    files: BTreeMap<String, String>,
    expected_revision: Option<String>,
) -> Result<Value, AppError> {
    if name.trim().is_empty() || files.is_empty() {
        return Err(err("name and at least one file are required"));
    }
    if files.len() > MAX_SCAN_FILES
        || files.values().map(|value| value.len()).sum::<usize>() > 4 * 1024 * 1024
    {
        return Err(err(
            "instruction bundle exceeds the file count or 4 MiB bundle limit",
        ));
    }
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let path = definition_path(&id)?;
    for (p, content) in &files {
        safe_file_path(p)?;
        if content.len() as u64 > crate::core::resource_store::MAX_FILE_BYTES {
            return Err(err(format!("instruction file exceeds 1 MiB: {p}")));
        }
    }
    let _lock = RepoLock::acquire_foreground("save instructions").map_err(AppError::io)?;
    crate::core::resource_store::ensure_root().map_err(AppError::io)?;
    if fs::symlink_metadata(path.parent().unwrap())
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(err("instruction bundle cannot be a symlink"));
    }
    if path.exists() {
        let (current, _) = load_definition(&id)?;
        if expected_revision.as_deref() != Some(&current.revision) {
            return Err(err("instruction bundle changed since it was loaded"));
        }
    } else if expected_revision.is_some() {
        return Err(AppError::not_found("instruction bundle not found"));
    }
    let revision = revision();
    let def = Definition {
        id: id.clone(),
        name,
        description,
        revision,
        updated_at: chrono::Utc::now().timestamp(),
    };
    let bytes = serde_json::to_vec_pretty(&def).map_err(AppError::internal)?;
    if bytes.len() as u64 > crate::core::resource_store::MAX_FILE_BYTES {
        return Err(err("instruction metadata exceeds 1 MiB"));
    }
    let instructions_root = lib_root();
    crate::core::resource_store::reject_symlinks(&instructions_root).map_err(AppError::io)?;
    fs::create_dir_all(&instructions_root).map_err(AppError::io)?;
    let transactions_root = central_repo::base_dir().join("resource-transactions");
    crate::core::resource_store::reject_symlinks(&transactions_root).map_err(AppError::io)?;
    fs::create_dir_all(&transactions_root).map_err(AppError::io)?;
    let stage = tempfile::Builder::new()
        .prefix(".instructions-stage-")
        .tempdir_in(&transactions_root)
        .map_err(AppError::io)?;
    let stage_path = stage.path().to_path_buf();
    let backup_path = transactions_root.join(format!(".instructions-backup-{}", Uuid::new_v4()));
    let target_dir = path.parent().unwrap().to_path_buf();
    let journal_id = Uuid::new_v4().to_string();
    let journal = json!({"id":journal_id,"kind":"library_save","status":"building","bundle_id":id,"target_path":target_dir,"stage_path":stage_path,"backup_path":backup_path,"new_revision":def.revision,"created_at":chrono::Utc::now().timestamp()});
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &journal))?;
    let files_root = stage.path().join("files");
    fs::create_dir_all(&files_root).map_err(AppError::io)?;
    for (p, content) in &files {
        let dest = files_root.join(safe_file_path(p)?);
        crate::core::resource_store::atomic_write(&dest, content.as_bytes())
            .map_err(AppError::io)?;
    }
    crate::core::resource_store::atomic_write(&stage.path().join("definition.json"), &bytes)
        .map_err(AppError::io)?;
    let mut swapping = journal.clone();
    swapping["status"] = json!("swapping");
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &swapping))?;
    crate::core::resource_store::reject_symlinks(&target_dir).map_err(AppError::io)?;
    if fs::symlink_metadata(&target_dir).is_ok() {
        let target_meta = fs::symlink_metadata(&target_dir).map_err(AppError::io)?;
        if target_meta.file_type().is_symlink() || !target_meta.is_dir() {
            return Err(err("instruction bundle target is not a real directory"));
        }
        fs::rename(&target_dir, &backup_path).map_err(AppError::io)?;
    }
    if let Err(error) = fs::rename(&stage_path, &target_dir) {
        if backup_path.exists() && fs::rename(&backup_path, &target_dir).is_err() {
            return Err(err(format!(
                "bundle swap failed and original bundle remains at {} for recovery: {error}",
                backup_path.display()
            )));
        }
        return Err(AppError::io(error));
    }
    let mut complete = journal.clone();
    complete["status"] = json!("complete");
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &complete))?;
    if backup_path.exists() {
        fs::remove_dir_all(&backup_path).map_err(AppError::io)?;
    }
    let mut result = serde_json::to_value(def).map_err(AppError::internal)?;
    result["files"] = json!(files);
    Ok(result)
}

fn remove(ctx: &HostCtx, id: &str, detach: bool) -> Result<Vec<String>, AppError> {
    let path = definition_path(id)?;
    let _lock = RepoLock::acquire_foreground("remove instructions").map_err(AppError::io)?;
    crate::core::resource_store::check_schema().map_err(AppError::io)?;
    let deployments: Vec<Value> = db(ctx.store.resource_state_list("instructions_deployment"))?
        .iter()
        .filter(|d| d["instruction_id"] == id)
        .cloned()
        .collect();
    if !deployments.is_empty() && !detach {
        return Err(err(
            "bundle has linked deployments; retry with detach=true to keep target files and remove the links",
        ));
    }
    let mut detached = Vec::new();
    if !path.exists() {
        if fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
            || fs::symlink_metadata(path.parent().unwrap())
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
        {
            return Err(err("instruction bundle cannot be a symlink"));
        }
        for deployment in &deployments {
            if let Some(deployment_id) = deployment["id"].as_str() {
                db(ctx
                    .store
                    .resource_state_delete("instructions_deployment", deployment_id))?;
                detached.push(deployment_id.to_owned());
            }
        }
        return Ok(detached);
    }
    let instructions_root = lib_root();
    crate::core::resource_store::reject_symlinks(&instructions_root).map_err(AppError::io)?;
    if path.exists() {
        let dir = path.parent().unwrap();
        crate::core::resource_store::reject_symlinks(dir).map_err(AppError::io)?;
        let tx_root = central_repo::base_dir().join("resource-transactions");
        crate::core::resource_store::reject_symlinks(&tx_root).map_err(AppError::io)?;
        fs::create_dir_all(&tx_root).map_err(AppError::io)?;
        let backup = tx_root.join(format!(".instructions-remove-{}", Uuid::new_v4()));
        let journal_id = Uuid::new_v4().to_string();
        let ids: Vec<String> = deployments
            .iter()
            .filter_map(|d| d["id"].as_str().map(ToOwned::to_owned))
            .collect();
        let journal = json!({"id":journal_id,"kind":"library_remove","status":"removing","bundle_id":id,"target_path":dir,"backup_path":backup,"detach":detach,"deployment_ids":ids,"created_at":chrono::Utc::now().timestamp()});
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
        fs::rename(dir, &backup).map_err(AppError::io)?;
        for deployment in &deployments {
            if let Some(deployment_id) = deployment["id"].as_str() {
                db(ctx
                    .store
                    .resource_state_delete("instructions_deployment", deployment_id))?;
                detached.push(deployment_id.to_owned());
            }
        }
        if let Err(error) = fs::remove_dir_all(&backup) {
            return Err(err(format!(
                "library bundle moved to transaction backup for recovery: {error}"
            )));
        }
        let mut complete = journal;
        complete["status"] = json!("complete");
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &complete))?;
    }
    Ok(detached)
}

fn resolve_target(ctx: &HostCtx, target: &Target) -> Result<PathBuf, AppError> {
    let home = dirs::home_dir().ok_or_else(|| err("home directory is unavailable"))?;
    let agent_dir = match target.agent_key.as_str() {
        "claude_code" => ".claude",
        "codex" => ".codex",
        "antigravity" => ".gemini",
        "hermes" => ".hermes",
        "cursor" => ".cursor",
        _ => return Err(err("unsupported instruction agent")),
    };
    if target.project_id.is_none() && target.agent_key == "cursor" {
        return Err(err("global Cursor instructions are not supported"));
    }
    if target.project_id.is_none()
        && target
            .relative_dir
            .as_deref()
            .is_some_and(|v| !v.is_empty())
    {
        return Err(err(
            "relative directories are available only for project instruction scopes",
        ));
    }
    let base = match target.project_id.as_deref() {
        Some(id) => {
            let project = PathBuf::from(
                db(ctx.store.get_project_by_id(id))?
                    .ok_or_else(|| AppError::not_found("project not found"))?
                    .path,
            );
            if !project.is_dir() {
                return Err(AppError::not_found("project directory is unavailable"));
            }
            fs::canonicalize(project).map_err(AppError::io)?
        }
        None => {
            let override_path = match target.agent_key.as_str() {
                "codex" => std::env::var_os("CODEX_HOME"),
                "hermes" => std::env::var_os("HERMES_HOME"),
                _ => None,
            };
            if let Some(path) = override_path {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(err("agent home override must be absolute"));
                }
                path
            } else {
                home.join(agent_dir)
            }
        }
    };
    let rel = target.relative_dir.as_deref().unwrap_or("");
    let extra = safe_relative_optional(rel)?;
    Ok(base.join(extra))
}

fn safe_relative_optional(s: &str) -> Result<PathBuf, AppError> {
    if s.is_empty() {
        return Ok(PathBuf::new());
    }
    crate::core::resource_store::safe_relative(s).map_err(err)
}
fn safe_file_path(s: &str) -> Result<PathBuf, AppError> {
    let p = crate::core::resource_store::safe_relative(s).map_err(err)?;
    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
    if !matches!(ext, "md" | "mdc") {
        return Err(err("instruction files must end in .md or .mdc"));
    }
    Ok(p)
}

fn supported_instruction_file(agent: &str, path: &str, project: bool) -> bool {
    let p = Path::new(path);
    let name = p.file_name().and_then(|v| v.to_str()).unwrap_or("");
    if !project {
        return match agent {
            "claude_code" => {
                path == "CLAUDE.md" || (p.starts_with("rules") && name.ends_with(".md"))
            }
            "codex" => matches!(path, "AGENTS.md" | "AGENTS.override.md"),
            "antigravity" => {
                matches!(path, "AGENTS.md" | "GEMINI.md" | "antigravity/GEMINI.md")
                    || (p.starts_with("antigravity/rules") && name.ends_with(".md"))
                    || (p.starts_with("config/rules") && name.ends_with(".md"))
            }
            "hermes" => path == "SOUL.md",
            "cursor" => false,
            _ => false,
        };
    }
    if name == "AGENTS.md" {
        return true;
    }
    let value = p.to_string_lossy();
    match agent {
        "claude_code" => {
            matches!(name, "CLAUDE.md" | "CLAUDE.local.md")
                || (value.contains(".claude/rules/") && name.ends_with(".md"))
                || (!project && p.starts_with("rules") && name.ends_with(".md"))
        }
        "codex" => name == "AGENTS.override.md",
        "cursor" => {
            (value.contains(".cursor/rules/") && name.ends_with(".mdc"))
                || (!project && p.starts_with("rules") && name.ends_with(".mdc"))
        }
        "antigravity" => {
            name == "GEMINI.md"
                || ((value.contains(".gemini/antigravity/rules/")
                    || value.contains(".gemini/config/rules/")
                    || value.contains(".agents/rules/"))
                    && name.ends_with(".md"))
                || (!project && p.starts_with("antigravity/rules") && name.ends_with(".md"))
                || (!project && p == Path::new("antigravity/GEMINI.md"))
        }
        "hermes" => {
            if project {
                name == ".hermes.md" || (value.contains(".hermes/rules/") && name.ends_with(".md"))
            } else {
                p == Path::new("SOUL.md")
            }
        }
        _ => false,
    }
}

fn scan(ctx: &HostCtx, target: &Target, include_dirs: &[String]) -> Result<Value, AppError> {
    let root = resolve_target(ctx, target)?;
    let mut excluded = Vec::new();
    if !root.exists() {
        return Ok(
            json!({"target":target,"files":[],"excluded":[{"path":root,"reason":"missing"}],"references":[],"warnings":[]}),
        );
    }
    let root_c = fs::canonicalize(&root).map_err(AppError::io)?;
    let mut roots = vec![(root.clone(), false)];
    for d in include_dirs {
        match safe_relative_optional(d) {
            Ok(p) if !p.as_os_str().is_empty() => roots.push((root.join(p), true)),
            _ => excluded.push(json!({"path":d,"reason":"invalid include directory"})),
        }
    }
    let mut candidates: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut scanned_entries = 0usize;
    for (scope, include_all) in roots {
        if scanned_entries >= MAX_SCAN_FILES * 20 {
            break;
        }
        if !scope.exists() {
            excluded.push(json!({"path":scope,"reason":"missing"}));
            continue;
        }
        let canon_scope = fs::canonicalize(&scope).map_err(AppError::io)?;
        if !canon_scope.starts_with(&root_c) {
            excluded
                .push(json!({"path":scope,"reason":"include directory escapes instruction scope"}));
            continue;
        }
        let mut stack = vec![(scope, 0usize)];
        let mut visited_dirs = BTreeSet::new();
        while let Some((dir, depth)) = stack.pop() {
            if scanned_entries >= MAX_SCAN_FILES * 20 {
                break;
            }
            if depth > MAX_SCAN_DEPTH {
                excluded.push(json!({"path":dir,"reason":"depth limit"}));
                continue;
            }
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    excluded.push(json!({"path":dir,"reason":e.to_string()}));
                    continue;
                }
            };
            let canonical_dir = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if !visited_dirs.insert(canonical_dir) {
                excluded.push(json!({"path":dir,"reason":"directory cycle or duplicate"}));
                continue;
            }
            for e in entries.flatten() {
                scanned_entries += 1;
                if scanned_entries >= MAX_SCAN_FILES * 20 {
                    excluded.push(json!({"path":dir,"reason":"file limit"}));
                    break;
                }
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                if p.is_dir()
                    && matches!(
                        name.as_str(),
                        ".git"
                            | "node_modules"
                            | ".agents-manager"
                            | "target"
                            | "dist"
                            | "build"
                            | ".cache"
                            | ".next"
                            | ".turbo"
                            | ".venv"
                            | "vendor"
                            | "coverage"
                    )
                {
                    excluded.push(json!({"path":p,"reason":"ignored directory"}));
                    continue;
                }
                let ty = match e.file_type() {
                    Ok(t) => t,
                    Err(error) => {
                        excluded.push(json!({"path":p,"reason":error.to_string()}));
                        continue;
                    }
                };
                if ty.is_symlink() {
                    if let Ok(canon) = fs::canonicalize(&p) {
                        if !canon.starts_with(&canon_scope) {
                            excluded.push(json!({"path":p,"reason":"symlink escapes scope"}));
                            continue;
                        }
                        if canon.is_dir() {
                            stack.push((p, depth + 1));
                            continue;
                        }
                    } else {
                        excluded.push(json!({"path":p,"reason":"broken symlink"}));
                        continue;
                    }
                }
                if p.is_dir() {
                    stack.push((p, depth + 1));
                    continue;
                }
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                if !matches!(ext, "md" | "mdc") {
                    continue;
                }
                let rel = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                if include_all
                    || supported_instruction_file(
                        &target.agent_key,
                        &rel,
                        target.project_id.is_some(),
                    )
                {
                    candidates.insert(rel, p);
                }
            }
        }
    }
    let mut files_by_path: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: Vec<(String, PathBuf)> = candidates.into_iter().collect();
    let mut cursor = 0;
    while cursor < queue.len() && files_by_path.len() < MAX_SCAN_FILES {
        let (rel, path) = queue[cursor].clone();
        cursor += 1;
        if files_by_path.contains_key(&rel) {
            continue;
        }
        let canonical = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) => {
                excluded.push(json!({"path":rel,"reason":error.to_string()}));
                continue;
            }
        };
        if !canonical.starts_with(&root_c) {
            excluded.push(json!({"path":rel,"reason":"symlink escapes instruction scope"}));
            continue;
        }
        let content = match crate::core::resource_store::read_limited(&path) {
            Ok(content) => content,
            Err(error) => {
                excluded.push(json!({"path":rel,"reason":error.to_string()}));
                continue;
            }
        };
        for reference in extract_links(&rel, &content) {
            if reference["kind"] != "relative" && reference["kind"] != "native" {
                continue;
            }
            let path_ref = reference["target_path"].as_str().unwrap_or("");
            let Ok(relative) = safe_file_path(path_ref) else {
                continue;
            };
            let linked = root.join(relative);
            if linked.exists() && !files_by_path.contains_key(path_ref) {
                queue.push((path_ref.into(), linked));
            }
        }
        files_by_path.insert(rel, content);
    }
    if cursor < queue.len() {
        excluded.push(json!({"path":root,"reason":"file limit"}));
    }
    let deployments = db(ctx.store.resource_state_list("instructions_deployment"))?;
    let mut files: Vec<Value> = files_by_path.iter().map(|(rel,content)| {
        let managed_entry = deployments.iter().find_map(|deployment| {
            (deployment["target"] == json!(target))
                .then(|| deployment["files"].as_array().and_then(|entries| entries.iter().find(|entry| entry["path"] == *rel)))
                .flatten()
        });
        let managed = managed_entry.is_some();
        let conflict = managed_entry.and_then(|entry| entry["deployed"].as_str()).is_some_and(|deployed| deployed != content);
        let canonical = fs::canonicalize(root.join(rel)).ok();
        let symlink_target = canonical.as_ref().filter(|p| *p != &root_c.join(rel)).map(|p| p.to_string_lossy().to_string());
        json!({"path":rel,"kind":instruction_kind(rel),"exists":true,"content":content,"managed":managed,"conflict":conflict,"symlink_target":symlink_target,"applicable":supported_instruction_file(&target.agent_key,rel,target.project_id.is_some())})
    }).collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let mut references: Vec<Value> = files
        .iter()
        .flat_map(|f| {
            extract_links(
                f["path"].as_str().unwrap_or(""),
                f["content"].as_str().unwrap_or(""),
            )
        })
        .take(MAX_REFERENCES)
        .collect();
    let known: BTreeSet<String> = files
        .iter()
        .filter_map(|f| f["path"].as_str().map(ToOwned::to_owned))
        .collect();
    for reference in &mut references {
        if reference["kind"] == "relative" || reference["kind"] == "native" {
            if let Some(target) = reference["target_path"].as_str() {
                let linked = root.join(target);
                match fs::canonicalize(&linked) {
                    Ok(path) if !path.starts_with(&root_c) => {
                        reference["kind"] = json!("out_of_scope");
                    }
                    Ok(path) if !path.is_file() => {
                        reference["kind"] = json!("missing");
                    }
                    Ok(_) if !known.contains(target) => {
                        reference["kind"] = json!("unreadable");
                    }
                    Err(_) => {
                        reference["kind"] = json!("missing");
                    }
                    _ => {}
                }
            }
        }
    }
    mark_reference_cycles(&mut references);
    let mut warnings = Vec::new();
    if target.agent_key == "claude_code"
        && files.iter().any(|file| {
            Path::new(file["path"].as_str().unwrap_or(""))
                .file_name()
                .is_some_and(|name| name == "AGENTS.md")
        })
    {
        warnings.push("Claude Code AGENTS.md fallback requires version 2.1.277 or later; a CLAUDE.md in the same directory or an ancestor can suppress it. File presence does not confirm agent loading.".to_string());
    }
    if target.agent_key == "codex"
        && files.iter().any(|file| {
            Path::new(file["path"].as_str().unwrap_or(""))
                .file_name()
                .is_some_and(|name| name == "AGENTS.override.md")
        })
    {
        warnings.push("Codex prefers AGENTS.override.md over AGENTS.md in the same directory and reads at most one instruction file per directory.".to_string());
    }
    if target.agent_key == "hermes" && target.project_id.is_none() {
        warnings.push("Hermes SOUL.md is global identity context; project instructions use .hermes.md or recognized AGENTS.md files.".to_string());
    }
    Ok(
        json!({"target":target,"files":files,"excluded":excluded,"references":references,"warnings":warnings}),
    )
}

fn instruction_kind(path: &str) -> &'static str {
    let basename = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if basename.to_ascii_lowercase().contains("override") {
        "override"
    } else if Path::new(path).extension().and_then(|e| e.to_str()) == Some("mdc") {
        "native"
    } else if Path::new(path)
        .parent()
        .is_none_or(|p| p.as_os_str().is_empty())
        && matches!(
            basename,
            "AGENTS.md" | "CLAUDE.md" | "GEMINI.md" | "SOUL.md"
        )
    {
        "root"
    } else {
        "nested"
    }
}

fn read_file(ctx: &HostCtx, target: &Target, path: &str) -> Result<Value, AppError> {
    let rel = safe_file_path(path)?;
    let root = resolve_target(ctx, target)?;
    let p = guarded_existing(&root, &rel)?;
    let content = crate::core::resource_store::read_limited(&p).map_err(AppError::io)?;
    let token = save_read_token(ctx, &p, &content)?;
    Ok(json!({"path":path,"content":content,"revision":token}))
}
fn write_file(
    ctx: &HostCtx,
    target: &Target,
    path: &str,
    content: &str,
    expected: &str,
) -> Result<Value, AppError> {
    let rel = safe_file_path(path)?;
    let root = resolve_target(ctx, target)?;
    let p = guarded_for_preview(&root, &rel)?;
    let current = if p.exists() {
        fs::read(&p).map_err(AppError::io)?
    } else {
        Vec::new()
    };
    let token = db(ctx.store.resource_state_get("instructions_read", expected))?
        .ok_or_else(|| err("read revision expired; read the file again"))?;
    let token_time = token["created_at"].as_i64().unwrap_or_default();
    if chrono::Utc::now().timestamp().saturating_sub(token_time) > 1800 {
        return Err(err("read revision expired; read the file again"));
    }
    let current_text = String::from_utf8_lossy(&current);
    let current_path = p.to_string_lossy();
    if token["path"].as_str() != Some(current_path.as_ref())
        || token["content"].as_str() != Some(current_text.as_ref())
    {
        return Err(err("file changed since it was read"));
    }
    let _lock = RepoLock::acquire_foreground("write instructions").map_err(AppError::io)?;
    let current_target = resolve_target(ctx, target)?;
    if current_target != root {
        return Err(err("instruction target changed since it was read"));
    }
    let current_path = guarded_for_preview(&current_target, &rel)?;
    if current_path != p || fs::read(&current_path).map_err(AppError::io)? != current {
        return Err(err("file changed before write"));
    }
    let p = guarded_for_write(&current_target, &rel)?;
    if p != current_path {
        return Err(err("instruction file path changed before write"));
    }
    crate::core::resource_store::atomic_write(&p, content.as_bytes()).map_err(AppError::io)?;
    let token = save_read_token(ctx, &p, content)?;
    Ok(json!({"path":path,"revision":token}))
}
fn save_read_token(ctx: &HostCtx, path: &Path, content: &str) -> Result<String, AppError> {
    let id = revision();
    let now = chrono::Utc::now().timestamp();
    let mut tokens = db(ctx.store.resource_state_list("instructions_read"))?;
    tokens.sort_by_key(|v| v["created_at"].as_i64().unwrap_or_default());
    for old in tokens
        .iter()
        .filter(|v| now.saturating_sub(v["created_at"].as_i64().unwrap_or_default()) > 1800)
        .chain(tokens.iter().take(tokens.len().saturating_sub(499)))
    {
        if let Some(old_id) = old["id"].as_str() {
            db(ctx.store.resource_state_delete("instructions_read", old_id))?;
        }
    }
    db(ctx.store.resource_state_put(
        "instructions_read",
        &id,
        &json!({"id":id,"path":path,"content":content,"created_at":now}),
    ))?;
    Ok(id)
}

fn extract_links(source: &str, content: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        if out.len() >= 1000 {
            break;
        }
        for token in line.split_whitespace() {
            let token = token.trim_matches(|c: char| {
                matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | ':' | '<' | '>')
            });
            let Some(import) = token.strip_prefix('@') else {
                continue;
            };
            let import = import.trim_end_matches(['.', ',', ';', ')']);
            if !(import.ends_with(".md") || import.ends_with(".mdc")) {
                continue;
            }
            let target = normalize_reference(source, import);
            out.push(json!({"source_path":source,"target_path":target.clone().unwrap_or_default(),"kind":if target.is_some(){"native"}else{"out_of_scope"},"loading":"eager","line":i+1}));
            if out.len() >= 1000 {
                break;
            }
        }
        while let Some(start) = rest.find("](") {
            let tail = &rest[start + 2..];
            if let Some(end) = tail.find(')') {
                let link = &tail[..end];
                if link.is_empty() {
                    rest = &tail[end + 1..];
                    continue;
                }
                let external =
                    link.contains("://") || link.starts_with("mailto:") || link.starts_with('#');
                let normalized = if external {
                    None
                } else {
                    normalize_reference(source, link.split('#').next().unwrap_or(link))
                };
                let target = if external {
                    link.to_string()
                } else {
                    normalized.clone().unwrap_or_default()
                };
                out.push(json!({"source_path":source,"target_path":target,"kind":if external{"external"}else if normalized.is_some(){"relative"}else{"out_of_scope"},"loading":"on_demand","line":i+1}));
                rest = &tail[end + 1..];
            } else {
                break;
            }
        }
    }
    out
}

fn normalize_reference(source: &str, link: &str) -> Option<String> {
    let mut parts: Vec<String> = Path::new(source)
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(ToOwned::to_owned))
        .collect();
    for component in Path::new(link).components() {
        match component {
            Component::Normal(s) => parts.push(s.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn mark_reference_cycles(references: &mut [Value]) {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in references
        .iter()
        .filter(|r| r["kind"] == "relative" || r["kind"] == "native")
    {
        graph
            .entry(r["source_path"].as_str().unwrap_or("").into())
            .or_default()
            .push(r["target_path"].as_str().unwrap_or("").into());
    }
    for r in references
        .iter_mut()
        .filter(|r| r["kind"] == "relative" || r["kind"] == "native")
    {
        let from = r["source_path"].as_str().unwrap_or("").to_string();
        let to = r["target_path"].as_str().unwrap_or("").to_string();
        if reachable(&graph, &to, &from, &mut BTreeSet::new()) {
            r["kind"] = json!("cycle");
        }
    }
}
fn reachable(
    graph: &BTreeMap<String, Vec<String>>,
    from: &str,
    to: &str,
    visited: &mut BTreeSet<String>,
) -> bool {
    if from == to {
        return true;
    }
    if !visited.insert(from.to_string()) {
        return false;
    }
    graph
        .get(from)
        .is_some_and(|next| next.iter().any(|n| reachable(graph, n, to, visited)))
}

/// Use the same three-way merge rules people use for text in Git, with all
/// inputs staged in an ephemeral directory and never touching the user's file.
fn merge_non_overlapping(base: &str, local: &str, incoming: &str) -> Option<String> {
    let temp = tempfile::tempdir().ok()?;
    let ours = temp.path().join("ours.md");
    let ancestor = temp.path().join("base.md");
    let theirs = temp.path().join("theirs.md");
    fs::write(&ours, local).ok()?;
    fs::write(&ancestor, base).ok()?;
    fs::write(&theirs, incoming).ok()?;
    let output = Command::new("git")
        .args(["merge-file", "--stdout", "--diff3"])
        .arg(&ours)
        .arg(&ancestor)
        .arg(&theirs)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn guarded_existing(root: &Path, rel: &Path) -> Result<PathBuf, AppError> {
    let root_c = fs::canonicalize(root).map_err(AppError::io)?;
    let p = root.join(rel);
    let canon = fs::canonicalize(&p).map_err(AppError::io)?;
    if !canon.starts_with(&root_c) || !canon.is_file() {
        return Err(err("path escapes the instruction scope or is not a file"));
    }
    Ok(canon)
}
fn guarded_for_write(root: &Path, rel: &Path) -> Result<PathBuf, AppError> {
    crate::core::resource_store::reject_symlinks(root).map_err(AppError::io)?;
    let mut current = root.to_path_buf();
    if !root.exists() {
        fs::create_dir_all(root).map_err(AppError::io)?;
    }
    let root_c = fs::canonicalize(root).map_err(AppError::io)?;
    let parent_rel = rel.parent().unwrap_or(Path::new(""));
    for component in parent_rel.components() {
        let Component::Normal(part) = component else {
            return Err(err("invalid target path"));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(err("refusing a symlink in the target path"))
            }
            Ok(meta) if !meta.is_dir() => return Err(err("target parent is not a directory")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(AppError::io)?
            }
            Err(e) => return Err(AppError::io(e)),
        }
        if !fs::canonicalize(&current)
            .map_err(AppError::io)?
            .starts_with(&root_c)
        {
            return Err(err("target path escapes the instruction scope"));
        }
    }
    let name = rel.file_name().ok_or_else(|| err("invalid target path"))?;
    let leaf = current.join(name);
    if leaf
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(err("refusing a symlink target"));
    }
    let parent_c = fs::canonicalize(&current).map_err(AppError::io)?;
    if !parent_c.starts_with(root_c) {
        return Err(err("target path escapes the instruction scope"));
    }
    Ok(parent_c.join(name))
}

/// Validate a path during a read-only preview without creating directories.
fn guarded_for_preview(root: &Path, rel: &Path) -> Result<PathBuf, AppError> {
    let path = root.join(rel);
    let root_c = if root.exists() {
        fs::canonicalize(root).map_err(AppError::io)?
    } else {
        root.to_path_buf()
    };
    if path.exists() {
        let resolved = fs::canonicalize(&path).map_err(AppError::io)?;
        if !resolved.starts_with(&root_c) || !resolved.is_file() {
            return Err(err("path escapes the instruction scope or is not a file"));
        }
        Ok(resolved)
    } else {
        let mut parent = path.parent().ok_or_else(|| err("invalid target path"))?;
        while !parent.exists() {
            parent = parent.parent().ok_or_else(|| err("invalid target path"))?;
        }
        let resolved_parent = fs::canonicalize(parent).map_err(AppError::io)?;
        if !resolved_parent.starts_with(&root_c) && root.exists() {
            return Err(err("path escapes the instruction scope"));
        }
        Ok(path)
    }
}

fn preview(ctx: &HostCtx, target: Target, id: &str, dry_run: bool) -> Result<Value, AppError> {
    let (def, incoming) = load_definition(id)?;
    if !incoming.keys().any(|path| {
        supported_instruction_file(&target.agent_key, path, target.project_id.is_some())
    }) {
        return Err(err(
            "bundle has no entrypoint supported by this target; add its native instruction file",
        ));
    }
    let root = resolve_target(ctx, &target)?;
    let resolved_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let mut changes = Vec::new();
    let mut resolved_paths = BTreeMap::new();
    let mut seen_destinations = BTreeSet::new();
    let deployment = db(ctx.store.resource_state_list("instructions_deployment"))?
        .into_iter()
        .find(|d| d["instruction_id"] == id && d["target"] == json!(target));
    for (p, content) in &incoming {
        let rel = safe_file_path(p)?;
        let target_path = guarded_for_preview(&root, &rel)?;
        if !seen_destinations.insert(target_path.clone()) {
            return Err(err("bundle paths resolve to the same target file"));
        }
        resolved_paths.insert(p.clone(), target_path.to_string_lossy().into_owned());
        let old = if target_path.exists() {
            Some(fs::read_to_string(&target_path).map_err(AppError::io)?)
        } else {
            None
        };
        let baseline = deployment
            .as_ref()
            .and_then(|d| d["files"].as_array())
            .and_then(|fs| fs.iter().find(|f| f["path"] == p.as_str()))
            .and_then(|f| f["baseline"].as_str());
        let conflict = match (old.as_deref(), baseline) {
            (None, Some(base)) if content != base => {
                Some("file was removed locally while the library changed".to_string())
            }
            (Some(local), Some(base)) if local != base && local != content && content != base => {
                Some("local file differs from deployed baseline and incoming version".to_string())
            }
            (Some(local), None) if local != content => {
                Some("existing file has no managed baseline".to_string())
            }
            _ => None,
        };
        let merged = if let (Some(local), Some(base)) = (old.as_deref(), baseline) {
            if conflict.is_some() {
                merge_non_overlapping(base, local, content)
            } else {
                None
            }
        } else {
            None
        };
        let conflict = if merged.is_some() { None } else { conflict };
        let status = if merged.is_some() {
            "merge"
        } else if conflict.is_some() {
            "conflict"
        } else if old.as_deref() == Some(content) || baseline == Some(content.as_str()) {
            // The library version is unchanged, so keep the local edit.
            "unchanged"
        } else if old.is_some() {
            "replace"
        } else {
            "create"
        };
        changes.push(Change {
            path: p.clone(),
            status: status.into(),
            conflict,
            content: merged.or_else(|| Some(content.clone())),
            previous: old,
            baseline: baseline.map(str::to_owned),
            incoming: Some(content.clone()),
        });
    }
    if let Some(files) = deployment.as_ref().and_then(|d| d["files"].as_array()) {
        for previous in files {
            let Some(path) = previous["path"].as_str() else {
                continue;
            };
            if incoming.contains_key(path) {
                continue;
            }
            let rel = safe_file_path(path)?;
            let target_path = guarded_for_preview(&root, &rel)?;
            if !seen_destinations.insert(target_path.clone()) {
                return Err(err("bundle paths resolve to the same target file"));
            }
            resolved_paths.insert(path.to_owned(), target_path.to_string_lossy().into_owned());
            let local = if target_path.exists() {
                Some(fs::read_to_string(&target_path).map_err(AppError::io)?)
            } else {
                None
            };
            let baseline = previous["baseline"].as_str();
            let conflict = match (local.as_deref(), baseline) {
                (Some(local), Some(base)) if local != base => {
                    Some("library removed this file but local content has edits".to_string())
                }
                (Some(_), None) => Some("library removed a file without a baseline".to_string()),
                _ => None,
            };
            let status = if conflict.is_some() {
                "conflict"
            } else if local.is_some() {
                "delete"
            } else {
                "unchanged"
            };
            changes.push(Change {
                path: path.to_owned(),
                status: status.into(),
                content: None,
                conflict,
                previous: local,
                baseline: baseline.map(ToOwned::to_owned),
                incoming: None,
            });
        }
    }
    let pid = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let v = json!({"preview_id":pid,"instruction_id":id,"definition_revision":def.revision,"target":target,"resolved_root":resolved_root,"resolved_paths":resolved_paths,"changes":changes,"warnings":[],"created_at":now});
    if !dry_run {
        let mut old = db(ctx.store.resource_state_list("instructions_preview"))?;
        old.sort_by_key(|v| v["created_at"].as_i64().unwrap_or_default());
        for item in old
            .iter()
            .filter(|v| now.saturating_sub(v["created_at"].as_i64().unwrap_or_default()) > 86400)
            .chain(old.iter().take(old.len().saturating_sub(249)))
        {
            if let Some(old_id) = item["preview_id"].as_str() {
                db(ctx
                    .store
                    .resource_state_delete("instructions_preview", old_id))?;
            }
        }
        db(ctx
            .store
            .resource_state_put("instructions_preview", &pid, &v))?;
    }
    Ok(v)
}

fn apply(
    ctx: &HostCtx,
    pid: &str,
    resolutions: BTreeMap<String, String>,
    dry_run: bool,
) -> Result<Value, AppError> {
    let preview = db(ctx.store.resource_state_get("instructions_preview", pid))?
        .ok_or_else(|| AppError::not_found("preview not found"))?;
    if chrono::Utc::now()
        .timestamp()
        .saturating_sub(preview["created_at"].as_i64().unwrap_or_default())
        > 86400
    {
        db(ctx.store.resource_state_delete("instructions_preview", pid))?;
        return Err(err("preview expired; create a new preview"));
    }
    let instruction_id = preview["instruction_id"]
        .as_str()
        .ok_or_else(|| err("invalid preview instruction id"))?;
    let target: Target =
        serde_json::from_value(preview["target"].clone()).map_err(AppError::internal)?;
    let mut changes: Vec<Change> =
        serde_json::from_value(preview["changes"].clone()).map_err(AppError::internal)?;
    for c in &mut changes {
        if c.status == "conflict" {
            match resolutions.get(&c.path).map(String::as_str) {
                Some("take_library") => {
                    c.status = if c.incoming.is_some() {
                        "replace"
                    } else {
                        "delete"
                    }
                    .into();
                    c.content = c.incoming.clone();
                }
                Some("keep_local") => c.status = "keep_local".into(),
                _ => {
                    return Err(err(format!(
                        "conflict requires an explicit resolution for {}",
                        c.path
                    )))
                }
            }
        }
    }
    if dry_run {
        return Ok(
            json!({"applied":[],"failed":[],"partial":false,"dry_run":true,"preview_id":pid}),
        );
    }
    let _lock = RepoLock::acquire_foreground("apply instructions").map_err(AppError::io)?;
    let (definition, _) = load_definition(instruction_id)?;
    if preview["definition_revision"].as_str() != Some(definition.revision.as_str()) {
        return Err(err(
            "instruction bundle changed after preview; create a new preview",
        ));
    }
    let root = resolve_target(ctx, &target)?;
    let resolved_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    if preview["resolved_root"].as_str() != Some(resolved_root.to_string_lossy().as_ref()) {
        return Err(err(
            "target root changed after preview; create a new preview",
        ));
    }
    // Check every destination before creating directories or replacing files.
    for c in &changes {
        let p = guarded_for_preview(&root, &safe_file_path(&c.path)?)?;
        if preview["resolved_paths"][&c.path].as_str() != Some(p.to_string_lossy().as_ref()) {
            return Err(err(format!(
                "target path changed after preview: {}",
                c.path
            )));
        }
        let current = if p.exists() {
            Some(fs::read(&p).map_err(AppError::io)?)
        } else {
            None
        };
        if current.as_deref().map(String::from_utf8_lossy).as_deref() != c.previous.as_deref() {
            return Err(err(format!("target changed after preview: {}", c.path)));
        }
    }
    let previous_deployment = db(ctx.store.resource_state_list("instructions_deployment"))?
        .into_iter()
        .find(|d| d["instruction_id"] == instruction_id && d["target"] == json!(target));
    let deployment_id = previous_deployment
        .as_ref()
        .and_then(|d| d["id"].as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let journal_id = Uuid::new_v4().to_string();
    let mut journal = json!({"id":journal_id,"kind":"deployment","target":target,"resolved_root":resolved_root,"resolved_paths":preview["resolved_paths"],"status":"applying","files":[],"deployment_id":deployment_id,"previous_deployment":previous_deployment.clone()});
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &journal))?;
    let mut applied: Vec<String> = Vec::new();
    let mut failed = Vec::new();
    struct CommittedFile {
        path: PathBuf,
        before: Option<Vec<u8>>,
        written: Option<Vec<u8>>,
        relative: String,
    }
    let mut committed = Vec::new();
    let mut deployed_files = Vec::new();
    for c in changes {
        let prior_deployed = previous_deployment
            .as_ref()
            .and_then(|record| record["files"].as_array())
            .and_then(|files| files.iter().find(|file| file["path"] == c.path))
            .and_then(|file| file["deployed"].as_str())
            .map(ToOwned::to_owned);
        if c.status == "keep_local" {
            if c.incoming.is_some() {
                deployed_files
                    .push(json!({"path":c.path,"baseline":c.incoming,"deployed":prior_deployed}));
            }
            continue;
        }
        if c.status == "unchanged" {
            if c.incoming.is_none() {
                continue;
            }
            let baseline = if c.previous == c.content {
                c.content.clone()
            } else {
                c.baseline.clone()
            };
            deployed_files
                .push(json!({"path":c.path,"baseline":baseline,"deployed":prior_deployed}));
            continue;
        }
        let rel = safe_file_path(&c.path)?;
        let p = guarded_for_write(&root, &rel)?;
        let before = if p.exists() {
            Some(fs::read(&p).map_err(AppError::io)?)
        } else {
            None
        };
        if before.as_deref().map(String::from_utf8_lossy).as_deref() != c.previous.as_deref() {
            failed.push(json!({"path":c.path,"error":"target changed after preview"}));
            break;
        }
        let bytes = c.content.clone().map(String::into_bytes);
        journal["files"].as_array_mut().unwrap().push(json!({
            "path": c.path,
            "before": before.as_ref().map(|b| String::from_utf8_lossy(b).into_owned()),
            "written": bytes.as_ref().map(|b|String::from_utf8_lossy(b).into_owned()),
            "removed": bytes.is_none(),
        }));
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
        let write_result = match bytes.as_deref() {
            Some(bytes) => crate::core::resource_store::atomic_write(&p, bytes),
            None => fs::remove_file(&p).map_err(anyhow::Error::from),
        };
        if let Err(e) = write_result {
            failed.push(json!({"path":c.path,"error":e.to_string()}));
            break;
        }
        if let Some(bytes) = bytes.as_deref() {
            deployed_files.push(json!({"path":c.path,"baseline":c.incoming,"deployed":String::from_utf8_lossy(bytes)}));
        }
        applied.push(c.path.clone());
        committed.push(CommittedFile {
            path: p,
            before,
            written: bytes,
            relative: c.path,
        });
    }
    if !failed.is_empty() {
        let mut rollback_issues = Vec::new();
        let mut retained = Vec::new();
        for CommittedFile {
            path: p,
            before,
            written,
            relative,
        } in committed.iter().rev()
        {
            let current = match fs::read(p) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    rollback_issues.push(json!({"path":relative,"error":error.to_string()}));
                    retained.push(p.clone());
                    continue;
                }
            };
            if current.as_deref() == written.as_deref() {
                match before {
                    Some(b) => {
                        if let Err(e) = crate::core::resource_store::atomic_write(p, b) {
                            rollback_issues.push(json!({"path":relative,"error":e.to_string()}));
                            retained.push(p.clone());
                        }
                    }
                    None => {
                        if let Err(e) = fs::remove_file(p) {
                            rollback_issues.push(json!({"path":relative,"error":e.to_string()}));
                            retained.push(p.clone());
                        }
                    }
                }
            } else {
                rollback_issues
                    .push(json!({"path":relative,"error":"newer edit preserved during rollback"}));
                retained.push(p.clone());
            }
        }
        applied = retained
            .iter()
            .filter_map(|p| {
                p.strip_prefix(&root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
            .collect();
        journal["rollback_issues"] = json!(rollback_issues);
    }
    journal["applied"] = json!(applied);
    journal["failed"] = json!(failed);
    if journal["failed"].as_array().is_none_or(|v| v.is_empty()) {
        let record = json!({"id":deployment_id,"instruction_id":preview["instruction_id"],"definition_revision":preview["definition_revision"],"target":target,"resolved_root":resolved_root,"resolved_paths":preview["resolved_paths"],"files":deployed_files,"updated_at":chrono::Utc::now().timestamp()});
        journal["pending_deployment"] = record.clone();
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
        db(ctx
            .store
            .resource_state_put("instructions_deployment", &deployment_id, &record))?;
        journal["status"] = json!("complete");
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
        db(ctx.store.resource_state_delete("instructions_preview", pid))?;
    } else {
        journal["status"] = json!(if journal["rollback_issues"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        {
            "recovery_incomplete"
        } else {
            "rolled_back"
        });
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
    }
    Ok(
        json!({"applied":applied,"failed":journal["failed"],"partial":journal["status"]=="recovery_incomplete","transaction_id":journal_id,"status":journal["status"]}),
    )
}

fn deployments(ctx: &HostCtx) -> Result<Vec<Value>, AppError> {
    db(ctx.store.resource_state_list("instructions_deployment"))
}
fn undeploy(ctx: &HostCtx, id: &str) -> Result<Value, AppError> {
    let mut record = db(ctx.store.resource_state_get("instructions_deployment", id))?
        .ok_or_else(|| AppError::not_found("deployment not found"))?;
    let _lock = RepoLock::acquire_foreground("undeploy instructions").map_err(AppError::io)?;
    let fresh = db(ctx.store.resource_state_get("instructions_deployment", id))?
        .ok_or_else(|| AppError::not_found("deployment not found"))?;
    if fresh != record {
        return Err(err("deployment changed while undeploy was starting"));
    }
    record = fresh;
    let target: Target =
        serde_json::from_value(record["target"].clone()).map_err(AppError::internal)?;
    let root = resolve_target(ctx, &target)?;
    let resolved_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    if record["resolved_root"].as_str() != Some(resolved_root.to_string_lossy().as_ref()) {
        return Err(err(
            "deployment target moved; refusing to undeploy a different location",
        ));
    }
    let mut removed = Vec::new();
    let mut planned = Vec::new();
    if let Some(files) = record["files"].as_array() {
        for f in files {
            let path = f["path"].as_str().unwrap_or("");
            let rel = safe_file_path(path)?;
            let p = guarded_for_preview(&root, &rel)?;
            if record["resolved_paths"][path].as_str() != Some(p.to_string_lossy().as_ref()) {
                return Err(err(format!(
                    "deployment path moved; refusing to undeploy {path}"
                )));
            }
            if f["deployed"].as_str().is_some() && p.exists() {
                let current = fs::read(&p).map_err(AppError::io)?;
                if Some(String::from_utf8_lossy(&current).as_ref()) == f["deployed"].as_str() {
                    planned.push((path.to_owned(), p, current));
                }
            }
        }
    }
    let journal_id = Uuid::new_v4().to_string();
    let mut journal = json!({"id":journal_id,"kind":"undeploy","status":"applying","deployment_id":id,"deployment_record":record,"target":target,"files":[]});
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &journal))?;
    for (path, p, before) in planned {
        journal["files"].as_array_mut().unwrap().push(json!({"path":path,"before":String::from_utf8_lossy(&before),"written":null,"removed":true}));
        db(ctx
            .store
            .resource_state_put("instructions_journal", &journal_id, &journal))?;
        if let Err(error) = fs::remove_file(&p) {
            let mut remaining = Vec::new();
            for f in journal["files"].as_array().unwrap().iter().rev() {
                let path = f["path"].as_str().unwrap_or("");
                let rel = safe_file_path(path)?;
                let dest = guarded_for_preview(&root, &rel)?;
                if !dest.exists() {
                    if let Some(old) = f["before"].as_str() {
                        if let Err(e) = crate::core::resource_store::atomic_write(
                            &guarded_for_write(&root, &rel)?,
                            old.as_bytes(),
                        ) {
                            remaining.push(json!({"path":path,"error":e.to_string()}));
                        }
                    }
                }
            }
            journal["status"] = json!(if remaining.is_empty() {
                "rolled_back"
            } else {
                "recovery_incomplete"
            });
            journal["remaining"] = json!(remaining);
            db(ctx
                .store
                .resource_state_put("instructions_journal", &journal_id, &journal))?;
            return Err(AppError::io(error));
        }
        removed.push(path);
    }
    db(ctx
        .store
        .resource_state_delete("instructions_deployment", id))?;
    journal["status"] = json!("complete");
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, &journal))?;
    Ok(json!({"detached":true,"removed":removed}))
}
fn recover(ctx: &HostCtx) -> Result<Value, AppError> {
    let mut items = db(ctx.store.resource_state_list("instructions_journal"))?;
    for journal in &mut items {
        if journal["status"] != "applying"
            && journal["status"] != "building"
            && journal["status"] != "swapping"
            && journal["status"] != "removing"
            && journal["status"] != "recovery_incomplete"
            && !(journal["kind"] == "library_save" && journal["status"] == "complete")
            && !(journal["kind"] == "library_remove" && journal["status"] == "complete")
        {
            continue;
        }
        if journal["kind"] == "library_save" {
            recover_library_save(ctx, journal)?;
            continue;
        }
        if journal["kind"] == "library_remove" {
            recover_library_remove(ctx, journal)?;
            continue;
        }
        let _lock = RepoLock::acquire_foreground("recover instructions").map_err(AppError::io)?;
        let target: Target =
            serde_json::from_value(journal["target"].clone()).map_err(AppError::internal)?;
        let root = resolve_target(ctx, &target)?;
        let actual_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let reviewed = if journal["kind"] == "undeploy" {
            &journal["deployment_record"]
        } else {
            &*journal
        };
        let expected_root = reviewed["resolved_root"]
            .as_str()
            .ok_or_else(|| err("transaction has no reviewed target root"))?;
        if actual_root.to_string_lossy() != expected_root {
            return Err(err(
                "transaction target moved; recovery stopped to protect another location",
            ));
        }
        for file in journal["files"].as_array().into_iter().flatten() {
            let path = file["path"]
                .as_str()
                .ok_or_else(|| err("transaction has an invalid file path"))?;
            let current_path = guarded_for_preview(&root, &safe_file_path(path)?)?;
            if reviewed["resolved_paths"][path].as_str()
                != Some(current_path.to_string_lossy().as_ref())
            {
                return Err(err(format!(
                    "transaction path moved; recovery stopped to protect {path}"
                )));
            }
        }
        let deployment_id = journal["deployment_id"]
            .as_str()
            .ok_or_else(|| err("transaction has no deployment id"))?;
        let current_deployment = db(ctx
            .store
            .resource_state_get("instructions_deployment", deployment_id))?;
        let previous = if journal["kind"] == "undeploy" {
            &journal["deployment_record"]
        } else {
            &journal["previous_deployment"]
        };
        let pending = &journal["pending_deployment"];
        let expected_previous = previous.as_object().map(|_| previous.clone());
        let expected_pending = pending.as_object().map(|_| pending.clone());
        if current_deployment != expected_previous && current_deployment != expected_pending {
            journal["status"] = json!("recovery_incomplete");
            journal["remaining"] = json!([{"error":"deployment changed after this transaction; preserving newer state"}]);
            let journal_id = journal["id"]
                .as_str()
                .ok_or_else(|| err("invalid journal id"))?;
            db(ctx
                .store
                .resource_state_put("instructions_journal", journal_id, journal))?;
            continue;
        }
        let mut remaining = Vec::new();
        if journal["kind"] == "undeploy" {
            let deployment_record = journal["deployment_record"].clone();
            for f in journal["files"].as_array().into_iter().flatten().rev() {
                let rel = safe_file_path(f["path"].as_str().unwrap_or(""))?;
                let p = guarded_for_preview(&root, &rel)?;
                if p.exists() {
                    let current = fs::read(&p).map_err(AppError::io)?;
                    if Some(String::from_utf8_lossy(&current).as_ref()) != f["before"].as_str() {
                        remaining.push(json!({"path":f["path"],"error":"newer edit preserved during recovery"}));
                    }
                } else if let Some(old) = f["before"].as_str() {
                    if let Err(e) = crate::core::resource_store::atomic_write(
                        &guarded_for_write(&root, &rel)?,
                        old.as_bytes(),
                    ) {
                        remaining.push(json!({"path":f["path"],"error":e.to_string()}));
                    }
                }
            }
            let deployment_id = journal["deployment_id"]
                .as_str()
                .ok_or_else(|| err("invalid undeploy journal"))?;
            db(ctx.store.resource_state_put(
                "instructions_deployment",
                deployment_id,
                &deployment_record,
            ))?;
        } else {
            // Interrupted deployment writes are rolled back only when the target
            // still contains the exact bytes written by this transaction.
            if let Some(deployment_id) = journal["deployment_id"].as_str() {
                db(ctx
                    .store
                    .resource_state_delete("instructions_deployment", deployment_id))?;
                if let Some(previous) = journal["previous_deployment"].as_object() {
                    if let Some(previous_id) = previous.get("id").and_then(Value::as_str) {
                        db(ctx.store.resource_state_put(
                            "instructions_deployment",
                            previous_id,
                            &Value::Object(previous.clone()),
                        ))?;
                    }
                }
            }
            if let Some(files) = journal["files"].as_array() {
                for f in files.iter().rev() {
                    let rel = safe_file_path(f["path"].as_str().unwrap_or(""))?;
                    let p = guarded_for_preview(&root, &rel)?;
                    let current = match fs::read(&p) {
                        Ok(bytes) => Some(bytes),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => {
                            remaining.push(json!({"path":f["path"],"error":error.to_string()}));
                            continue;
                        }
                    };
                    if f["removed"] == true {
                        if current.is_none() {
                            if let Some(old) = f["before"].as_str() {
                                if let Err(e) = crate::core::resource_store::atomic_write(
                                    &guarded_for_write(&root, &rel)?,
                                    old.as_bytes(),
                                ) {
                                    remaining.push(json!({"path":f["path"],"error":e.to_string()}));
                                }
                            }
                        } else if current.as_deref().map(String::from_utf8_lossy).as_deref()
                            != f["before"].as_str()
                        {
                            remaining.push(json!({"path":f["path"],"error":"newer edit preserved during recovery"}));
                        }
                        continue;
                    }
                    if current.is_none() {
                        if f["before"].is_null() {
                            continue;
                        }
                        remaining.push(json!({"path":f["path"],"error":"newer deletion preserved during recovery"}));
                        continue;
                    }
                    if current.as_deref().map(String::from_utf8_lossy).as_deref()
                        != f["written"].as_str()
                    {
                        if current.as_deref().map(String::from_utf8_lossy).as_deref()
                            != f["before"].as_str()
                        {
                            remaining.push(json!({"path":f["path"],"error":"newer edit preserved during recovery"}));
                        }
                        continue;
                    }
                    match f["before"].as_str() {
                        Some(old) => {
                            if let Err(e) = crate::core::resource_store::atomic_write(
                                &guarded_for_write(&root, &rel)?,
                                old.as_bytes(),
                            ) {
                                remaining.push(json!({"path":f["path"],"error":e.to_string()}));
                            }
                        }
                        None => {
                            if let Err(e) = fs::remove_file(&p) {
                                remaining.push(json!({"path":f["path"],"error":e.to_string()}));
                            }
                        }
                    }
                }
            }
        }
        let id = journal["id"]
            .as_str()
            .ok_or_else(|| err("invalid journal id"))?
            .to_string();
        journal["status"] = json!(if remaining.is_empty() {
            "recovered"
        } else {
            "recovery_incomplete"
        });
        journal["remaining"] = json!(remaining);
        db(ctx
            .store
            .resource_state_put("instructions_journal", &id, journal))?;
    }
    let summaries:Vec<Value>=items.iter().map(|journal| {
        let files=journal["files"].as_array().map(|files|files.iter().map(|f|json!({"path":f["path"]})).collect::<Vec<_>>()).unwrap_or_default();
        json!({"transaction_id":journal["id"],"kind":journal["kind"],"status":journal["status"],"files":files,"failed":journal["failed"],"remaining":journal["remaining"],"created_at":journal["created_at"]})
    }).collect();
    Ok(json!({"items":summaries}))
}

fn recover_library_save(ctx: &HostCtx, journal: &mut Value) -> Result<(), AppError> {
    let id = journal["bundle_id"]
        .as_str()
        .ok_or_else(|| err("invalid library journal id"))?;
    let bundle = definition_path(id)?.parent().unwrap().to_path_buf();
    let stage = PathBuf::from(
        journal["stage_path"]
            .as_str()
            .ok_or_else(|| err("invalid stage path"))?,
    );
    let backup = PathBuf::from(
        journal["backup_path"]
            .as_str()
            .ok_or_else(|| err("invalid backup path"))?,
    );
    let recorded_target = PathBuf::from(
        journal["target_path"]
            .as_str()
            .ok_or_else(|| err("invalid target path"))?,
    );
    let transactions_root = central_repo::base_dir().join("resource-transactions");
    if recorded_target != bundle
        || stage.parent() != Some(transactions_root.as_path())
        || backup.parent() != Some(transactions_root.as_path())
        || !stage
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .starts_with(".instructions-stage-")
        || !backup
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .starts_with(".instructions-backup-")
    {
        return Err(err("library recovery paths failed validation"));
    }
    let _lock = RepoLock::acquire_foreground("recover instruction bundle").map_err(AppError::io)?;
    crate::core::resource_store::reject_symlinks(&bundle).map_err(AppError::io)?;
    crate::core::resource_store::reject_symlinks(&transactions_root).map_err(AppError::io)?;
    for p in [&bundle, &stage, &backup] {
        if fs::symlink_metadata(p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(err(
                "library transaction path is a symlink; recovery stopped",
            ));
        }
    }
    if journal["status"] == "complete" {
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(AppError::io)?;
        }
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(AppError::io)?;
        }
        return Ok(());
    }
    let mut remaining = Vec::new();
    let expected = journal["new_revision"]
        .as_str()
        .ok_or_else(|| err("missing staged revision"))?;
    let installed_revision = || -> Option<String> {
        let p = bundle.join("definition.json");
        let raw = crate::core::resource_store::read_limited(&p).ok()?;
        serde_json::from_str::<Definition>(&raw)
            .ok()
            .map(|d| d.revision)
    };
    if installed_revision().as_deref() == Some(expected) {
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(AppError::io)?;
        }
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(AppError::io)?;
        }
        journal["status"] = json!("complete");
    } else if !bundle.exists() && backup.exists() {
        fs::rename(&backup, &bundle).map_err(AppError::io)?;
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(AppError::io)?;
        }
        journal["status"] = json!("recovered");
    } else if !backup.exists() {
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(AppError::io)?;
        }
        journal["status"] = json!("rolled_back");
    } else {
        remaining.push(json!({"path":bundle,"error":"cannot safely decide whether to restore or keep the library bundle"}));
    }
    let journal_id = journal["id"]
        .as_str()
        .ok_or_else(|| err("invalid journal id"))?
        .to_owned();
    if !remaining.is_empty() {
        journal["status"] = json!("recovery_incomplete");
    }
    journal["remaining"] = json!(remaining);
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, journal))?;
    Ok(())
}

fn recover_library_remove(ctx: &HostCtx, journal: &mut Value) -> Result<(), AppError> {
    let id = journal["bundle_id"]
        .as_str()
        .ok_or_else(|| err("invalid library removal id"))?;
    let target = definition_path(id)?.parent().unwrap().to_path_buf();
    let recorded = PathBuf::from(
        journal["target_path"]
            .as_str()
            .ok_or_else(|| err("invalid removal target"))?,
    );
    let backup = PathBuf::from(
        journal["backup_path"]
            .as_str()
            .ok_or_else(|| err("invalid removal backup"))?,
    );
    let tx_root = central_repo::base_dir().join("resource-transactions");
    if target != recorded
        || backup.parent() != Some(tx_root.as_path())
        || !backup
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .starts_with(".instructions-remove-")
    {
        return Err(err("library removal recovery paths failed validation"));
    }
    let _lock =
        RepoLock::acquire_foreground("recover instruction removal").map_err(AppError::io)?;
    crate::core::resource_store::reject_symlinks(&target).map_err(AppError::io)?;
    crate::core::resource_store::reject_symlinks(&tx_root).map_err(AppError::io)?;
    if fs::symlink_metadata(&target)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
        || fs::symlink_metadata(&backup)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    {
        return Err(err("library removal path is a symlink; recovery stopped"));
    }
    if journal["status"] == "complete" {
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(AppError::io)?;
        }
        return Ok(());
    }
    if target.exists() && backup.exists() {
        journal["status"] = json!("recovery_incomplete");
        journal["remaining"] = json!([{"path":target,"error":"both original and transaction backup exist; neither was removed"}]);
    } else if !target.exists() && backup.exists() {
        if journal["detach"] == true {
            for deployment_id in journal["deployment_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                db(ctx
                    .store
                    .resource_state_delete("instructions_deployment", deployment_id))?;
            }
        }
        fs::remove_dir_all(&backup).map_err(AppError::io)?;
        journal["status"] = json!("complete");
    } else if !target.exists() && !backup.exists() {
        if journal["detach"] == true {
            for deployment_id in journal["deployment_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                db(ctx
                    .store
                    .resource_state_delete("instructions_deployment", deployment_id))?;
            }
        }
        journal["status"] = json!("complete");
    } else {
        journal["status"] = json!("rolled_back");
    }
    let journal_id = journal["id"]
        .as_str()
        .ok_or_else(|| err("invalid removal journal id"))?
        .to_owned();
    db(ctx
        .store
        .resource_state_put("instructions_journal", &journal_id, journal))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        host::NoopEvents,
        skill_store::{ProjectRecord, SkillStore},
        test_support::test_repo,
    };
    use std::sync::Arc;

    fn project_target(ctx: &HostCtx, path: &Path) -> Target {
        let id = Uuid::new_v4().to_string();
        ctx.store
            .insert_project(&ProjectRecord {
                id: id.clone(),
                name: "instructions test".into(),
                path: path.to_string_lossy().into_owned(),
                workspace_type: "directory".into(),
                linked_agent_key: None,
                linked_agent_name: None,
                disabled_path: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
                agent_keys: None,
                deploy_mode: "link".into(),
            })
            .unwrap();
        Target {
            agent_key: "claude_code".into(),
            project_id: Some(id),
            relative_dir: None,
        }
    }

    #[test]
    fn bundle_round_trip_and_stale_revision_rejection() {
        let repo = test_repo();
        crate::core::central_repo::set_runtime_base_dir_override(Some(
            crate::core::central_repo::base_dir()
                .canonicalize()
                .unwrap(),
        ));
        let store = SkillStore::new(&repo._tmp.path().join("bundle.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let files = BTreeMap::from([("nested/CLAUDE.md".into(), "first".into())]);
        let saved = save(&ctx, None, "Team rules".into(), None, files, None).unwrap();
        let id = saved["id"].as_str().unwrap();
        let got = get(id).unwrap();
        assert_eq!(got["files"]["nested/CLAUDE.md"], "first");
        assert!(save(
            &ctx,
            Some(id.into()),
            "Team rules".into(),
            None,
            BTreeMap::from([("nested/CLAUDE.md".into(), "stale".into())]),
            None
        )
        .is_err());
        let revision = got["revision"].as_str().unwrap().to_string();
        save(
            &ctx,
            Some(id.into()),
            "Team rules".into(),
            None,
            BTreeMap::from([("nested/CLAUDE.md".into(), "second".into())]),
            Some(revision),
        )
        .unwrap();
        assert_eq!(get(id).unwrap()["files"]["nested/CLAUDE.md"], "second");
        drop(repo);
    }

    #[test]
    fn linked_update_reports_conflict_and_stale_apply_preserves_newer_edit() {
        let repo = test_repo();
        crate::core::central_repo::set_runtime_base_dir_override(Some(
            crate::core::central_repo::base_dir()
                .canonicalize()
                .unwrap(),
        ));
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let test_store = SkillStore::new(&repo._tmp.path().join("instructions.db")).unwrap();
        let ctx = HostCtx::for_tests(test_store, Arc::new(NoopEvents));
        let target = project_target(&ctx, &project);
        let bundle = save(
            &ctx,
            None,
            "Rules".into(),
            None,
            BTreeMap::from([
                ("CLAUDE.md".into(), "base".into()),
                (".claude/rules/notes.md".into(), "note 0".into()),
            ]),
            None,
        )
        .unwrap();
        let id = bundle["id"].as_str().unwrap().to_string();
        let first = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":id}),
        )
        .unwrap();
        let first_id = first["preview_id"].as_str().unwrap().to_string();
        dispatch(&ctx, json!({"action":"apply","preview_id":first_id})).unwrap();
        let target_file = project.join("CLAUDE.md");
        fs::write(
            &target_file,
            "user edit\n\nSee [nested](.claude/rules/CLAUDE.md)",
        )
        .unwrap();
        fs::create_dir_all(project.join(".claude/rules")).unwrap();
        fs::write(
            project.join(".claude/rules/CLAUDE.md"),
            "See [root](../../CLAUDE.md)",
        )
        .unwrap();
        let previews_before = ctx
            .store
            .resource_state_list("instructions_preview")
            .unwrap()
            .len();
        let scan = dispatch(&ctx, json!({"action":"scan","target":target})).unwrap();
        assert!(scan["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "CLAUDE.md"));
        assert!(scan["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reference| reference["kind"] == "cycle"));
        assert_eq!(
            ctx.store
                .resource_state_list("instructions_preview")
                .unwrap()
                .len(),
            previews_before
        );
        let read = dispatch(
            &ctx,
            json!({"action":"read","target":target,"path":"CLAUDE.md"}),
        )
        .unwrap();
        fs::remove_file(&target_file).unwrap();
        assert!(dispatch(&ctx,json!({"action":"write","target":target,"path":"CLAUDE.md","content":"resurrected","expected_revision":read["revision"]})).is_err());
        assert!(!target_file.exists());
        fs::write(
            &target_file,
            "user edit\n\nSee [nested](.claude/rules/CLAUDE.md)",
        )
        .unwrap();
        let rev = bundle["revision"].as_str().unwrap().to_string();
        save(
            &ctx,
            Some(id.clone()),
            "Rules".into(),
            None,
            BTreeMap::from([
                ("CLAUDE.md".into(), "base".into()),
                (".claude/rules/notes.md".into(), "note 1".into()),
            ]),
            Some(rev),
        )
        .unwrap();
        let next = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":id}),
        )
        .unwrap();
        assert!(next["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["path"] == "CLAUDE.md" && change["status"] == "unchanged"));
        assert!(next["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["path"] == ".claude/rules/notes.md"
                && change["status"] == "replace"));
        dispatch(
            &ctx,
            json!({"action":"apply","preview_id":next["preview_id"]}),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&target_file).unwrap(),
            "user edit\n\nSee [nested](.claude/rules/CLAUDE.md)"
        );
        assert_eq!(
            fs::read_to_string(project.join(".claude/rules/notes.md")).unwrap(),
            "note 1"
        );

        let revision = get(&id).unwrap()["revision"].as_str().unwrap().to_string();
        save(
            &ctx,
            Some(id.clone()),
            "Rules".into(),
            None,
            BTreeMap::from([("CLAUDE.md".into(), "base".into())]),
            Some(revision),
        )
        .unwrap();
        let removed = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":id}),
        )
        .unwrap();
        assert!(removed["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |change| change["path"] == ".claude/rules/notes.md" && change["status"] == "delete"
            ));
        dispatch(
            &ctx,
            json!({"action":"apply","preview_id":removed["preview_id"]}),
        )
        .unwrap();
        assert!(!project.join(".claude/rules/notes.md").exists());

        let revision = get(&id).unwrap()["revision"].as_str().unwrap().to_string();
        save(
            &ctx,
            Some(id.clone()),
            "Rules".into(),
            None,
            BTreeMap::from([
                ("CLAUDE.md".into(), "library v2".into()),
                (".claude/rules/notes.md".into(), "note 2".into()),
            ]),
            Some(revision),
        )
        .unwrap();
        let conflict = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":id}),
        )
        .unwrap();
        assert!(conflict["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["path"] == "CLAUDE.md" && change["status"] == "conflict"));
        fs::write(&target_file, "newer user edit").unwrap();
        let result = dispatch(
            &ctx,
            json!({"action":"apply","preview_id":conflict["preview_id"],"resolutions":{"CLAUDE.md":"take_library"}}),
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(target_file).unwrap(), "newer user edit");

        let keep = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":id}),
        )
        .unwrap();
        dispatch(
            &ctx,
            json!({"action":"apply","preview_id":keep["preview_id"],"resolutions":{"CLAUDE.md":"keep_local"}}),
        )
        .unwrap();
        let deployment = deployments(&ctx)
            .unwrap()
            .into_iter()
            .find(|record| record["instruction_id"] == id)
            .unwrap();
        let kept = deployment["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "CLAUDE.md")
            .unwrap();
        assert_eq!(kept["baseline"], "library v2");
        assert_eq!(kept["deployed"], "base");
        undeploy(&ctx, deployment["id"].as_str().unwrap()).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("CLAUDE.md")).unwrap(),
            "newer user edit"
        );
    }

    #[test]
    fn rejects_path_escape_and_out_of_scope_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("scope");
        let outside = tmp.path().join("outside.md");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, "safe").unwrap();
        assert!(safe_file_path("../outside.md").is_err());
        assert!(safe_file_path("docs//duplicate.md").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("linked.md")).unwrap();
            assert!(guarded_existing(&root, Path::new("linked.md")).is_err());
            assert!(guarded_for_write(&root, Path::new("linked.md")).is_err());
            assert_eq!(fs::read_to_string(outside).unwrap(), "safe");
        }
    }

    #[test]
    fn non_overlapping_line_edits_merge_without_conflict() {
        assert_eq!(
            merge_non_overlapping(
                "alpha\nmiddle\nbeta\n",
                "local alpha\nmiddle\nbeta\n",
                "alpha\nmiddle\nremote beta\n"
            )
            .as_deref(),
            Some("local alpha\nmiddle\nremote beta\n")
        );
    }

    #[test]
    #[cfg(unix)]
    fn apply_refuses_a_project_root_relocated_after_preview() {
        let repo = test_repo();
        crate::core::central_repo::set_runtime_base_dir_override(Some(
            crate::core::central_repo::base_dir()
                .canonicalize()
                .unwrap(),
        ));
        let test_store = SkillStore::new(&repo._tmp.path().join("relocation.db")).unwrap();
        let ctx = HostCtx::for_tests(test_store, Arc::new(NoopEvents));
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let target = project_target(&ctx, &project);
        let bundle = save(
            &ctx,
            None,
            "Rules".into(),
            None,
            BTreeMap::from([("AGENTS.md".into(), "rules".into())]),
            None,
        )
        .unwrap();
        let preview = dispatch(
            &ctx,
            json!({"action":"preview","target":target,"instruction_id":bundle["id"]}),
        )
        .unwrap();
        let moved = repo._tmp.path().join("moved-project");
        fs::rename(&project, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &project).unwrap();
        assert!(dispatch(
            &ctx,
            json!({"action":"apply","preview_id":preview["preview_id"]})
        )
        .is_err());
        assert!(!moved.join("AGENTS.md").exists());
    }

    #[test]
    fn scan_separates_native_imports_from_document_links_and_reports_targets() {
        let repo = test_repo();
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::create_dir_all(project.join(".cache")).unwrap();
        fs::write(
            project.join("CLAUDE.md"),
            "@docs/native.md\nSee [manual](docs/manual.md) and [missing](docs/missing.md).\n",
        )
        .unwrap();
        fs::write(project.join("docs/native.md"), "Native context").unwrap();
        fs::write(project.join("docs/manual.md"), "Read when needed").unwrap();
        fs::write(project.join("orphan.md"), "Unreferenced").unwrap();
        fs::write(project.join(".cache/AGENTS.md"), "Generated").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(project.join(".cache"), project.join("node_modules")).unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("scan.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = project_target(&ctx, &project);
        let scanned = scan(&ctx, &target, &[]).unwrap();
        let files = scanned["files"].as_array().unwrap();
        assert!(files.iter().any(|file| file["path"] == "docs/native.md"));
        assert!(files.iter().any(|file| file["path"] == "docs/manual.md"));
        assert!(!files.iter().any(|file| file["path"] == "orphan.md"));
        assert!(!files.iter().any(|file| file["path"] == ".cache/AGENTS.md"));
        #[cfg(unix)]
        assert!(!files
            .iter()
            .any(|file| file["path"] == "node_modules/AGENTS.md"));
        let references = scanned["references"].as_array().unwrap();
        assert!(references.iter().any(|reference| {
            reference["target_path"] == "docs/native.md"
                && reference["kind"] == "native"
                && reference["loading"] == "eager"
        }));
        assert!(references.iter().any(|reference| {
            reference["target_path"] == "docs/manual.md"
                && reference["kind"] == "relative"
                && reference["loading"] == "on_demand"
        }));
        assert!(references.iter().any(|reference| {
            reference["target_path"] == "docs/missing.md" && reference["kind"] == "missing"
        }));
        let included = scan(&ctx, &target, &[".cache".into()]).unwrap();
        assert!(included["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == ".cache/AGENTS.md"));
        #[cfg(unix)]
        {
            let linked = scan(&ctx, &target, &["node_modules".into()]).unwrap();
            assert!(linked["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|file| file["path"] == "node_modules/AGENTS.md"));
        }
    }

    #[test]
    fn recover_restores_files_and_deployment_state_after_interrupted_operations() {
        let repo = test_repo();
        crate::core::central_repo::set_runtime_base_dir_override(Some(
            crate::core::central_repo::base_dir()
                .canonicalize()
                .unwrap(),
        ));
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("recovery.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = project_target(&ctx, &project);
        let saved = save(
            &ctx,
            None,
            "Rules".into(),
            None,
            BTreeMap::from([("AGENTS.md".into(), "first".into())]),
            None,
        )
        .unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        let first_preview = preview(&ctx, target.clone(), &id, false).unwrap();
        apply(
            &ctx,
            first_preview["preview_id"].as_str().unwrap(),
            BTreeMap::new(),
            false,
        )
        .unwrap();
        let previous = deployments(&ctx).unwrap().pop().unwrap();
        let deployment_id = previous["id"].as_str().unwrap().to_owned();
        save(
            &ctx,
            Some(id.clone()),
            "Rules".into(),
            None,
            BTreeMap::from([("AGENTS.md".into(), "second".into())]),
            Some(saved["revision"].as_str().unwrap().into()),
        )
        .unwrap();
        let next_preview = preview(&ctx, target.clone(), &id, false).unwrap();
        let pending = json!({"id":deployment_id,"new":"deployment"});
        fs::write(project.join("AGENTS.md"), "second").unwrap();
        ctx.store
            .resource_state_put("instructions_deployment", &deployment_id, &pending)
            .unwrap();
        let journal_id = Uuid::new_v4().to_string();
        ctx.store
            .resource_state_put(
                "instructions_journal",
                &journal_id,
                &json!({"id":journal_id,"kind":"deployment","status":"applying","target":target,"resolved_root":next_preview["resolved_root"],"resolved_paths":next_preview["resolved_paths"],"deployment_id":deployment_id,"previous_deployment":previous,"pending_deployment":pending,"files":[{"path":"AGENTS.md","before":"first","written":"second","removed":false}]}),
            )
            .unwrap();
        recover(&ctx).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "first"
        );
        assert_eq!(
            ctx.store
                .resource_state_get("instructions_deployment", &deployment_id)
                .unwrap(),
            Some(previous.clone())
        );
        assert_eq!(
            ctx.store
                .resource_state_get("instructions_journal", &journal_id)
                .unwrap()
                .unwrap()["status"],
            "recovered"
        );

        fs::remove_file(project.join("AGENTS.md")).unwrap();
        ctx.store
            .resource_state_delete("instructions_deployment", &deployment_id)
            .unwrap();
        let undeploy_id = Uuid::new_v4().to_string();
        ctx.store
            .resource_state_put(
                "instructions_journal",
                &undeploy_id,
                &json!({"id":undeploy_id,"kind":"undeploy","status":"applying","target":target,"deployment_id":deployment_id,"deployment_record":previous,"files":[{"path":"AGENTS.md","before":"first","written":null,"removed":true}]}),
            )
            .unwrap();
        recover(&ctx).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "first"
        );
        assert_eq!(
            ctx.store
                .resource_state_get("instructions_deployment", &deployment_id)
                .unwrap(),
            Some(previous)
        );
    }

    #[test]
    #[cfg(unix)]
    fn recovery_refuses_a_retargeted_file() {
        let repo = test_repo();
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "original").unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("retarget.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = project_target(&ctx, &project);
        let root = resolve_target(&ctx, &target).unwrap();
        let original = root.join("AGENTS.md");
        let journal_id = Uuid::new_v4().to_string();
        ctx.store
            .resource_state_put(
                "instructions_journal",
                &journal_id,
                &json!({"id":journal_id,"kind":"deployment","status":"applying","target":target,"resolved_root":root,"resolved_paths":{"AGENTS.md":original},"deployment_id":Uuid::new_v4().to_string(),"previous_deployment":null,"files":[{"path":"AGENTS.md","before":"original","written":"changed","removed":false}]}),
            )
            .unwrap();
        fs::rename(&original, project.join("redirected.md")).unwrap();
        std::os::unix::fs::symlink(project.join("redirected.md"), &original).unwrap();
        assert!(recover(&ctx).is_err());
        assert_eq!(
            fs::read_to_string(project.join("redirected.md")).unwrap(),
            "original"
        );
    }

    #[test]
    fn undeploy_preserves_an_identical_file_it_never_wrote() {
        let repo = test_repo();
        crate::core::central_repo::set_runtime_base_dir_override(Some(
            crate::core::central_repo::base_dir()
                .canonicalize()
                .unwrap(),
        ));
        let project = repo._tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "already here").unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("adoption.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = project_target(&ctx, &project);
        let saved = save(
            &ctx,
            None,
            "Rules".into(),
            None,
            BTreeMap::from([("AGENTS.md".into(), "already here".into())]),
            None,
        )
        .unwrap();
        let preview = preview(&ctx, target, saved["id"].as_str().unwrap(), false).unwrap();
        apply(
            &ctx,
            preview["preview_id"].as_str().unwrap(),
            BTreeMap::new(),
            false,
        )
        .unwrap();
        let record = deployments(&ctx).unwrap().pop().unwrap();
        assert!(record["files"][0]["deployed"].is_null());
        undeploy(&ctx, record["id"].as_str().unwrap()).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "already here"
        );
    }
}
