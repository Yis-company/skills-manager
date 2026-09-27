use super::{
    model::{AuthRefs, Definition, DefinitionInput, Operation, ServerConfig, Target, Transport},
    store,
};
use crate::core::{error::AppError, host::HostCtx, repo_lock::RepoLock, resource_store};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn capabilities() -> Value {
    let agents = [
        (
            "claude_code",
            vec!["global", "project"],
            vec!["stdio", "http", "sse"],
        ),
        ("codex", vec!["global", "project"], vec!["stdio", "http"]),
        (
            "cursor",
            vec!["global", "project"],
            vec!["stdio", "http", "sse"],
        ),
        (
            "antigravity",
            vec!["global", "project"],
            vec!["stdio", "http"],
        ),
        ("hermes", vec!["global"], vec!["stdio", "http", "sse"]),
    ];
    let targets: Vec<Value> = agents.iter().map(|(key, scopes, features)| json!({
        "agentKey": key,
        "scopes": scopes.iter().map(|s| json!({"kind":s,"supported":true})).collect::<Vec<_>>(),
        "features": features,
        "limitations": match *key {
            "antigravity" => vec!["Credential interpolation and an explicit SSE transport selector are not verified"],
            "codex" => vec!["Project MCP config loads only from a trusted workspace"],
            "hermes" => vec!["Project MCP configuration is unsupported"],
            _ => vec![],
        }
    })).collect();
    json!({"targets": targets})
}

struct ConfigTarget {
    path: PathBuf,
    format: Format,
    entry_path: Vec<&'static str>,
    agent: String,
}
#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
    Yaml,
}
fn resolve(ctx: &HostCtx, target: &Target) -> Result<ConfigTarget, AppError> {
    let project = match &target.project_id {
        Some(id) => Some(
            ctx.store
                .get_project_by_id(id)
                .map_err(AppError::db)?
                .ok_or_else(|| AppError::not_found("Project not found"))?,
        ),
        None => None,
    };
    let project_path = project
        .as_ref()
        .map(|p| canonical_directory(Path::new(&p.path)))
        .transpose()?;
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::internal("Could not locate user home directory"))?;
    let home = canonical_directory(&home)?;
    let (path, format, entry_path) = match (target.agent_key.as_str(), project_path.as_ref()) {
        ("claude_code", Some(root)) => (root.join(".mcp.json"), Format::Json, vec!["mcpServers"]),
        ("claude_code", None) => (home.join(".claude.json"), Format::Json, vec!["mcpServers"]),
        ("codex", Some(root)) => (
            root.join(".codex/config.toml"),
            Format::Toml,
            vec!["mcp_servers"],
        ),
        ("codex", None) => (
            codex_home().join("config.toml"),
            Format::Toml,
            vec!["mcp_servers"],
        ),
        ("cursor", Some(root)) => (
            root.join(".cursor/mcp.json"),
            Format::Json,
            vec!["mcpServers"],
        ),
        ("cursor", None) => (
            home.join(".cursor/mcp.json"),
            Format::Json,
            vec!["mcpServers"],
        ),
        ("antigravity", Some(root)) => (
            root.join(".agents/mcp_config.json"),
            Format::Json,
            vec!["mcpServers"],
        ),
        ("antigravity", None) => (
            home.join(".gemini/config/mcp_config.json"),
            Format::Json,
            vec!["mcpServers"],
        ),
        ("hermes", Some(_)) => {
            return Err(AppError::invalid_input(
                "Hermes supports user-global MCP configuration only",
            ))
        }
        ("hermes", None) => (
            hermes_home().join("config.yaml"),
            Format::Yaml,
            vec!["mcp_servers"],
        ),
        _ => return Err(AppError::invalid_input("Unsupported MCP agent key")),
    };
    Ok(ConfigTarget {
        path,
        format,
        entry_path,
        agent: target.agent_key.clone(),
    })
}
fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"))
}
fn hermes_home() -> PathBuf {
    std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".hermes"))
}
fn canonical_directory(path: &Path) -> Result<PathBuf, AppError> {
    let path = path.canonicalize().map_err(AppError::io)?;
    if !path.is_dir() {
        return Err(AppError::invalid_input(
            "MCP configuration root must be an existing directory",
        ));
    }
    Ok(path)
}

pub fn inspect(ctx: &HostCtx, target: &Target) -> Result<Value, AppError> {
    let cfg = resolve(ctx, target)?;
    let entries = read_entries(&cfg)?;
    let definitions = store::list(ctx)?;
    let links = deployments_for(ctx, target)?;
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, value) in entries {
        seen.insert(name.clone());
        let link = links
            .iter()
            .find(|link| link.get("serverName").and_then(Value::as_str) == Some(name.as_str()));
        let linked = link.is_some();
        let managed_id = link.and_then(|link| link.get("definitionId").and_then(Value::as_str));
        if linked && managed_id.is_none() {
            return Err(AppError::invalid_input(
                "MCP deployment link has no definition id",
            ));
        }
        if let Some(id) = managed_id {
            resource_store::validate_id(id).map_err(|_| {
                AppError::invalid_input("MCP deployment link has an invalid definition id")
            })?;
        }
        let unchanged = link.is_some_and(|link| link.get("expectedEntry") == Some(&value));
        let override_value =
            link.and_then(|link| safe_link_override(&definitions, &cfg.agent, link));
        match normalize(&cfg.agent, &name, &value) {
            Ok(imported) => result.push(json!({"name":name,"managedId":managed_id,"status":if linked && unchanged {"managed"}else if linked {"conflict"}else{"unmanaged"},"importable":true,"definition":imported,"override":override_value})),
            Err(_) => result.push(json!({"name":name,"managedId":managed_id,"status":"conflict","importable":false,"reason":"Entry contains unsupported or opaque values","override":override_value})),
        }
    }
    for link in &links {
        let Some(name) = link.get("serverName").and_then(Value::as_str) else {
            continue;
        };
        if seen.contains(name) {
            continue;
        }
        let id = link
            .get("definitionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::invalid_input("MCP deployment link has no definition id"))?;
        resource_store::validate_id(id).map_err(|_| {
            AppError::invalid_input("MCP deployment link has an invalid definition id")
        })?;
        result.push(json!({"name":name,"managedId":id,"status":"conflict","importable":false,"reason":"Managed native entry is missing","override":safe_link_override(&definitions,&cfg.agent,link)}));
    }
    Ok(json!({"entries":result,"recoveries":recoveries_for(ctx,Some(target))?}))
}

fn safe_link_override(definitions: &[Definition], agent: &str, link: &Value) -> Option<Value> {
    let override_value = link
        .get("overrideConfig")
        .filter(|value| !value.is_null())?;
    let def_id = link.get("definitionId")?.as_str()?;
    let def = definitions.iter().find(|def| def.id == def_id)?;
    let overridden = apply_override(def, override_value).ok()?;
    let rendered = render_entry(agent, &overridden).ok()?;
    normalize(agent, &def.name, &rendered).ok()?;
    Some(override_value.clone())
}

pub fn import(
    ctx: &HostCtx,
    target: &Target,
    name: &str,
    conflict: Option<&str>,
    new_name: Option<&str>,
) -> Result<Value, AppError> {
    let cfg = resolve(ctx, target)?;
    let value = read_entries(&cfg)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| AppError::not_found("MCP server entry not found"))?;
    let mut input = normalize(&cfg.agent, name, &value)?;
    let existing = store::list(ctx)?;
    let mut expected_revision = None;
    if let Some(def) = existing.iter().find(|d| d.name == name) {
        match conflict.unwrap_or("keep") {
            "keep" => return Ok(json!({"definition":def,"kept":true})),
            "replace" => {
                input.id = Some(def.id.clone());
                expected_revision = Some(def.revision.clone());
            }
            "rename" => {
                input.name = new_name
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| AppError::invalid_input("rename requires newName"))?
                    .to_string();
                if existing.iter().any(|d| d.name == input.name) {
                    return Err(AppError::invalid_input(
                        "An MCP definition with this name already exists",
                    ));
                }
            }
            _ => {
                return Err(AppError::invalid_input(
                    "conflict must be keep, replace, or rename",
                ))
            }
        }
    }
    store::validate_portable_input(&input)?;
    Ok(json!({"draft":input,"expectedRevision":expected_revision,"source":"native-config"}))
}

