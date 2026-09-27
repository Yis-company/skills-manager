//! Release source shared by desktop update checks and remote CLI installs.

pub const REPOSITORY: &str = "Yis-company/skills-manager";

pub fn latest_release_api_url() -> String {
    format!("https://api.github.com/repos/{REPOSITORY}/releases/latest")
}

pub fn releases_url() -> String {
    format!("https://github.com/{REPOSITORY}/releases")
}

/// Callers supply the running app's version and a supported CLI asset name.
pub fn cli_download_url(version: &str, asset: &str) -> String {
    format!("{}/download/v{version}/{asset}", releases_url())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_installer_and_release_checks_use_the_same_fork() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["updater"]["endpoints"],
            serde_json::json!([format!("{}/latest/download/latest.json", releases_url())])
        );
        assert_eq!(
            latest_release_api_url(),
            "https://api.github.com/repos/Yis-company/skills-manager/releases/latest"
        );
    }

    #[test]
    fn cli_download_is_pinned_to_the_requested_version() {
        assert_eq!(
            cli_download_url("1.41.2", "skills-manager-cli-Linux-arm64"),
            "https://github.com/Yis-company/skills-manager/releases/download/v1.41.2/skills-manager-cli-Linux-arm64"
        );
    }
}
