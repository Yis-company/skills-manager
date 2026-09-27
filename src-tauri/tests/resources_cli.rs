//! Real-process acceptance checks, isolated from the user's library and agents.
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn cli(
    base: &Path,
    kind: &str,
    action: &str,
    input: Value,
    dry_run: bool,
) -> (bool, Value, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_skills-manager-cli"));
    command.args([
        "--base-dir",
        base.to_str().unwrap(),
        "--json",
        kind,
        action,
        "--input",
        "-",
    ]);
    if dry_run {
        command.arg("--dry-run");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&input).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let value = serde_json::from_str(if output.status.success() {
        &stdout
    } else {
        &stderr
    })
    .unwrap_or_else(|_| json!({"unparsed":stderr}));
    (output.status.success(), value, format!("{stdout}{stderr}"))
}

fn project(base: &Path) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let root = base.join("project");
    std::fs::create_dir_all(&root).unwrap();
    let store =
        app_lib::core::skill_store::SkillStore::new(&base.join("skills-manager.db")).unwrap();
    store
        .insert_project(&app_lib::core::skill_store::ProjectRecord {
            id: id.clone(),
            name: "Resource fixture".into(),
            path: root.to_string_lossy().into(),
            workspace_type: "directory".into(),
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
    id
}

#[test]
fn instructions_cli_previews_deploys_and_preserves_stale_target() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (ok, saved, log) = cli(
        &base,
        "instructions",
        "save",
        json!({"name":"Team rules","files":{"AGENTS.md":"Use pnpm.\n"}}),
        false,
    );
    assert!(ok, "{log}");
    let id = saved["item"]["id"].as_str().unwrap();
    let project_id = project(&base);
    let input = json!({"instruction_id":id,"target":{"agent_key":"codex","project_id":project_id}});
    let db = rusqlite::Connection::open(base.join("skills-manager.db")).unwrap();
    let before: i64 = db
        .query_row("SELECT count(*) FROM resource_state", [], |r| r.get(0))
        .unwrap();
    let (ok, _, log) = cli(&base, "instructions", "preview", input.clone(), true);
    assert!(ok, "{log}");
    let after: i64 = db
        .query_row("SELECT count(*) FROM resource_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after, "dry run persisted preview state");
    assert!(!base.join("project/AGENTS.md").exists());
    let (ok, preview, log) = cli(&base, "instructions", "preview", input.clone(), false);
    assert!(ok, "{log}");
    let (ok, _, log) = cli(
        &base,
        "instructions",
        "apply",
        json!({"preview_id":preview["preview_id"]}),
        false,
    );
    assert!(ok, "{log}");
    assert_eq!(
        std::fs::read_to_string(base.join("project/AGENTS.md")).unwrap(),
        "Use pnpm.\n"
    );
    let (_, preview, _) = cli(&base, "instructions", "preview", input, false);
    std::fs::write(base.join("project/AGENTS.md"), "New user edit.\n").unwrap();
    let _ = cli(
        &base,
        "instructions",
        "apply",
        json!({"preview_id":preview["preview_id"]}),
        false,
    );
    assert_eq!(
        std::fs::read_to_string(base.join("project/AGENTS.md")).unwrap(),
        "New user edit.\n"
    );
}

#[test]
fn mcp_cli_changes_only_selected_entry_without_exposing_other_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (ok, saved, log) = cli(
        &base,
        "mcps",
        "save",
        json!({"definition":{"name":"example","transport":"stdio","server":{"command":"node","args":["server.js"]}}}),
        false,
    );
    assert!(ok, "{log}");
    let project_id = project(&base);
    std::fs::create_dir_all(base.join("project/.codex")).unwrap();
    let config = base.join("project/.codex/config.toml");
    std::fs::write(&config,"# keep this comment\nmodel = \"fixture-model\"\n[unrelated]\ntoken = \"SENTINEL_PRIVATE_VALUE\"\n").unwrap();
    let input = json!({"target":{"agentKey":"codex","projectId":project_id},"operations":[{"kind":"deploy","definitionId":saved["definition"]["id"]}]});
    let (ok, _, log) = cli(&base, "mcps", "preview", input.clone(), true);
    assert!(ok, "{log}");
    assert!(!log.contains("SENTINEL_PRIVATE_VALUE"));
    let (ok, preview, log) = cli(&base, "mcps", "preview", input, false);
    assert!(ok, "{log}");
    assert!(!log.contains("SENTINEL_PRIVATE_VALUE"));
    let (ok, _, log) = cli(
        &base,
        "mcps",
        "apply",
        json!({"previewId":preview["previewId"]}),
        false,
    );
    assert!(ok, "{log}");
    assert!(!log.contains("SENTINEL_PRIVATE_VALUE"));
    let result = std::fs::read_to_string(config).unwrap();
    assert!(result.contains("# keep this comment"));
    assert!(result.contains("SENTINEL_PRIVATE_VALUE"));
    let parsed = result.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["example"]["command"].as_str(),
        Some("node")
    );
    let db = rusqlite::Connection::open(base.join("skills-manager.db")).unwrap();
    let secrets: i64 = db
        .query_row(
            "SELECT count(*) FROM resource_state WHERE value LIKE '%SENTINEL_PRIVATE_VALUE%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(secrets, 0);
}

