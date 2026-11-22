//! `skills-manager-cli mcp`: Project setup as a local stdio MCP server, for
//! coding agents such as Codex and Claude Code.
//!
//! Newline-delimited JSON-RPC 2.0 in both directions, answered in order. The
//! server handles `initialize`, `ping`, `tools/list` and `tools/call`; other
//! requests get "method not found", a batch gets one "invalid request" error,
//! and notifications get no reply. Nothing else may reach stdout. The server
//! exits when stdin closes.
//!
//! Tool names, inputs and output fields are frozen at version 1: add optional
//! fields or new tools, never change what callers rely on.
//!
//! Each call reads through a fresh read-only connection, so a library created
//! or changed after the server started is seen. The first apply that may
//! write opens the library for writing, once, after the request is checked
//! read-only. A request naming a `host` runs on that registered SSH host
//! instead; see [`remote`].
//!
//! A message longer than [`MAX_FRAME`] is refused before it is read whole.

use std::cell::OnceCell;
use std::io::{self, BufRead, Read, Write};
use std::panic::{self, AssertUnwindSafe};

use serde_json::{json, Value};

use super::error::AppError;
use super::host::HostCtx;
use super::remote_session::CliCommand;
use super::{mcps, project_setup};

mod remote;

/// Newest first: a client asking for another version gets the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The longest message accepted, in bytes, without its newline.
pub const MAX_FRAME: usize = 4 << 20;

const DISCOVER: &str = "discover_project_setup";
const APPLY: &str = "apply_project_setup";
const INSPECT: &str = "inspect_project_setup_result";

const HOST_DESCRIPTION: &str = "The name of a registered SSH host to run on instead of this \
    computer; checkout_path is then a path there. An unreachable host is a tool error, \
    unlike a reachable host whose Sync is offline. A host on another version is refused, \
    never installed or updated.";

/// What the server answers from: this computer's library, opened read-only
/// for each call and writable on demand, and registered SSH hosts, through
/// `cli` ([`super::remote_host::cli_command`] outside tests).
pub struct Server<'a> {
    open_read_only: &'a dyn Fn() -> Result<HostCtx, AppError>,
    open_writable: &'a dyn Fn() -> Result<HostCtx, AppError>,
    writable: OnceCell<HostCtx>,
    hosts: remote::Hosts<'a>,
}

impl<'a> Server<'a> {
    pub fn new(
        open_read_only: &'a dyn Fn() -> Result<HostCtx, AppError>,
        open_writable: &'a dyn Fn() -> Result<HostCtx, AppError>,
        cli: &'a CliCommand,
    ) -> Self {
        Self {
            open_read_only,
            open_writable,
            writable: OnceCell::new(),
            hosts: remote::Hosts::new(cli),
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
pub fn run(mut reader: impl BufRead, mut writer: impl Write, server: &Server) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(MAX_FRAME as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let too_long = read > MAX_FRAME && line.last() != Some(&b'\n');
        if !too_long && line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Checked before parsing: an oversized line is never parsed.
        let reply = if too_long {
            skip_line(&mut reader)?;
            Some(failure(
                Value::Null,
                INVALID_REQUEST,
                format!("Message longer than {MAX_FRAME} bytes"),
            ))
        } else {
            match serde_json::from_slice::<Value>(&line) {
                Ok(Value::Array(_)) => Some(failure(
                    Value::Null,
                    INVALID_REQUEST,
                    "Batches are not supported; send one message per line".into(),
                )),
                Ok(message) => answer(server, &message),
                Err(err) => Some(failure(
                    Value::Null,
                    PARSE_ERROR,
                    format!("Parse error: {err}"),
                )),
            }
        };
        if let Some(reply) = reply {
            let mut bytes = serde_json::to_vec(&reply)?;
            bytes.push(b'\n');
            writer.write_all(&bytes)?;
            writer.flush()?;
        }
    }
}

/// Discard the rest of the current line without keeping it.
fn skip_line(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                reader.consume(end + 1);
                return Ok(());
            }
            None => {
                let len = buffer.len();
                reader.consume(len);
            }
        }
    }
}

