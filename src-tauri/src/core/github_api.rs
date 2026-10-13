//! Minimal GitHub REST client for the guided backup setup (backup redesign
//! Phase 2, PAT mode): validate a token, then find or create the private
//! backup repository. The token itself never appears in URLs, logs, or error
//! messages — callers store it in the OS keychain.
//!
//! Errors carry stable prefixes (`GITHUB_TOKEN_INVALID`, `GITHUB_SCOPE`,
//! `GITHUB_NETWORK`) the frontend maps to plain-language copy.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::skillssh_api::build_http_client;

const API_BASE: &str = "https://api.github.com";

/// Public OAuth App client id for the GitHub Device Flow (backup redesign
/// §3.2). Client ids are not secrets — shipping one in an open-source app is
/// the standard device-flow setup; there is deliberately no client secret.
pub const OAUTH_CLIENT_ID: &str = "Ov23li4a3SMdhIiKo7IE";

#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubConnectInfo {
    pub login: String,
    pub repo_full_name: String,
    /// Credential-free HTTPS clone URL.
    pub url: String,
    pub repo_created: bool,
    /// False when the user connected a pre-existing PUBLIC repository — the
    /// UI warns, since a public backup is almost never intentional.
    /// Repositories created by the app are always private.
    pub repo_private: bool,
}

#[derive(Deserialize)]
struct UserResp {
    login: String,
}

#[derive(Deserialize)]
struct RepoResp {
    full_name: String,
    /// Missing field is treated as private so a parsing gap can only ever
    /// suppress the warning, never raise a false alarm.
    private: Option<bool>,
}

/// GitHub repository name rules (subset): ASCII letters, digits, `-`, `_`,
/// `.`; not empty, not `.`/`..`, max 100 chars.
pub fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn request(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    client
        .request(method, url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

/// Validate the token, then ensure the backup repository exists under the
/// token owner's account (creating it as a private repo when missing).
pub fn connect_backup_repo(
    token: &str,
    repo_name: &str,
    proxy_url: Option<&str>,
) -> Result<GithubConnectInfo> {
    if !is_valid_repo_name(repo_name) {
        bail!("Invalid repository name");
    }
    let client = build_http_client(proxy_url, 20);

    // Who owns this token? Also serves as token validation.
    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/user"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    let login = match resp.status().as_u16() {
        200 => resp.json::<UserResp>().context("Unexpected /user response")?.login,
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        403 => bail!("GITHUB_TOKEN_INVALID: GitHub denied access (403); the token may lack permissions or be rate-limited"),
        s => bail!("GitHub /user returned HTTP {s}"),
    };

    // Find or create the repository.
    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/repos/{login}/{repo_name}"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;

    let (repo_created, repo) = match resp.status().as_u16() {
        200 => (
            false,
            resp.json::<RepoResp>().context("Unexpected repo response")?,
        ),
        404 => {
            let resp = request(&client, reqwest::Method::POST, &format!("{API_BASE}/user/repos"), token)
                .json(&serde_json::json!({
                    "name": repo_name,
                    "private": true,
                    "auto_init": false,
                    "description": "Skills Manager backup",
                }))
                .send()
                .context("GITHUB_NETWORK: could not reach api.github.com")?;
            match resp.status().as_u16() {
                201 => (
                    true,
                    resp.json::<RepoResp>().context("Unexpected create-repo response")?,
                ),
                401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
                // Classic PATs without `repo` scope and fine-grained tokens
                // without Administration:write both land here.
                403 | 404 => bail!(
                    "GITHUB_SCOPE: the token cannot create repositories — it needs the 'repo' scope (classic) or Administration: write (fine-grained)"
                ),
                s => bail!("GitHub create-repo returned HTTP {s}"),
            }
        }
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        403 => bail!("GITHUB_SCOPE: the token cannot read this repository (403); grant it access to {login}/{repo_name}"),
        s => bail!("GitHub repo lookup returned HTTP {s}"),
    };

    let full_name = repo.full_name;
    let repo_private = repo.private.unwrap_or(true);
    log::info!(
        "github connect: using repository {full_name} (created={repo_created}, private={repo_private})"
    );
    Ok(GithubConnectInfo {
        login,
        url: format!("https://github.com/{full_name}.git"),
        repo_full_name: full_name,
        repo_created,
        repo_private,
    })
}

// ── Host deploy keys (SID-103, SID-111) ──

/// `owner/name` of a GitHub remote, from its HTTPS or SSH URL.
pub fn repo_full_name(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@ssh.github.com:443/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let name = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = name.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) if !owner.is_empty() && is_valid_repo_name(repo) => {
            Some(name.to_string())
        }
        _ => None,
    }
}