pub fn preview(
    ctx: &HostCtx,
    target: &Target,
    operations: &[Operation],
    dry_run: bool,
) -> Result<Value, AppError> {
    let cfg = resolve(ctx, target)?;
    let entries = read_entries(&cfg)?;
    let defs = store::list(ctx)?;
    let current = deployments_for(ctx, target)?;
    let mut prepared = Vec::new();
    let mut changes = Vec::new();
    let mut names = std::collections::HashSet::new();
    let mut definition_ids = std::collections::HashSet::new();
    if operations.is_empty() || operations.len() > 64 {
        return Err(AppError::invalid_input(
            "An MCP preview needs 1-64 operations",
        ));
    }
    for op in operations {
        if !["deploy", "update", "undeploy"].contains(&op.kind.as_str()) {
            return Err(AppError::invalid_input(
                "MCP operation kind must be deploy, update, or undeploy",
            ));
        }
        if !definition_ids.insert(&op.definition_id) {
            return Err(AppError::invalid_input(
                "An MCP preview can modify each definition only once",
            ));
        }
        let def = defs.iter().find(|d| d.id == op.definition_id);
        let prior = current.iter().find(|d| {
            d.get("definitionId").and_then(Value::as_str) == Some(op.definition_id.as_str())
        });
        if op.kind != "undeploy" && def.is_none() {
            return Err(AppError::not_found("MCP definition not found"));
        }
        if op.kind == "undeploy" && prior.is_none() {
            return Err(AppError::not_found("MCP deployment not found"));
        }
        let prior_name = prior.and_then(|p| p.get("serverName").and_then(Value::as_str));
        if prior.is_some()
            && op
                .server_name
                .as_deref()
                .is_some_and(|name| Some(name) != prior_name)
        {
            return Err(AppError::invalid_input(
                "An existing MCP deployment must use its linked server name",
            ));
        }
        let name = op
            .server_name
            .clone()
            .or_else(|| prior_name.map(str::to_owned))
            .or_else(|| def.map(|d| d.name.clone()))
            .ok_or_else(|| AppError::invalid_input("MCP deployment has no server name"))?;
        validate_server_name(&name)?;
        let existing = entries.iter().find(|(n, _)| n == &name).map(|(_, v)| v);
        let mut final_name = name.clone();
        let mut precondition_name = name.clone();
        let base = prior.and_then(|p| p.get("expectedEntry"));
        if op.kind == "undeploy" && op.override_config.is_some() {
            return Err(AppError::invalid_input("Undeploy cannot set MCP overrides"));
        }
        let effective_override = if op.kind == "undeploy" {
            None
        } else {
            op.override_config
                .clone()
                .or_else(|| prior.and_then(|p| p.get("overrideConfig").cloned()))
        };
        let mut desired = if op.kind == "undeploy" {
            None
        } else {
            let def = def.ok_or_else(|| AppError::not_found("MCP definition not found"))?;
            let effective = if let Some(value) = effective_override.as_ref() {
                apply_override(def, value)?
            } else {
                def.clone()
            };
            Some(render_entry(&cfg.agent, &effective)?)
        };
        if let Some(existing_entry) = existing.filter(|_| op.kind != "undeploy" && prior.is_none())
        {
            match op.conflict.as_deref().unwrap_or("keep") {
                "keep" => {
                    changes
                        .push(json!({"name":name,"kind":"keep","conflict":"kept","warnings":[]}));
                    prepared.push(
                        json!({"kind":"keep","definitionId":op.definition_id,"serverName":name}),
                    );
                    continue;
                }
                "replace" => {
                    normalize(&cfg.agent, &name, existing_entry)?;
                }
                "rename" => {
                    final_name = op
                        .new_name
                        .as_deref()
                        .filter(|v| !v.trim().is_empty())
                        .ok_or_else(|| AppError::invalid_input("rename requires newName"))?
                        .to_owned();
                    validate_server_name(&final_name)?;
                    if entries.iter().any(|(n, _)| n == &final_name) {
                        return Err(AppError::invalid_input("Rename target already exists"));
                    }
                    precondition_name = final_name.clone();
                }
                _ => {
                    return Err(AppError::invalid_input(
                        "conflict must be keep, replace, or rename",
                    ))
                }
            }
        }
        if op.kind == "undeploy" {
            if existing != base {
                changes.push(json!({"name":name,"kind":"preserve_edited","warnings":["Native entry changed outside Skills Manager"]}));
                prepared
                    .push(json!({"kind":"keep","definitionId":op.definition_id,"serverName":name}));
                continue;
            } else {
                changes.push(json!({"name":name,"kind":"remove","warnings":[]}));
            }
        } else {
            if let Some(base) = base {
                normalize(&cfg.agent, &name, base)?;
                if existing != Some(base) {
                    if existing.is_none() && op.conflict.as_deref() == Some("keep") {
                        changes.push(json!({"name":name,"kind":"preserve_edited","warnings":["Managed native entry was deleted outside Agents Manager"]}));
                        prepared.push(json!({"kind":"keep","definitionId":op.definition_id,"serverName":name}));
                        continue;
                    }
                    if let Some(current_entry) = existing {
                        if normalize(&cfg.agent, &name, current_entry).is_err() {
                            if op.conflict.as_deref() == Some("keep") {
                                changes.push(json!({"name":name,"kind":"preserve_edited","warnings":["Native entry cannot be safely represented"]}));
                                prepared.push(json!({"kind":"keep","definitionId":op.definition_id,"serverName":name}));
                                continue;
                            }
                            return Err(AppError::invalid_input("Edited MCP entry contains unsupported or opaque values; keep it or review manually"));
                        }
                    }
                    desired = merge_native_entry(
                        base,
                        existing,
                        desired.as_ref(),
                        op.conflict.as_deref(),
                    )?;
                }
            }
            let result = desired
                .as_ref()
                .ok_or_else(|| AppError::internal("MCP update produced no entry"))?;
            normalize(&cfg.agent, &final_name, result)?;
            let reviewed_existing = if precondition_name == name {
                existing
            } else {
                None
            };
            changes.push(json!({"name":final_name,"kind":if reviewed_existing.is_some(){"update"}else{"add"},"beforeSummary":reviewed_existing.map(summary),"afterSummary":summary(result),"warnings":[]}));
        }
        if !names.insert(precondition_name.clone()) {
            return Err(AppError::invalid_input(
                "A preview can modify each server name only once",
            ));
        }
        let selected = entries
            .iter()
            .find(|(n, _)| n == &precondition_name)
            .map(|(_, v)| v);
        if let Some(selected) = selected {
            normalize(&cfg.agent, &precondition_name, selected)?;
        }
        prepared.push(json!({"kind":op.kind,"definitionId":op.definition_id,"serverName":precondition_name,"finalName":final_name,"revision":def.map(|d|d.revision.as_str()).or_else(||prior.and_then(|p|p.get("revision").and_then(Value::as_str))),"linkedId":prior.and_then(|p|p.get("id")),"linkedRevision":prior.and_then(|p|p.get("revision")),"entry":desired,"overrideConfig":effective_override,"precondition":{"present":selected.is_some(),"entry":selected}}));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let preview = json!({"id":id,"createdAt":now,"expiresAt":now+1800,"target":target,"configPath":cfg.path.to_string_lossy(),"operations":prepared,"changes":changes});
    if !dry_run {
        prune_previews(ctx, now)?;
        ctx.store
            .resource_state_put("mcps_preview", &id, &preview)
            .map_err(AppError::db)?
    }
    let mut response = preview;
    if let Some(o) = response.as_object_mut() {
        o.remove("configPath");
        o.remove("operations");
        o.remove("createdAt");
        o.remove("expiresAt");
        o.insert("previewId".into(), json!(id));
        o.insert("dryRun".into(), json!(dry_run));
    }
    Ok(response)
}
pub fn apply(ctx: &HostCtx, id: &str) -> Result<Value, AppError> {
    resource_store::validate_id(id).map_err(|e| AppError::invalid_input(e.to_string()))?;
    let _lock = RepoLock::acquire_foreground("apply MCP deployment").map_err(AppError::db)?;
    if !ctx
        .store
        .resource_state_list("mcps_recovery")
        .map_err(AppError::db)?
        .is_empty()
    {
        return Err(AppError::invalid_input(
            "An MCP deployment needs recovery before another can be applied",
        ));
    }
    let preview = ctx
        .store
        .resource_state_get("mcps_preview", id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("MCP preview not found"))?;
    let target: Target = serde_json::from_value(
        preview
            .get("target")
            .cloned()
            .ok_or_else(|| AppError::invalid_input("MCP preview is invalid"))?,
    )
    .map_err(|e| AppError::invalid_input(e.to_string()))?;
    if preview
        .get("expiresAt")
        .and_then(Value::as_i64)
        .is_none_or(|expiry| chrono::Utc::now().timestamp() > expiry)
    {
        return Err(AppError::not_found(
            "MCP preview expired; create a new preview",
        ));
    }
    let cfg = resolve(ctx, &target)?;
    if preview.get("configPath").and_then(Value::as_str)
        != Some(cfg.path.to_string_lossy().as_ref())
    {
        return Err(AppError::invalid_input("MCP preview target changed"));
    }
    let ops = preview
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid_input("MCP preview is invalid"))?;
    let defs = store::list(ctx)?;
    let original = read_snapshot(&cfg)?;
    let mut entries = entries_from_snapshot(&cfg, original.as_deref())?;
    let mut raw = original.clone().unwrap_or_default();
    let mut puts = Vec::new();
    let mut deletes = Vec::new();
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut changed = Vec::new();
    let current = ctx
        .store
        .resource_state_list("mcps_deployment")
        .map_err(AppError::db)?;
    for op in ops {
        let kind = op.get("kind").and_then(Value::as_str).unwrap_or("");
        let name = op.get("serverName").and_then(Value::as_str).unwrap_or("");
        let final_name = op.get("finalName").and_then(Value::as_str).unwrap_or(name);
        let def_id = op.get("definitionId").and_then(Value::as_str).unwrap_or("");
        if kind == "keep" {
            continue;
        }
        if !matches!(kind, "deploy" | "update" | "undeploy") {
            return Err(AppError::invalid_input(
                "MCP preview contains an invalid operation",
            ));
        }
        validate_server_name(name)?;
        validate_server_name(final_name)?;
        let def = defs.iter().find(|d| d.id == def_id);
        if kind != "undeploy" && def.is_none() {
            return Err(AppError::not_found(
                "MCP definition was removed after preview",
            ));
        }
        if let Some(def) = def {
            if op.get("revision").and_then(Value::as_str) != Some(def.revision.as_str()) {
                return Err(AppError::invalid_input(
                    "MCP definition changed since preview; create a new preview",
                ));
            }
        }
        let old = current.iter().find(|d| {
            d.get("target") == Some(&json!(target))
                && d.get("definitionId").and_then(Value::as_str) == Some(def_id)
        });
        if op.get("linkedId").and_then(Value::as_str)
            != old.and_then(|d| d.get("id").and_then(Value::as_str))
            || op.get("linkedRevision").and_then(Value::as_str)
                != old.and_then(|d| d.get("revision").and_then(Value::as_str))
        {
            return Err(AppError::invalid_input(
                "MCP deployment link changed since preview; create a new preview",
            ));
        }
        let pre = op
            .get("precondition")
            .ok_or_else(|| AppError::invalid_input("MCP preview is missing a precondition"))?;
        let expected_present = pre
            .get("present")
            .and_then(Value::as_bool)
            .ok_or_else(|| AppError::invalid_input("MCP preview has an invalid precondition"))?;
        let expected = pre.get("entry").cloned().unwrap_or(Value::Null);
        if !entry_matches(&entries, name, expected_present, &expected) {
            return Err(AppError::invalid_input(
                "Selected MCP entry changed since preview; create a new preview",
            ));
        }
        if expected_present {
            normalize(&cfg.agent, name, &expected)?;
        }
        before.push(json!({"name":name,"present":expected_present,"entry":expected}));
        if kind == "undeploy" {
            raw = patch_text(cfg.format, &raw, cfg.entry_path[0], name, None)?;
            entries.retain(|(n, _)| n != name);
            after.push(json!({"name":name,"present":false,"entry":null}));
            let sid = old
                .and_then(|d| d.get("id").and_then(Value::as_str))
                .ok_or_else(|| {
                    AppError::invalid_input("MCP deployment was detached since preview")
                })?;
            deletes.push(sid.to_owned());
            changed.push(json!({"name":name,"removed":true,"preservedEdited":false}));
        } else {
            let entry = op
                .get("entry")
                .cloned()
                .ok_or_else(|| AppError::invalid_input("MCP preview entry missing"))?;
            normalize(&cfg.agent, final_name, &entry)?;
            raw = patch_text(
                cfg.format,
                &raw,
                cfg.entry_path[0],
                final_name,
                Some(&entry),
            )?;
            if let Some(i) = entries.iter().position(|(n, _)| n == final_name) {
                entries[i].1 = entry.clone()
            } else {
                entries.push((final_name.to_owned(), entry.clone()))
            };
            after.push(json!({"name":final_name,"present":true,"entry":entry}));
            let def =
                def.ok_or_else(|| AppError::not_found("MCP definition was removed after preview"))?;
            let state_id = old
                .and_then(|d| d.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let state = json!({"id":state_id,"target":target,"definitionId":def.id,"serverName":final_name,"revision":def.revision,"overrideConfig":op.get("overrideConfig"),"expectedEntry":entry});
            puts.push(state);
            changed.push(json!({"name":final_name,"kind":"deployed"}));
        }
    }
    if before.is_empty() {
        ctx.store
            .resource_state_delete("mcps_preview", id)
            .map_err(AppError::db)?;
        return Ok(json!({"applied":true,"changes":changed}));
    }
    verify_scoped_patch(&cfg, original.as_deref(), &raw, &after)?;
    let recovery_id = uuid::Uuid::new_v4().to_string();
    let journal = json!({"id":recovery_id,"previewId":id,"target":target,"configPath":cfg.path.to_string_lossy(),"before":before,"after":after,"puts":puts,"deletes":deletes});
    ctx.store
        .resource_state_put("mcps_recovery", &recovery_id, &journal)
        .map_err(AppError::db)?;
    let fresh = match read_snapshot(&cfg) {
        Ok(value) => value,
        Err(_) => {
            return Ok(
                json!({"applied":false,"partial":true,"recoveryId":recovery_id,"message":"MCP config could not be rechecked; recover before another apply","changes":changed}),
            );
        }
    };
    if fresh != original {
        ctx.store
            .resource_state_delete("mcps_recovery", &recovery_id)
            .map_err(AppError::db)?;
        return Err(AppError::invalid_input(
            "MCP config changed while applying; create a new preview",
        ));
    }
    if raw != original.as_deref().unwrap_or_default()
        && resource_store::atomic_write(&cfg.path, raw.as_bytes()).is_err()
    {
        return Ok(
            json!({"applied":false,"partial":true,"recoveryId":recovery_id,"message":"MCP config write was interrupted; recover before another apply","changes":changed}),
        );
    }
    if finalize_recovery(ctx, &journal).is_err() {
        return Ok(
            json!({"applied":false,"partial":true,"recoveryId":recovery_id,"message":"MCP config was written, but deployment state needs recovery","changes":changed}),
        );
    }
    Ok(json!({"applied":true,"changes":changed}))
}

fn verify_scoped_patch(
    cfg: &ConfigTarget,
    original: Option<&str>,
    patched: &str,
    expected: &[Value],
) -> Result<(), AppError> {
    let mut before = parse_root(cfg.format, original)?;
    let mut after = parse_root(cfg.format, Some(patched))?;
    let entries = entries_from_snapshot(cfg, Some(patched))?;
    for item in expected {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::invalid_input("MCP recovery entry is invalid"))?;
        let present = item
            .get("present")
            .and_then(Value::as_bool)
            .ok_or_else(|| AppError::invalid_input("MCP recovery entry is invalid"))?;
        let entry = item.get("entry").unwrap_or(&Value::Null);
        if !entry_matches(&entries, name, present, entry) {
            return Err(AppError::invalid_input(
                "MCP patch did not produce the reviewed entry",
            ));
        }
        for root in [&mut before, &mut after] {
            let map_key = cfg.entry_path[0];
            if let Some(map) = root.get_mut(map_key).and_then(Value::as_object_mut) {
                map.remove(name);
                if map.is_empty() {
                    root.as_object_mut().unwrap().remove(map_key);
                }
            }
        }
    }
    if before != after {
        return Err(AppError::invalid_input(
            "MCP patch changed unrelated configuration fields",
        ));
    }
    Ok(())
}

