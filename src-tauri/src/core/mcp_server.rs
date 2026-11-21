//! `skills-manager-cli mcp`: Project setup as a local stdio MCP server, for
//! coding agents such as Codex and Claude Code.
//!
//! Newline-delimited JSON-RPC 2.0 in both directions, answered in order. The
//! server handles `initialize`, `ping`, `tools/list` and `tools/call`; other
//! requests get "method not found" and notifications get no reply. Nothing
//! else may reach stdout. The server exits when stdin closes.
//!
//! Tool names, inputs and output fields are frozen at version 1: add optional
//! fields or new tools, never change what callers rely on.
//!
//! The library opens read-only. The first apply that may write opens it for
//! writing, once.

use std::cell::OnceCell;
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};

use serde_json::{json, Value};

use super::error::AppError;
use super::host::HostCtx;
use super::project_setup;

/// Newest first: a client asking for another version gets the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const DISCOVER: &str = "discover_project_setup";
const APPLY: &str = "apply_project_setup";
const INSPECT: &str = "inspect_project_setup_result";

/// How the server reaches the library: read-only, and writable on demand.
pub struct Library<'a> {
    read_only: &'a HostCtx,
    open_writable: &'a dyn Fn() -> Result<HostCtx, AppError>,
    writable: OnceCell<HostCtx>,
}

impl<'a> Library<'a> {
    pub fn new(
        read_only: &'a HostCtx,
        open_writable: &'a dyn Fn() -> Result<HostCtx, AppError>,
    ) -> Self {
        Self {
            read_only,
            open_writable,
            writable: OnceCell::new(),
        }
    }

    fn writable(&self) -> Result<&HostCtx, AppError> {
        if let Some(ctx) = self.writable.get() {
            return Ok(ctx);
        }
        let ctx = (self.open_writable)()?;
        Ok(self.writable.get_or_init(|| ctx))
    }
}

/// Answer JSON-RPC messages from `reader` on `writer` until `reader` ends.
pub fn run(mut reader: impl BufRead, mut writer: impl Write, ctx: &Library) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(message) => answer(ctx, &message),
            Err(err) => Some(failure(
                Value::Null,
                PARSE_ERROR,
                format!("Parse error: {err}"),
            )),
        };
        if let Some(reply) = reply {
            let mut bytes = serde_json::to_vec(&reply)?;
            bytes.push(b'\n');
            writer.write_all(&bytes)?;
            writer.flush()?;
        }
    }
}

/// The reply to one message, or `None` for a notification or a response.
fn answer(ctx: &Library, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str().unwrap_or_default();
    let params = &message["params"];
    let result = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [discover_tool(), apply_tool(), inspect_tool()] })),
        "tools/call" => call_tool(ctx, params),
        _ => Err((METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => failure(id, code, message),
    })
}

fn failure(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params["protocolVersion"].as_str();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| Some(**v) == asked)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "agents-manager",
            "title": "Agents Manager",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "Pass the active checkout's absolute path to discover_project_setup \
            to read its Project setup, and to apply_project_setup to apply and verify it. \
            After a lost reply or an unclear result, inspect_project_setup_result reads it \
            back without writing. A linked worktree is its own checkout.",
    })
}

/// A tool's own failure is a result the agent reads (`isError`), not a
/// protocol error; only an unknown tool is one.
fn call_tool(ctx: &Library, params: &Value) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().unwrap_or_default();
    let action = match name {
        DISCOVER => "discover",
        APPLY => "apply",
        INSPECT => "inspect",
        _ => return Err((INVALID_PARAMS, format!("Unknown tool: {name}"))),
    };
    let mut request = match &params["arguments"] {
        Value::Object(arguments) => arguments.clone(),
        Value::Null => Default::default(),
        _ => return Err((INVALID_PARAMS, "Tool arguments must be an object".into())),
    };
    request.insert("action".into(), json!(action));
    // Proposing a first setup reads only; confirming it writes.
    let proposes = request.contains_key("items") && !request.contains_key("confirm");
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let ctx = match action {
            "apply" if !proposes => ctx.writable()?,
            _ => ctx.read_only,
        };
        project_setup::dispatch(ctx, Value::Object(request))
    }))
    .unwrap_or_else(|_| Err(AppError::internal(format!("{name} panicked"))));
    Ok(match outcome {
        Ok(found) => json!({
            "content": [{ "type": "text", "text": found.to_string() }],
            "structuredContent": found,
            "isError": false,
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": json!(error).to_string() }],
            "isError": true,
        }),
    })
}

