//! Install this build's standalone CLI on a remote POSIX host over SSH.
//!
//! The release asset is downloaded by the desktop app, then streamed as bytes
//! to a private staging directory. The host's configured CLI is changed only
//! after the staged binary reports the exact app version.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use super::app_release::cli_download_url;
use super::error::AppError;
use super::remote_host::shell_quote;
use super::skill_store::{RemoteHostRecord, SkillStore};

const SSH_TIMEOUT: Duration = Duration::from_secs(120);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BINARY_BYTES: usize = 100 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 16 * 1024;

static ACTIVE_HOSTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemotePlatform {
    os: &'static str,
    arch: &'static str,
    asset: &'static str,
}

fn active_hosts() -> &'static Mutex<HashSet<String>> {
    ACTIVE_HOSTS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct InstallGuard(String);
impl Drop for InstallGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = active_hosts().lock() {
            active.remove(&self.0);
        }
    }
}

fn acquire(host_id: &str) -> Result<InstallGuard, AppError> {
    let mut active = active_hosts().lock().unwrap_or_else(|e| e.into_inner());
    if !active.insert(host_id.to_owned()) {
        return Err(AppError::invalid_input(
            "A CLI install is already running for this host",
        ));
    }
    Ok(InstallGuard(host_id.to_owned()))
}

fn platform(os: &str, arch: &str) -> Result<RemotePlatform, AppError> {
    let os = match os {
        "Linux" => "Linux",
        "Darwin" => "macOS",
        other => {
            return Err(AppError::invalid_input(format!(
                "Remote OS {other} is not supported"
            )))
        }
    };
    let arch = match arch {
        "x86_64" | "amd64" => "x64",
        "aarch64" | "arm64" => "arm64",
        other => {
            return Err(AppError::invalid_input(format!(
                "Remote architecture {other} is not supported"
            )))
        }
    };
    let asset = match (os, arch) {
        ("Linux", "x64") => "skills-manager-cli-Linux-x64",
        ("Linux", "arm64") => "skills-manager-cli-Linux-arm64",
        ("macOS", "x64") => "skills-manager-cli-macOS-x64",
        ("macOS", "arm64") => "skills-manager-cli-macOS-arm64",
        _ => unreachable!(),
    };
    Ok(RemotePlatform { os, arch, asset })
}

/// Run SSH with bounded output and a hard process deadline. The reader threads
/// prevent a full pipe from blocking the child while preserving only a small
/// diagnostic tail.
fn run_ssh(
    host: &RemoteHostRecord,
    remote_command: &str,
    input: Option<&[u8]>,
) -> Result<(i32, Vec<u8>, String), AppError> {
    let mut cmd = ssh_base(host);
    cmd.arg(remote_command);
    run_command(cmd, input, SSH_TIMEOUT)
}

fn run_command(
    mut cmd: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<(i32, Vec<u8>, String), AppError> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::io(format!("Cannot start ssh: {e}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_thread = thread::spawn(move || read_bounded_and_drain(stdout, OUTPUT_LIMIT));
    let stderr_thread = thread::spawn(move || read_bounded_and_drain(stderr, OUTPUT_LIMIT));

    let writer_thread = if let Some(bytes) = input {
        let mut stdin = child.stdin.take().expect("piped stdin");
        let bytes = bytes.to_vec();
        Some(thread::spawn(move || {
            stdin.write_all(&bytes).and_then(|_| stdin.flush())
        }))
    } else {
        drop(child.stdin.take());
        None
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::from(error));
            }
        };
        if let Some(status) = status {
            if stdout_thread.is_finished()
                && stderr_thread.is_finished()
                && writer_thread
                    .as_ref()
                    .is_none_or(thread::JoinHandle::is_finished)
            {
                break status;
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::network("Remote CLI installation timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let writer_result = writer_thread.map(|writer| {
        writer
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("SSH writer thread panicked")))
    });
    let (stdout, stdout_truncated) = stdout_thread.join().unwrap_or_default();
    let (stderr, stderr_truncated) = stderr_thread.join().unwrap_or_default();
    if stdout_truncated || stderr_truncated {
        return Err(AppError::network(
            "Remote SSH command exceeded the output limit",
        ));
    }
    if let Some(Err(error)) = writer_result {
        return Err(AppError::network(format!(
            "SSH binary transfer failed: {error}"
        )));
    }
    Ok((
        status.code().unwrap_or(-1),
        stdout,
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

fn read_bounded_and_drain(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::with_capacity(limit);
    let mut chunk = [0; 4096];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = limit.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..count.min(remaining)]);
                truncated |= count > remaining;
            }
        }
    }
    (buf, truncated)
}

