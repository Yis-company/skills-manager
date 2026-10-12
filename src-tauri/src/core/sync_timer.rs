//! The systemd user timer that syncs a CLI-only host every 5 minutes
//! (SID-103), so it keeps syncing while the app is closed. It runs the CLI
//! through `remote-cli/current`, which stays put across versions (SID-101).
//! Hosts without systemd user services sync when the app connects or on
//! Sync now.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

const UNIT: &str = "agents-manager-sync";

#[derive(Debug, Serialize)]
pub struct TimerState {
    pub installed: bool,
    /// Why it isn't installed, or what keeps it from running unattended.
    pub warning: Option<String>,
}

fn service_unit() -> String {
    "[Unit]\n\
     Description=Agents Manager Sync\n\n\
     [Service]\n\
     Type=oneshot\n\
     ExecStart=%h/.skills-manager/remote-cli/current/skills-manager-cli sync now\n"
        .to_string()
}

fn timer_unit() -> String {
    "[Unit]\n\
     Description=Agents Manager Sync every 5 minutes\n\n\
     [Timer]\n\
     OnBootSec=1min\n\
     OnUnitActiveSec=5min\n\
     Persistent=true\n\n\
     [Install]\n\
     WantedBy=timers.target\n"
        .to_string()
}

fn unit_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))?;
    Some(config.join("systemd/user"))
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Install and start the timer where this host has systemd user services.
pub fn install() -> TimerState {
    let Some(dir) =
        unit_dir().filter(|_| cfg!(target_os = "linux") && systemctl(&["show-environment"]))
    else {
        return TimerState {
            installed: false,
            warning: Some(
                "This host has no systemd user services; it syncs when the app connects or on Sync now"
                    .to_string(),
            ),
        };
    };
    let written = fs::create_dir_all(&dir)
        .and_then(|_| fs::write(dir.join(format!("{UNIT}.service")), service_unit()))
        .and_then(|_| fs::write(dir.join(format!("{UNIT}.timer")), timer_unit()));
    if let Err(e) = written {
        return TimerState {
            installed: false,
            warning: Some(format!("Couldn't write the sync timer: {e}")),
        };
    }
    if !systemctl(&["daemon-reload"]) || !systemctl(&["enable", "--now", &format!("{UNIT}.timer")])
    {
        return TimerState {
            installed: false,
            warning: Some("systemd refused to start the sync timer".to_string()),
        };
    }
    TimerState {
        installed: true,
        warning: (!linger_enabled()).then(|| {
            "Linger is off, so the timer stops when you log out. Run `loginctl enable-linger` to keep it running"
                .to_string()
        }),
    }
}

/// Stop and remove the timer. Nothing to do where it was never installed.
pub fn remove() {
    let Some(dir) = unit_dir().filter(|_| cfg!(target_os = "linux")) else {
        return;
    };
    let timer = dir.join(format!("{UNIT}.timer"));
    if !timer.exists() {
        return;
    }
    systemctl(&["disable", "--now", &format!("{UNIT}.timer")]);
    let _ = fs::remove_file(timer);
    let _ = fs::remove_file(dir.join(format!("{UNIT}.service")));
    systemctl(&["daemon-reload"]);
}

fn linger_enabled() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "Linger=yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_runs_the_current_cli_every_five_minutes() {
        let service = service_unit();
        assert!(service.contains(
            "ExecStart=%h/.skills-manager/remote-cli/current/skills-manager-cli sync now"
        ));
        assert!(service.contains("Type=oneshot"));
        let timer = timer_unit();
        assert!(timer.contains("OnUnitActiveSec=5min"));
        assert!(timer.contains("WantedBy=timers.target"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn hosts_without_systemd_are_told_how_they_sync() {
        let state = install();
        assert!(!state.installed);
        assert!(state
            .warning
            .unwrap()
            .contains("syncs when the app connects"));
    }
}