fn finalize_recovery(ctx: &HostCtx, journal: &Value) -> Result<(), AppError> {
    let id = journal
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    let puts = journal
        .get("puts")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    let deletes = journal
        .get("deletes")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    for state in puts {
        let state_id = state
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::invalid_input("MCP recovery state is invalid"))?;
        ctx.store
            .resource_state_put("mcps_deployment", state_id, state)
            .map_err(AppError::db)?;
    }
    for state_id in deletes {
        let state_id = state_id
            .as_str()
            .ok_or_else(|| AppError::invalid_input("MCP recovery state is invalid"))?;
        ctx.store
            .resource_state_delete("mcps_deployment", state_id)
            .map_err(AppError::db)?;
    }
    let preview_id = journal
        .get("previewId")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    ctx.store
        .resource_state_delete("mcps_preview", preview_id)
        .map_err(AppError::db)?;
    ctx.store
        .resource_state_delete("mcps_recovery", id)
        .map_err(AppError::db)?;
    Ok(())
}

fn matches_snapshots(entries: &[(String, Value)], snapshots: &[Value]) -> bool {
    snapshots.iter().all(|item| {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            return false;
        };
        let Some(present) = item.get("present").and_then(Value::as_bool) else {
            return false;
        };
        entry_matches(
            entries,
            name,
            present,
            item.get("entry").unwrap_or(&Value::Null),
        )
    })
}

pub fn recover(ctx: &HostCtx, id: &str) -> Result<Value, AppError> {
    resource_store::validate_id(id)
        .map_err(|_| AppError::invalid_input("Invalid MCP recovery identifier"))?;
    let _lock = RepoLock::acquire_foreground("recover MCP deployment").map_err(AppError::db)?;
    let journal = ctx
        .store
        .resource_state_get("mcps_recovery", id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("MCP recovery journal not found"))?;
    let target: Target =
        serde_json::from_value(journal.get("target").cloned().unwrap_or(Value::Null))
            .map_err(|_| AppError::invalid_input("MCP recovery target is invalid"))?;
    let cfg = resolve(ctx, &target)?;
    if journal.get("configPath").and_then(Value::as_str)
        != Some(cfg.path.to_string_lossy().as_ref())
    {
        return Err(AppError::invalid_input("MCP recovery target changed"));
    }
    let entries = read_entries(&cfg)?;
    let before = journal
        .get("before")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    let after = journal
        .get("after")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid_input("MCP recovery journal is invalid"))?;
    if matches_snapshots(&entries, after) {
        finalize_recovery(ctx, &journal)?;
        return Ok(json!({"recovered":true,"fileApplied":true,"recoveryId":id}));
    }
    if matches_snapshots(&entries, before) {
        ctx.store
            .resource_state_delete("mcps_recovery", id)
            .map_err(AppError::db)?;
        return Ok(json!({"recovered":true,"fileApplied":false,"recoveryId":id}));
    }
    Err(AppError::invalid_input(
        "MCP selected entries changed during recovery; inspect and restore them before retrying",
    ))
}

fn recoveries_for(ctx: &HostCtx, target: Option<&Target>) -> Result<Vec<Value>, AppError> {
    Ok(ctx
        .store
        .resource_state_list("mcps_recovery")
        .map_err(AppError::db)?
        .into_iter()
        .filter(|journal| target.is_none_or(|t| journal.get("target") == Some(&json!(t))))
        .filter_map(|journal| {
            journal.get("id").and_then(Value::as_str).map(
                |id| json!({"id":id,"status":"pending","message":"MCP deployment needs recovery"}),
            )
        })
        .collect())
}
pub fn deployments(ctx: &HostCtx, target: Option<&Target>) -> Result<Value, AppError> {
    let all = ctx
        .store
        .resource_state_list("mcps_deployment")
        .map_err(AppError::db)?;
    let filtered: Vec<Value> = all
        .into_iter()
        .filter(|d| target.is_none_or(|t| d.get("target") == Some(&json!(t))))
        .map(|mut d| {
            if let Some(o) = d.as_object_mut() {
                o.remove("expectedEntry");
            }
            d
        })
        .collect();
    Ok(json!({"deployments":filtered,"recoveries":recoveries_for(ctx,target)?}))
}
pub fn undeploy(
    ctx: &HostCtx,
    target: &Target,
    definition_id: &str,
    server_name: Option<&str>,
) -> Result<Value, AppError> {
    let links = ctx
        .store
        .resource_state_list("mcps_deployment")
        .map_err(AppError::db)?;
    let name = server_name
        .map(str::to_owned)
        .or_else(|| {
            links
                .iter()
                .find(|d| {
                    d.get("target") == Some(&json!(target))
                        && d.get("definitionId").and_then(Value::as_str) == Some(definition_id)
                })
                .and_then(|d| {
                    d.get("serverName")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        })
        .ok_or_else(|| AppError::not_found("MCP deployment not found"))?;
    let planned = preview(
        ctx,
        target,
        &[Operation {
            kind: "undeploy".into(),
            definition_id: definition_id.to_owned(),
            server_name: Some(name),
            conflict: None,
            new_name: None,
            override_config: None,
        }],
        false,
    )?;
    let id = planned
        .get("previewId")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::internal("MCP undeploy preview was not persisted"))?;
    let mut result = apply(ctx, id)?;
    let preserved_edited = planned["changes"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["kind"] == "preserve_edited"));
    let removed = result["applied"] == true
        && result["changes"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["removed"] == true));
    if let Some(object) = result.as_object_mut() {
        object.insert("removed".into(), json!(removed));
        object.insert("preservedEdited".into(), json!(preserved_edited));
    }
    Ok(result)
}

fn deployments_for(ctx: &HostCtx, target: &Target) -> Result<Vec<Value>, AppError> {
    Ok(ctx
        .store
        .resource_state_list("mcps_deployment")
        .map_err(AppError::db)?
        .into_iter()
        .filter(|d| d.get("target") == Some(&json!(target)))
        .collect())
}
fn entry_matches(entries: &[(String, Value)], name: &str, present: bool, expected: &Value) -> bool {
    let found = entries.iter().find(|(n, _)| n == name).map(|(_, v)| v);
    found.is_some() == present && (!present || found == Some(expected))
}
fn summary(value: &Value) -> Value {
    value.clone()
}

fn validate_server_name(name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || name.len() > 128
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(AppError::invalid_input(
            "MCP server name must be 1-128 printable characters without path separators",
        ));
    }
    Ok(())
}

fn merge_native_entry(
    base: &Value,
    current: Option<&Value>,
    desired: Option<&Value>,
    choice: Option<&str>,
) -> Result<Option<Value>, AppError> {
    merge_native_value(Some(base), current, desired, choice)
}

fn merge_native_value(
    base: Option<&Value>,
    current: Option<&Value>,
    desired: Option<&Value>,
    choice: Option<&str>,
) -> Result<Option<Value>, AppError> {
    if current == base {
        return Ok(desired.cloned());
    }
    if desired == base || current == desired {
        return Ok(current.cloned());
    }
    if let (Some(current), Some(desired)) = (
        current.and_then(Value::as_object),
        desired.and_then(Value::as_object),
    ) {
        let base = base.and_then(Value::as_object);
        let mut keys = std::collections::BTreeSet::new();
        if let Some(base) = base {
            keys.extend(base.keys());
        }
        keys.extend(current.keys());
        keys.extend(desired.keys());
        let mut merged = Map::new();
        for key in keys {
            if let Some(value) = merge_native_value(
                base.and_then(|v| v.get(key)),
                current.get(key),
                desired.get(key),
                choice,
            )? {
                merged.insert(key.clone(), value);
            }
        }
        return Ok(Some(Value::Object(merged)));
    }
    match choice {
        Some("keep") => Ok(current.cloned()),
        Some("replace") => Ok(desired.cloned()),
        _ => Err(AppError::invalid_input("MCP target and library changed the same field; choose keep or replace and preview again")),
    }
}

