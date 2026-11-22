//! Project setup routed to a registered SSH host. The MCP server runs in
//! process with a stand-in for `ssh`: each "host" is the real CLI's
//! `serve --stdio` on its own temp base dir and HOME, so nothing here
//! contacts a real host or this machine's library. Unix only, like the
//! remote hosts themselves.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use app_lib::core::error::AppError;
use app_lib::core::host::{HostCtx, NoopEvents};
use app_lib::core::mcp_server::{self, Server};
use app_lib::core::project_setup;
use app_lib::core::remote_session::RemoteSession;
use app_lib::core::skill_store::{ProjectRecord, RemoteHostRecord, SkillStore};
use serde_json::{json, Value};
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");
const HOST: &str = "build box";

fn project(id: &str, path: &Path) -> ProjectRecord {
    ProjectRecord {
        id: id.into(),
        name: "app".into(),
        path: path.to_string_lossy().into_owned(),
        workspace_type: "project".into(),
        linked_agent_key: None,
        linked_agent_name: None,
        disabled_path: None,
        sort_order: 0,
        created_at: 0,
        updated_at: 0,
        agent_keys: None,
        deploy_mode: "link".into(),
    }
}

/// A machine with its own base dir, HOME and an `app` checkout.
struct Machine {
    tmp: TempDir,
}

impl Machine {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("home/.claude")).unwrap();
        std::fs::create_dir_all(tmp.path().join("work/app")).unwrap();
        Self { tmp }
    }

    fn base(&self) -> PathBuf {
        self.tmp.path().join("base")
    }

    fn checkout(&self) -> PathBuf {
        self.tmp.path().join("work/app").canonicalize().unwrap()
    }

    fn store(&self) -> SkillStore {
        std::fs::create_dir_all(self.base()).unwrap();
        SkillStore::new(&self.base().join("skills-manager.db")).unwrap()
    }

    /// The real CLI on this machine, whatever host it is asked for.
    fn cli(&self) -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
        let (base, home) = (self.base(), self.tmp.path().join("home"));
        move |_: &RemoteHostRecord, args: &[&str]| {
            let mut cmd = Command::new(CLI);
            cmd.arg("--base-dir")
                .arg(&base)
                .args(args)
                .env("HOME", &home)
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("XDG_DATA_HOME");
            cmd
        }
    }

    /// One app command over this machine's own `serve --stdio`.
    fn call(&self, cmd: &str, request: Value) -> Value {
        let session =
            RemoteSession::spawn(&self.cli(), &host_record(), Arc::new(NoopEvents)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(session.call(cmd, json!({ "request": request })))
            .unwrap()
    }

    /// A registered `app` project whose saved setup holds one Skill, one
    /// Instruction bundle and one MCP connection, all for Claude Code.
    fn one_of_each(&self) {
        let skill = self.call(
            "create_skill",
            json!({ "name": "review",
                "content": { "kind": "written", "description": "Demo", "instructions": "Do it." } }),
        );
        let skill_id = skill["skill_id"].clone();
        let bundle = self.call(
            "instructions_request",
            json!({ "action": "save", "name": "Rules", "files": { "CLAUDE.md": "Be brief.\n" } }),
        );
        let mcp = self.call(
            "mcps_request",
            json!({ "action": "save", "definition": { "name": "docs", "transport": "stdio",
                "server": { "command": "npx", "args": ["-y", "docs"] } } }),
        );
        let definition = &mcp["definition"];
        let store = self.store();
        store
            .insert_project(&project("p1", &self.checkout()))
            .unwrap();
        let items = json!([
            { "kind": "skill", "id": skill_id, "agents": ["claude_code"] },
            { "kind": "instruction", "id": bundle["item"]["id"], "agents": ["claude_code"] },
            { "kind": "mcp", "id": definition["id"], "agents": ["claude_code"],
              "approved": { "claude_code": project_setup::mcp_value(definition) } },
        ]);
        project_setup::save_setup(&store, "p1", 0, serde_json::from_value(items).unwrap()).unwrap();
    }
}

fn host_record() -> RemoteHostRecord {
    RemoteHostRecord {
        id: "h1".into(),
        name: HOST.into(),
        ssh_target: "me@build".into(),
        cli_path: None,
        created_at: 0,
    }
}