/// The reply to one message, or `None` for a notification or a response.
/// Anything else is an invalid request.
fn answer(server: &Server, message: &Value) -> Option<Value> {
    let invalid = |id: Option<&Value>| {
        let id = id.cloned().unwrap_or(Value::Null);
        Some(failure(id, INVALID_REQUEST, "Invalid Request".into()))
    };
    let Some(fields) = message.as_object() else {
        return invalid(None);
    };
    let method = match fields.get("method") {
        Some(Value::String(method)) => method.as_str(),
        None if fields.contains_key("result") || fields.contains_key("error") => return None,
        _ => return invalid(fields.get("id")),
    };
    let id = fields.get("id")?.clone();
    let params = &message["params"];
    let result = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [discover_tool(), apply_tool(), inspect_tool()] })),
        "tools/call" => call_tool(server, params),
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
            "name": mcps::SELF_SERVER_NAME,
            "title": "Agents Manager",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "Pass the active checkout's absolute path to discover_project_setup \
            to read its Project setup, and to apply_project_setup to apply and verify it. \
            After a lost reply or an unclear result, inspect_project_setup_result reads it \
            back without writing. A linked worktree is its own checkout. To work on a \
            registered SSH host, pass its name as host to any tool.",
    })
}

/// A tool's own failure is a result the agent reads (`isError`), not a
/// protocol error; only an unknown tool is one.
fn call_tool(server: &Server, params: &Value) -> Result<Value, (i64, String)> {
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
    let host = match request.remove("host") {
        None => None,
        Some(Value::String(host)) => Some(host),
        Some(_) => return Err((INVALID_PARAMS, "host must be a string".into())),
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let read_only = (server.open_read_only)()?;
        if let Some(host) = &host {
            return server.hosts.call(&read_only, host, request);
        }
        // Checked read-only first: a request that cannot write never opens
        // the library for writing.
        project_setup::serve(&read_only, &|| server.writable(), Value::Object(request))
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
        "description": "Read the Project setup for one exact checkout on this computer or \
            the named host: its registered project, saved setup, Library options (50 per \
            page), read-only Sync status and conflicts. When the checkout is not registered, \
            up to 10 registered projects that contain it or sit inside it are listed \
            as candidates. Writes nothing.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": HOST_DESCRIPTION,
                },
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
                    "description": "This host's last recorded Sync state. checked_at is \
                        the time of that state, null when this host has none. verified is \
                        false: discovery does not contact the Sync remote.",
                    "properties": {
                        "state": { "type": "string" },
                        "reason": { "type": ["string", "null"] },
                        "remote": { "type": ["string", "null"] },
                        "last_sync_at": { "type": ["integer", "null"] },
                        "checked_at": { "type": ["integer", "null"] },
                        "verified": { "type": "boolean" },
                    },
                    "required": ["state", "reason", "remote", "last_sync_at", "checked_at", "verified"],
                },
                "conflicts": {
                    "type": "object",
                    "description": "Sync's items that need review, at most 50 of each kind.",
                    "properties": {
                        "skills": { "type": "array" },
                        "resources": { "type": "array" },
                        "same_name_presets": { "type": "array" },
                        "edited_here": { "type": "array" },
                        "total": {
                            "type": "integer",
                            "description": "Every item that needs review, listed or not.",
                        },
                    },
                    "required": ["skills", "resources", "same_name_presets", "edited_here", "total"],
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
            "host": { "type": "string" },
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
                        "paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "With first_setup or unapproved_agents: the native paths, \
                                relative to the checkout, that the other agents also read.",
                        },
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
            "host", "checkout_path", "project_id", "register", "deploy_mode",
            "setup_version", "decisions",
        ],
    });
    json!({
        "name": APPLY,
        "title": "Apply Project setup",
        "description": "Apply the saved Project setup to one exact registered checkout on this \
            computer or the named host and verify the result, in one call. Absent items are \
            added to their agents; items that already match are left alone; edited, unapproved \
            or ambiguous items are reported and never overwritten, while independent items continue. Reusing \
            an operation_id returns the recorded result instead of applying again. Per-item \
            results come only with evidence: true. A checkout with no saved setup gets a \
            proposal for the items passed; items that need approval get one too. Send a \
            proposal back as confirm, with a choice on each decision, to register the \
            checkout if needed, save the setup and apply it. A stale proposal is refused.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": HOST_DESCRIPTION,
                },
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
                    "maxItems": 500,
                    "description": "First setup only: the Library items and agents to propose.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": kind,
                            "id": { "type": "string" },
                            "agents": {
                                "type": "array",
                                "items": { "type": "string" },
                                "maxItems": project_setup::MAX_AGENTS,
                            },
                        },
                        "required": ["kind", "id", "agents"],
                    },
                },
                "agents": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": project_setup::MAX_AGENTS,
                    "description": "Agents for this run only, in place of each saved item's \
                        agents; the saved setup is unchanged, also when a proposal for this run \
                        is confirmed with the same agents. Omit to use the saved agents. Not \
                        with items.",
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
                    "description": "Null until a first setup is confirmed, and with \
                        outcome_unknown.",
                },
                "setup_version": {
                    "type": ["integer", "null"],
                    "description": "Null with outcome_unknown: the request recorded no result.",
                },
                "outcome": {
                    "type": "string",
                    "enum": [
                        "verified_changed", "verified_no_change", "partial_success",
                        "needs_review", "retryable_failure", "outcome_unknown",
                        "confirmation_needed", "nothing_set_up",
                    ],
                    "description": "partial_success: some items changed, others need a \
                        decision, retry or recovery. needs_review: no change was verified \
                        and an item needs a decision or recovery. retryable_failure: no change \
                        was verified and every unsettled item failed in a way a new apply may fix. \
                        nothing_set_up: a first setup with no item approved; nothing was \
                        registered, saved or applied.",
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
                            "agents": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "The agents this item was set up or checked for.",
                            },
                            "unsupported": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "The item's agents it cannot be set up for here.",
                            },
                            "status": {
                                "type": "string",
                                "enum": ["changed", "unchanged", "needs_review", "blocked", "missing", "failed"],
                            },
                            "reason": { "type": "string" },
                            "retryable": {
                                "type": "boolean",
                                "description": "For a failed item: whether applying again \
                                    may succeed with nothing else done first.",
                            },
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
            // An approved would_delete decision deletes Instruction files.
            "destructiveHint": true,
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
        "description": "Read back an apply_project_setup operation on this computer or the \
            named host without writing or resubmitting: its recorded result, and the checkout's \
            current state against its saved setup. A missing receipt, or one with no recorded \
            result, is outcome_unknown; a matching checkout is reported as verified state, not \
            as this operation's change. Per-item results come only with evidence: true.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": HOST_DESCRIPTION,
                },
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
                "project_id": {
                    "type": ["string", "null"],
                    "description": "Null when the checkout is not a registered project.",
                },
                "outcome": {
                    "type": "string",
                    "enum": [
                        "verified_changed", "verified_no_change", "partial_success",
                        "needs_review", "retryable_failure", "outcome_unknown",
                    ],
                },
                "receipt": {
                    "type": "string",
                    "enum": ["result", "error", "intent_only", "missing"],
                    "description": "What this host recorded for the operation_id.",
                },
                "error": {
                    "type": "object",
                    "description": "With receipt error: the error the operation recorded \
                        before it applied any item.",
                    "properties": {
                        "kind": { "type": "string" },
                        "message": { "type": "string" },
                    },
                    "required": ["kind", "message"],
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
                    "description": "The operation recorded its result and some items are \
                        not applied, so apply under a new operation_id would set them up. \
                        Always false while the outcome is unknown.",
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

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{answer, Server};
    use crate::core::error::AppError;
    use crate::core::host::HostCtx;
    use crate::core::project_setup::apply_tests::{approved, Fixture};
    use crate::core::remote_session::CliCommand;

    /// The JSON Schema rules the tools' output schemas use: type, enum,
    /// required, properties, items and anyOf. Returns the first mismatch.
    fn check(value: &Value, schema: &Value, at: &str) -> Result<(), String> {
        if let Some(options) = schema["anyOf"].as_array() {
            return match options.iter().any(|o| check(value, o, at).is_ok()) {
                true => Ok(()),
                false => Err(format!("{at}: {value} matches no anyOf option")),
            };
        }
        let types: Vec<&str> = match &schema["type"] {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let type_of = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if !types.is_empty() && !types.contains(&type_of) {
            return Err(format!("{at}: {value} is not of type {types:?}"));
        }
        if let Some(allowed) = schema["enum"].as_array() {
            if !allowed.contains(value) {
                return Err(format!("{at}: {value} is not one of {allowed:?}"));
            }
        }
        if let Value::Object(fields) = value {
            for key in schema["required"].as_array().into_iter().flatten() {
                let key = key.as_str().unwrap_or_default();
                if !fields.contains_key(key) {
                    return Err(format!("{at}: {key} is missing"));
                }
            }
            for (key, property) in schema["properties"].as_object().into_iter().flatten() {
                if let Some(field) = fields.get(key) {
                    check(field, property, &format!("{at}.{key}"))?;
                }
            }
        }
        if let (Value::Array(items), Some(schema)) = (value, schema.get("items")) {
            for (n, item) in items.iter().enumerate() {
                check(item, schema, &format!("{at}[{n}]"))?;
            }
        }
        Ok(())
    }

    const KIND: &str = "project_setup_receipt";

    #[test]
    fn every_tool_result_matches_its_output_schema() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp) },
        ]));
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let open = || Ok::<HostCtx, _>(fixture.ctx.clone());
        let cli: &CliCommand = &|_, _| unreachable!("no SSH host is named");
        let server = Server::new(&open, &open, cli);
        let tools = answer(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" }),
        )
        .unwrap();
        let schema = |name: &str| {
            tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap()["outputSchema"]
                .clone()
        };
        let checkout = &fixture.checkout;
        let unclaim = |operation_id: &str| {
            let store = &fixture.ctx.store;
            let mut receipt = store
                .resource_state_get(KIND, operation_id)
                .unwrap()
                .unwrap();
            receipt["version"] = json!(1);
            receipt.as_object_mut().unwrap().remove("result");
            store
                .resource_state_put(KIND, operation_id, &receipt)
                .unwrap();
        };
        let mut calls = vec![
            (
                "discover_project_setup",
                json!({ "checkout_path": checkout }),
            ),
            ("discover_project_setup", json!({ "checkout_path": fresh })),
            (
                "apply_project_setup",
                json!({ "checkout_path": fresh, "operation_id": "op-0",
                "items": [{ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] }] }),
            ),
            (
                "apply_project_setup",
                json!({ "checkout_path": checkout, "operation_id": "op-1",
                "evidence": true }),
            ),
            (
                "apply_project_setup",
                json!({ "checkout_path": checkout, "operation_id": "op-1" }),
            ),
            (
                "inspect_project_setup_result",
                json!({ "checkout_path": checkout,
                "operation_id": "op-1", "evidence": true }),
            ),
            (
                "inspect_project_setup_result",
                json!({ "checkout_path": checkout,
                "operation_id": "op-9" }),
            ),
            ("unclaim", json!("op-1")),
            (
                "apply_project_setup",
                json!({ "checkout_path": checkout, "operation_id": "op-1" }),
            ),
            (
                "inspect_project_setup_result",
                json!({ "checkout_path": checkout,
                "operation_id": "op-1" }),
            ),
        ];
        // A recorded error: a first setup whose agent folder leads outside
        // its checkout is refused after its operation_id is claimed.
        #[cfg(unix)]
        {
            let tmp = fixture.repo._tmp.path();
            let linked = tmp.join("work/linked");
            std::fs::create_dir_all(&linked).unwrap();
            std::fs::create_dir_all(tmp.join("shared/claude")).unwrap();
            std::os::unix::fs::symlink(tmp.join("shared/claude"), linked.join(".claude")).unwrap();
            let call = |name: &str, arguments: Value| {
                answer(
                    &server,
                    &json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/call",
                             "params": { "name": name, "arguments": arguments } }),
                )
                .unwrap()["result"]
                    .clone()
            };
            let apply = |arguments| call("apply_project_setup", arguments);
            let proposed = apply(json!({ "checkout_path": linked, "operation_id": "op-l1",
                "items": [{ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] }] }));
            let mut proposal = proposed["structuredContent"]["proposal"].clone();
            proposal["decisions"][0]["choice"] = json!("approve");
            let refused = apply(
                json!({ "checkout_path": linked, "operation_id": "op-l2", "confirm": proposal }),
            );
            assert_eq!(refused["isError"], true, "{refused}");
            let inspect = json!({ "checkout_path": linked, "operation_id": "op-l2" });
            let inspected = call("inspect_project_setup_result", inspect.clone());
            assert_eq!(
                inspected["structuredContent"]["receipt"], "error",
                "{inspected}"
            );
            calls.push(("inspect_project_setup_result", inspect));
        }

        for (n, (name, arguments)) in calls.into_iter().enumerate() {
            if name == "unclaim" {
                unclaim(arguments.as_str().unwrap());
                continue;
            }
            let reply = answer(
                &server,
                &json!({ "jsonrpc": "2.0", "id": n, "method": "tools/call",
                         "params": { "name": name, "arguments": arguments } }),
            )
            .unwrap();
            let result = &reply["result"];
            assert_eq!(result["isError"], false, "call {n}: {reply}");
            let found = &result["structuredContent"];
            if let Err(mismatch) = check(found, &schema(name), name) {
                panic!("call {n}: {mismatch}\n{found}");
            }
        }
    }

    /// Answer `input` the way `cli mcp` does; one reply per line.
    fn replies(server: &Server, input: &str) -> Vec<Value> {
        let mut output = Vec::new();
        super::run(input.as_bytes(), &mut output, server).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn a_batch_is_answered_with_one_invalid_request_error() {
        let fixture = Fixture::new();
        let open = || Ok::<HostCtx, _>(fixture.ctx.clone());
        let cli: &CliCommand = &|_, _| unreachable!("no SSH host is named");
        let server = Server::new(&open, &open, cli);
        let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

        let found = replies(&server, &format!("{}\n{ping}\n", json!([ping, ping])));

        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0]["error"]["code"], -32600, "{found:?}");
        assert_eq!(found[0]["id"], Value::Null);
        assert_eq!(found[1]["id"], 1);
    }

    #[test]
    fn a_message_that_is_not_a_request_or_response_gets_an_invalid_request_error() {
        let fixture = Fixture::new();
        let open = || Ok::<HostCtx, _>(fixture.ctx.clone());
        let cli: &CliCommand = &|_, _| unreachable!("no SSH host is named");
        let server = Server::new(&open, &open, cli);
        let input = [
            json!({ "jsonrpc": "2.0", "id": 1 }),
            json!(7),
            json!({ "jsonrpc": "2.0", "id": 2, "method": 3 }),
            json!({ "jsonrpc": "2.0", "id": 9, "result": {} }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        ]
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();

        let found = replies(&server, &input);

        let ids: Vec<&Value> = found.iter().map(|reply| &reply["id"]).collect();
        assert_eq!(ids, [&json!(1), &Value::Null, &json!(2)], "{found:?}");
        assert!(found.iter().all(|reply| reply["error"]["code"] == -32600));
    }

    #[test]
    fn a_stale_confirmation_is_refused_without_opening_the_library_for_writing() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let open = || Ok::<HostCtx, _>(fixture.ctx.clone());
        let refuse = || -> Result<HostCtx, AppError> { panic!("opened for writing") };
        let cli: &CliCommand = &|_, _| unreachable!("no SSH host is named");
        let server = Server::new(&open, &refuse, cli);
        let call = |id: u64, arguments: Value| {
            answer(
                &server,
                &json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                         "params": { "name": "apply_project_setup", "arguments": arguments } }),
            )
            .unwrap()["result"]
                .clone()
        };
        let proposed = call(
            1,
            json!({ "checkout_path": fresh, "operation_id": "op-1",
            "items": [{ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] }] }),
        );
        let mut proposal = proposed["structuredContent"]["proposal"].clone();
        proposal["decisions"][0]["choice"] = json!("approve");
        proposal["setup_version"] = json!(7);

        let refused = call(
            2,
            json!({ "checkout_path": fresh, "operation_id": "op-2",
            "confirm": proposal }),
        );

        assert_eq!(refused["isError"], true, "{refused}");
        let text = refused["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("stale"), "{text}");
    }

    #[test]
    fn apply_is_annotated_as_destructive_because_it_may_delete_files() {
        let fixture = Fixture::new();
        let open = || Ok::<HostCtx, _>(fixture.ctx.clone());
        let cli: &CliCommand = &|_, _| unreachable!("no SSH host is named");
        let server = Server::new(&open, &open, cli);

        let tools = answer(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" }),
        )
        .unwrap();

        let apply = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "apply_project_setup")
            .unwrap();
        assert_eq!(apply["annotations"]["destructiveHint"], true);
    }
}
