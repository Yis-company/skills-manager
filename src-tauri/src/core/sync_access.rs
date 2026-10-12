//! A host's own access to the Sync remote on GitHub (SID-103): a deploy key
//! created on the host, so the private key never leaves it; GitHub's
//! published SSH host keys in a known_hosts of its own, instead of trusting
//! the first key it sees; and an SSH remote that uses both. Port 22 first,
//! then ssh.github.com:443 where 22 is blocked.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::error::AppError;
use super::remote_host::shell_quote;
use super::{central_repo, git_backup};

fn key_dir() -> PathBuf {
    central_repo::base_dir().join("sync-key")
}

fn key_path() -> PathBuf {
    key_dir().join("id_ed25519")
}

fn known_hosts_path() -> PathBuf {
    key_dir().join("known_hosts")
}

/// This host's public deploy key, created on first use.
pub fn public_key(comment: &str) -> Result<String, AppError> {
    let key = key_path();
    if !key.exists() {
        fs::create_dir_all(key_dir()).map_err(AppError::io)?;
        let out = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
            .arg(&key)
            .output()
            .map_err(AppError::io)?;
        if !out.status.success() {
            return Err(AppError::internal(format!(
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    fs::read_to_string(key.with_extension("pub"))
        .map(|key| key.trim().to_string())
        .map_err(AppError::io)
}

/// Trust exactly GitHub's published host keys, on port 22 and on 443.
pub fn write_known_hosts(host_keys: &[String]) -> Result<(), AppError> {
    if host_keys.is_empty() {
        return Err(AppError::invalid_input("No GitHub host keys given"));
    }
    fs::create_dir_all(key_dir()).map_err(AppError::io)?;
    fs::write(known_hosts_path(), known_hosts(host_keys)).map_err(AppError::io)
}

fn known_hosts(host_keys: &[String]) -> String {
    host_keys
        .iter()
        .flat_map(|key| {
            [
                format!("github.com {key}"),
                format!("[ssh.github.com]:443 {key}"),
            ]
        })
        .map(|line| line + "\n")
        .collect()
}

/// `GIT_SSH_COMMAND` for an SSH remote on a host that has a deploy key, so
/// every git call (clone, fetch, push, ls-remote) uses this key and these
/// host keys and nothing else.
pub fn ssh_env_for_url(url: &str) -> Vec<(String, String)> {
    if !is_ssh_url(url) || !key_path().exists() {
        return Vec::new();
    }
    vec![("GIT_SSH_COMMAND".to_string(), ssh_command())]
}

fn ssh_command() -> String {
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o UserKnownHostsFile={} -o StrictHostKeyChecking=yes -o BatchMode=yes",
        shell_quote(&key_path().to_string_lossy()),
        shell_quote(&known_hosts_path().to_string_lossy()),
    )
}

fn is_ssh_url(url: &str) -> bool {
    url.starts_with("ssh://") || (!url.contains("://") && url.contains('@') && url.contains(':'))
}

/// The SSH URLs of a GitHub repo (`owner/name`): port 22, then the 443
/// fallback.
pub fn ssh_urls(full_name: &str) -> [String; 2] {
    [
        format!("git@github.com:{full_name}.git"),
        format!("ssh://git@ssh.github.com:443/{full_name}.git"),
    ]
}

/// Whether the remote answers this host's key.
pub fn check(url: &str) -> Result<(), AppError> {
    git_backup::remote_has_heads(url)
        .map(|_| ())
        .map_err(|e| AppError::classify_git_error(format!("{e:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::test_repo;

    #[test]
    fn the_key_is_made_once_and_only_its_public_half_is_returned() {
        let _repo = test_repo();
        let first = public_key("agents-manager test").unwrap();
        assert!(first.starts_with("ssh-ed25519 "), "{first}");
        assert!(first.ends_with("agents-manager test"));
        assert_eq!(public_key("other comment").unwrap(), first);
        assert!(!first.contains("PRIVATE"));
    }

    #[test]
    fn known_hosts_trust_githubs_keys_on_both_ports() {
        let text = known_hosts(&[
            "ssh-ed25519 AAAA1".to_string(),
            "ecdsa-sha2 AAAA2".to_string(),
        ]);
        assert_eq!(
            text,
            "github.com ssh-ed25519 AAAA1\n[ssh.github.com]:443 ssh-ed25519 AAAA1\n\
             github.com ecdsa-sha2 AAAA2\n[ssh.github.com]:443 ecdsa-sha2 AAAA2\n"
        );
    }

    #[test]
    fn only_ssh_remotes_get_the_key_and_only_once_it_exists() {
        let _repo = test_repo();
        let [port22, port443] = ssh_urls("acme/skills");
        assert_eq!(port22, "git@github.com:acme/skills.git");
        assert_eq!(port443, "ssh://git@ssh.github.com:443/acme/skills.git");
        assert!(ssh_env_for_url(&port22).is_empty(), "no key yet");

        public_key("test").unwrap();
        let env = ssh_env_for_url(&port443);
        assert_eq!(env[0].0, "GIT_SSH_COMMAND");
        assert!(env[0].1.contains("IdentitiesOnly=yes"));
        assert!(env[0].1.contains("StrictHostKeyChecking=yes"));
        assert!(ssh_env_for_url("https://github.com/acme/skills.git").is_empty());
    }
}