/// This computer: a library with `build box` registered as an SSH host.
struct Local {
    machine: Machine,
    ctx: HostCtx,
}

impl Local {
    fn new() -> Self {
        let machine = Machine::new();
        let store = machine.store();
        store.insert_remote_host(&host_record()).unwrap();
        Self {
            ctx: HostCtx::headless(Arc::new(store)),
            machine,
        }
    }

    /// Answer `messages` the way `cli mcp` does, reaching hosts through `cli`.
    fn mcp(
        &self,
        cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
        messages: &[Value],
    ) -> Vec<Value> {
        run(
            &Server::new(&|| Ok(self.ctx.clone()), &refuse_writes, &cli),
            messages,
        )
    }

    /// One tool call; its result.
    fn tool(
        &self,
        cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
        name: &str,
        arguments: Value,
    ) -> Value {
        self.mcp(cli, &[tool_call(1, name, arguments)])[0]["result"].clone()
    }
}

fn refuse_writes() -> Result<HostCtx, AppError> {
    panic!("a routed request opened the local library")
}

fn tool_call(id: u64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": arguments } })
}

/// Answer `messages` on `library`, the way `cli mcp` does.
fn run(server: &Server, messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let mut output = Vec::new();
    mcp_server::run(input.as_bytes(), &mut output, server).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_named_host_applies_and_verifies_there_and_leaves_a_same_name_project_here_alone() {
    let local = Local::new();
    let here = local.machine.checkout();
    local
        .ctx
        .store
        .insert_project(&project("local-app", &here))
        .unwrap();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();

    let started = Instant::now();
    let applied = local.tool(
        remote.cli(),
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
    );
    eprintln!("SSH apply took {:?}", started.elapsed());

    assert_eq!(applied["isError"], false, "{applied}");
    let applied = &applied["structuredContent"];
    assert_eq!(applied["outcome"], "verified_changed", "{applied}");
    assert_eq!(applied["counts"]["changed"], 3, "{applied}");
    assert!(checkout.join(".claude/skills/review/SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(checkout.join("CLAUDE.md")).unwrap(),
        "Be brief.\n"
    );
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(checkout.join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(config["mcpServers"]["docs"]["command"], "npx");

    let inspected = local.tool(
        remote.cli(),
        "inspect_project_setup_result",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
    );
    let inspected = &inspected["structuredContent"];
    assert_eq!(inspected["receipt"], "result", "{inspected}");
    assert_eq!(inspected["destination"]["state"], "matches");

    assert_eq!(std::fs::read_dir(&here).unwrap().count(), 0);
    assert_eq!(
        local
            .ctx
            .store
            .resource_state_get("project_setup_receipt", "op-1")
            .unwrap(),
        None
    );
}

/// A stand-in `ssh` that runs `script` with `$1` set to `hello`, and records
/// the CLI arguments it is asked for.
fn fake_host(
    script: &'static str,
    hello: String,
    asked: Arc<Mutex<Vec<Vec<String>>>>,
) -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
    move |_: &RemoteHostRecord, args: &[&str]| {
        asked
            .lock()
            .unwrap()
            .push(args.iter().map(|arg| arg.to_string()).collect());
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script, "sh", &hello]);
        cmd
    }
}

fn hello(version: &str) -> String {
    json!({ "hello": {
        "protocol": app_lib::core::serve::PROTOCOL, "version": version, "os": "linux",
        "arch": "x86_64", "home": "/home/me", "base_dir": "/home/me/.skills-manager", "pid": 1,
    }})
    .to_string()
}

