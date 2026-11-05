//! The `--json` reports agents parse, as `skills/manage-skills/SKILL.md`
//! documents them. These pin the shape — top-level keys, value types, and the
//! stable `code` on failures — not the values, so they survive data changes.
//! Each test gets its own temp base dir and temp HOME, so nothing here reads
//! or writes this machine's library or agent folders. Unix only, like
//! `serve_stdio.rs`: on Windows the home folder does not come from HOME.
#![cfg(unix)]

use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");

struct Sandbox(TempDir);

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        Sandbox(tmp)
    }

    /// A library holding one local skill, `demo`, tagged `web` and a member
    /// of the preset `Web`, set up through the CLI's own commands.
    fn seeded() -> Self {
        let sandbox = Self::new();
        let source = sandbox.0.path().join("source/demo");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: demo\ndescription: A demo skill\n---\nbody\n",
        )
        .unwrap();
        sandbox.ok(&["skills", "install", source.to_str().unwrap()]);
        sandbox.ok(&["presets", "create", "Web"]);
        sandbox.ok(&["presets", "add-skill", "Web", "demo"]);
        sandbox.ok(&["skills", "tag", "add", "demo", "web"]);
        sandbox
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(CLI)
            .arg("--json")
            .arg("--base-dir")
            .arg(self.0.path().join("base"))
            .args(args)
            .env("HOME", self.0.path().join("home"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .output()
            .unwrap()
    }

    /// The report on stdout of a command that must succeed.
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The exit code and the envelope on stderr of a command that must fail.
    fn fail(&self, args: &[&str]) -> (i32, Value) {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        (
            out.status.code().unwrap(),
            serde_json::from_slice(&out.stderr).unwrap(),
        )
    }
}

/// Assert `value` is an object carrying each key with a value of the given
/// JSON type: "string", "bool", "number", "array", "object", or "string?"
/// for a string that may be null.
fn assert_shape(value: &Value, fields: &[(&str, &str)]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"));
    for (key, kind) in fields {
        let field = object
            .get(*key)
            .unwrap_or_else(|| panic!("missing `{key}` in {value}"));
        let matches = match *kind {
            "string" => field.is_string(),
            "string?" => field.is_string() || field.is_null(),
            "bool" => field.is_boolean(),
            "number" => field.is_number(),
            "array" => field.is_array(),
            "object" => field.is_object(),
            other => panic!("unknown kind {other}"),
        };
        assert!(matches, "`{key}` is not {kind} in {value}");
    }
}

const SKILL_FIELDS: &[(&str, &str)] = &[
    ("id", "string"),
    ("name", "string"),
    ("description", "string?"),
    ("path", "string"),
    ("enabled", "bool"),
    ("tags", "array"),
    ("source_type", "string"),
    ("source_ref", "string?"),
    ("preset_ids", "array"),
    ("presets", "array"),
    ("deployed_to", "array"),
];

const PRESET_FIELDS: &[(&str, &str)] = &[
    ("id", "string"),
    ("name", "string"),
    ("description", "string?"),
    ("skill_count", "number"),
    ("active", "bool"),
];

#[test]
fn skills_list_reports_each_skill_and_filters() {
    let sandbox = Sandbox::seeded();

    let all = sandbox.ok(&["skills", "list"]);
    let skills = all.as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_shape(&skills[0], SKILL_FIELDS);
    assert_eq!(skills[0]["presets"][0], "Web");
    assert_eq!(skills[0]["tags"][0], "web");

    let tagged = sandbox.ok(&["skills", "list", "--tag", "web"]);
    assert_eq!(tagged.as_array().unwrap().len(), 1);
    let other = sandbox.ok(&["skills", "list", "--tag", "other"]);
    assert_eq!(other, Value::Array(vec![]));
}

#[test]
fn skills_status_adds_per_agent_deployment_rows() {
    let sandbox = Sandbox::seeded();

    let status = sandbox.ok(&["skills", "status", "demo"]);
    assert_shape(&status, SKILL_FIELDS);
    assert_shape(&status, &[("agents", "array")]);
    let agents = status["agents"].as_array().unwrap();
    assert!(!agents.is_empty());
    assert_shape(
        &agents[0],
        &[
            ("key", "string"),
            ("display_name", "string"),
            ("installed", "bool"),
            ("globally_enabled", "bool"),
            ("deployed", "bool"),
            ("target_path", "string?"),
        ],
    );
}

