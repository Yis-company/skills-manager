use super::model::{Definition, DefinitionInput, Transport};
use crate::core::{error::AppError, host::HostCtx, repo_lock::RepoLock, resource_store};
use std::{fs, path::PathBuf};

fn directory() -> PathBuf {
    resource_store::root().join("mcps")
}
fn path(id: &str) -> Result<PathBuf, AppError> {
    resource_store::validate_id(id).map_err(|e| AppError::invalid_input(e.to_string()))?;
    let p = directory().join(format!("{id}.json"));
    Ok(p)
}

pub fn list(_ctx: &HostCtx) -> Result<Vec<Definition>, AppError> {
    resource_store::check_schema().map_err(AppError::io)?;
    let dir = directory();
    resource_store::reject_symlinks(&dir).map_err(AppError::io)?;
    let entries = match fs::read_dir(&dir) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(AppError::io(e)),
    };
    let mut values = Vec::new();
    for entry in entries {
        let entry = entry.map_err(AppError::io)?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(AppError::io)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let data = resource_store::read_limited(&entry.path()).map_err(AppError::io)?;
        let item: Definition = serde_json::from_str(&data)
            .map_err(|e| AppError::internal(format!("Invalid MCP library definition: {e}")))?;
        validate_id_matches(&item, &entry.path())?;
        values.push(item);
    }
    values.sort_by_key(|a| a.name.to_lowercase());
    Ok(values)
}

pub fn get(ctx: &HostCtx, id: &str) -> Result<Definition, AppError> {
    list(ctx)?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| AppError::not_found("MCP definition not found"))
}