fn ssh_base(host: &RemoteHostRecord) -> Command {
    let mut cmd = Command::new("ssh");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.arg("-T").args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        &host.ssh_target,
    ]);
    cmd
}

fn wrap_sh(script: &str) -> String {
    format!("sh -c {}", shell_quote(script))
}

fn probe_script() -> &'static str {
    "printf '%s\\n' \"$HOME\" \"$(uname -s)\" \"$(uname -m)\""
}

fn parse_probe(output: &[u8]) -> Result<(String, RemotePlatform), AppError> {
    let mut lines = output.split(|byte| *byte == b'\n');
    let home = std::str::from_utf8(lines.next().unwrap_or_default())
        .map_err(|_| AppError::invalid_input("Remote HOME is not valid UTF-8"))?;
    let os = std::str::from_utf8(lines.next().unwrap_or_default())
        .map_err(|_| AppError::invalid_input("Remote OS is not valid UTF-8"))?;
    let arch = std::str::from_utf8(lines.next().unwrap_or_default())
        .map_err(|_| AppError::invalid_input("Remote architecture is not valid UTF-8"))?;
    if !home.starts_with('/') || home.chars().any(char::is_control) {
        return Err(AppError::invalid_input(
            "Remote HOME must be an absolute path",
        ));
    }
    if os.is_empty() || arch.is_empty() {
        return Err(AppError::network("Could not read remote platform over SSH"));
    }
    Ok((home.to_owned(), platform(os, arch)?))
}