fn error_text(result: &Value) -> String {
    assert_eq!(result["isError"], true, "{result}");
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

#[test]
fn a_host_on_another_version_is_a_blocker_and_nothing_is_installed() {
    let local = Local::new();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let host = fake_host(
        r#"printf '%s\n' "$1"; cat >/dev/null"#,
        hello("0.1.0"),
        asked.clone(),
    );

    let result = local.tool(
        host,
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": "/srv/app", "operation_id": "op-1" }),
    );

    let error: Value = serde_json::from_str(&error_text(&result)).unwrap();
    assert_eq!(error["kind"], "remote_version_mismatch", "{error}");
    let message = error["message"].as_str().unwrap();
    for expected in [
        HOST,
        "0.1.0",
        env!("CARGO_PKG_VERSION"),
        "Agents Manager app",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }
    // Only the one start was asked for: no install, no update, no retry.
    assert_eq!(*asked.lock().unwrap(), vec![vec!["serve", "--stdio"]]);
}

#[test]
fn an_unreachable_host_is_reported_as_unreachable() {
    let local = Local::new();
    let host = fake_host(
        "echo 'ssh: connect to host build port 22: Connection refused' >&2; exit 255",
        String::new(),
        Arc::default(),
    );

    let result = local.tool(
        host,
        "discover_project_setup",
        json!({ "host": HOST, "checkout_path": "/srv/app" }),
    );

    let error: Value = serde_json::from_str(&error_text(&result)).unwrap();
    assert_eq!(error["kind"], "network", "{error}");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .starts_with("build box is unreachable"));
}

#[test]
fn a_host_name_must_pick_exactly_one_registered_host() {
    let local = Local::new();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let call = |name: &str| {
        local.tool(
            fake_host("exit 1", String::new(), asked.clone()),
            "discover_project_setup",
            json!({ "host": name, "checkout_path": "/srv/app" }),
        )
    };

    assert!(error_text(&call("gpu box")).contains("No SSH host named gpu box"));
    let twin = RemoteHostRecord {
        id: "h2".into(),
        ..host_record()
    };
    local.ctx.store.insert_remote_host(&twin).unwrap();
    assert!(error_text(&call(HOST)).contains("2 SSH hosts are named build box"));
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn a_host_whose_connection_details_change_is_reached_at_its_new_address() {
    let local = Local::new();
    let (old, new) = (Machine::new(), Machine::new());
    new.one_of_each();
    let (old_cli, new_cli) = (old.cli(), new.cli());
    let cli = move |host: &RemoteHostRecord, args: &[&str]| match host.ssh_target.as_str() {
        "me@build" => old_cli(host, args),
        _ => new_cli(host, args),
    };
    let read_only = || Ok(local.ctx.clone());
    let server = Server::new(&read_only, &refuse_writes, &cli);
    let discover = |checkout: &Path| {
        let found = run(
            &server,
            &[tool_call(
                1,
                "discover_project_setup",
                json!({ "host": HOST, "checkout_path": checkout }),
            )],
        );
        found[0]["result"]["structuredContent"].clone()
    };
    assert_eq!(discover(&old.checkout())["project"], Value::Null);

    local
        .ctx
        .store
        .update_remote_host("h1", HOST, "me@new-build", None)
        .unwrap();

    let found = discover(&new.checkout());
    assert_eq!(found["project"]["id"], "p1", "{found}");
}

/// `remote` reached through `tests/fixtures/lost_response.py` the first time,
/// which loses the first request (`before_admission`) or its reply
/// (`after_admission`), and directly after that. Counts the starts.
fn losing_once(
    remote: &Machine,
    mode: &'static str,
    starts: Arc<AtomicUsize>,
) -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
    let direct = remote.cli();
    let (base, home) = (remote.base(), remote.tmp.path().join("home"));
    // Released up front: the fixture closes its server as soon as it is done.
    let release = remote.tmp.path().join("release");
    std::fs::write(&release, "").unwrap();
    move |host: &RemoteHostRecord, args: &[&str]| {
        if starts.fetch_add(1, Ordering::SeqCst) > 0 {
            return direct(host, args);
        }
        let mut cmd = Command::new("python3");
        cmd.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lost_response.py"
        ))
        .args([Path::new(CLI), &base])
        .arg(mode)
        .arg(&release)
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME");
        cmd
    }
}

fn apply_on_host(
    local: &Local,
    cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
    checkout: &Path,
) -> Value {
    local.tool(
        cli,
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
    )
}

#[test]
fn a_reply_lost_after_commit_reconciles_from_the_receipt_without_writing_again() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let starts = Arc::new(AtomicUsize::new(0));

    let result = apply_on_host(
        &local,
        losing_once(&remote, "after_admission", starts.clone()),
        &remote.checkout(),
    );

    let result = &result["structuredContent"];
    assert_eq!(result["outcome"], "verified_changed", "{result}");
    assert_eq!(result["from_receipt"], true, "{result}");
    assert_eq!(result["counts"]["changed"], 3);
    assert_eq!(starts.load(Ordering::SeqCst), 2, "one reconnect, no more");
}

