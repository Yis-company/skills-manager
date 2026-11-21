//! `skills-manager-cli mcp register`: writes the user-scope registration that
//! lets Codex and Claude Code start `skills-manager-cli mcp`. Each test gets
//! its own temp base dir, HOME and CODEX_HOME, so nothing here reads or writes
//! this machine's agent configuration.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");

struct Machine {
    tmp: TempDir,
}

impl Machine {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        std::fs::create_dir_all(tmp.path().join("codex")).unwrap();
        Self { tmp }
    }

    fn claude_config(&self) -> PathBuf {
        self.tmp.path().join("home/.claude.json")
    }

    fn codex_config(&self) -> PathBuf {
        self.tmp.path().join("codex/config.toml")
    }

    fn register(&self, client: &str) -> Output {
        Command::new(CLI)
            .arg("--base-dir")
            .arg(self.tmp.path().join("base"))
            .args(["--json", "mcp", "register", "--client", client])
            .env("HOME", self.tmp.path().join("home"))
            .env("CODEX_HOME", self.tmp.path().join("codex"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .output()
            .unwrap()
    }
}

fn report(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "register failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn cli_path() -> String {
    Path::new(CLI)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn codex_registration_keeps_other_settings_and_a_rerun_changes_nothing() {
    let machine = Machine::new();
    let original = "# my settings\nmodel = \"gpt\"\n\n[mcp_servers.other]\ncommand = \"keep\"\n";
    std::fs::write(machine.codex_config(), original).unwrap();

    let first = report(&machine.register("codex"));
    assert_eq!(first["status"], "added");
    assert!(first["nextStep"].as_str().unwrap().contains("T3"));
    let written = std::fs::read_to_string(machine.codex_config()).unwrap();
    let parsed: toml_edit::DocumentMut = written.parse().unwrap();
    assert_eq!(parsed["model"].as_str(), Some("gpt"));
    assert_eq!(
        parsed["mcp_servers"]["other"]["command"].as_str(),
        Some("keep")
    );
    let entry = &parsed["mcp_servers"]["skills-manager"];
    assert_eq!(entry["command"].as_str(), Some(cli_path().as_str()));
    let args: Vec<_> = entry["args"].as_array().unwrap().iter().collect();
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].as_str(), Some("mcp"));
    assert!(written.contains("# my settings"));

    let second = report(&machine.register("codex"));
    assert_eq!(second["status"], "unchanged");
    assert_eq!(
        std::fs::read_to_string(machine.codex_config()).unwrap(),
        written
    );
}

#[test]
fn claude_registration_creates_the_user_config_and_a_rerun_changes_nothing() {
    let machine = Machine::new();

    let first = report(&machine.register("claude"));
    assert_eq!(first["status"], "added");
    assert!(first["nextStep"].as_str().unwrap().contains("Claude Code"));
    let written = std::fs::read_to_string(machine.claude_config()).unwrap();
    let parsed: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        parsed["mcpServers"]["skills-manager"],
        json!({"command": cli_path(), "args": ["mcp"]})
    );

    let second = report(&machine.register("claude"));
    assert_eq!(second["status"], "unchanged");
    assert_eq!(
        std::fs::read_to_string(machine.claude_config()).unwrap(),
        written
    );
}

#[test]
fn an_existing_registration_written_by_claude_mcp_add_counts_as_unchanged() {
    let machine = Machine::new();
    let original = json!({
        "mcpServers": {"skills-manager": {
            "type": "stdio", "command": cli_path(), "args": ["mcp"], "env": {}
        }}
    })
    .to_string();
    std::fs::write(machine.claude_config(), &original).unwrap();

    assert_eq!(report(&machine.register("claude"))["status"], "unchanged");
    assert_eq!(
        std::fs::read_to_string(machine.claude_config()).unwrap(),
        original
    );
}

#[test]
fn a_different_registration_under_the_same_name_is_reported_and_left_alone() {
    let machine = Machine::new();
    let original = json!({
        "projects": {"/work": {"allowedTools": []}},
        "mcpServers": {"skills-manager": {"command": "/somewhere/else", "args": ["mcp"]}}
    })
    .to_string();
    std::fs::write(machine.claude_config(), &original).unwrap();

    let output = machine.register("claude");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "TARGET_CONFLICT");
    assert_eq!(
        std::fs::read_to_string(machine.claude_config()).unwrap(),
        original
    );
}
