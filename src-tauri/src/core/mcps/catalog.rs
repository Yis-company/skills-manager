use crate::core::{error::AppError, host::HostCtx};
use serde_json::{json, Value};
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BASE: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";
const CACHE_TTL_SECS: i64 = 30 * 60;

pub fn search(
    ctx: &HostCtx,
    query: Option<&str>,
    cursor: Option<&str>,
    server_id: Option<&str>,
    dry_run: bool,
) -> Result<Value, AppError> {
    if query.is_some_and(|s| s.len() > 256) || cursor.is_some_and(|s| s.len() > 2048) {
        return Err(AppError::invalid_input(
            "Catalog search or cursor is too long",
        ));
    }
    if let Some(id) = server_id {
        return detail(ctx, id, dry_run);
    }
    let query = query.map(str::trim).filter(|s| !s.is_empty());
    // The default first page is safe to cache locally. Search terms and cursors
    // stay request-local rather than becoming durable user history.
    if !dry_run && query.is_none() && cursor.is_none() {
        if let Some(cached) = cache_get(ctx, "latest-list")? {
            return Ok(cached);
        }
    }
    let mut url = reqwest::Url::parse(BASE).map_err(AppError::internal)?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("version", "latest");
        if let Some(q) = query {
            qp.append_pair("search", q);
        }
        if let Some(c) = cursor.filter(|s| !s.is_empty()) {
            qp.append_pair("cursor", c);
        }
    }
    let raw = get_json(url)?;
    let servers = raw
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::network("MCP registry response did not include servers"))?;
    let normalized: Vec<Value> = servers.iter().filter_map(|entry| {
        let server = entry.get("server").unwrap_or(entry);
        let name = server.get("name")?.as_str()?;
        Some(json!({
            "id": name,
            "name": name,
            "title": server.get("title").and_then(Value::as_str),
            "description": server.get("description").and_then(Value::as_str),
            "repository": server.get("repository").and_then(|r|r.get("url")).and_then(Value::as_str),
            "version": server.get("version").and_then(Value::as_str),
            "packages": safe_packages(server.get("packages")),
            "remotes": safe_remotes(server.get("remotes"))
        }))
    }).collect();
    let result = json!({"servers":normalized,"nextCursor":raw.get("metadata").and_then(|m|m.get("nextCursor")).and_then(Value::as_str)});
    if !dry_run && query.is_none() && cursor.is_none() {
        cache_put(ctx, "latest-list", &result)?;
    }
    Ok(result)
}

fn detail(ctx: &HostCtx, id: &str, dry_run: bool) -> Result<Value, AppError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(AppError::invalid_input("MCP registry server id is invalid"));
    }
    let cache_id = format!("detail:{id}");
    if !dry_run {
        if let Some(cached) = cache_get(ctx, &cache_id)? {
            return Ok(cached);
        }
    }
    let mut url = reqwest::Url::parse(BASE).map_err(AppError::internal)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| AppError::internal("Invalid MCP registry URL"))?;
        segments
            .pop_if_empty()
            .push(id)
            .push("versions")
            .push("latest");
    }
    let raw = get_json(url)?;
    let server = raw.get("server").cloned().unwrap_or(raw.clone());
    let drafts = map_drafts(&server);
    let draft = drafts.first().map(|choice| &choice["definition"]);
    let result = json!({"server":safe_server(&server),"draft":draft,"drafts":drafts,
        "manualSetup":"Other package formats, custom runtimes and unresolved URL templates need manual configuration."});
    if !dry_run {
        cache_put(ctx, &cache_id, &result)?;
    }
    Ok(result)
}

