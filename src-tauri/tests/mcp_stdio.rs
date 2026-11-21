//! The real `skills-manager-cli mcp`, driven the way Codex and Claude Code
//! drive a stdio MCP server: newline-delimited JSON-RPC on stdin and stdout.
//! Each server gets its own temp base dir and temp HOME.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use app_lib::core::project_setup;
use app_lib::core::skill_store::{ProjectRecord, SkillStore};
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
        Self { tmp }
    }

    fn base(&self) -> PathBuf {
        self.tmp.path().join("base")
    }

    fn cli(&self) -> Command {
        let mut cmd = Command::new(CLI);
        cmd.arg("--base-dir")
            .arg(self.base())
            .env("HOME", self.tmp.path().join("home"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME");
        cmd
    }

    /// Send `messages` to `cli mcp`, close stdin, and return every line it
    /// wrote. Each line must be JSON-RPC: anything else breaks the client.
    fn mcp(&self, messages: &[Value]) -> Vec<Value> {
        let mut child = self
            .cli()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for message in messages {
            writeln!(stdin, "{message}").unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "mcp exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect()
    }
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn initialized() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

fn reply(lines: &[Value], id: u64) -> &Value {
    lines
        .iter()
        .find(|line| line["id"] == id)
        .unwrap_or_else(|| panic!("no reply to {id}: {lines:?}"))
}

fn discover(id: u64, checkout: &Path) -> Value {
    request(
        id,
        "tools/call",
        json!({ "name": "discover_project_setup", "arguments": { "checkout_path": checkout } }),
    )
}

/// Every file under `dir` with its bytes, to prove nothing was written.
/// SQLite's `-wal` and `-shm` sidecars are left out: reading a WAL database,
/// even read-only, creates them empty, and the database itself stays as it was.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy();
            if name.ends_with(".db-wal") || name.ends_with(".db-shm") {
                continue;
            }
            if path.is_dir() {
                pending.push(path.clone());
                files.insert(path, Vec::new());
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The paths whose presence or bytes differ between two snapshots.
fn changed(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = before.keys().chain(after.keys()).cloned().collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .collect()
}

#[test]
fn the_handshake_matches_what_codex_and_claude_code_send() {
    let machine = Machine::new();
    // Codex asks for 2025-06-18 with elicitation; Claude Code asks for
    // 2025-11-25 with roots. Both then list tools and may ping.
    let clients = [
        (
            "2025-06-18",
            json!({ "elicitation": {} }),
            "codex-mcp-client",
        ),
        ("2025-11-25", json!({ "roots": {} }), "claude-code"),
    ];
    for (version, capabilities, client) in clients {
        let lines = machine.mcp(&[
            request(
                1,
                "initialize",
                json!({
                    "protocolVersion": version,
                    "capabilities": capabilities,
                    "clientInfo": { "name": client, "version": "1.0.0" },
                }),
            ),
            initialized(),
            request(2, "tools/list", json!({})),
            request(3, "ping", json!({})),
            request(4, "resources/list", json!({})),
        ]);

        // The notification gets no reply; every request gets exactly one.
        assert_eq!(lines.len(), 4, "{client}: {lines:?}");
        for line in &lines {
            assert_eq!(line["jsonrpc"], "2.0");
        }
        let init = &reply(&lines, 1)["result"];
        assert_eq!(init["protocolVersion"], version, "{client}");
        assert!(init["capabilities"]["tools"].is_object());
        assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(init["serverInfo"]["name"], "skills-manager");

        let tools = reply(&lines, 2)["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let tool = &tools[0];
        assert_eq!(tool["name"], "discover_project_setup");
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["required"], json!(["checkout_path"]));
        assert_eq!(tool["outputSchema"]["type"], "object");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        let apply = &tools[1];
        assert_eq!(apply["name"], "apply_project_setup");
        assert_eq!(
            apply["inputSchema"]["required"],
            json!(["checkout_path", "operation_id"])
        );
        assert_eq!(apply["annotations"]["readOnlyHint"], false);
        let inspect = &tools[2];
        assert_eq!(inspect["name"], "inspect_project_setup_result");
        assert_eq!(
            inspect["inputSchema"]["required"],
            json!(["checkout_path", "operation_id"])
        );
        assert_eq!(inspect["annotations"]["readOnlyHint"], true);

        assert_eq!(reply(&lines, 3)["result"], json!({}));
        assert_eq!(reply(&lines, 4)["error"]["code"], -32601);
    }
}

#[test]
fn an_unknown_protocol_version_gets_the_latest_one() {
    let machine = Machine::new();
    let lines = machine.mcp(&[request(
        1,
        "initialize",
        json!({ "protocolVersion": "1999-01-01", "capabilities": {}, "clientInfo": { "name": "x" } }),
    )]);
    assert_eq!(reply(&lines, 1)["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn a_malformed_line_is_answered_and_the_stream_goes_on() {
    let machine = Machine::new();
    let mut child = machine
        .cli()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "not json\n\n{}", request(2, "ping", json!({}))).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["id"], Value::Null);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(reply(&lines, 2)["result"], json!({}));
}

#[test]
fn a_cold_start_discovery_writes_nothing() {
    let machine = Machine::new();
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let before = snapshot(machine.tmp.path());

    let lines = machine.mcp(&[
        request(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "x" } })),
        initialized(),
        discover(2, &checkout),
    ]);

    assert_eq!(
        changed(&before, &snapshot(machine.tmp.path())),
        Vec::<PathBuf>::new()
    );
    assert!(!machine.base().exists());
    let result = &reply(&lines, 2)["result"];
    assert_eq!(result["isError"], false);
    let found = &result["structuredContent"];
    assert_eq!(
        found["checkout_path"],
        json!(checkout.canonicalize().unwrap())
    );
    assert_eq!(found["project"], Value::Null);
    assert_eq!(found["candidates"], json!([]));
    assert_eq!(found["library"]["items"], json!([]));
    // The text form carries the same result for clients that only read text.
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, found);
}