fn prune_previews(ctx: &HostCtx, now: i64) -> Result<(), AppError> {
    let mut previews = ctx
        .store
        .resource_state_list("mcps_preview")
        .map_err(AppError::db)?;
    previews.sort_by_key(|v| v.get("createdAt").and_then(Value::as_i64).unwrap_or(0));
    let excess = previews.len().saturating_sub(255);
    for (index, preview) in previews.iter().enumerate() {
        if index < excess
            || preview
                .get("expiresAt")
                .and_then(Value::as_i64)
                .is_none_or(|expiry| expiry < now)
        {
            if let Some(id) = preview.get("id").and_then(Value::as_str) {
                ctx.store
                    .resource_state_delete("mcps_preview", id)
                    .map_err(AppError::db)?;
            }
        }
    }
    Ok(())
}
fn render_entry(agent: &str, def: &Definition) -> Result<Value, AppError> {
    let mut obj = Map::new();
    let stdio = matches!(def.transport, Transport::Stdio);
    if matches!(def.transport, Transport::Sse) && matches!(agent, "codex" | "antigravity") {
        return Err(AppError::invalid_input(
            "This agent has no verified explicit SSE MCP transport",
        ));
    }
    if agent == "antigravity"
        && (def.auth.is_some()
            || def.server.env.values().any(|v| v.contains("${"))
            || def.server.headers.values().any(|v| v.contains("${"))
            || def.server.args.iter().any(|v| v.contains("${"))
            || def
                .server
                .command
                .as_deref()
                .is_some_and(|v| v.contains("${"))
            || def.server.url.as_deref().is_some_and(|v| v.contains("${")))
    {
        return Err(AppError::invalid_input(
            "Antigravity credential interpolation is not verified",
        ));
    }
    if !stdio
        && (!def.server.env.is_empty() || def.server.env_file.is_some() || def.server.cwd.is_some())
    {
        return Err(AppError::invalid_input(
            "Remote MCP servers cannot use stdio environment, envFile, or cwd",
        ));
    }
    if stdio && (!def.server.headers.is_empty() || def.auth.is_some()) {
        return Err(AppError::invalid_input(
            "Stdio MCP servers cannot use HTTP headers or authentication",
        ));
    }
    if let Some(cwd) = &def.server.cwd {
        if agent != "codex" && agent != "antigravity" {
            return Err(AppError::invalid_input(
                "This agent does not support MCP cwd",
            ));
        }
        obj.insert("cwd".into(), json!(cwd));
    }
    if let Some(env_file) = &def.server.env_file {
        if agent != "cursor" {
            return Err(AppError::invalid_input(
                "This agent does not support MCP envFile",
            ));
        }
        obj.insert("envFile".into(), json!(env_file));
    }
    if stdio {
        obj.insert("command".into(), json!(def.server.command));
        if !def.server.args.is_empty() {
            obj.insert("args".into(), json!(def.server.args));
        }
        if agent == "cursor" {
            obj.insert("type".into(), json!("stdio"));
        }
        if !def.server.env.is_empty() {
            if agent == "codex" {
                let mut names = Vec::new();
                for (key, value) in &def.server.env {
                    if env_name(value) != Some(key.as_str()) {
                        return Err(AppError::invalid_input(
                            "Codex env_vars can only forward a variable under its own name",
                        ));
                    }
                    names.push(key);
                }
                obj.insert("env_vars".into(), json!(names));
            } else {
                obj.insert(
                    "env".into(),
                    json!(def
                        .server
                        .env
                        .iter()
                        .map(|(key, value)| (key, native_reference(agent, value)))
                        .collect::<std::collections::BTreeMap<_, _>>()),
                );
            }
        }
    } else {
        obj.insert(
            if agent == "antigravity" {
                "serverUrl"
            } else {
                "url"
            }
            .into(),
            json!(def.server.url),
        );
        if agent == "claude_code" {
            obj.insert(
                "type".into(),
                json!(if matches!(def.transport, Transport::Sse) {
                    "sse"
                } else {
                    "http"
                }),
            );
        } else if agent == "hermes" && matches!(def.transport, Transport::Sse) {
            obj.insert("transport".into(), json!("sse"));
        }
        let mut headers = def.server.headers.clone();
        if let Some(auth) = &def.auth {
            if let Some(env) = &auth.bearer_token_env_var {
                if agent != "codex" {
                    return Err(AppError::invalid_input(
                        "Only Codex supports bearerTokenEnvVar",
                    ));
                }
                obj.insert("bearer_token_env_var".into(), json!(env));
            }
            for (key, value) in &auth.credential_refs {
                if headers.insert(key.clone(), value.clone()).is_some() {
                    return Err(AppError::invalid_input(
                        "MCP header is set in both server and auth",
                    ));
                }
            }
        }
        if agent == "codex" {
            let mut public = Map::new();
            let mut referenced = Map::new();
            for (key, value) in headers {
                if let Some(name) = key
                    .eq_ignore_ascii_case("authorization")
                    .then(|| value.strip_prefix("Bearer ").and_then(env_name))
                    .flatten()
                {
                    if obj.contains_key("bearer_token_env_var") {
                        return Err(AppError::invalid_input(
                            "MCP Authorization is configured twice",
                        ));
                    }
                    obj.insert("bearer_token_env_var".into(), json!(name));
                } else if let Some(name) = env_name(&value) {
                    referenced.insert(key, json!(name));
                } else if is_public_header(&key) {
                    public.insert(key, json!(value));
                } else {
                    return Err(AppError::invalid_input(
                        "Codex HTTP headers must use an environment variable reference",
                    ));
                }
            }
            if !public.is_empty() {
                obj.insert("http_headers".into(), Value::Object(public));
            }
            if !referenced.is_empty() {
                obj.insert("env_http_headers".into(), Value::Object(referenced));
            }
        } else if !headers.is_empty() {
            obj.insert(
                "headers".into(),
                json!(headers
                    .iter()
                    .map(|(key, value)| (key, native_reference(agent, value)))
                    .collect::<std::collections::BTreeMap<_, _>>()),
            );
        }
    }
    Ok(Value::Object(obj))
}

fn env_name(value: &str) -> Option<&str> {
    value
        .strip_prefix("${env:")
        .and_then(|v| v.strip_suffix('}'))
        .filter(|v| valid_env_name(v))
}

fn valid_env_name(value: &str) -> bool {
    !value.is_empty()
        && !value.as_bytes()[0].is_ascii_digit()
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn portable_reference(agent: &str, value: &str) -> Option<String> {
    if env_name(value).is_some() {
        return Some(value.to_owned());
    }
    if let Some(inner) = value.strip_prefix("Bearer ") {
        return portable_reference(agent, inner).map(|v| format!("Bearer {v}"));
    }
    if matches!(agent, "claude_code" | "hermes") {
        let name = value.strip_prefix("${")?.strip_suffix('}')?;
        if valid_env_name(name) {
            return Some(format!("${{env:{name}}}"));
        }
    }
    None
}

fn native_reference(agent: &str, value: &str) -> String {
    if let Some(inner) = value.strip_prefix("Bearer ") {
        return format!("Bearer {}", native_reference(agent, inner));
    }
    match (agent, env_name(value)) {
        ("claude_code", Some(name)) => format!("${{{name}}}"),
        _ => value.to_owned(),
    }
}

fn is_public_header(key: &str) -> bool {
    matches!(key.to_ascii_lowercase().as_str(), "content-type" | "accept")
}
fn apply_override(definition: &Definition, override_value: &Value) -> Result<Definition, AppError> {
    if override_value
        .as_object()
        .is_none_or(|o| o.keys().any(|k| k != "server"))
    {
        return Err(AppError::invalid_input(
            "overrideConfig may contain only server fields",
        ));
    }
    let server = override_value
        .get("server")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::invalid_input("overrideConfig must contain server object"))?;
    let mut result = definition.clone();
    let mut portable = serde_json::to_value(&result.server).map_err(AppError::internal)?;
    let object = portable
        .as_object_mut()
        .ok_or_else(|| AppError::internal("MCP server is not an object"))?;
    for (key, value) in server {
        if !["command", "args", "env", "url", "headers", "envFile", "cwd"].contains(&key.as_str()) {
            return Err(AppError::invalid_input(
                "overrideConfig contains an unsupported MCP field",
            ));
        }
        object.insert(key.clone(), value.clone());
    }
    result.server = serde_json::from_value(portable)
        .map_err(|_| AppError::invalid_input("overrideConfig contains invalid server fields"))?;
    store::validate_portable_input(&DefinitionInput {
        id: Some(result.id.clone()),
        name: result.name.clone(),
        transport: result.transport.clone(),
        server: result.server.clone(),
        auth: result.auth.clone(),
        source: result.source.clone(),
    })?;
    Ok(result)
}