#[test]
fn presets_status_reports_the_preset_and_its_agents() {
    let sandbox = Sandbox::seeded();

    let status = sandbox.ok(&["presets", "status", "Web", "--agent", "claude_code"]);
    assert_shape(&status, &[("preset", "object"), ("agents", "array")]);
    assert_shape(&status["preset"], PRESET_FIELDS);
    assert_shape(
        &status["agents"][0],
        &[
            ("key", "string"),
            ("display_name", "string"),
            ("deployed", "number"),
            ("total", "number"),
            ("status", "string"),
        ],
    );
}

#[test]
fn repo_status_and_agents_list_answer_the_health_check() {
    let sandbox = Sandbox::seeded();

    let repo = sandbox.ok(&["repo", "status"]);
    assert_shape(
        &repo,
        &[
            ("base_dir", "string"),
            ("skills_dir", "string"),
            ("db_path", "string"),
            ("skill_count", "number"),
            ("preset_count", "number"),
            ("active_preset_id", "string?"),
        ],
    );

    let agents = sandbox.ok(&["agents", "list"]);
    let agents = agents.as_array().unwrap();
    assert!(!agents.is_empty());
    assert_shape(
        &agents[0],
        &[
            ("key", "string"),
            ("display_name", "string"),
            ("installed", "bool"),
            ("skills_dir", "string"),
            ("enabled", "bool"),
            ("is_custom", "bool"),
        ],
    );
}

#[test]
fn failures_carry_a_stable_code_on_stderr() {
    let sandbox = Sandbox::new();
    let envelope = [
        ("ok", "bool"),
        ("code", "string"),
        ("message", "string"),
        ("error", "string"),
    ];

    let (exit, missing) = sandbox.fail(&["skills", "status", "no-such-skill"]);
    assert_eq!(exit, 1);
    assert_shape(&missing, &envelope);
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["code"], "COMMAND_FAILED");

    let (exit, unknown) = sandbox.fail(&["no-such-command"]);
    assert_eq!(exit, 2);
    assert_shape(&unknown, &envelope);
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["code"], "INVALID_ARGUMENT");
}

impl Sandbox {
    fn path(&self, relative: &str) -> std::path::PathBuf {
        self.0.path().join(relative)
    }

    /// A body file, for the `--body-file` flag.
    fn body(&self, text: &str) -> String {
        let path = self.path("body.md");
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn create(&self, name: &str, extra: &[&str]) -> Value {
        let body = self.body("Do the thing.\n");
        let mut args = vec![
            "skills",
            "create",
            name,
            "--description",
            "A demo skill",
            "--body-file",
            &body,
        ];
        args.extend_from_slice(extra);
        self.ok(&args)
    }

    /// Makes Claude Code count as installed, and returns its skills folder.
    fn install_claude_code(&self) -> std::path::PathBuf {
        let dir = self.path("home/.claude");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("skills")
    }

    fn library_names(&self) -> Vec<String> {
        self.ok(&["skills", "list"])
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill["name"].as_str().unwrap().to_owned())
            .collect()
    }

    fn skill_md(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(&format!("base/skills/{name}/SKILL.md"))).unwrap()
    }
}

#[test]
fn skills_create_writes_a_library_skill_with_no_source() {
    let sandbox = Sandbox::new();
    let report = sandbox.create("release-notes", &[]);
    assert_shape(
        &report,
        &[
            ("skill_id", "string"),
            ("name", "string"),
            ("path", "string"),
            ("source_type", "string"),
            ("dry_run", "bool"),
            ("targets", "array"),
            ("partial", "bool"),
        ],
    );
    assert_eq!(report["partial"], false);

    let listed = sandbox.ok(&["skills", "list"]);
    assert_eq!(listed[0]["name"], "release-notes");
    assert_eq!(listed[0]["description"], "A demo skill");
    assert_eq!(listed[0]["source_type"], "local");
    assert_eq!(listed[0]["source_ref"], Value::Null);
    assert!(sandbox.skill_md("release-notes").contains("Do the thing."));
    assert_eq!(
        sandbox.ok(&["skills", "show", "release-notes"])["name"],
        "release-notes"
    );
}