#[test]
fn a_lost_reply_to_a_proposal_is_asked_for_again_once() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let saved = remote
        .store()
        .resource_state_get("project_setup", "p1")
        .unwrap()
        .unwrap();
    let items: Vec<Value> = saved["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| json!({ "kind": item["kind"], "id": item["id"], "agents": item["agents"] }))
        .collect();
    let fresh = remote.tmp.path().join("work/fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    let starts = Arc::new(AtomicUsize::new(0));

    let result = local.tool(
        losing_once(&remote, "after_admission", starts.clone()),
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": fresh, "operation_id": "op-1", "items": items }),
    );

    let proposed = &result["structuredContent"];
    assert_eq!(proposed["outcome"], "confirmation_needed", "{result}");
    assert_eq!(starts.load(Ordering::SeqCst), 2, "one reconnect, no more");
}

#[test]
fn a_request_lost_with_no_receipt_is_outcome_unknown_and_never_replayed() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let starts = Arc::new(AtomicUsize::new(0));

    let result = apply_on_host(
        &local,
        losing_once(&remote, "before_admission", starts.clone()),
        &remote.checkout(),
    );

    // A missing receipt is not proof that nothing ran, so apply is not sent again.
    let text = error_text(&result);
    assert!(text.contains("Outcome unknown"), "{result}");
    assert!(text.contains("Inspect again later"), "{result}");
    assert!(!remote.checkout().join(".mcp.json").exists());
    assert_eq!(starts.load(Ordering::SeqCst), 2, "one reconnect, no more");
}

#[test]
fn a_lost_request_another_claimed_is_outcome_unknown_and_not_resumed() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    let probe = local.tool(
        remote.cli(),
        "inspect_project_setup_result",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
    );
    let intent = json!({ "host": probe["structuredContent"]["host"], "checkout_path": checkout });
    remote
        .store()
        .resource_state_put(
            "project_setup_receipt",
            "op-1",
            &json!({ "version": 1, "intent": intent }),
        )
        .unwrap();
    let starts = Arc::new(AtomicUsize::new(0));

    let result = apply_on_host(
        &local,
        losing_once(&remote, "before_admission", starts.clone()),
        &checkout,
    );

    assert!(error_text(&result).contains("Outcome unknown"), "{result}");
    assert!(!checkout.join(".mcp.json").exists());
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn two_simultaneous_calls_with_the_same_operation_id_apply_once() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    let cli = remote.cli();
    let (warming, barrier) = (Mutex::new(()), std::sync::Barrier::new(2));
    let call = |id, name, arguments: Value| tool_call(id, name, arguments);

    let results: Vec<Value> = std::thread::scope(|scope| {
        let calls: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let read_only = || Ok(local.ctx.clone());
    let server = Server::new(&read_only, &refuse_writes, &cli);
                    // Two `serve` startups on one library can collide; this
                    // test races the applies, so each session starts first.
                    let started = warming.lock().unwrap();
                    run(&server, &[call(1, "discover_project_setup",
                        json!({ "host": HOST, "checkout_path": checkout }))]);
                    drop(started);
                    barrier.wait();
                    run(&server, &[call(2, "apply_project_setup",
                        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }))])
                        .remove(0)["result"]
                        .take()
                })
            })
            .collect();
        calls.into_iter().map(|call| call.join().unwrap()).collect()
    });

    let applied: Vec<&Value> = results
        .iter()
        .map(|result| &result["structuredContent"])
        .filter(|result| result["from_receipt"] == false)
        .collect();
    assert_eq!(applied.len(), 1, "{results:?}");
    assert_eq!(applied[0]["outcome"], "verified_changed", "{}", applied[0]);
    for result in &results {
        let outcome = &result["structuredContent"]["outcome"];
        assert!(
            outcome == "verified_changed" || outcome == "outcome_unknown",
            "{result}"
        );
    }
}