fn patch_text(
    format: Format,
    text: &str,
    map_key: &str,
    name: &str,
    value: Option<&Value>,
) -> Result<String, AppError> {
    match format {
        Format::Json => patch_jsonc(text, map_key, name, value),
        Format::Toml => patch_toml(text, name, value),
        Format::Yaml => patch_yaml(text, name, value),
    }
}
fn patch_toml(text: &str, name: &str, value: Option<&Value>) -> Result<String, AppError> {
    let mut doc: toml_edit::DocumentMut = if text.trim().is_empty() {
        "".parse().unwrap()
    } else {
        text.parse()
            .map_err(|_| AppError::invalid_input("MCP TOML config is malformed or ambiguous"))?
    };
    if doc.get("mcp_servers").is_none() && value.is_some() {
        doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if let Some(item) = doc.get_mut("mcp_servers") {
        let table = item
            .as_table_mut()
            .ok_or_else(|| AppError::invalid_input("mcp_servers TOML value must be a table"))?;
        if let Some(value) = value {
            table.insert(name, to_toml_item(value)?);
        } else {
            table.remove(name);
        }
    }
    Ok(doc.to_string())
}
fn to_toml_item(value: &Value) -> Result<toml_edit::Item, AppError> {
    Ok(toml_edit::Item::Value(to_toml_value(value)?))
}
fn to_toml_value(value: &Value) -> Result<toml_edit::Value, AppError> {
    use toml_edit::{Array, InlineTable, Value as T};
    Ok(match value {
        Value::String(s) => T::from(s.as_str()),
        Value::Bool(b) => T::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                T::from(i)
            } else if let Some(f) = n.as_f64() {
                T::from(f)
            } else {
                return Err(AppError::invalid_input(
                    "MCP number cannot be represented in TOML",
                ));
            }
        }
        Value::Array(values) => {
            let mut a = Array::new();
            for v in values {
                a.push(to_toml_value(v)?);
            }
            T::Array(a)
        }
        Value::Object(values) => {
            let mut t = InlineTable::new();
            for (k, v) in values {
                t.insert(k, to_toml_value(v)?);
            }
            T::InlineTable(t)
        }
        Value::Null => {
            return Err(AppError::invalid_input(
                "MCP config cannot contain null TOML values",
            ))
        }
    })
}
fn patch_yaml(text: &str, name: &str, value: Option<&Value>) -> Result<String, AppError> {
    yaml_to_json(text)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header = lines
        .iter()
        .position(|l| yaml_line_key(l, 0).as_deref() == Some("mcp_servers"));
    if let Some(h) = header {
        if !lines[h].trim_end().ends_with(':') {
            return Err(AppError::invalid_input(
                "Inline mcp_servers YAML maps cannot be safely edited",
            ));
        }
    }
    let mut start = None;
    let mut end = None;
    if let Some(h) = header {
        let mut i = h + 1;
        while i < lines.len() {
            let t = lines[i];
            if !t.trim().is_empty() && !t.trim_start().starts_with('#') && !t.starts_with(' ') {
                break;
            }
            if yaml_line_key(t, 2).as_deref() == Some(name) {
                start = Some(i);
                let mut j = i + 1;
                while j < lines.len() {
                    let x = lines[j];
                    if yaml_line_key(x, 2).is_some() {
                        break;
                    }
                    if !x.trim().is_empty() && !x.starts_with(' ') && !x.starts_with('#') {
                        break;
                    }
                    j += 1
                }
                let mut boundary = j;
                while boundary > i + 1 {
                    let prev = lines[boundary - 1];
                    let trimmed = prev.trim_start();
                    if (prev.starts_with("  ")
                        && !prev.starts_with("    ")
                        && trimmed.starts_with('#'))
                        || prev.trim().is_empty()
                    {
                        boundary -= 1
                    } else {
                        break;
                    }
                }
                end = Some(boundary);
                break;
            }
            i += 1
        }
    }
    if let Some(s) = start {
        let e = end.unwrap_or(s + 1);
        if let Some(value) = value {
            let block = yaml_entry(name, value)?;
            return Ok(format!(
                "{}{}{}",
                lines[..s].concat(),
                block,
                lines[e..].concat()
            ));
        }
        return Ok(format!("{}{}", lines[..s].concat(), lines[e..].concat()));
    }
    if value.is_none() {
        return Ok(text.to_owned());
    }
    let block = yaml_entry(name, value.unwrap())?;
    if let Some(h) = header {
        let mut insertion = h + 1;
        while insertion < lines.len()
            && (lines[insertion].starts_with("  ")
                || lines[insertion].starts_with('#')
                || lines[insertion].trim().is_empty())
        {
            insertion += 1
        }
        return Ok(format!(
            "{}{}{}",
            lines[..insertion].concat(),
            block,
            lines[insertion..].concat()
        ));
    }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n')
    }
    out.push_str("mcp_servers:\n");
    out.push_str(&block);
    Ok(out)
}
fn yaml_entry(name: &str, value: &Value) -> Result<String, AppError> {
    let yaml = serde_yaml::to_string(value).map_err(AppError::internal)?;
    let body = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let body = body
        .lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "  {}:\n{}\n",
        serde_json::to_string(name).map_err(AppError::internal)?,
        body
    ))
}
fn yaml_line_key(line: &str, indent: usize) -> Option<String> {
    if line.chars().take_while(|c| *c == ' ').count() != indent {
        return None;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('{') || trimmed.starts_with('[') {
        return None;
    }
    let b = trimmed.as_bytes();
    let (mut quote, mut single, mut escape) = (false, false, false);
    let mut split = None;
    for (i, c) in b.iter().copied().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if quote && c == b'\\' {
            escape = true;
            continue;
        }
        if !single && c == b'"' {
            quote = !quote;
            continue;
        }
        if !quote && c == b'\'' {
            single = !single;
            continue;
        }
        if !quote && !single && c == b':' {
            split = Some(i);
            break;
        }
    }
    let key = trimmed.get(..split?).unwrap_or("").trim();
    let parsed: serde_yaml::Value = serde_yaml::from_str(key).ok()?;
    parsed.as_str().map(str::to_owned)
}
fn patch_jsonc(
    text: &str,
    map_key: &str,
    name: &str,
    value: Option<&Value>,
) -> Result<String, AppError> {
    if text.trim().is_empty() {
        let mut map = Map::new();
        if let Some(value) = value {
            map.insert(name.to_owned(), value.clone());
        }
        return Ok(serde_json::to_string_pretty(&json!({map_key:map}))
            .map_err(AppError::internal)?
            + "\n");
    }
    parse_jsonc(text)?;
    let root_start = text
        .find('{')
        .ok_or_else(|| AppError::invalid_input("MCP JSON root must be an object"))?;
    let root_end = matching_json(text, root_start)?;
    let root_member = find_json_member(text, root_start, root_end, map_key)?;
    let Some((_, root_value_start, _)) = root_member else {
        if let Some(value) = value {
            let server = json!({name:value});
            return insert_json_member(text, root_start, root_end, map_key, &server);
        }
        return Ok(text.to_owned());
    };
    if text.as_bytes()[root_value_start] != b'{' {
        return Err(AppError::invalid_input("MCP server map is not an object"));
    }
    let map_end = matching_json(text, root_value_start)?;
    let member = find_json_member(text, root_value_start, map_end, name)?;
    match (member, value) {
        (Some((_, start, end)), Some(value)) => {
            let mut out = text.to_owned();
            out.replace_range(
                start..end,
                &serde_json::to_string(value).map_err(AppError::internal)?,
            );
            Ok(out)
        }
        (Some((key_start, _, end)), None) => {
            let b = text.as_bytes();
            let after = skip_json_trivia(b, end)?;
            let (from, to) = if after < map_end && b[after] == b',' {
                (key_start, after + 1)
            } else {
                let mut before = key_start;
                while before > root_value_start + 1 && b[before - 1].is_ascii_whitespace() {
                    before -= 1
                }
                if before > root_value_start + 1 && b[before - 1] == b',' {
                    (before - 1, end)
                } else {
                    (key_start, end)
                }
            };
            let mut out = text.to_owned();
            out.replace_range(from..to, "");
            Ok(out)
        }
        (None, Some(value)) => insert_json_member(text, root_value_start, map_end, name, value),
        (None, None) => Ok(text.to_owned()),
    }
}
fn insert_json_member(
    text: &str,
    obj_start: usize,
    obj_end: usize,
    key: &str,
    value: &Value,
) -> Result<String, AppError> {
    let members = json_members(text, obj_start, obj_end)?;
    let pair = format!(
        "{}: {}",
        serde_json::to_string(key).map_err(AppError::internal)?,
        serde_json::to_string(value).map_err(AppError::internal)?
    );
    let insert = if !members.is_empty() {
        format!(", {pair}")
    } else {
        pair
    };
    let at = members.last().map(|m| m.2).unwrap_or(obj_start + 1);
    let mut out = text.to_owned();
    out.insert_str(at, &insert);
    Ok(out)
}
fn json_members(
    text: &str,
    obj_start: usize,
    obj_end: usize,
) -> Result<Vec<(usize, usize, usize)>, AppError> {
    let b = text.as_bytes();
    let mut i = skip_json_trivia(b, obj_start + 1)?;
    let mut out = Vec::new();
    let mut keys = std::collections::HashSet::new();
    while i < obj_end {
        if b[i] == b'}' {
            break;
        }
        if b[i] != b'"' {
            return Err(AppError::invalid_input("MCP JSON object is ambiguous"));
        }
        let key_end = json_string_end(b, i)?;
        let key: String = serde_json::from_str(&text[i..key_end]).map_err(AppError::internal)?;
        if !keys.insert(key) {
            return Err(AppError::invalid_input(
                "MCP JSON contains duplicate object keys",
            ));
        }
        let j = skip_json_trivia(b, key_end)?;
        if j >= obj_end || b[j] != b':' {
            return Err(AppError::invalid_input("MCP JSON object is malformed"));
        }
        let start = skip_json_trivia(b, j + 1)?;
        let end = json_value_end(b, start)?;
        out.push((i, start, end));
        i = skip_json_trivia(b, end)?;
        if i < obj_end && b[i] == b',' {
            i = skip_json_trivia(b, i + 1)?;
        } else if i < obj_end && b[i] != b'}' {
            return Err(AppError::invalid_input("MCP JSON object is malformed"));
        }
    }
    Ok(out)
}
fn find_json_member(
    text: &str,
    obj_start: usize,
    obj_end: usize,
    key: &str,
) -> Result<Option<(usize, usize, usize)>, AppError> {
    let members = json_members(text, obj_start, obj_end)?;
    let b = text.as_bytes();
    for (k, s, e) in members {
        let key_end = json_string_end(b, k)?;
        let parsed: String = serde_json::from_str(&text[k..key_end]).map_err(AppError::internal)?;
        if parsed == key {
            return Ok(Some((k, s, e)));
        }
    }
    Ok(None)
}
fn skip_json_trivia(b: &[u8], mut i: usize) -> Result<usize, AppError> {
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1
            }
            if i + 1 >= b.len() {
                return Err(AppError::invalid_input("Unterminated JSON comment"));
            }
            i += 2;
            continue;
        }
        return Ok(i);
    }
}
fn json_string_end(b: &[u8], start: usize) -> Result<usize, AppError> {
    let (mut i, mut esc) = (start + 1, false);
    while i < b.len() {
        let c = b[i];
        if esc {
            esc = false
        } else if c == b'\\' {
            esc = true
        } else if c == b'"' {
            return Ok(i + 1);
        }
        i += 1
    }
    Err(AppError::invalid_input("Unterminated JSON string"))
}
fn matching_json(text: &str, start: usize) -> Result<usize, AppError> {
    let b = text.as_bytes();
    let open = b[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let (mut depth, mut i) = (0usize, start);
    while i < b.len() {
        if b[i] == b'"' {
            i = json_string_end(b, i)?;
            continue;
        }
        if b[i] == b'/' && i + 1 < b.len() && (b[i + 1] == b'/' || b[i + 1] == b'*') {
            i = skip_json_trivia(b, i)?;
            continue;
        }
        if b[i] == open {
            depth += 1
        } else if b[i] == close {
            depth -= 1;
            if depth == 0 {
                return Ok(i);
            }
        }
        i += 1
    }
    Err(AppError::invalid_input("Unclosed JSON container"))
}
fn json_value_end(b: &[u8], start: usize) -> Result<usize, AppError> {
    match b.get(start) {
        Some(b'"') => json_string_end(b, start),
        Some(b'{') | Some(b'[') => {
            let text = std::str::from_utf8(b).map_err(AppError::internal)?;
            Ok(matching_json(text, start)? + 1)
        }
        Some(_) => {
            let mut i = start;
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']') {
                i += 1
            }
            while i > start && b[i - 1].is_ascii_whitespace() {
                i -= 1
            }
            Ok(i)
        }
        None => Err(AppError::invalid_input("Missing JSON value")),
    }
}

fn read_entries(cfg: &ConfigTarget) -> Result<Vec<(String, Value)>, AppError> {
    entries_from_snapshot(cfg, read_snapshot(cfg)?.as_deref())
}