#[test]
fn skills_create_keeps_a_description_with_yaml_syntax_intact() {
    let sandbox = Sandbox::new();
    let description = "Use when: \"quoted\" and 'single' --- dashes\nand a second line";
    let body = sandbox.body("Body\n");
    sandbox.ok(&[
        "skills",
        "create",
        "tricky",
        "--description",
        description,
        "--body-file",
        &body,
    ]);
    assert_eq!(
        sandbox.ok(&["skills", "list"])[0]["description"],
        description
    );
}

#[test]
fn skills_create_reads_the_body_from_stdin_or_a_file_alike() {
    use std::io::Write;
    use std::process::Stdio;

    let sandbox = Sandbox::new();
    let body = "# Steps\n\nDo the thing.\n";
    let file = sandbox.body(body);
    sandbox.ok(&[
        "skills",
        "create",
        "same",
        "--description",
        "Same",
        "--body-file",
        &file,
    ]);
    let from_file = sandbox.skill_md("same");
    sandbox.ok(&["skills", "remove", "same", "--yes"]);

    let mut child = Command::new(CLI)
        .args(["--json", "--base-dir"])
        .arg(sandbox.path("base"))
        .args([
            "skills",
            "create",
            "same",
            "--description",
            "Same",
            "--body-file",
            "-",
        ])
        .env("HOME", sandbox.path("home"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());

    assert_eq!(sandbox.skill_md("same"), from_file);
}

#[test]
fn skills_create_from_a_folder_copies_every_file_and_records_no_source() {
    let sandbox = Sandbox::new();
    let source = sandbox.path("draft");
    std::fs::create_dir_all(source.join("references")).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: multi\ndescription: Has extra files\n---\nBody\n",
    )
    .unwrap();
    std::fs::write(source.join("references/notes.md"), "Notes").unwrap();

    sandbox.ok(&[
        "skills",
        "create",
        "multi",
        "--from",
        source.to_str().unwrap(),
    ]);

    let listed = sandbox.ok(&["skills", "list"]);
    assert_eq!(listed[0]["source_ref"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(sandbox.path("base/skills/multi/references/notes.md")).unwrap(),
        "Notes"
    );
}

#[test]
fn skills_create_from_a_folder_requires_a_matching_name() {
    let sandbox = Sandbox::new();
    let source = sandbox.path("draft");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: other\ndescription: Named differently\n---\nBody\n",
    )
    .unwrap();

    sandbox.fail(&[
        "skills",
        "create",
        "multi",
        "--from",
        source.to_str().unwrap(),
    ]);
    assert!(sandbox.library_names().is_empty());
}

#[test]
fn skills_create_refuses_a_taken_name_and_leaves_the_skill_alone() {
    let sandbox = Sandbox::new();
    sandbox.create("taken", &[]);
    let before = sandbox.skill_md("taken");

    let body = sandbox.body("Replacement\n");
    let (exit, envelope) = sandbox.fail(&[
        "skills",
        "create",
        "taken",
        "--description",
        "Other",
        "--body-file",
        &body,
    ]);

    assert_eq!(exit, 1);
    assert_eq!(envelope["code"], "SKILL_EXISTS");
    assert_eq!(sandbox.skill_md("taken"), before);
    assert_eq!(sandbox.library_names(), ["taken"]);
}

#[test]
fn skills_create_refuses_invalid_names_without_writing() {
    let sandbox = Sandbox::new();
    let body = sandbox.body("Body\n");
    let too_long = "a".repeat(65);
    for name in [
        "Upper",
        "two words",
        "-edge",
        "edge-",
        "dou--ble",
        "../up",
        too_long.as_str(),
    ] {
        let (exit, _) = sandbox.fail(&[
            "skills",
            "create",
            "--description",
            "Demo",
            "--body-file",
            &body,
            "--",
            name,
        ]);
        assert_eq!(exit, 1, "{name} should be refused");
    }
    assert!(sandbox.library_names().is_empty());
}

#[test]
fn skills_create_needs_a_description_and_instructions() {
    let sandbox = Sandbox::new();
    let body = sandbox.body("Body\n");
    let blank = sandbox.path("blank.md");
    std::fs::write(&blank, "  \n").unwrap();

    sandbox.fail(&[
        "skills",
        "create",
        "demo",
        "--description",
        " ",
        "--body-file",
        &body,
    ]);
    sandbox.fail(&[
        "skills",
        "create",
        "demo",
        "--description",
        "Demo",
        "--body-file",
        blank.to_str().unwrap(),
    ]);
    assert!(sandbox.library_names().is_empty());
}