/// The rows that hold projects, agents, the Library and preset membership,
/// and every path in the checkout.
fn state(machine: &Machine) -> Vec<String> {
    let db = rusqlite::Connection::open_with_flags(
        machine.base().join("skills-manager.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut rows = Vec::new();
    for table in [
        "projects",
        "project_skill_agents",
        "skills",
        "skill_targets",
        "scenarios",
        "scenario_skills",
        "scenario_skill_tools",
        "settings",
        "resource_state",
    ] {
        let mut query = db.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns = query.column_count();
        let mut found = query.query([]).unwrap();
        while let Some(row) = found.next().unwrap() {
            let values: Vec<String> = (0..columns)
                .map(|i| match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(text) => String::from_utf8_lossy(text).into(),
                    other => format!("{other:?}"),
                })
                .collect();
            rows.push(format!("{table}: {}", values.join(" | ")));
        }
    }
    rows.sort();
    let mut pending = vec![machine.checkout()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() && !path.is_symlink() {
                pending.push(path.clone());
            }
            rows.push(path.to_string_lossy().into_owned());
        }
    }
    rows
}

#[test]
fn discovery_and_inspection_on_a_host_change_nothing_after_serve_starts() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    apply_on_host(&local, remote.cli(), &checkout);
    let cli = remote.cli();
    let read_only = || Ok(local.ctx.clone());
    let server = Server::new(&read_only, &refuse_writes, &cli);
    let discover = |id| {
        tool_call(
            id,
            "discover_project_setup",
            json!({ "host": HOST, "checkout_path": checkout }),
        )
    };
    // The first call starts `serve`, which may refresh the library.
    run(&server, &[discover(1)]);
    let before = state(&remote);

    let lines = run(
        &server,
        &[
            discover(2),
            tool_call(
                3,
                "inspect_project_setup_result",
                json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
            ),
        ],
    );

    let (discovered, inspected) = (&lines[0]["result"], &lines[1]["result"]);
    assert_eq!(
        discovered["structuredContent"]["project"]["id"], "p1",
        "{discovered}"
    );
    assert_eq!(
        inspected["structuredContent"]["destination"]["state"], "matches",
        "{inspected}"
    );
    assert_eq!(state(&remote), before);
}

#[test]
fn a_lost_request_reusing_an_operation_id_never_returns_the_other_requests_result() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    let first = apply_on_host(&local, remote.cli(), &checkout);
    assert_eq!(first["structuredContent"]["outcome"], "verified_changed", "{first}");
    let starts = Arc::new(AtomicUsize::new(0));

    // The same operation_id, for this run's agents only: a different intent.
    let result = local.tool(
        losing_once(&remote, "before_admission", starts.clone()),
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1",
                "agents": ["claude_code"] }),
    );

    let text = error_text(&result);
    assert!(text.contains("different request"), "{result}");
}

#[test]
fn a_lost_apply_with_null_items_is_never_sent_again() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    let starts = Arc::new(AtomicUsize::new(0));

    // Core reads null items as no items: an apply of the saved setup.
    let result = local.tool(
        losing_once(&remote, "before_admission", starts.clone()),
        "apply_project_setup",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1",
                "items": null }),
    );

    assert!(error_text(&result).contains("Outcome unknown"), "{result}");
    assert!(!checkout.join(".mcp.json").exists());
}

#[test]
fn a_lost_request_whose_receipt_recorded_an_error_returns_that_error() {
    let local = Local::new();
    let remote = Machine::new();
    remote.one_of_each();
    let checkout = remote.checkout();
    let probe = local.tool(
        remote.cli(),
        "inspect_project_setup_result",
        json!({ "host": HOST, "checkout_path": checkout, "operation_id": "op-1" }),
    );
    let intent = json!({ "host": probe["structuredContent"]["host"], "checkout_path": checkout });
    let error = AppError::target_conflict("taken", Vec::new());
    remote
        .store()
        .resource_state_put(
            "project_setup_receipt",
            "op-1",
            &json!({ "version": 2, "intent": intent, "error": error, "settled_at": 1 }),
        )
        .unwrap();
    let starts = Arc::new(AtomicUsize::new(0));

    let result = apply_on_host(
        &local,
        losing_once(&remote, "before_admission", starts.clone()),
        &checkout,
    );

    let text = error_text(&result);
    assert!(text.contains("taken"), "{result}");
    assert!(!text.contains("Outcome unknown"), "{result}");
}