fn discover_tool() -> Value {
    let nullable_object = json!({ "type": ["object", "null"] });
    let project = json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "name": { "type": "string" },
            "path": { "type": "string", "description": "Canonical checkout path." },
            "agent_keys": { "type": ["array", "null"], "items": { "type": "string" } },
            "deploy_mode": { "type": "string", "enum": ["link", "copy"] },
        },
        "required": ["id", "name", "path", "agent_keys", "deploy_mode"],
    });
    json!({
        "name": DISCOVER,
        "title": "Discover Project setup",
        "description": "Read the Project setup for one exact checkout on this computer: \
            its registered project, saved setup, Library options (50 per page), \
            read-only Sync status and conflicts. When the checkout is not registered, \
            up to 10 registered projects that contain it or sit inside it are listed \
            as candidates. Writes nothing.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "checkout_path": {
                    "type": "string",
                    "description": "Absolute path of the active checkout.",
                },
                "cursor": {
                    "type": "string",
                    "description": "library.next_cursor from the previous page.",
                },
            },
            "required": ["checkout_path"],
            "additionalProperties": false,
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "host": { "type": "string" },
                "checkout_path": { "type": "string" },
                "project": {
                    "anyOf": [project, { "type": "null" }],
                    "description": "The project registered at exactly this checkout.",
                },
                "candidates": { "type": "array", "items": project, "maxItems": 10 },
                "saved_setup": nullable_object,
                "library": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "maxItems": 50,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "kind": { "type": "string", "enum": ["skill", "instruction", "mcp"] },
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                },
                                "required": ["kind", "id", "name"],
                            },
                        },
                        "next_cursor": { "type": ["string", "null"] },
                    },
                    "required": ["items", "next_cursor"],
                },
                "sync": {
                    "type": "object",
                    "description": "This host's last recorded Sync state. verified is \
                        false: discovery does not contact the Sync remote.",
                    "properties": {
                        "state": { "type": "string" },
                        "reason": { "type": ["string", "null"] },
                        "remote": { "type": ["string", "null"] },
                        "last_sync_at": { "type": ["integer", "null"] },
                        "checked_at": { "type": "integer" },
                        "verified": { "type": "boolean" },
                    },
                    "required": ["state", "reason", "remote", "last_sync_at", "checked_at", "verified"],
                },
                "conflicts": {
                    "type": "object",
                    "properties": {
                        "skills": { "type": "array" },
                        "resources": { "type": "array" },
                        "same_name_presets": { "type": "array" },
                        "edited_here": { "type": "array" },
                    },
                    "required": ["skills", "resources", "same_name_presets", "edited_here"],
                },
            },
            "required": [
                "host", "checkout_path", "project", "candidates",
                "saved_setup", "library", "sync", "conflicts",
            ],
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

fn apply_tool() -> Value {
    let count = json!({ "type": "integer", "minimum": 0 });
    let kind = json!({ "type": "string", "enum": ["skill", "instruction", "mcp"] });
    let proposal = json!({
        "type": "object",
        "description": "The decisions this request needs, grouped. Show them to \
            the user; send the proposal back as confirm to go ahead.",
        "properties": {
            "checkout_path": { "type": "string" },
            "project_id": { "type": ["string", "null"] },
            "register": { "type": "boolean" },
            "deploy_mode": { "type": "string", "enum": ["link", "copy"] },
            "setup_version": { "type": "integer" },
            "decisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": kind,
                        "id": { "type": "string" },
                        "name": { "type": ["string", "null"] },
                        "agents": { "type": "array", "items": { "type": "string" } },
                        "reason": { "type": "string" },
                        "proposed": {},
                        "choices": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["approve", "skip", "restore", "replace", "revise"],
                            },
                        },
                    },
                    "required": ["kind", "id", "name", "agents", "reason", "proposed", "choices"],
                },
            },
        },
        "required": [
            "checkout_path", "project_id", "register", "deploy_mode",
            "setup_version", "decisions",
        ],
    });
    json!({
        "name": APPLY,
        "title": "Apply Project setup",
        "description": "Apply the saved Project setup to one exact registered checkout on this \
            computer and verify the result, in one call. Absent items are deployed; items \
            that already match are left alone; edited, unapproved or ambiguous items are \
            reported and never overwritten, while independent items continue. Reusing an \
            operation_id returns the recorded result instead of applying again. Per-item \
            results come only with evidence: true. A checkout with no saved setup gets a \
            proposal for the items passed; items that need approval get one too. Send a \
            proposal back as confirm, with a choice on each decision, to register the \
            checkout if needed, save the setup and apply it. A stale proposal is refused.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "checkout_path": {
                    "type": "string",
                    "description": "Absolute path of the active checkout.",
                },
                "operation_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "description": "A new ID per intended change, reused only to retry it.",
                },
                "evidence": {
                    "type": "boolean",
                    "description": "Include the per-item results.",
                },
                "items": {
                    "type": "array",
                    "description": "First setup only: the Library items and agents to propose.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": kind,
                            "id": { "type": "string" },
                            "agents": { "type": "array", "items": { "type": "string" } },
                        },
                        "required": ["kind", "id", "agents"],
                    },
                },
                "confirm": {
                    "type": "object",
                    "description": "A proposal returned earlier, with choice set on each \
                        decision (and replacement, a Library ID, for replace).",
                },
            },
            "required": ["checkout_path", "operation_id"],
            "additionalProperties": false,
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "operation_id": { "type": "string" },
                "host": { "type": "string" },
                "checkout_path": { "type": "string" },
                "project_id": {
                    "type": ["string", "null"],
                    "description": "Null until a first setup is confirmed.",
                },
                "setup_version": { "type": "integer" },
                "outcome": {
                    "type": "string",
                    "enum": [
                        "verified_changed", "verified_no_change", "attention_needed",
                        "outcome_unknown", "confirmation_needed",
                    ],
                },
                "agents": { "type": "array", "items": { "type": "string" } },
                "counts": {
                    "type": "object",
                    "properties": {
                        "changed": count, "unchanged": count, "needs_review": count,
                        "blocked": count, "missing": count, "failed": count,
                    },
                    "required": ["changed", "unchanged", "needs_review", "blocked", "missing", "failed"],
                },
                "next_action": { "type": "string" },
                "notes": { "type": "array", "items": { "type": "string" } },
                "from_receipt": {
                    "type": "boolean",
                    "description": "Answered from this operation_id's receipt; nothing was applied.",
                },
                "proposal": proposal,
                "items": {
                    "type": "array",
                    "description": "Only with evidence: true.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": kind,
                            "id": { "type": "string" },
                            "agents": { "type": "array", "items": { "type": "string" } },
                            "status": {
                                "type": "string",
                                "enum": ["changed", "unchanged", "needs_review", "blocked", "missing", "failed"],
                            },
                            "reason": { "type": "string" },
                        },
                        "required": ["kind", "id", "agents", "status"],
                    },
                },
            },
            "required": [
                "operation_id", "host", "checkout_path", "project_id", "setup_version",
                "outcome", "agents", "counts", "next_action", "notes", "from_receipt",
            ],
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