fn read_snapshot(cfg: &ConfigTarget) -> Result<Option<String>, AppError> {
    resource_store::reject_symlinks(&cfg.path).map_err(AppError::io)?;
    match fs::symlink_metadata(&cfg.path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            resource_store::read_limited(&cfg.path)
                .map(Some)
                .map_err(AppError::io)
        }
        Ok(_) => Err(AppError::invalid_input(
            "MCP config must be a regular file without symlinks",
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::io(err)),
    }
}

fn parse_root(format: Format, text: Option<&str>) -> Result<Value, AppError> {
    let Some(text) = text else {
        return Ok(json!({}));
    };
    let root = match format {
        Format::Json => parse_jsonc(text)?,
        Format::Toml => toml_to_json(text)?,
        Format::Yaml => yaml_to_json(text)?,
    };
    if !root.is_object() {
        return Err(AppError::invalid_input("MCP config root must be an object"));
    }
    Ok(root)
}

fn entries_from_snapshot(
    cfg: &ConfigTarget,
    text: Option<&str>,
) -> Result<Vec<(String, Value)>, AppError> {
    let root = parse_root(cfg.format, text)?;
    let Some(node) = root.get(cfg.entry_path[0]) else {
        return Ok(vec![]);
    };
    let map = node
        .as_object()
        .ok_or_else(|| AppError::invalid_input("MCP server map must be an object"))?;
    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn normalize(agent: &str, name: &str, value: &Value) -> Result<DefinitionInput, AppError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AppError::invalid_input("MCP entry must be an object"))?;
    let allowed: &[&str] = match agent {
        "claude_code" => &["command", "args", "env", "url", "headers", "type"],
        "codex" => &[
            "command",
            "args",
            "env_vars",
            "url",
            "http_headers",
            "env_http_headers",
            "bearer_token_env_var",
            "cwd",
        ],
        "cursor" => &[
            "command", "args", "env", "url", "headers", "type", "envFile",
        ],
        "antigravity" => &["command", "args", "env", "serverUrl", "headers", "cwd"],
        "hermes" => &["command", "args", "env", "url", "headers", "transport"],
        _ => return Err(AppError::invalid_input("Unsupported MCP agent key")),
    };
    if obj.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(AppError::invalid_input(
            "MCP entry contains unsupported fields; review it manually before import",
        ));
    }
    for (key, field) in obj {
        let valid = match key.as_str() {
            "command"
            | "url"
            | "serverUrl"
            | "cwd"
            | "envFile"
            | "type"
            | "transport"
            | "bearer_token_env_var" => field.is_string(),
            "args" | "env_vars" => field
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_string)),
            "env" | "headers" | "http_headers" | "env_http_headers" => field
                .as_object()
                .is_some_and(|v| v.values().all(Value::is_string)),
            _ => false,
        };
        if !valid {
            return Err(AppError::invalid_input(format!(
                "MCP native field '{key}' has an unsupported type"
            )));
        }
    }
    let command = obj
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if command.as_deref().is_some_and(is_shell_command) {
        return Err(AppError::invalid_input(
            "Cannot import MCP command shells; review and add a direct executable manually",
        ));
    }
    let args = obj
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| AppError::invalid_input("MCP args must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    if args.iter().any(|a| {
        matches!(
            a.to_ascii_lowercase().as_str(),
            "-c" | "/c" | "-command" | "-encodedcommand"
        )
    }) {
        return Err(AppError::invalid_input(
            "Cannot import opaque shell command arguments",
        ));
    }
    let mut env = std::collections::BTreeMap::new();
    if let Some(values) = obj.get("env").and_then(Value::as_object) {
        for (k, v) in values {
            let s = v
                .as_str()
                .ok_or_else(|| AppError::invalid_input("MCP environment values must be strings"))?;
            let Some(reference) = portable_reference(agent, s) else {
                return Err(AppError::invalid_input(format!("Cannot import opaque environment value '{k}'; replace it with an environment reference first")));
            };
            env.insert(k.clone(), reference);
        }
    }
    if let Some(vars) = obj.get("env_vars").and_then(Value::as_array) {
        for variable in vars {
            let variable = variable.as_str().unwrap_or_default();
            if !valid_env_name(variable) {
                return Err(AppError::invalid_input(
                    "Codex env_vars contains an invalid variable name",
                ));
            }
            env.insert(variable.to_owned(), format!("${{env:{variable}}}"));
        }
    }
    let url = obj
        .get("url")
        .or_else(|| obj.get("serverUrl"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(url) = &url {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| AppError::invalid_input("MCP server URL is invalid"))?;
        if parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AppError::invalid_input(
                "Cannot import a URL containing user information, query values, or fragments",
            ));
        }
    }
    if args_have_credentials(&args) {
        return Err(AppError::invalid_input(
            "Cannot import command arguments that may contain credentials",
        ));
    }
    let mut headers = std::collections::BTreeMap::new();
    if let Some(values) = obj
        .get("headers")
        .or_else(|| obj.get("http_headers"))
        .and_then(Value::as_object)
    {
        for (k, v) in values {
            let s = v
                .as_str()
                .ok_or_else(|| AppError::invalid_input("MCP headers must be strings"))?;
            if !is_public_header(k) && portable_reference(agent, s).is_none() {
                return Err(AppError::invalid_input(format!("Cannot import opaque HTTP header '{k}'; replace it with an environment reference first")));
            }
            headers.insert(
                k.clone(),
                portable_reference(agent, s).unwrap_or_else(|| s.to_owned()),
            );
        }
    }
    if let Some(values) = obj.get("env_http_headers").and_then(Value::as_object) {
        for (key, variable) in values {
            let variable = variable.as_str().unwrap_or_default();
            if !valid_env_name(variable) || headers.contains_key(key) {
                return Err(AppError::invalid_input(
                    "Codex env_http_headers is invalid or conflicts with a static header",
                ));
            }
            headers.insert(key.clone(), format!("${{env:{variable}}}"));
        }
    }
    let env_file = obj
        .get("envFile")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let cwd = obj.get("cwd").and_then(Value::as_str).map(str::to_owned);
    let kind = obj
        .get("type")
        .or_else(|| obj.get("transport"))
        .and_then(Value::as_str);
    if kind.is_some_and(|kind| !matches!(kind, "stdio" | "http" | "sse")) {
        return Err(AppError::invalid_input("Unsupported MCP transport type"));
    }
    let transport = if command.is_some() {
        if kind.is_some_and(|kind| kind != "stdio") {
            return Err(AppError::invalid_input(
                "MCP transport type conflicts with command",
            ));
        }
        Transport::Stdio
    } else if kind == Some("sse") || url.as_deref().is_some_and(|u| u.ends_with("/sse")) {
        Transport::Sse
    } else {
        if kind == Some("stdio") {
            return Err(AppError::invalid_input("MCP stdio entry has no command"));
        }
        Transport::Http
    };
    if matches!(transport, Transport::Sse) && matches!(agent, "codex" | "antigravity") {
        return Err(AppError::invalid_input(
            "This agent has no verified explicit SSE MCP transport",
        ));
    }
    let bearer = obj
        .get("bearer_token_env_var")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let input = DefinitionInput {
        id: None,
        name: name.to_string(),
        transport,
        server: ServerConfig {
            command,
            args,
            env,
            url,
            headers,
            env_file,
            cwd,
        },
        auth: bearer.map(|bearer_token_env_var| AuthRefs {
            bearer_token_env_var: Some(bearer_token_env_var),
            ..Default::default()
        }),
        source: None,
    };
    store::validate_portable_input(&input)?;
    Ok(input)
}
fn is_shell_command(command: &str) -> bool {
    matches!(
        command
            .rsplit('/')
            .next()
            .unwrap_or(command)
            .to_ascii_lowercase()
            .as_str(),
        "sh" | "bash" | "zsh" | "fish" | "cmd" | "cmd.exe" | "powershell" | "pwsh"
    )
}
fn args_have_credentials(args: &[String]) -> bool {
    for (index, arg) in args.iter().enumerate() {
        let a = arg.to_ascii_lowercase();
        if [
            "token=",
            "secret=",
            "password=",
            "api_key=",
            "apikey=",
            "authorization=",
            "client_secret=",
            "access_key=",
        ]
        .iter()
        .any(|s| a.contains(s))
        {
            return true;
        }
        if [
            "--token",
            "--password",
            "--secret",
            "--api-key",
            "--apikey",
            "--authorization",
            "--client-secret",
            "--access-key",
        ]
        .contains(&a.as_str())
            && args.get(index + 1).is_some_and(|next| !reference(next))
        {
            return true;
        }
    }
    false
}
fn reference(v: &str) -> bool {
    portable_reference("claude_code", v).is_some()
}

fn parse_jsonc(text: &str) -> Result<Value, AppError> {
    let cleaned = strip_jsonc(text)?;
    let value = serde_json::from_str(&cleaned)
        .map_err(|_| AppError::invalid_input("MCP JSON config is malformed or ambiguous"))?;
    validate_json_duplicates(&cleaned)?;
    Ok(value)
}
fn validate_json_duplicates(text: &str) -> Result<(), AppError> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i = json_string_end(b, i)?;
            continue;
        }
        if b[i] == b'{' {
            let end = matching_json(text, i)?;
            let _ = json_members(text, i, end)?;
        }
        i += 1
    }
    Ok(())
}
fn strip_jsonc(s: &str) -> Result<String, AppError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let (mut i, mut quoted, mut escape) = (0, false, false);
    while i < b.len() {
        let c = b[i];
        if quoted {
            out.push(c);
            if escape {
                escape = false
            } else if c == b'\\' {
                escape = true
            } else if c == b'"' {
                quoted = false
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            quoted = true;
            out.push(b'"');
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1
            }
            if i + 1 >= b.len() {
                return Err(AppError::invalid_input("Unterminated JSON comment"));
            }
            i += 2;
            continue;
        }
        if c == b',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1
            }
            if j < b.len() && (b[j] == b'}' || b[j] == b']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1
    }
    if quoted {
        return Err(AppError::invalid_input("Unterminated JSON string"));
    }
    String::from_utf8(out).map_err(|_| AppError::invalid_input("MCP JSON config is not UTF-8"))
}
fn toml_to_json(text: &str) -> Result<Value, AppError> {
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| AppError::invalid_input("MCP TOML config is malformed or ambiguous"))?;
    toml_value(doc.as_item())
}
fn toml_value(item: &toml_edit::Item) -> Result<Value, AppError> {
    use toml_edit::Item;
    match item {
        Item::Value(v) => toml_value_inner(v),
        Item::Table(t) => {
            let mut m = Map::new();
            for (k, v) in t {
                m.insert(k.to_string(), toml_value(v)?);
            }
            Ok(Value::Object(m))
        }
        Item::ArrayOfTables(a) => Ok(Value::Array(
            a.iter()
                .map(|t| {
                    let mut m = Map::new();
                    for (k, v) in t {
                        m.insert(k.to_string(), toml_value(v)?);
                    }
                    Ok(Value::Object(m))
                })
                .collect::<Result<Vec<_>, AppError>>()?,
        )),
        Item::None => Ok(Value::Null),
    }
}
fn toml_value_inner(v: &toml_edit::Value) -> Result<Value, AppError> {
    use toml_edit::Value as T;
    Ok(match v {
        T::String(x) => Value::String(x.value().clone()),
        T::Integer(x) => json!(x.value()),
        T::Float(x) => json!(x.value()),
        T::Boolean(x) => json!(x.value()),
        T::Datetime(x) => Value::String(x.value().to_string()),
        T::Array(a) => Value::Array(a.iter().map(toml_value_inner).collect::<Result<_, _>>()?),
        T::InlineTable(t) => {
            let mut m = Map::new();
            for (k, v) in t {
                m.insert(k.to_string(), toml_value_inner(v)?);
            }
            Value::Object(m)
        }
    })
}
fn yaml_to_json(text: &str) -> Result<Value, AppError> {
    if text.contains("\n---")
        || text.trim_start().starts_with("---")
        || text.contains("&")
        || text.contains("*")
    {
        return Err(AppError::invalid_input(
            "MCP YAML aliases and multi-document files require manual review",
        ));
    }
    validate_yaml_server_keys(text)?;
    serde_yaml::from_str(text)
        .map_err(|_| AppError::invalid_input("MCP YAML config is malformed or ambiguous"))
}