#[test]
fn resource_commands_agree_over_the_remote_stdio_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (ok, saved, log) = cli(
        &base,
        "instructions",
        "save",
        json!({
            "name":"Remote fixture", "files":{"AGENTS.md":"Use the host's project paths.\n"}
        }),
        false,
    );
    assert!(ok, "{log}");
    let (_, local, _) = cli(&base, "instructions", "list", json!({}), false);
    let mut child = Command::new(env!("CARGO_BIN_EXE_skills-manager-cli"))
        .args(["--base-dir", base.to_str().unwrap(), "serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        json!({"id":1,"cmd":"instructions_request","args":{"request":{"action":"list"}}}),
        json!({"id":2,"cmd":"instructions_request","args":{"request":{"action":"get","id":saved["item"]["id"]}}}),
        json!({"id":3,"cmd":"mcps_request","args":{"request":{"action":"capabilities"}}}),
        json!({"id":4,"cmd":"resource_sync_request","args":{"request":{"action":"list"}}}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for request in requests {
            writeln!(stdin, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(lines[0].get("hello").is_some());
    for id in 1..=4 {
        let response = lines.iter().find(|value| value["id"] == id).unwrap();
        assert_eq!(response["ok"], true, "{response}");
    }
    assert_eq!(
        lines.iter().find(|value| value["id"] == 1).unwrap()["result"],
        local
    );
    assert_eq!(
        lines.iter().find(|value| value["id"] == 2).unwrap()["result"]["item"]["files"]
            ["AGENTS.md"],
        "Use the host's project paths.\n"
    );
}

#[test]
fn dry_run_reads_a_pre_feature_database_without_migrating_it() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (ok, saved, log) = cli(
        &base,
        "instructions",
        "save",
        json!({
            "name":"Upgrade fixture", "files":{"AGENTS.md":"Rules\n"}
        }),
        false,
    );
    assert!(ok, "{log}");
    let project_id = project(&base);
    let database = base.join("skills-manager.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch("DROP TABLE resource_state; PRAGMA user_version = 11;")
        .unwrap();
    drop(conn);
    let before = std::fs::read(&database).unwrap();
    let (ok, _, log) = cli(
        &base,
        "instructions",
        "preview",
        json!({
            "instruction_id":saved["item"]["id"],"target":{"agent_key":"codex","project_id":project_id}
        }),
        true,
    );
    assert!(ok, "{log}");
    assert_eq!(std::fs::read(&database).unwrap(), before);
    assert!(!base.join("project/AGENTS.md").exists());
}