#[test]
fn skills_create_adds_to_an_agent() {
    let sandbox = Sandbox::new();
    let skills_dir = sandbox.install_claude_code();

    let report = sandbox.create("for-agent", &["--agent", "claude_code"]);

    assert_eq!(report["targets"][0]["kind"], "agent");
    assert_eq!(report["targets"][0]["error"], Value::Null);
    assert!(skills_dir.join("for-agent/SKILL.md").exists());
    assert_eq!(
        sandbox.ok(&["skills", "list"])[0]["deployed_to"][0],
        "claude_code"
    );
}

#[test]
fn skills_create_stops_before_writing_when_a_foreign_folder_is_in_the_way() {
    let sandbox = Sandbox::new();
    let skills_dir = sandbox.install_claude_code();
    let foreign = skills_dir.join("blocked");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("mine.txt"), "keep me").unwrap();

    let body = sandbox.body("Body\n");
    let (exit, envelope) = sandbox.fail(&[
        "skills",
        "create",
        "blocked",
        "--description",
        "Demo",
        "--body-file",
        &body,
        "--agent",
        "claude_code",
    ]);

    assert_eq!(exit, 1);
    assert_eq!(envelope["code"], "TARGET_CONFLICT");
    assert_eq!(
        std::fs::read_to_string(foreign.join("mine.txt")).unwrap(),
        "keep me"
    );
    assert!(sandbox.library_names().is_empty());
    assert!(!sandbox.path("base/skills/blocked").exists());
}

#[test]
fn skills_create_refuses_unknown_targets_without_writing() {
    let sandbox = Sandbox::new();
    let body = sandbox.body("Body\n");
    for target in [
        ["--agent", "no-such-agent"],
        ["--preset", "No such preset"],
        ["--project", "no-such-project"],
    ] {
        sandbox.fail(&[
            "skills",
            "create",
            "demo",
            "--description",
            "Demo",
            "--body-file",
            &body,
            target[0],
            target[1],
        ]);
    }
    assert!(sandbox.library_names().is_empty());
}

#[test]
fn skills_create_with_a_preset_changes_membership_only() {
    let sandbox = Sandbox::new();
    let skills_dir = sandbox.install_claude_code();
    sandbox.ok(&["presets", "create", "Web"]);

    sandbox.create("in-preset", &["--preset", "Web"]);

    assert_eq!(sandbox.ok(&["skills", "list"])[0]["presets"][0], "Web");
    assert!(!skills_dir.join("in-preset").exists());
}

#[test]
fn skills_create_dry_run_writes_nothing() {
    let sandbox = Sandbox::new();
    let skills_dir = sandbox.install_claude_code();

    let report = sandbox.create("preview", &["--agent", "claude_code", "--dry-run"]);

    assert_eq!(report["dry_run"], true);
    assert_eq!(report["skill_id"], Value::Null);
    assert_eq!(report["targets"][0]["name"], "claude_code");
    assert!(sandbox.library_names().is_empty());
    assert!(!sandbox.path("base/skills/preview").exists());
    assert!(!skills_dir.join("preview").exists());
}

#[test]
fn skills_create_refuses_paths_that_only_exist_here_with_host() {
    let sandbox = Sandbox::new();
    let (exit, _) = sandbox.fail(&["skills", "create", "demo", "--from", ".", "--host", "h"]);
    assert_eq!(exit, 2);

    let body = sandbox.body("Body\n");
    let (exit, envelope) = sandbox.fail(&[
        "skills",
        "create",
        "demo",
        "--description",
        "Demo",
        "--body-file",
        &body,
        "--project",
        ".",
        "--host",
        "h",
    ]);
    assert_eq!(exit, 1);
    assert!(envelope["message"].as_str().unwrap().contains("--host"));
}

#[test]
fn skills_update_all_skips_a_created_skill_without_an_error() {
    let sandbox = Sandbox::new();
    sandbox.create("no-source", &[]);
    assert_eq!(
        sandbox.ok(&["skills", "update", "--all"]),
        serde_json::json!([])
    );
}