fn get_json(url: reqwest::Url) -> Result<Value, AppError> {
    static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST_REQUEST
            .lock()
            .map_err(|_| AppError::internal("Catalog request lock failed"))?;
        if last.is_some_and(|time| time.elapsed() < Duration::from_millis(500)) {
            return Err(AppError::invalid_input(
                "Wait a moment before another catalog request",
            ));
        }
        *last = Some(Instant::now());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("SkillsManager/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(AppError::network)?;
    let response = client.get(url).send().map_err(AppError::network)?;
    if !response.status().is_success() {
        return Err(AppError::network(format!(
            "MCP registry returned HTTP {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(AppError::network)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(AppError::network("MCP registry response is too large"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::network("MCP registry returned invalid JSON"))
}

fn safe_server(server: &Value) -> Value {
    json!({"name":server.get("name"),"title":server.get("title"),"description":server.get("description"),"version":server.get("version"),"repository":server.get("repository").and_then(|r|r.get("url")),"packages":safe_packages(server.get("packages")),"remotes":safe_remotes(server.get("remotes"))})
}
fn safe_packages(value: Option<&Value>) -> Value {
    let Some(items) = value.and_then(Value::as_array) else {
        return Value::Array(vec![]);
    };
    Value::Array(items.iter().map(|p| {
        let vars = p.get("environmentVariables").or_else(||p.get("environment_variables")).and_then(Value::as_array).map(|a|a.iter().filter_map(|v|v.as_str().or_else(||v.get("name").and_then(Value::as_str)).map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
        json!({"registryType":p.get("registryType"),"identifier":p.get("identifier"),"version":p.get("version"),"environmentVariables":vars})
    }).collect())
}
fn safe_remotes(value: Option<&Value>) -> Value {
    let Some(items) = value.and_then(Value::as_array) else {
        return Value::Array(vec![]);
    };
    Value::Array(
        items
            .iter()
            .filter_map(|r| {
                let url = r.get("url").and_then(Value::as_str)?;
                if !safe_url(url) {
                    return None;
                }
                let headers = r
                    .get("headers")
                    .and_then(Value::as_array)
                    .map(|h| {
                        h.iter()
                            .filter_map(|v| {
                                v.get("name")
                                    .and_then(Value::as_str)
                                    .map(|n| json!({"name":n}))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some(json!({"type":r.get("type"),"url":url,"headers":headers}))
            })
            .collect(),
    )
}
fn safe_url(raw: &str) -> bool {
    reqwest::Url::parse(raw).ok().is_some_and(|u| {
        let q = u.query().unwrap_or("").to_ascii_lowercase();
        matches!(u.scheme(), "http" | "https")
            && u.username().is_empty()
            && u.password().is_none()
            && !["token", "secret", "api_key", "apikey"]
                .iter()
                .any(|k| q.contains(k))
    })
}

fn env_name(name: &str) -> String {
    let mut normalized = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if normalized.as_bytes().first().is_none_or(u8::is_ascii_digit) {
        normalized.insert_str(0, "MCP_");
    }
    normalized
}

/// Metadata describes choices, not a command to execute. Keep unsupported
/// configurations visible in the detail response instead of guessing a runtime.
fn map_drafts(server: &Value) -> Vec<Value> {
    let Some(name) = server.get("name").and_then(Value::as_str) else {
        return vec![];
    };
    let Some(version) = server
        .get("version")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return vec![];
    };
    let safe_name = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .replace(['@', ':'], "-");
    let source = json!({"registry":"official","serverId":name,"version":version});
    let mut choices = Vec::new();
    for remote in server
        .get("remotes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(url) = remote
            .get("url")
            .and_then(Value::as_str)
            .filter(|s| safe_url(s) && !s.contains(['{', '}']))
        else {
            continue;
        };
        let transport = match remote.get("type").and_then(Value::as_str) {
            Some("sse") => "sse",
            Some("streamable-http") => "http",
            _ => continue,
        };
        let mut requirements = Vec::new();
        let mut headers = std::collections::BTreeMap::new();
        for header in remote
            .get("headers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = header.get("name").and_then(Value::as_str) else {
                continue;
            };
            let variable = env_name(name);
            headers.insert(name.to_owned(), format!("${{env:{variable}}}"));
            requirements.push(format!(
                "Set {variable} on the target host for header {name}."
            ));
        }
        choices.push(json!({"label":format!("{transport}: {url}"),"requirements":requirements,"definition":{
            "name":safe_name,"transport":transport,"server":{"url":url,"headers":headers},"source":source
        }}));
    }
    for package in server
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(choice) = package_draft(package, &safe_name, &source) {
            choices.push(choice);
        }
    }
    choices
}

fn package_draft(package: &Value, name: &str, source: &Value) -> Option<Value> {
    let registry = package.get("registryType")?.as_str()?;
    let command = match registry {
        "npm" => "npx",
        "pypi" => "uvx",
        _ => return None,
    };
    let registry_url = if registry == "npm" {
        "https://registry.npmjs.org"
    } else {
        "https://pypi.org"
    };
    if package
        .get("registryBaseUrl")
        .and_then(Value::as_str)
        .is_some_and(|url| url.trim_end_matches('/') != registry_url)
    {
        return None;
    }
    if package.pointer("/transport/type")?.as_str()? != "stdio"
        || package
            .get("runtimeHint")
            .and_then(Value::as_str)
            .is_some_and(|hint| hint != command)
        || package
            .get("runtimeArguments")
            .and_then(Value::as_array)
            .is_some_and(|args| !args.is_empty())
    {
        return None;
    }
    let identifier = package.get("identifier")?.as_str()?;
    let version = package.get("version")?.as_str()?;
    if identifier.is_empty()
        || identifier.starts_with('-')
        || !identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/._-".contains(c))
        || version.is_empty()
        || version == "latest"
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-_".contains(c))
        || !version.chars().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    if registry == "npm" {
        // Bare owner/repo names are Git shorthand to npx, not npm packages.
        if identifier.contains('/')
            && (!identifier.starts_with('@') || identifier.matches('/').count() != 1)
            || identifier.contains('@') && !identifier.starts_with('@')
        {
            return None;
        }
    } else if identifier.contains(['/', '@']) {
        return None;
    }
    let mut args = if registry == "npm" {
        vec!["-y".to_owned(), format!("{identifier}@{version}")]
    } else {
        vec![
            "--from".to_owned(),
            format!("{identifier}=={version}"),
            identifier.to_owned(),
        ]
    };
    let mut requirements = Vec::new();
    if registry == "pypi" {
        requirements.push("Verify that the package exposes this command; change Arguments if its executable name differs.".to_owned());
    }
    for (index, arg) in package
        .get("packageArguments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if arg.get("isSecret").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        let value = arg
            .get("value")
            .or_else(|| arg.get("default"))
            .and_then(Value::as_str)
            .filter(|value| !value.contains(['{', '}']) && !value.chars().any(char::is_control));
        if value.is_none() && arg.get("isRequired").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let value = value.map(str::to_owned).unwrap_or_else(|| {
            let placeholder = format!("__REQUIRED_ARGUMENT_{index}__");
            requirements.push(format!(
                "Replace {placeholder} with {} before saving.",
                arg.get("valueHint")
                    .and_then(Value::as_str)
                    .unwrap_or("the required argument")
            ));
            placeholder
        });
        match arg.get("type").and_then(Value::as_str)? {
            "named" => {
                let flag = arg.get("name")?.as_str()?;
                if !flag.starts_with('-') || flag.chars().any(char::is_whitespace) {
                    return None;
                }
                args.push(format!("{flag}={value}"));
            }
            "positional" => args.push(value),
            _ => return None,
        }
    }
    let mut env = std::collections::BTreeMap::new();
    for variable in package
        .get("environmentVariables")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = variable.get("name")?.as_str()?;
        if env_name(key) != key.to_ascii_uppercase()
            || key.as_bytes().first().is_none_or(u8::is_ascii_digit)
        {
            return None;
        }
        env.insert(key.to_owned(), format!("${{env:{key}}}"));
        requirements.push(format!("Set {key} on the target host."));
    }
    Some(
        json!({"label":format!("{registry}: {identifier}@{version}"),"requirements":requirements,"definition":{
            "name":name,"transport":"stdio","server":{"command":command,"args":args,"env":env},"source":source
        }}),
    )
}

fn cache_get(ctx: &HostCtx, id: &str) -> Result<Option<Value>, AppError> {
    let Some(root) = ctx
        .store
        .resource_state_get("mcps_catalog_cache", "registry")
        .map_err(AppError::db)?
    else {
        return Ok(None);
    };
    let Some(v) = root.get("entries").and_then(|e| e.get(id)) else {
        return Ok(None);
    };
    let age =
        chrono::Utc::now().timestamp() - v.get("fetchedAt").and_then(Value::as_i64).unwrap_or(0);
    if !(0..=CACHE_TTL_SECS).contains(&age) {
        return Ok(None);
    }
    Ok(v.get("data").cloned())
}
fn cache_put(ctx: &HostCtx, id: &str, data: &Value) -> Result<(), AppError> {
    let mut entries = ctx
        .store
        .resource_state_get("mcps_catalog_cache", "registry")
        .map_err(AppError::db)?
        .and_then(|v| v.get("entries").cloned())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    entries.retain(|_, v| {
        now - v.get("fetchedAt").and_then(Value::as_i64).unwrap_or(0) <= CACHE_TTL_SECS
    });
    entries.insert(id.to_owned(), json!({"fetchedAt":now,"data":data}));
    while entries.len() > 30 {
        let oldest = entries
            .iter()
            .min_by_key(|(_, v)| v.get("fetchedAt").and_then(Value::as_i64).unwrap_or(0))
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            entries.remove(&k);
        } else {
            break;
        }
    }
    ctx.store
        .resource_state_put(
            "mcps_catalog_cache",
            "registry",
            &json!({"entries":entries}),
        )
        .map_err(AppError::db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_npm_metadata_maps_to_stdio_draft_with_env_refs() {
        let server = json!({"name":"io.example/weather","version":"2.0.0","packages":[{"registryType":"npm","identifier":"@example/weather-mcp","version":"1.2.0","transport":{"type":"stdio"},"environmentVariables":[{"name":"WEATHER_KEY","isRequired":true}]}]});
        let drafts = map_drafts(&server);
        let draft = &drafts[0]["definition"];
        assert_eq!(draft["transport"], "stdio");
        assert_eq!(draft["server"]["command"], "npx");
        assert_eq!(
            draft["server"]["args"],
            json!(["-y", "@example/weather-mcp@1.2.0"])
        );
        assert_eq!(draft["server"]["env"]["WEATHER_KEY"], "${env:WEATHER_KEY}");
    }

    #[test]
    fn registry_remote_maps_without_exposing_literal_header_values() {
        let server = json!({"name":"io.example/remote","version":"1.0.0","remotes":[{"type":"streamable-http","url":"https://example.test/mcp","headers":[{"name":"Authorization","value":"DO_NOT_EXPOSE"}]}]});
        let drafts = map_drafts(&server);
        let draft = &drafts[0]["definition"];
        assert_eq!(draft["transport"], "http");
        assert_eq!(
            draft["server"]["headers"]["Authorization"],
            "${env:AUTHORIZATION}"
        );
        assert!(!safe_server(&server).to_string().contains("DO_NOT_EXPOSE"));
    }

    #[test]
    fn unsafe_remote_urls_are_not_mapped_or_returned() {
        let server = json!({"name":"io.example/unsafe","remotes":[{"url":"https://user:secret@example.test/mcp?token=secret","headers":[]}]});
        assert!(map_drafts(&server).is_empty());
        assert_eq!(safe_remotes(server.get("remotes")), json!([]));
    }

    #[test]
    fn catalog_keeps_transport_choices_and_pins_python_version_before_the_command() {
        let server = json!({"name":"io.example/weather","version":"3.0.0",
            "remotes":[{"type":"streamable-http","url":"https://example.test/mcp"}],
            "packages":[{"registryType":"pypi","identifier":"weather-mcp","version":"1.2.3","transport":{"type":"stdio"}}]});
        let choices = map_drafts(&server);
        assert_eq!(choices.len(), 2);
        assert_eq!(
            choices[1]["definition"]["server"]["args"],
            json!(["--from", "weather-mcp==1.2.3", "weather-mcp"])
        );
        assert_eq!(choices[1]["definition"]["source"]["version"], "3.0.0");
    }

    #[test]
    fn catalog_requires_missing_inputs_and_does_not_guess_custom_runtime_commands() {
        let mut package = json!({"registryType":"npm","identifier":"example","version":"1.0.0","transport":{"type":"stdio"},
            "packageArguments":[{"type":"positional","valueHint":"workspace path","isRequired":true}]});
        let choice = package_draft(&package, "example", &json!({})).unwrap();
        assert_eq!(
            choice["definition"]["server"]["args"][2],
            "__REQUIRED_ARGUMENT_0__"
        );
        assert!(choice["requirements"][0]
            .as_str()
            .unwrap()
            .contains("workspace path"));
        package["runtimeHint"] = json!("custom-launcher");
        assert!(package_draft(&package, "example", &json!({})).is_none());
        package["runtimeHint"] = json!("npx");
        package["packageArguments"][0]["isSecret"] = json!(true);
        assert!(package_draft(&package, "example", &json!({})).is_none());
    }
}