fn download(url: &str, proxy_url: Option<&str>) -> Result<Vec<u8>, AppError> {
    let client = super::skillssh_api::build_http_client(proxy_url, DOWNLOAD_TIMEOUT.as_secs());
    let response = client
        .get(url)
        .send()
        .map_err(|e| AppError::network(format!("Could not download the remote CLI: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::network(format!(
            "CLI download returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BINARY_BYTES as u64)
    {
        return Err(AppError::invalid_input(
            "CLI download exceeds the 100 MiB limit",
        ));
    }
    let mut bytes = Vec::new();
    response
        .take((MAX_BINARY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| AppError::network(format!("Could not read CLI download: {e}")))?;
    if bytes.is_empty() || bytes.len() > MAX_BINARY_BYTES {
        return Err(AppError::invalid_input(
            "CLI download is empty or exceeds the 100 MiB limit",
        ));
    }
    Ok(bytes)
}

fn install_script(home: &str, version: &str, install_id: &str) -> String {
    let root = format!("{home}/.skills-manager/remote-cli");
    let stage = format!("{root}/.install-{install_id}");
    let destination = format!("{root}/{version}");
    // All paths are fixed by local data or the validated HOME/version and are
    // still quoted independently at the shell boundary.
    format!(
        r#"set -eu
umask 077
root={root}
stage={stage}
final={destination}
lock={lock}
if [ -L "$HOME/.skills-manager" ] || [ -L "$root" ] || [ -L "$final" ]; then echo SYMLINK_REFUSED >&2; exit 41; fi
mkdir -p "$root"
chmod 700 "$root"
if ! mkdir "$lock" 2>/dev/null; then echo INSTALL_IN_PROGRESS >&2; exit 44; fi
trap 'rmdir "$lock" 2>/dev/null || true' EXIT HUP INT TERM
if [ -e "$final" ]; then
if [ -f "$final/skills-manager-cli" ] && [ ! -L "$final/skills-manager-cli" ] && [ -x "$final/skills-manager-cli" ] && [ "$("$final/skills-manager-cli" --version 2>/dev/null)" = {expected} ]; then cat >/dev/null; printf '%s\n' "$final/skills-manager-cli"; exit 0; fi
  echo VERSION_DIR_CONFLICT >&2; exit 42
fi
mkdir "$stage"
trap 'rm -rf "$stage"; rmdir "$lock" 2>/dev/null || true' EXIT HUP INT TERM
cat > "$stage/skills-manager-cli"
chmod 700 "$stage/skills-manager-cli"
actual=$("$stage/skills-manager-cli" --version 2>/dev/null || true)
if [ "$actual" != {expected} ]; then echo "VERSION_MISMATCH:$actual" >&2; exit 43; fi
mv "$stage" "$final"
printf '%s\n' "$final/skills-manager-cli"
"#,
        root = shell_quote(&root),
        stage = shell_quote(&stage),
        destination = shell_quote(&destination),
        lock = shell_quote(&format!("{root}/.install-lock-{version}")),
        expected = shell_quote(&format!("skills-manager-cli {version}"))
    )
}

fn version_mismatch(name: &str, actual: &str, expected: &str) -> AppError {
    AppError {
        kind: super::error::ErrorKind::RemoteVersionMismatch,
        message: format!("CLI on {name} reported {actual}; expected skills-manager-cli {expected}"),
        details: None,
    }
}

/// Complete the remote install and update the host's dedicated CLI path only
/// if its target and prior path still match the row used to start the install.
pub fn install_cli(
    store: &SkillStore,
    host: RemoteHostRecord,
    emit: impl Fn(&str) + Send + Sync,
) -> Result<RemoteHostRecord, AppError> {
    install_cli_with(store, host, emit, run_ssh, download)
}

fn install_cli_with(
    store: &SkillStore,
    host: RemoteHostRecord,
    emit: impl Fn(&str),
    run: impl Fn(&RemoteHostRecord, &str, Option<&[u8]>) -> Result<(i32, Vec<u8>, String), AppError>,
    download: impl Fn(&str, Option<&str>) -> Result<Vec<u8>, AppError>,
) -> Result<RemoteHostRecord, AppError> {
    let _guard = acquire(&host.id)?;
    let version = env!("CARGO_PKG_VERSION");
    emit("checking");
    let (code, probe, stderr) = run(&host, &wrap_sh(probe_script()), None)?;
    if code != 0 {
        return Err(AppError::network(format!(
            "Could not inspect {} over SSH: {}",
            host.ssh_target,
            stderr.trim()
        )));
    }
    let (home, platform) = parse_probe(&probe)?;
    emit("downloading");
    let url = cli_download_url(version, platform.asset);
    let binary = download(&url, store.proxy_url().as_deref())?;
    emit("installing");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let script = install_script(&home, version, &id);
    let (code, output, stderr) = run(&host, &wrap_sh(&script), Some(&binary))?;
    if code == 43 || stderr.contains("VERSION_MISMATCH") {
        let actual = stderr
            .lines()
            .find_map(|l| l.strip_prefix("VERSION_MISMATCH:"))
            .unwrap_or("unknown version");
        return Err(version_mismatch(&host.name, actual, version));
    }
    if code != 0 {
        return Err(AppError::network(format!(
            "Could not install CLI on {}: {}",
            host.ssh_target,
            stderr.trim()
        )));
    }
    let output_text = String::from_utf8_lossy(&output);
    let cli_path = output_text
        .strip_suffix('\n')
        .unwrap_or(output_text.as_ref())
        .to_owned();
    let expected_path = format!("{home}/.skills-manager/remote-cli/{version}/skills-manager-cli");
    if cli_path != expected_path {
        return Err(AppError::network(
            "Remote installer did not return a valid CLI path",
        ));
    }
    let candidate_host = RemoteHostRecord {
        cli_path: Some(cli_path.clone()),
        ..host.clone()
    };
    let handshake_command =
        super::remote_host::remote_command(&candidate_host, &["serve", "--stdio"]);
    let (code, hello, stderr) = run(&candidate_host, &handshake_command, None)?;
    if code != 0 {
        return Err(AppError::network(format!(
            "Installed CLI handshake failed on {}: {}",
            host.ssh_target,
            stderr.trim()
        )));
    }
    // Match the normal session reader: a login shell may print before hello,
    // and the CLI may emit more JSON lines before stdin EOF closes it.
    let hello: super::serve::HelloLine = hello
        .split(|byte| *byte == b'\n')
        .find_map(|line| serde_json::from_slice(line).ok())
        .ok_or_else(|| AppError::network("Installed CLI did not return a valid handshake"))?;
    super::remote_session::check_hello(&hello.hello, &host.name)?;
    let updated = store
        .set_remote_host_cli_path_if_unchanged(
            &host.id,
            &host.ssh_target,
            host.cli_path.as_deref(),
            &cli_path,
        )
        .map_err(AppError::db)?;
    if !updated {
        return Err(AppError::invalid_input("Remote host changed or was removed during CLI installation; its configured CLI was not updated"));
    }
    store
        .get_remote_host(&host.id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Remote host was removed during CLI installation"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::path::Path;
    #[cfg(unix)]
    use std::process::Output;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn run_installer(home: &std::path::Path, version: &str, bytes: &[u8]) -> Output {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(install_script(home.to_str().unwrap(), version, "fixture"))
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }

    #[cfg(unix)]
    fn fixture_cli(version: &str) -> Vec<u8> {
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && printf '%s\\n' 'skills-manager-cli {version}'\n"
        )
        .into_bytes()
    }

    #[cfg(unix)]
    fn serving_cli(version: &str, hello_version: &str, protocol: u32) -> Vec<u8> {
        let hello = serde_json::json!({
            "hello": {
                "protocol": protocol,
                "version": hello_version,
                "os": "Linux",
                "arch": "x86_64",
                "home": "/tmp",
                "base_dir": "/tmp",
                "pid": 1
            }
        });
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' 'skills-manager-cli {version}' ;;\n  serve) printf '%s\\n' 'login banner'; printf '%s\\n' {}; printf '%s\\n' '{{\"event\":\"app-files-changed\",\"payload\":{{}}}}' ;;\n  *) exit 2 ;;\nesac\n",
            shell_quote(&hello.to_string())
        )
        .into_bytes()
    }

    #[cfg(unix)]
    fn local_ssh(
        home: &Path,
        command: &str,
        input: Option<&[u8]>,
    ) -> Result<(i32, Vec<u8>, String), AppError> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command).env("HOME", home);
        run_command(cmd, input, Duration::from_secs(3))
    }

    #[cfg(unix)]
    fn host_fixture(home: &Path) -> (SkillStore, RemoteHostRecord, std::path::PathBuf) {
        let store = SkillStore::new(&home.join("store.db")).unwrap();
        let old_path = home.join("old-cli");
        std::fs::write(&old_path, b"previous CLI").unwrap();
        let host = RemoteHostRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: "fixture host".into(),
            ssh_target: "fixture@example".into(),
            cli_path: Some(old_path.to_str().unwrap().into()),
            created_at: 10,
        };
        store.insert_remote_host(&host).unwrap();
        (store, host, old_path)
    }

    #[cfg(unix)]
    #[test]
    fn install_persists_candidate_only_after_matching_hello() {
        use std::cell::RefCell;

        let home = tempdir().unwrap();
        let (store, host, old_path) = host_fixture(home.path());
        let version = env!("CARGO_PKG_VERSION");
        let probe = local_ssh(home.path(), &wrap_sh(probe_script()), None)
            .unwrap()
            .1;
        let (_, platform) = parse_probe(&probe).unwrap();
        let expected_url = format!(
            "https://github.com/Yis-company/skills-manager/releases/download/v{version}/{}",
            platform.asset
        );
        let stages = RefCell::new(Vec::new());
        let binary = serving_cli(version, version, super::super::serve::PROTOCOL);

        let updated = install_cli_with(
            &store,
            host.clone(),
            |stage| stages.borrow_mut().push(stage.to_owned()),
            |candidate, command, input| {
                if candidate.cli_path != host.cli_path {
                    assert!(input.is_none(), "handshake must not send a new binary");
                    assert_eq!(
                        store.get_remote_host(&host.id).unwrap().unwrap().cli_path,
                        host.cli_path,
                        "candidate path was saved before handshake"
                    );
                }
                local_ssh(home.path(), command, input)
            },
            |url, proxy| {
                assert_eq!(url, expected_url);
                assert_eq!(proxy, None);
                Ok(binary.clone())
            },
        )
        .unwrap();

        let installed = home.path().join(format!(
            ".skills-manager/remote-cli/{version}/skills-manager-cli"
        ));
        assert_eq!(updated.cli_path.as_deref(), installed.to_str());
        assert_eq!(
            store.get_remote_host(&host.id).unwrap().unwrap().cli_path,
            updated.cli_path
        );
        assert_eq!(std::fs::read(installed).unwrap(), binary);
        assert_eq!(std::fs::read(old_path).unwrap(), b"previous CLI");
        assert_eq!(
            stages.into_inner(),
            ["checking", "downloading", "installing"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn download_and_transfer_failures_keep_the_previous_cli_selection() {
        let version = env!("CARGO_PKG_VERSION");
        for fail_transfer in [false, true] {
            let home = tempdir().unwrap();
            let (store, host, old_path) = host_fixture(home.path());
            let result = install_cli_with(
                &store,
                host.clone(),
                |_| {},
                |_, command, input| {
                    if fail_transfer && input.is_some() {
                        return Err(AppError::network("fixture transfer failed"));
                    }
                    local_ssh(home.path(), command, input)
                },
                |_, _| {
                    if fail_transfer {
                        Ok(serving_cli(version, version, super::super::serve::PROTOCOL))
                    } else {
                        Err(AppError::network("fixture download failed"))
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(
                store.get_remote_host(&host.id).unwrap().unwrap().cli_path,
                host.cli_path
            );
            assert_eq!(std::fs::read(old_path).unwrap(), b"previous CLI");
            assert!(!home.path().join(".skills-manager/remote-cli").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_hello_keeps_the_previous_cli_selection() {
        let version = env!("CARGO_PKG_VERSION");
        for (hello_version, protocol) in [
            ("0.0.0", super::super::serve::PROTOCOL),
            (version, super::super::serve::PROTOCOL + 1),
        ] {
            let home = tempdir().unwrap();
            let (store, host, old_path) = host_fixture(home.path());
            let result = install_cli_with(
                &store,
                host.clone(),
                |_| {},
                |_, command, input| local_ssh(home.path(), command, input),
                |_, _| Ok(serving_cli(version, hello_version, protocol)),
            );
            assert!(result.is_err());
            assert_eq!(
                store.get_remote_host(&host.id).unwrap().unwrap().cli_path,
                host.cli_path
            );
            assert_eq!(std::fs::read(old_path).unwrap(), b"previous CLI");
            assert!(home
                .path()
                .join(format!(
                    ".skills-manager/remote-cli/{version}/skills-manager-cli"
                ))
                .is_file());
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_runner_times_out_after_child_closes_stdout() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("exec 1>&-; exec 2>&-; sleep 2");
        let started = Instant::now();
        let error = run_command(command, None, Duration::from_millis(100)).unwrap_err();
        assert!(error.message.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn command_runner_times_out_if_exited_child_leaves_a_pipe_open() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 0.5 & exit 0");
        let started = Instant::now();
        let error = run_command(command, None, Duration::from_millis(100)).unwrap_err();
        assert!(error.message.contains("timed out"));
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[cfg(unix)]
    #[test]
    fn command_runner_bounds_output_and_reports_broken_transfer() {
        let mut output_command = Command::new("sh");
        output_command.arg("-c").arg("printf '%020000d' 1");
        let error = run_command(output_command, None, Duration::from_secs(2)).unwrap_err();
        assert!(error.message.contains("output limit"));

        let mut transfer_command = Command::new("sh");
        transfer_command.arg("-c").arg("exit 0");
        let error = run_command(
            transfer_command,
            Some(&vec![0u8; 1024 * 1024]),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(error.message.contains("binary transfer failed"));
    }

    #[test]
    fn maps_supported_remote_platforms_to_release_assets() {
        assert_eq!(
            platform("Linux", "x86_64").unwrap().asset,
            "skills-manager-cli-Linux-x64"
        );
        assert_eq!(
            platform("Linux", "aarch64").unwrap().asset,
            "skills-manager-cli-Linux-arm64"
        );
        assert_eq!(
            platform("Darwin", "arm64").unwrap().asset,
            "skills-manager-cli-macOS-arm64"
        );
        assert!(platform("Windows", "x86_64").is_err());
        assert!(platform("Linux", "riscv64").is_err());
    }

    #[test]
    fn probe_requires_absolute_home_and_supported_platform() {
        assert!(parse_probe(b"relative\nLinux\nx86_64\n").is_err());
        assert!(parse_probe(b"/home/me\nLinux\naarch64\n").is_ok());
        assert!(parse_probe(b"/home/me\nFreeBSD\nx86_64\n").is_err());
        let (home, _) = parse_probe(b"/home/my user/ \nLinux\nx86_64\n").unwrap();
        assert_eq!(home, "/home/my user/ ");
    }

    #[cfg(unix)]
    #[test]
    fn installer_quotes_paths_and_refuses_symlink_destinations() {
        let script = install_script("/home/o'neil", "1.2.3", "abc");
        assert!(script.contains("'/home/o'\\''neil/.skills-manager/remote-cli'"));
        assert!(script.contains("[ -L \"$root\" ]"));
        assert!(script.contains("mv \"$stage\" \"$final\""));
        assert!(script.contains("skills-manager-cli 1.2.3"));
    }

    #[cfg(unix)]
    #[test]
    fn installer_script_transfers_and_atomically_installs_a_verified_binary() {
        let home = tempdir().unwrap();
        let output = run_installer(home.path(), "1.2.3", &fixture_cli("1.2.3"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert_eq!(
            path,
            format!(
                "{}/.skills-manager/remote-cli/1.2.3/skills-manager-cli",
                home.path().display()
            )
        );
        assert_eq!(std::fs::read(&path).unwrap(), fixture_cli("1.2.3"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn installer_rejects_wrong_version_and_cleans_only_its_staging_directory() {
        let home = tempdir().unwrap();
        let output = run_installer(home.path(), "1.2.3", &fixture_cli("9.9.9"));
        assert_eq!(output.status.code(), Some(43));
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("VERSION_MISMATCH:skills-manager-cli 9.9.9"));
        let root = home.path().join(".skills-manager/remote-cli");
        assert!(!root.join("1.2.3").exists());
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn installer_refuses_symlinked_private_destination() {
        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(".skills-manager")).unwrap();
        let output = run_installer(home.path(), "1.2.3", &fixture_cli("1.2.3"));
        assert_eq!(output.status.code(), Some(41));
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn failed_stage_creation_does_not_remove_a_preexisting_path() {
        let home = tempdir().unwrap();
        let stage = home
            .path()
            .join(".skills-manager/remote-cli/.install-fixture");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("keep"), b"owned by earlier process").unwrap();
        let output = run_installer(home.path(), "1.2.3", &fixture_cli("1.2.3"));
        assert!(!output.status.success());
        assert_eq!(
            std::fs::read(stage.join("keep")).unwrap(),
            b"owned by earlier process"
        );
        let root = home.path().join(".skills-manager/remote-cli");
        assert!(!root.join(".install-lock-1.2.3").exists());
    }

    #[test]
    fn overlapping_host_installs_are_refused_and_guard_releases() {
        let guard = acquire("test-remote-host").unwrap();
        assert!(acquire("test-remote-host").is_err());
        drop(guard);
        assert!(acquire("test-remote-host").is_ok());
    }

    #[test]
    fn host_cli_path_update_is_compare_and_swap_and_preserves_name_edits() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("remote-host-cas.db")).unwrap();
        let host = RemoteHostRecord {
            id: "cas-host".into(),
            name: "before".into(),
            ssh_target: "me@example".into(),
            cli_path: None,
            created_at: 10,
        };
        store.insert_remote_host(&host).unwrap();
        store
            .update_remote_host(&host.id, "renamed concurrently", &host.ssh_target, None)
            .unwrap();
        assert!(store
            .set_remote_host_cli_path_if_unchanged(
                &host.id,
                &host.ssh_target,
                None,
                "/home/me/.skills-manager/remote-cli/1.2.3/skills-manager-cli",
            )
            .unwrap());
        let current = store.get_remote_host(&host.id).unwrap().unwrap();
        assert_eq!(current.name, "renamed concurrently");
        assert!(current.cli_path.as_deref().unwrap().contains("/1.2.3/"));
        assert!(!store
            .set_remote_host_cli_path_if_unchanged(&host.id, &host.ssh_target, None, "/stale",)
            .unwrap());
        store.delete_remote_host(&host.id).unwrap();
        assert!(!store
            .set_remote_host_cli_path_if_unchanged(
                &host.id,
                &host.ssh_target,
                current.cli_path.as_deref(),
                "/removed-host",
            )
            .unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_remote_installer_lock_is_never_removed_by_a_losing_attempt() {
        let home = tempdir().unwrap();
        let lock = home
            .path()
            .join(".skills-manager/remote-cli/.install-lock-1.2.3");
        std::fs::create_dir_all(&lock).unwrap();
        let output = run_installer(home.path(), "1.2.3", &fixture_cli("1.2.3"));
        assert_eq!(output.status.code(), Some(44));
        assert!(lock.is_dir());
    }
}