fn validate_yaml_server_keys(text: &str) -> Result<(), AppError> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let headers: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| (yaml_line_key(l, 0).as_deref() == Some("mcp_servers")).then_some(i))
        .collect();
    if headers.len() > 1 {
        return Err(AppError::invalid_input(
            "MCP YAML contains duplicate mcp_servers maps",
        ));
    }
    let Some(&h) = headers.first() else {
        return Ok(());
    };
    let mut keys = std::collections::HashSet::new();
    for line in lines.iter().skip(h + 1) {
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') && !line.starts_with(' ')
        {
            break;
        }
        if let Some(key) = yaml_line_key(line, 2) {
            if !keys.insert(key) {
                return Err(AppError::invalid_input(
                    "MCP YAML contains duplicate server names",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        host::NoopEvents,
        skill_store::{ProjectRecord, SkillStore},
    };
    use std::sync::Arc;

    #[test]
    fn jsonc_patch_changes_only_selected_entry_and_keeps_comments_and_project_data() {
        let source = r#"{
  // top-level comment
  "mcpServers": {
    "alpha": {"command":"old"}, // alpha comment
    "beta": {"command":"keep"}
  },
  "projects": {"/private": {"allowed": true}},
  "sentinel": "SENTINEL_PRIVATE_VALUE"
}
"#;
        let changed = patch_jsonc(
            source,
            "mcpServers",
            "alpha",
            Some(&json!({"command":"new"})),
        )
        .unwrap();
        assert!(changed.contains("// top-level comment"));
        assert!(changed.contains("// alpha comment"));
        assert!(changed.contains("SENTINEL_PRIVATE_VALUE"));
        assert_eq!(
            parse_jsonc(&changed).unwrap()["projects"]["/private"]["allowed"],
            true
        );
        assert_eq!(
            parse_jsonc(&changed).unwrap()["mcpServers"]["beta"]["command"],
            "keep"
        );
        assert_eq!(
            parse_jsonc(&changed).unwrap()["mcpServers"]["alpha"]["command"],
            "new"
        );
    }

    #[test]
    fn jsonc_duplicate_mcp_keys_fail_closed() {
        let source = r#"{"mcpServers":{"a":{"command":"x"},"a":{"command":"y"}}}"#;
        assert!(parse_jsonc(source).is_err());
    }

    #[test]
    fn toml_patch_preserves_comments_and_unrelated_values() {
        let source = "# keep this comment\nmodel = \"gpt\"\n\n[mcp_servers]\n# neighbor comment\nother = { command = \"keep\" }\n";
        let changed = patch_toml(
            source,
            "target",
            Some(&json!({"command":"npx","args":["-y","pkg"]})),
        )
        .unwrap();
        let parsed = toml_to_json(&changed).unwrap();
        assert_eq!(parsed["model"], "gpt");
        assert_eq!(parsed["mcp_servers"]["other"]["command"], "keep");
        assert_eq!(parsed["mcp_servers"]["target"]["command"], "npx");
        assert!(changed.contains("# keep this comment"));
        assert!(changed.contains("# neighbor comment"));
    }

    #[test]
    fn yaml_patch_finds_plain_keys_and_preserves_neighbor_comment_and_server() {
        let source = "# top comment\nmcp_servers:\n  existing:\n    command: old\n  # leave with other\n  other:\n    command: keep\n";
        let changed = patch_yaml(source, "existing", Some(&json!({"command":"new"}))).unwrap();
        let parsed = yaml_to_json(&changed).unwrap();
        assert_eq!(parsed["mcp_servers"]["existing"]["command"], "new");
        assert_eq!(parsed["mcp_servers"]["other"]["command"], "keep");
        assert!(changed.contains("# top comment"));
        assert!(changed.contains("# leave with other"));
    }

    #[test]
    fn scoped_stale_check_compares_exact_entry_and_presence() {
        let initial = vec![("server".into(), json!({"command":"old"}))];
        assert!(entry_matches(
            &initial,
            "server",
            true,
            &json!({"command":"old"})
        ));
        assert!(!entry_matches(
            &initial,
            "server",
            true,
            &json!({"command":"new"})
        ));
        assert!(!entry_matches(
            &initial,
            "other",
            true,
            &json!({"command":"old"})
        ));
        assert!(entry_matches(&initial, "other", false, &Value::Null));
    }

    #[test]
    fn per_deployment_override_is_limited_to_supported_fields() {
        let entry: Definition = serde_json::from_value(json!({
            "id":"00000000-0000-4000-8000-000000000001",
            "name":"server","transport":"stdio",
            "server":{"command":"npx","args":["pkg"],"env":{"MODE":"${env:MODE}"}},
            "revision":"00000000-0000-4000-8000-000000000002","updatedAt":"2026-09-27T00:00:00Z"
        }))
        .unwrap();
        let result =
            apply_override(&entry, &json!({"server":{"args":["pkg","--region","eu"]}})).unwrap();
        assert_eq!(result.server.args, ["pkg", "--region", "eu"]);
        assert_eq!(result.server.env["MODE"], "${env:MODE}");
        assert!(apply_override(&entry, &json!({"server":{"secret":"literal"}})).is_err());
    }

    #[test]
    fn native_updates_merge_nonoverlapping_fields_and_require_a_choice_for_conflicts() {
        let base = json!({"command":"npx","args":["v1"],"env":{"MODE":"${env:MODE}"}});
        let current = json!({"command":"npx","args":["v1"],"env":{"MODE":"${env:MODE}","REGION":"${env:REGION}"}});
        let desired = json!({"command":"npx","args":["v2"],"env":{"MODE":"${env:MODE}"}});
        let merged = merge_native_entry(&base, Some(&current), Some(&desired), None)
            .unwrap()
            .unwrap();
        assert_eq!(merged["args"], json!(["v2"]));
        assert_eq!(merged["env"]["REGION"], "${env:REGION}");
        let conflicting = json!({"command":"npx","args":["local"],"env":{"MODE":"${env:MODE}"}});
        assert!(merge_native_entry(&base, Some(&conflicting), Some(&desired), None).is_err());
        assert_eq!(
            merge_native_entry(&base, Some(&conflicting), Some(&desired), Some("keep"))
                .unwrap()
                .unwrap()["args"],
            json!(["local"])
        );
        assert_eq!(
            merge_native_entry(&base, Some(&conflicting), Some(&desired), Some("replace"))
                .unwrap()
                .unwrap()["args"],
            json!(["v2"])
        );
    }

    #[test]
    fn five_agent_rendering_uses_supported_native_reference_syntax() {
        let capabilities = capabilities();
        assert!(capabilities["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["agentKey"] == "claude_code"));
        assert!(!capabilities["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["agentKey"] == "claude"));
        for agent in ["codex", "antigravity"] {
            let target = capabilities["targets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["agentKey"] == agent)
                .unwrap();
            assert!(!target["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|feature| feature == "sse"));
        }
        let stdio: Definition = serde_json::from_value(json!({
            "id":"00000000-0000-4000-8000-000000000001","name":"server","transport":"stdio",
            "server":{"command":"npx","args":["pkg"],"env":{"TOKEN":"${env:TOKEN}"}},
            "revision":"00000000-0000-4000-8000-000000000002","updatedAt":"2026-09-27T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(
            render_entry("codex", &stdio).unwrap()["env_vars"],
            json!(["TOKEN"])
        );
        assert_eq!(
            render_entry("claude_code", &stdio).unwrap()["env"]["TOKEN"],
            "${TOKEN}"
        );
        assert_eq!(render_entry("cursor", &stdio).unwrap()["type"], "stdio");
        assert_eq!(
            render_entry("hermes", &stdio).unwrap()["env"]["TOKEN"],
            "${env:TOKEN}"
        );
        assert!(render_entry("antigravity", &stdio).is_err());
        let mut remote = stdio.clone();
        remote.transport = Transport::Http;
        remote.server = serde_json::from_value(json!({
            "url":"https://example.com/mcp","headers":{"Authorization":"Bearer ${env:TOKEN}","Accept":"application/json"}
        })).unwrap();
        let codex = render_entry("codex", &remote).unwrap();
        assert_eq!(codex["bearer_token_env_var"], "TOKEN");
        assert_eq!(codex["http_headers"]["Accept"], "application/json");
        assert_eq!(
            render_entry("claude_code", &remote).unwrap()["type"],
            "http"
        );
        assert_eq!(
            render_entry("cursor", &remote).unwrap()["headers"]["Authorization"],
            "Bearer ${env:TOKEN}"
        );
        assert!(render_entry("antigravity", &remote).is_err());
        remote.server.headers.clear();
        assert_eq!(
            render_entry("antigravity", &remote).unwrap()["serverUrl"],
            "https://example.com/mcp"
        );
        remote.transport = Transport::Sse;
        assert!(render_entry("codex", &remote).is_err());
        assert!(render_entry("antigravity", &remote).is_err());
    }

    #[test]
    fn scoped_patch_checks_unrelated_semantics_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ConfigTarget {
            path: dir.path().join("mcp.json"),
            format: Format::Json,
            entry_path: vec!["mcpServers"],
            agent: "cursor".into(),
        };
        let original = "{\n  // owned by user\n  \"mcpServers\": {\"target\": {\"command\": \"old\"}},\n  \"private\": {\"secret\": \"NEVER_STORE_THIS\"}\n}\n";
        let selected = json!({"name":"target","present":true,"entry":{"command":"new"}});
        let patched = patch_text(
            cfg.format,
            original,
            "mcpServers",
            "target",
            Some(&json!({"command":"new"})),
        )
        .unwrap();
        verify_scoped_patch(
            &cfg,
            Some(original),
            &patched,
            std::slice::from_ref(&selected),
        )
        .unwrap();
        assert!(patched.contains("// owned by user"));
        assert!(patched.contains("NEVER_STORE_THIS"));
        let changed_private = patched.replace("NEVER_STORE_THIS", "CHANGED");
        assert!(verify_scoped_patch(&cfg, Some(original), &changed_private, &[selected]).is_err());
    }

    #[test]
    fn recovery_finalizes_only_matching_safe_entries_without_storing_unrelated_secrets() {
        let repo = crate::core::test_support::test_repo();
        let project = repo._tmp.path().join("project");
        fs::create_dir(&project).unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("state.db")).unwrap();
        store
            .insert_project(&ProjectRecord {
                id: "project-1".into(),
                name: "project".into(),
                path: project.to_string_lossy().into(),
                workspace_type: "project".into(),
                linked_agent_key: None,
                linked_agent_name: None,
                disabled_path: None,
                sort_order: 0,
                created_at: 0,
                updated_at: 0,
                agent_keys: None,
                deploy_mode: "link".into(),
            })
            .unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = Target {
            agent_key: "cursor".into(),
            project_id: Some("project-1".into()),
        };
        let cfg = resolve(&ctx, &target).unwrap();
        resource_store::atomic_write(&cfg.path, b"{\"mcpServers\":{\"safe\":{\"command\":\"npx\",\"type\":\"stdio\"}},\"private\":{\"token\":\"NEVER_STORE_THIS\"}}\n").unwrap();
        let recovery_id = uuid::Uuid::new_v4().to_string();
        let preview_id = uuid::Uuid::new_v4().to_string();
        let state_id = uuid::Uuid::new_v4().to_string();
        let entry = json!({"command":"npx","type":"stdio"});
        let state = json!({"id":state_id,"target":target,"definitionId":"def-1","serverName":"safe","revision":"rev-1","overrideConfig":null,"expectedEntry":entry});
        let journal = json!({"id":recovery_id,"previewId":preview_id,"target":target,"configPath":cfg.path.to_string_lossy(),
            "before":[{"name":"safe","present":false,"entry":null}],
            "after":[{"name":"safe","present":true,"entry":entry}],
            "puts":[state],"deletes":[]});
        ctx.store
            .resource_state_put("mcps_recovery", &recovery_id, &journal)
            .unwrap();
        assert!(!journal.to_string().contains("NEVER_STORE_THIS"));
        let result = recover(&ctx, &recovery_id).unwrap();
        assert_eq!(result["fileApplied"], true);
        assert_eq!(
            ctx.store
                .resource_state_list("mcps_deployment")
                .unwrap()
                .len(),
            1
        );
        assert!(ctx
            .store
            .resource_state_get("mcps_recovery", &recovery_id)
            .unwrap()
            .is_none());
        assert!(fs::read_to_string(&cfg.path)
            .unwrap()
            .contains("NEVER_STORE_THIS"));
    }

    #[test]
    fn project_preview_apply_and_import_keep_unrelated_credentials_out_of_state_and_ipc() {
        let repo = crate::core::test_support::test_repo();
        crate::core::central_repo::set_test_base_dir_override(Some(
            repo._tmp.path().canonicalize().unwrap().join("repo"),
        ));
        let project = repo._tmp.path().join("project");
        fs::create_dir(&project).unwrap();
        let store = SkillStore::new(&repo._tmp.path().join("mcps.db")).unwrap();
        store
            .insert_project(&ProjectRecord {
                id: "project-1".into(),
                name: "project".into(),
                path: project.to_string_lossy().into(),
                workspace_type: "project".into(),
                linked_agent_key: None,
                linked_agent_name: None,
                disabled_path: None,
                sort_order: 0,
                created_at: 0,
                updated_at: 0,
                agent_keys: None,
                deploy_mode: "link".into(),
            })
            .unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let target = Target {
            agent_key: "cursor".into(),
            project_id: Some("project-1".into()),
        };
        let cfg = resolve(&ctx, &target).unwrap();
        let native = "{\n  // user comment\n  \"mcpServers\": {\"existing\": {\"type\":\"stdio\",\"command\":\"node\",\"args\":[\"server.js\"]}},\n  \"private\": {\"token\": \"NEVER_STORE_THIS\"}\n}\n";
        resource_store::atomic_write(&cfg.path, native.as_bytes()).unwrap();
        let def = store::save(&ctx, serde_json::from_value(json!({
            "name":"safe","transport":"stdio","server":{"command":"npx","args":["-y","safe-server"]}
        })).unwrap(), None).unwrap();
        let planned = preview(
            &ctx,
            &target,
            &[Operation {
                kind: "deploy".into(),
                definition_id: def.id.clone(),
                server_name: None,
                conflict: None,
                new_name: None,
                override_config: None,
            }],
            false,
        )
        .unwrap();
        assert_eq!(planned["changes"][0]["afterSummary"]["command"], "npx");
        assert!(!planned.to_string().contains("NEVER_STORE_THIS"));
        let preview_id = planned["previewId"].as_str().unwrap();
        let applied = apply(&ctx, preview_id).unwrap();
        assert_eq!(applied["applied"], true);
        let final_native = fs::read_to_string(&cfg.path).unwrap();
        assert!(final_native.contains("NEVER_STORE_THIS"));
        assert!(final_native.contains("// user comment"));
        assert_eq!(
            parse_jsonc(&final_native).unwrap()["mcpServers"]["safe"]["command"],
            "npx"
        );
        let state = ctx.store.resource_state_list("mcps_deployment").unwrap();
        assert_eq!(state.len(), 1);
        assert!(!serde_json::to_string(&state)
            .unwrap()
            .contains("NEVER_STORE_THIS"));
        let imported = import(&ctx, &target, "existing", None, None).unwrap();
        assert_eq!(imported["draft"]["server"]["command"], "node");
        assert!(!imported.to_string().contains("NEVER_STORE_THIS"));
        assert_eq!(store::list(&ctx).unwrap().len(), 1);
        store::save(
            &ctx,
            serde_json::from_value(json!({
                "name":"existing","transport":"stdio","server":{"command":"other"}
            }))
            .unwrap(),
            None,
        )
        .unwrap();
        let inspected = inspect(&ctx, &target).unwrap();
        let inspected_entries = inspected["entries"].as_array().unwrap();
        let unrelated = inspected_entries
            .iter()
            .find(|entry| entry["name"] == "existing")
            .unwrap();
        assert_eq!(unrelated["status"], "unmanaged");
        assert_eq!(unrelated["managedId"], Value::Null);
        let managed = inspected_entries
            .iter()
            .find(|entry| entry["name"] == "safe")
            .unwrap();
        assert_eq!(managed["status"], "managed");
        assert_eq!(managed["managedId"], def.id);
        let faulted = store::save(&ctx, serde_json::from_value(json!({
            "name":"faulted","transport":"stdio","server":{"command":"npx","args":["fault-package"]}
        })).unwrap(), None).unwrap();
        let fault_preview = preview(
            &ctx,
            &target,
            &[Operation {
                kind: "deploy".into(),
                definition_id: faulted.id,
                server_name: None,
                conflict: None,
                new_name: None,
                override_config: None,
            }],
            false,
        )
        .unwrap();
        let connection = rusqlite::Connection::open(repo._tmp.path().join("mcps.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_mcp_state BEFORE INSERT ON resource_state WHEN NEW.kind = 'mcps_deployment' BEGIN SELECT RAISE(FAIL, 'simulated state failure'); END;").unwrap();
        let partial = apply(&ctx, fault_preview["previewId"].as_str().unwrap()).unwrap();
        assert_eq!(partial["partial"], true);
        assert_eq!(
            parse_jsonc(&fs::read_to_string(&cfg.path).unwrap()).unwrap()["mcpServers"]["faulted"]
                ["command"],
            "npx"
        );
        let journal_id = partial["recoveryId"].as_str().unwrap();
        let journal = ctx
            .store
            .resource_state_get("mcps_recovery", journal_id)
            .unwrap()
            .unwrap();
        assert!(!journal.to_string().contains("NEVER_STORE_THIS"));
        connection
            .execute_batch("DROP TRIGGER fail_mcp_state;")
            .unwrap();
        assert_eq!(recover(&ctx, journal_id).unwrap()["fileApplied"], true);
        assert_eq!(
            ctx.store
                .resource_state_list("mcps_deployment")
                .unwrap()
                .len(),
            2
        );
        let with_secret = patch_jsonc(
            &fs::read_to_string(&cfg.path).unwrap(),
            "mcpServers",
            "secret-collision",
            Some(&json!({
                "command":"node","env":{"API_KEY":"SECRET_NATIVE_VALUE"}
            })),
        )
        .unwrap();
        resource_store::atomic_write(&cfg.path, with_secret.as_bytes()).unwrap();
        let collision = store::save(&ctx, serde_json::from_value(json!({
            "name":"secret-collision","transport":"stdio","server":{"command":"npx","args":["safe-package"]}
        })).unwrap(), None).unwrap();
        let renamed = preview(
            &ctx,
            &target,
            &[Operation {
                kind: "deploy".into(),
                definition_id: collision.id.clone(),
                server_name: None,
                conflict: Some("rename".into()),
                new_name: Some("safe-renamed".into()),
                override_config: None,
            }],
            false,
        )
        .unwrap();
        assert_eq!(renamed["changes"][0]["beforeSummary"], Value::Null);
        assert!(!renamed.to_string().contains("SECRET_NATIVE_VALUE"));
        assert!(
            !serde_json::to_string(&ctx.store.resource_state_list("mcps_preview").unwrap())
                .unwrap()
                .contains("SECRET_NATIVE_VALUE")
        );
        assert_eq!(
            apply(&ctx, renamed["previewId"].as_str().unwrap()).unwrap()["applied"],
            true
        );
        let inspected = inspect(&ctx, &target).unwrap();
        let entries = inspected["entries"].as_array().unwrap();
        let renamed_entry = entries
            .iter()
            .find(|entry| entry["name"] == "safe-renamed")
            .unwrap();
        assert_eq!(renamed_entry["managedId"], collision.id);
        assert_eq!(renamed_entry["status"], "managed");
        let opaque = entries
            .iter()
            .find(|entry| entry["name"] == "secret-collision")
            .unwrap();
        assert_eq!(opaque["managedId"], Value::Null);
        assert!(!inspected.to_string().contains("SECRET_NATIVE_VALUE"));
        assert!(store::remove(&ctx, &def.id, false).is_err());
        let before_detach = fs::read_to_string(&cfg.path).unwrap();
        store::remove(&ctx, &def.id, true).unwrap();
        assert_eq!(fs::read_to_string(&cfg.path).unwrap(), before_detach);
        assert!(ctx
            .store
            .resource_state_list("mcps_deployment")
            .unwrap()
            .iter()
            .all(|link| link["definitionId"] != def.id));
    }
}