pub fn save(
    ctx: &HostCtx,
    input: DefinitionInput,
    expected: Option<&str>,
) -> Result<Definition, AppError> {
    validate(&input)?;
    let _lock = RepoLock::acquire_foreground("save MCP definition").map_err(AppError::db)?;
    if !ctx
        .store
        .resource_state_list("mcps_recovery")
        .map_err(AppError::db)?
        .is_empty()
    {
        return Err(AppError::invalid_input(
            "Recover the pending MCP deployment before editing library definitions",
        ));
    }
    resource_store::ensure_root().map_err(AppError::io)?;
    let id = match input.id {
        Some(id) => {
            resource_store::validate_id(&id).map_err(|e| AppError::invalid_input(e.to_string()))?;
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    if list(ctx)?
        .iter()
        .any(|d| d.name == input.name.trim() && d.id != id)
    {
        return Err(AppError::invalid_input(
            "An MCP definition with this name already exists",
        ));
    }
    let path = path(&id)?;
    let prior = if path.exists() {
        let raw = fs::read_to_string(&path).map_err(AppError::io)?;
        Some(
            serde_json::from_str::<Definition>(&raw)
                .map_err(|_| AppError::invalid_input("Existing MCP definition is invalid"))?,
        )
    } else {
        None
    };
    match (&prior, expected) {
        (Some(old), Some(rev)) if old.revision == rev => {}
        (Some(_), _) => {
            return Err(AppError::invalid_input(
                "MCP definition changed since it was loaded; reload before saving",
            ))
        }
        (None, Some(_)) => return Err(AppError::not_found("MCP definition no longer exists")),
        (None, None) => {}
    }
    let def = Definition {
        id,
        name: input.name.trim().to_string(),
        transport: input.transport,
        server: input.server,
        auth: input.auth,
        source: input.source,
        revision: uuid::Uuid::new_v4().to_string(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    write_json(&path, &def)?;
    Ok(def)
}

pub fn remove(ctx: &HostCtx, id: &str, detach: bool) -> Result<(), AppError> {
    resource_store::check_schema().map_err(AppError::io)?;
    let _lock = RepoLock::acquire_foreground("remove MCP definition").map_err(AppError::db)?;
    if !ctx
        .store
        .resource_state_list("mcps_recovery")
        .map_err(AppError::db)?
        .is_empty()
    {
        return Err(AppError::invalid_input(
            "Recover the pending MCP deployment before removing library definitions",
        ));
    }
    let p = path(id)?;
    let meta = fs::symlink_metadata(&p).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::not_found("MCP definition not found")
        } else {
            AppError::io(e)
        }
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(AppError::invalid_input(
            "Refusing to remove a non-regular MCP definition file",
        ));
    }
    let linked: Vec<_> = ctx
        .store
        .resource_state_list("mcps_deployment")
        .map_err(AppError::db)?
        .into_iter()
        .filter(|state| {
            state
                .get("definitionId")
                .and_then(serde_json::Value::as_str)
                == Some(id)
        })
        .collect();
    if !linked.is_empty() && !detach {
        return Err(AppError::invalid_input(
            "MCP definition is deployed; choose detach to keep native entries and remove its links",
        ));
    }
    for state in &linked {
        let state_id = state
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::invalid_input("MCP deployment state is invalid"))?;
        if let Err(error) = ctx.store.resource_state_delete("mcps_deployment", state_id) {
            for previous in &linked {
                if let Some(id) = previous.get("id").and_then(serde_json::Value::as_str) {
                    let _ = ctx
                        .store
                        .resource_state_put("mcps_deployment", id, previous);
                }
            }
            return Err(AppError::db(error));
        }
    }
    if let Err(error) = fs::remove_file(p) {
        for previous in &linked {
            if let Some(id) = previous.get("id").and_then(serde_json::Value::as_str) {
                let _ = ctx
                    .store
                    .resource_state_put("mcps_deployment", id, previous);
            }
        }
        return Err(AppError::io(error));
    }
    Ok(())
}

fn validate(input: &DefinitionInput) -> Result<(), AppError> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') || name.contains('\\') {
        return Err(AppError::invalid_input(
            "MCP name must be 1-128 characters and cannot contain path separators",
        ));
    }
    match input.transport {
        Transport::Stdio
            if input
                .server
                .command
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                || input.server.url.is_some() =>
        {
            return Err(AppError::invalid_input(
                "stdio MCP requires command and cannot have URL",
            ))
        }
        Transport::Http | Transport::Sse
            if input.server.url.as_deref().unwrap_or("").trim().is_empty()
                || input.server.command.is_some() =>
        {
            return Err(AppError::invalid_input(
                "HTTP/SSE MCP requires URL and cannot have command",
            ))
        }
        _ => {}
    }
    if let Some(url) = &input.server.url {
        let parsed =
            reqwest::Url::parse(url).map_err(|_| AppError::invalid_input("MCP URL is invalid"))?;
        let query = parsed.query().unwrap_or("").to_ascii_lowercase();
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || [
                "token",
                "secret",
                "password",
                "credential",
                "api_key",
                "apikey",
                "authorization",
                "signature",
                "sig=",
            ]
            .iter()
            .any(|s| query.contains(s))
        {
            return Err(AppError::invalid_input(
                "MCP URL must be HTTP(S) and cannot contain credentials",
            ));
        }
    }
    if input.server.args.iter().enumerate().any(|(i, arg)| {
        let a = arg.to_ascii_lowercase();
        arg.contains("__REQUIRED_ARGUMENT_")
            || [
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
            || [
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
                && input
                    .server
                    .args
                    .get(i + 1)
                    .is_some_and(|next| !is_reference(next))
    }) {
        return Err(AppError::invalid_input(
            "MCP arguments contain a required placeholder or credential-like value",
        ));
    }
    for (key, value) in input.server.env.iter().chain(input.server.headers.iter()) {
        if is_sensitive_key(key) && !is_reference(value) {
            return Err(AppError::invalid_input(format!(
                "Credential field '{key}' must use an environment reference"
            )));
        }
    }
    if input
        .server
        .env_file
        .as_deref()
        .is_some_and(|v| v.trim().is_empty() || v.contains('\0'))
    {
        return Err(AppError::invalid_input("envFile must be a non-empty path"));
    }
    if input
        .server
        .cwd
        .as_deref()
        .is_some_and(|v| v.trim().is_empty() || v.contains('\0'))
    {
        return Err(AppError::invalid_input("cwd must be a non-empty path"));
    }
    if let Some(auth) = &input.auth {
        if auth
            .bearer_token_env_var
            .as_deref()
            .is_some_and(|s| !valid_env_name(s))
        {
            return Err(AppError::invalid_input(
                "bearerTokenEnvVar must be an environment variable name",
            ));
        }
        if auth.credential_refs.values().any(|v| !is_reference(v)) {
            return Err(AppError::invalid_input(
                "Credential references must use ${env:NAME}",
            ));
        }
    }
    if let Some(source) = &input.source {
        if source.registry != "official"
            || source.server_id.is_empty()
            || source.server_id.len() > 256
            || source.version.is_empty()
            || source.version.len() > 128
            || source.version == "latest"
            || source.server_id.chars().any(char::is_control)
            || source.version.chars().any(char::is_control)
        {
            return Err(AppError::invalid_input(
                "MCP catalog source requires a concrete official version",
            ));
        }
    }
    Ok(())
}
pub(super) fn validate_portable_input(input: &DefinitionInput) -> Result<(), AppError> {
    validate(input)
}
fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "api_key",
        "apikey",
        "authorization",
    ]
    .iter()
    .any(|x| k.contains(x))
}
fn is_reference(v: &str) -> bool {
    let v = v.strip_prefix("Bearer ").unwrap_or(v);
    v.strip_prefix("${env:")
        .and_then(|x| x.strip_suffix('}'))
        .is_some_and(valid_env_name)
}
fn valid_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.as_bytes()[0].is_ascii_digit()
}
fn validate_id_matches(def: &Definition, file: &std::path::Path) -> Result<(), AppError> {
    let expected = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if def.id != expected || uuid::Uuid::parse_str(&def.id).is_err() {
        return Err(AppError::invalid_input(
            "MCP definition id does not match its filename",
        ));
    }
    let input = DefinitionInput {
        id: Some(def.id.clone()),
        name: def.name.clone(),
        transport: def.transport.clone(),
        server: def.server.clone(),
        auth: def.auth.clone(),
        source: def.source.clone(),
    };
    validate(&input)
}
fn write_json(path: &std::path::Path, value: &impl serde::Serialize) -> Result<(), AppError> {
    let data = serde_json::to_vec_pretty(value).map_err(|e| AppError::internal(e.to_string()))?;
    resource_store::atomic_write(path, &data).map_err(AppError::io)
}