fn inspect_tool() -> Value {
    let count = json!({ "type": "integer", "minimum": 0 });
    json!({
        "name": INSPECT,
        "title": "Inspect Project setup result",
        "description": "Read back an apply_project_setup operation on this computer without \
            writing or resubmitting: its recorded result, and the checkout's current state \
            against its saved setup. A missing receipt, or one with no recorded result, is \
            outcome_unknown; a matching checkout is reported as verified state, not as this \
            operation's change. Per-item results come only with evidence: true.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "checkout_path": {
                    "type": "string",
                    "description": "Absolute path of the checkout the operation targeted.",
                },
                "operation_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "description": "The operation_id passed to apply_project_setup.",
                },
                "evidence": {
                    "type": "boolean",
                    "description": "Include the recorded and current per-item results.",
                },
            },
            "required": ["checkout_path", "operation_id"],
            "additionalProperties": false,
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "operation_id": { "type": "string" },
                "host": { "type": "string" },
                "checkout_path": { "type": "string" },
                "project_id": { "type": "string" },
                "outcome": {
                    "type": "string",
                    "enum": ["verified_changed", "verified_no_change", "attention_needed", "outcome_unknown"],
                },
                "receipt": {
                    "type": "string",
                    "enum": ["result", "intent_only", "missing"],
                    "description": "What this host recorded for the operation_id.",
                },
                "recorded": {
                    "type": ["object", "null"],
                    "description": "The recorded apply result, when there is one.",
                },
                "destination": {
                    "type": ["object", "null"],
                    "description": "The checkout's current state against its saved setup; \
                        null when there is no saved setup.",
                    "properties": {
                        "setup_version": { "type": "integer" },
                        "state": { "type": "string", "enum": ["matches", "differs"] },
                        "counts": {
                            "type": "object",
                            "properties": {
                                "unchanged": count, "not_applied": count, "needs_review": count,
                                "blocked": count, "missing": count, "failed": count,
                            },
                            "required": ["unchanged", "not_applied", "needs_review", "blocked", "missing", "failed"],
                        },
                        "items": {
                            "type": "array",
                            "description": "Only with evidence: true.",
                        },
                    },
                    "required": ["setup_version", "state", "counts"],
                },
                "recovery_possible": {
                    "type": "boolean",
                    "description": "Some items are not applied, and apply under a new \
                        operation_id would set them up.",
                },
                "next_action": { "type": "string" },
            },
            "required": [
                "operation_id", "host", "checkout_path", "project_id", "outcome", "receipt",
                "recorded", "destination", "recovery_possible", "next_action",
            ],
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}