#[test]
fn discovery_on_an_existing_library_leaves_it_untouched() {
    let machine = Machine::new();
    let made = machine
        .cli()
        .args(["--json", "repo", "status"])
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    assert!(machine.base().exists());
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let before = snapshot(machine.tmp.path());

    let lines = machine.mcp(&[discover(1, &checkout)]);

    assert_eq!(
        changed(&before, &snapshot(machine.tmp.path())),
        Vec::<PathBuf>::new()
    );
    assert_eq!(reply(&lines, 1)["result"]["isError"], false);
}

#[test]
fn a_failed_discovery_is_a_tool_error_the_agent_can_read() {
    let machine = Machine::new();
    let lines = machine.mcp(&[
        request(
            1,
            "tools/call",
            json!({ "name": "discover_project_setup", "arguments": { "checkout_path": "relative/path" } }),
        ),
        request(2, "tools/call", json!({ "name": "remove_project_setup", "arguments": {} })),
    ]);

    let failed = &reply(&lines, 1)["result"];
    assert_eq!(failed["isError"], true);
    assert!(failed["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("absolute"));
    assert_eq!(reply(&lines, 2)["error"]["code"], -32602);
}

#[test]
fn apply_sets_up_a_saved_mcp_connection_and_inspection_reads_it_back_without_writing() {
    let machine = Machine::new();
    let saved = machine
        .cli()
        .args(["--json", "mcps", "save", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            let definition = json!({ "definition": { "name": "docs", "transport": "stdio",
                "server": { "command": "npx", "args": ["-y", "docs"] } } });
            writeln!(child.stdin.take().unwrap(), "{definition}")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(saved.status.success());
    let definition: Value = serde_json::from_slice(&saved.stdout).unwrap();
    let definition = &definition["definition"];
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let store = SkillStore::new(&machine.base().join("skills-manager.db")).unwrap();
    store
        .insert_project(&ProjectRecord {
            id: "p1".into(),
            name: "checkout".into(),
            path: checkout.to_string_lossy().into_owned(),
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
    let item = json!({ "kind": "mcp", "id": definition["id"], "agents": ["claude_code"],
        "approved": { "claude_code": project_setup::mcp_value(definition) } });
    project_setup::save_setup(&store, "p1", 0, vec![serde_json::from_value(item).unwrap()])
        .unwrap();
    drop(store);
    let apply = |id| {
        request(
            id,
            "tools/call",
            json!({ "name": "apply_project_setup",
                    "arguments": { "checkout_path": checkout, "operation_id": "op-1" } }),
        )
    };

    let lines = machine.mcp(&[apply(1), apply(2)]);

    let first = &reply(&lines, 1)["result"];
    assert_eq!(first["isError"], false, "{first}");
    assert_eq!(first["structuredContent"]["outcome"], "verified_changed");
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(checkout.join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(config["mcpServers"]["docs"]["command"], "npx");
    let retry = &reply(&lines, 2)["result"]["structuredContent"];
    assert_eq!(retry["from_receipt"], true);
    assert_eq!(retry["outcome"], "verified_changed");

    let inspect = |id, operation_id| {
        request(
            id,
            "tools/call",
            json!({ "name": "inspect_project_setup_result",
                    "arguments": { "checkout_path": checkout, "operation_id": operation_id } }),
        )
    };
    let before = (snapshot(&machine.base()), snapshot(&checkout));
    let lines = machine.mcp(&[inspect(3, "op-1"), inspect(4, "op-unknown")]);

    let recorded = &reply(&lines, 3)["result"];
    assert_eq!(recorded["isError"], false, "{recorded}");
    assert_eq!(recorded["structuredContent"]["outcome"], "verified_changed");
    assert_eq!(
        recorded["structuredContent"]["destination"]["state"],
        "matches"
    );
    let unknown = &reply(&lines, 4)["result"]["structuredContent"];
    assert_eq!(unknown["outcome"], "outcome_unknown");
    assert_eq!(unknown["receipt"], "missing");
    let after = (snapshot(&machine.base()), snapshot(&checkout));
    assert_eq!(changed(&before.0, &after.0), Vec::<PathBuf>::new());
    assert_eq!(changed(&before.1, &after.1), Vec::<PathBuf>::new());
}

#[test]
fn a_first_setup_is_proposed_then_confirmed_through_apply() {
    let machine = Machine::new();
    let saved = machine
        .cli()
        .args(["--json", "mcps", "save", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            let definition = json!({ "definition": { "name": "docs", "transport": "stdio",
                "server": { "command": "npx", "args": ["-y", "docs"] } } });
            writeln!(child.stdin.take().unwrap(), "{definition}")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(saved.status.success());
    let definition: Value = serde_json::from_slice(&saved.stdout).unwrap();
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let apply = |arguments: Value| {
        let lines = machine.mcp(&[request(
            1,
            "tools/call",
            json!({ "name": "apply_project_setup", "arguments": arguments }),
        )]);
        reply(&lines, 1)["result"]["structuredContent"].clone()
    };

    let proposed = apply(json!({ "checkout_path": checkout, "operation_id": "op-1",
        "items": [{ "kind": "mcp", "id": definition["definition"]["id"], "agents": ["claude_code"] }] }));
    assert_eq!(proposed["outcome"], "confirmation_needed", "{proposed}");
    assert_eq!(std::fs::read_dir(&checkout).unwrap().count(), 0);

    let mut confirm = proposed["proposal"].clone();
    confirm["decisions"][0]["choice"] = json!("approve");
    let applied = apply(json!({ "checkout_path": checkout, "operation_id": "op-2",
        "confirm": confirm }));
    assert_eq!(applied["outcome"], "verified_changed", "{applied}");
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(checkout.join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(config["mcpServers"]["docs"]["command"], "npx");
}

fn save_docs_definition(machine: &Machine) -> Value {
    let saved = machine
        .cli()
        .args(["--json", "mcps", "save", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            let definition = json!({ "definition": { "name": "docs", "transport": "stdio",
                "server": { "command": "npx", "args": ["-y", "docs"] } } });
            writeln!(child.stdin.take().unwrap(), "{definition}")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(saved.status.success());
    let saved: Value = serde_json::from_slice(&saved.stdout).unwrap();
    saved["definition"].clone()
}

#[test]
fn a_setup_confirmed_at_cold_start_is_discovered_in_the_same_session() {
    let machine = Machine::new();
    let definition = save_docs_definition(&machine);
    for entry in std::fs::read_dir(machine.base()).unwrap() {
        let path = entry.unwrap().path();
        if path.to_string_lossy().contains("skills-manager.db") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let apply = |id, arguments: Value| {
        request(
            id,
            "tools/call",
            json!({ "name": "apply_project_setup", "arguments": arguments }),
        )
    };
    let proposed = machine.mcp(&[apply(
        1,
        json!({ "checkout_path": checkout,
        "operation_id": "op-1",
        "items": [{ "kind": "mcp", "id": definition["id"], "agents": ["claude_code"] }] }),
    )]);
    let mut confirm = reply(&proposed, 1)["result"]["structuredContent"]["proposal"].clone();
    confirm["decisions"][0]["choice"] = json!("approve");

    let lines = machine.mcp(&[
        apply(
            2,
            json!({ "checkout_path": checkout, "operation_id": "op-2", "confirm": confirm }),
        ),
        discover(3, &checkout),
    ]);

    let applied = &reply(&lines, 2)["result"]["structuredContent"];
    assert_eq!(applied["outcome"], "verified_changed", "{applied}");
    let found = &reply(&lines, 3)["result"]["structuredContent"];
    assert_eq!(found["project"]["id"], applied["project_id"], "{found}");
    assert_eq!(found["saved_setup"]["version"], 1, "{found}");
}

#[test]
fn an_apply_with_nothing_to_apply_is_refused_without_creating_the_library() {
    let machine = Machine::new();
    let checkout = machine.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();

    let lines = machine.mcp(&[request(
        1,
        "tools/call",
        json!({ "name": "apply_project_setup",
                "arguments": { "checkout_path": checkout, "operation_id": "op-1" } }),
    )]);

    let result = &reply(&lines, 1)["result"];
    assert_eq!(result["isError"], true, "{result}");
    assert!(!machine.base().exists(), "the library was created");
}

#[test]
fn a_message_over_4_mib_is_refused_and_the_stream_goes_on() {
    let machine = Machine::new();
    let padding = "x".repeat(5 << 20);

    let lines = machine.mcp(&[
        request(1, "ping", json!({ "padding": padding })),
        request(2, "ping", json!({})),
    ]);

    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["error"]["code"], -32600, "{}", lines[0]);
    assert_eq!(reply(&lines, 2)["result"], json!({}));
}