/// Register a host's public key as a read/write deploy key of the Sync
/// repository. Returns the key id, which leaving needs to delete it.
pub fn create_deploy_key(
    token: &str,
    repo_full_name: &str,
    title: &str,
    public_key: &str,
    proxy_url: Option<&str>,
) -> Result<u64> {
    #[derive(Deserialize)]
    struct KeyResp {
        id: u64,
    }
    let client = build_http_client(proxy_url, 20);
    let resp = request(
        &client,
        reqwest::Method::POST,
        &format!("{API_BASE}/repos/{repo_full_name}/keys"),
        token,
    )
    .json(&serde_json::json!({ "title": title, "key": public_key, "read_only": false }))
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    match resp.status().as_u16() {
        201 => Ok(resp
            .json::<KeyResp>()
            .context("Unexpected deploy-key response")?
            .id),
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        s => bail!("GITHUB_SCOPE: GitHub refused to add the deploy key (HTTP {s})"),
    }
}

/// Delete a host's deploy key. A key that is already gone counts as deleted.
pub fn delete_deploy_key(
    token: &str,
    repo_full_name: &str,
    key_id: u64,
    proxy_url: Option<&str>,
) -> Result<()> {
    let client = build_http_client(proxy_url, 20);
    let resp = request(
        &client,
        reqwest::Method::DELETE,
        &format!("{API_BASE}/repos/{repo_full_name}/keys/{key_id}"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    match resp.status().as_u16() {
        204 | 404 => Ok(()),
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        s => bail!("GITHUB_SCOPE: GitHub refused to delete the deploy key (HTTP {s})"),
    }
}

/// GitHub's published SSH host keys (`/meta`), so a host trusts exactly
/// these instead of the first key it sees.
pub fn ssh_host_keys(proxy_url: Option<&str>) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Meta {
        ssh_keys: Vec<String>,
    }
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .get(format!("{API_BASE}/meta"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("GITHUB_NETWORK: could not reach api.github.com")?;
    if !resp.status().is_success() {
        bail!("GitHub /meta returned HTTP {}", resp.status());
    }
    Ok(resp
        .json::<Meta>()
        .context("Unexpected /meta response")?
        .ssh_keys)
}

// ── Device Flow (§3.2) ──

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceFlowStart {
    pub device_code: String,
    /// The 8-character code the user types at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the codes expire (GitHub: 900).
    pub expires_in: u64,
    /// Minimum seconds between polls (GitHub: 5).
    pub interval: u64,
}

pub enum DevicePollOutcome {
    Pending,
    /// Polled too fast — caller must add 5 seconds to its interval.
    SlowDown,
    Authorized {
        token: String,
    },
}

/// Request a device + user code pair to start the flow.
pub fn device_flow_start(proxy_url: Option<&str>) -> Result<DeviceFlowStart> {
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", OAUTH_CLIENT_ID), ("scope", "repo")])
        .send()
        .context("GITHUB_NETWORK: could not reach github.com")?;
    if !resp.status().is_success() {
        bail!(
            "GitHub device-code endpoint returned HTTP {}",
            resp.status()
        );
    }
    let v: serde_json::Value = resp.json().context("Unexpected device-code response")?;
    let field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .with_context(|| format!("device-code response missing {k}"))
    };
    Ok(DeviceFlowStart {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        expires_in: v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(900),
        interval: v.get("interval").and_then(|x| x.as_u64()).unwrap_or(5),
    })
}

/// One poll of the token endpoint. The caller owns the pacing loop
/// (`interval` seconds between calls, +5s on `SlowDown`, stop at expiry).
pub fn device_flow_poll(device_code: &str, proxy_url: Option<&str>) -> Result<DevicePollOutcome> {
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", OAUTH_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .context("GITHUB_NETWORK: could not reach github.com")?;
    let v: serde_json::Value = resp.json().context("Unexpected token response")?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return match err {
            "authorization_pending" => Ok(DevicePollOutcome::Pending),
            "slow_down" => Ok(DevicePollOutcome::SlowDown),
            "expired_token" => bail!("GITHUB_DEVICE_EXPIRED: the verification code expired"),
            "access_denied" => bail!("GITHUB_DEVICE_DENIED: authorization was declined on GitHub"),
            other => bail!("GitHub device flow failed: {other}"),
        };
    }
    let token = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .context("token response missing access_token")?;
    Ok(DevicePollOutcome::Authorized {
        token: token.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_full_name_reads_https_and_ssh_remotes() {
        for url in [
            "https://github.com/acme/skills.git",
            "https://github.com/acme/skills",
            "git@github.com:acme/skills.git",
            "ssh://git@ssh.github.com:443/acme/skills.git",
        ] {
            assert_eq!(repo_full_name(url).as_deref(), Some("acme/skills"), "{url}");
        }
        assert_eq!(repo_full_name("https://gitlab.com/acme/skills.git"), None);
        assert_eq!(repo_full_name("https://github.com/acme"), None);
    }

    #[test]
    fn repo_name_validation() {
        assert!(is_valid_repo_name("skills-manager-backup"));
        assert!(is_valid_repo_name("My_Backup.2026"));
        assert!(!is_valid_repo_name(""));
        assert!(!is_valid_repo_name("."));
        assert!(!is_valid_repo_name(".."));
        assert!(!is_valid_repo_name("has space"));
        assert!(!is_valid_repo_name("has/slash"));
        assert!(!is_valid_repo_name(&"x".repeat(101)));
    }
}
