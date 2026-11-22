//! Project setup for one exact checkout, behind a single host-scoped request
//! with a tagged `action`. Later actions join [`Request`]; they never add
//! commands.
//!
//! Discovery is read-only: it never registers projects, deploys, or writes
//! the store, so it can run on a store opened read-only. Apply writes; see
//! [`apply`]. Inspection is read-only too; see [`inspect`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::error::AppError;
use crate::core::host::HostCtx;
use crate::core::skill_store::{ProjectRecord, SkillStore};
use crate::core::sync_service;

mod apply;
mod confirm;
mod inspect;
mod items;
pub use apply::outcome_unknown;
#[cfg(test)]
pub(crate) use apply::tests as apply_tests;
pub use inspect::ReceiptState;
pub use items::mcp::mcp_value;

/// Library options per page.
pub const PAGE_SIZE: usize = 50;
/// Registered projects offered when the checkout is not one of them.
pub const MAX_CANDIDATES: usize = 10;
/// The most items a saved setup, a proposal or a confirmation may hold.
pub const MAX_ITEMS: usize = 500;
/// The most agents one item, or one run, may name.
pub const MAX_AGENTS: usize = 64;
/// The `resource_state` kind that holds each project's saved setup.
const SETUP_KIND: &str = "project_setup";

/// A project's saved setup: the selected Library items and the agents each
/// targets. Written only through [`save_setup`], so each write bumps
/// `version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSetup {
    pub version: u64,
    pub items: Vec<SetupItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupItem {
    pub kind: ItemKind,
    /// The Library ID. Names are for people; a renamed item keeps its ID.
    pub id: String,
    /// Always explicit: an empty list never means every agent.
    pub agents: Vec<String>,
    /// For an MCP connection, the effective value the user approved for each
    /// agent, by agent key, kept verbatim. See [`items::mcp::mcp_value`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<Value>,
}

/// Ordered as the Library lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Skill,
    Instruction,
    Mcp,
}

/// Replace a project's saved setup when it is still at `expected_version`
/// (0 for none), and return the stored record.
pub fn save_setup(
    store: &SkillStore,
    project_id: &str,
    expected_version: u64,
    items: Vec<SetupItem>,
) -> Result<SavedSetup, AppError> {
    check_size(items.len())?;
    let setup = SavedSetup {
        version: expected_version + 1,
        items,
    };
    let value = serde_json::to_value(&setup).map_err(AppError::internal)?;
    if !store
        .resource_state_put_versioned(SETUP_KIND, project_id, expected_version, &value)
        .map_err(AppError::db)?
    {
        return Err(AppError::invalid_input(
            "The saved Project setup changed in another session; discover it again",
        ));
    }
    Ok(setup)
}

/// Refuse more than [`MAX_ITEMS`] items before any work is done for them.
fn check_size(items: usize) -> Result<(), AppError> {
    if items > MAX_ITEMS {
        return Err(AppError::invalid_input(format!(
            "A Project setup holds at most {MAX_ITEMS} items; this request has {items}"
        )));
    }
    Ok(())
}

/// Refuse a list of more than [`MAX_AGENTS`] agents.
fn check_agents(agents: usize) -> Result<(), AppError> {
    if agents > MAX_AGENTS {
        return Err(AppError::invalid_input(format!(
            "An item or a run names at most {MAX_AGENTS} agents; this one names {agents}"
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Discover {
        checkout_path: String,
        #[serde(default)]
        cursor: Option<String>,
    },
    Apply(apply::Args),
    Inspect {
        checkout_path: String,
        operation_id: String,
        #[serde(default)]
        evidence: bool,
    },
}

fn parse(request: Value) -> Result<Request, AppError> {
    serde_json::from_value(request)
        .map_err(|e| AppError::invalid_input(format!("Invalid Project setup request: {e}")))
}

/// Whether `request` only reads, judged from the request alone: discovery,
/// inspection, or an apply that only proposes a first setup.
pub fn reads_only(request: &Value) -> Result<bool, AppError> {
    Ok(match parse(request.clone())? {
        Request::Apply(args) => apply::proposes_only(&args),
        Request::Discover { .. } | Request::Inspect { .. } => true,
    })
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    serve(ctx, &|| Ok(ctx), request)
}

/// Answer `request` on `read_only`, which it parses once. Only an apply
/// that has been checked and has something to apply opens the library for
/// writing, through `writable`.
pub fn serve<'c>(
    read_only: &'c HostCtx,
    writable: &dyn Fn() -> Result<&'c HostCtx, AppError>,
    request: Value,
) -> Result<Value, AppError> {
    let ctx = read_only;
    match parse(request)? {
        Request::Discover {
            checkout_path,
            cursor,
        } => discover(ctx, &checkout_path, cursor.as_deref()),
        Request::Apply(args) => apply::apply(ctx, writable, args),
        Request::Inspect {
            checkout_path,
            operation_id,
            evidence,
        } => inspect::inspect(ctx, &checkout_path, &operation_id, evidence),
    }
}

fn discover(ctx: &HostCtx, checkout_path: &str, cursor: Option<&str>) -> Result<Value, AppError> {
    let checkout = canonical_checkout(checkout_path)?;
    let projects = registered_projects(ctx)?;
    let exact = projects.iter().find(|(_, path)| *path == checkout);
    let candidates: Vec<Value> = match exact {
        Some(_) => Vec::new(),
        None => projects
            .iter()
            .filter(|(_, path)| checkout.starts_with(path) || path.starts_with(&checkout))
            .take(MAX_CANDIDATES)
            .map(|(project, path)| project_json(project, path))
            .collect(),
    };
    let saved_setup = match exact {
        Some((project, _)) => ctx
            .store
            .resource_state_get(SETUP_KIND, &project.id)
            .map_err(AppError::db)?,
        None => None,
    };
    let status = sync_service::sync_status(&ctx.store);
    Ok(json!({
        "host": status.host,
        "checkout_path": checkout,
        "project": exact.map(|(project, path)| project_json(project, path)),
        "candidates": candidates,
        "saved_setup": saved_setup,
        "library": library_page(ctx, cursor)?,
        // Read from this host's last sync; discovery never contacts the remote.
        "sync": {
            "state": status.state,
            "reason": status.reason,
            "remote": status.remote,
            "last_sync_at": status.last_sync,
            // The last-known status is as of the last sync.
            "checked_at": status.last_sync,
            "verified": false,
        },
        "conflicts": {
            "skills": status.conflicts,
            "resources": status.resource_conflicts,
            "same_name_presets": status.same_name_presets,
            "edited_here": status.edited_here,
        },
    }))
}

/// Registered projects with their canonical paths. Both sides canonical and
/// compared whole, so a linked worktree or a folder inside a project never
/// resolves to the project around it.
fn registered_projects(ctx: &HostCtx) -> Result<Vec<(ProjectRecord, PathBuf)>, AppError> {
    Ok(ctx
        .store
        .get_all_projects()
        .map_err(AppError::db)?
        .into_iter()
        .filter(|project| project.workspace_type == "project")
        .filter_map(|project| {
            let path = Path::new(&project.path).canonicalize().ok()?;
            Some((project, path))
        })
        .collect())
}

/// The canonical checkout and the project registered at exactly that path,
/// with `path` set to the canonical checkout.
fn find_project(
    ctx: &HostCtx,
    checkout_path: &str,
) -> Result<(PathBuf, Option<ProjectRecord>), AppError> {
    let checkout = canonical_checkout(checkout_path)?;
    let project = registered_projects(ctx)?
        .into_iter()
        .find(|(_, path)| *path == checkout)
        .map(|(project, path)| ProjectRecord {
            path: path.to_string_lossy().into_owned(),
            ..project
        });
    Ok((checkout, project))
}

fn canonical_checkout(checkout_path: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(checkout_path);
    if !path.is_absolute() {
        return Err(AppError::invalid_input(
            "checkout_path must be an absolute path",
        ));
    }
    path.canonicalize()
        .map_err(|e| AppError::invalid_input(format!("Cannot open checkout {checkout_path}: {e}")))
}

fn project_json(project: &ProjectRecord, path: &Path) -> Value {
    json!({
        "id": project.id,
        "name": project.name,
        "path": path,
        "agent_keys": project.agent_keys,
        "deploy_mode": project.deploy_mode,
    })
}

/// One page of Skills, Instruction bundles and MCP connections, in that
/// order and by ID within each kind. The cursor is the last item's kind and
/// ID, `kind:id`, so items added or removed elsewhere never shift a page.
/// A kind is read only when the page reaches it.
fn library_page(ctx: &HostCtx, cursor: Option<&str>) -> Result<Value, AppError> {
    let after: Option<(ItemKind, &str)> = match cursor {
        None => None,
        Some(cursor) => {
            let invalid = || AppError::invalid_input("Invalid Library cursor");
            let (kind, id) = cursor.split_once(':').ok_or_else(invalid)?;
            let kind = serde_json::from_value(json!(kind)).map_err(|_| invalid())?;
            Some((kind, id))
        }
    };
    // One more than a page, to tell whether another page follows.
    let mut items: Vec<(ItemKind, String, String)> = Vec::new();
    for kind in [ItemKind::Skill, ItemKind::Instruction, ItemKind::Mcp] {
        let after_id = match after {
            Some((last, _)) if kind < last => continue,
            Some((last, id)) if kind == last => Some(id),
            _ => None,
        };
        let wanted = PAGE_SIZE + 1 - items.len();
        if wanted == 0 {
            break;
        }
        // Each kind is paged where it is stored: only the page is read.
        let found: Vec<(String, String)> = match kind {
            ItemKind::Skill => ctx
                .store
                .get_skills_after(after_id, wanted)
                .map_err(AppError::db)?
                .into_iter()
                .map(|skill| (skill.id, skill.name))
                .collect(),
            ItemKind::Instruction => crate::core::instructions::names_after(after_id, wanted)?,
            ItemKind::Mcp => crate::core::mcps::names_after(after_id, wanted)?,
        };
        items.extend(found.into_iter().map(|(id, name)| (kind, id, name)));
    }
    let next_cursor = (items.len() > PAGE_SIZE).then(|| {
        items.truncate(PAGE_SIZE);
        let (kind, id, _) = &items[PAGE_SIZE - 1];
        format!("{}:{id}", json!(kind).as_str().unwrap_or_default())
    });
    let items: Vec<Value> = items
        .into_iter()
        .map(|(kind, id, name)| json!({ "kind": kind, "id": id, "name": name }))
        .collect();
    Ok(json!({ "items": items, "next_cursor": next_cursor }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::{json, Value};

    use crate::core::error::{AppError, ErrorKind};
    use crate::core::host::{HostCtx, NoopEvents};
    use crate::core::host_dispatch::dispatch;
    use crate::core::skill_store::{ProjectRecord, SkillStore};
    use crate::core::test_support::{sample_skill, test_repo, write_skill_dir, TestRepo};

    fn project(id: &str, path: &Path) -> ProjectRecord {
        ProjectRecord {
            id: id.to_string(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            workspace_type: "project".to_string(),
            linked_agent_key: None,
            linked_agent_name: None,
            disabled_path: None,
            sort_order: 0,
            created_at: 0,
            updated_at: 0,
            agent_keys: Some(vec!["codex".to_string()]),
            deploy_mode: "link".to_string(),
        }
    }

    /// A temp library with its own store, and a folder for checkouts.
    struct Host {
        repo: TestRepo,
        ctx: HostCtx,
    }

    impl Host {
        fn new() -> Self {
            let repo = test_repo();
            let store = SkillStore::new(&repo._tmp.path().join("repo/test.db")).unwrap();
            let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
            Self { repo, ctx }
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let dir = self.repo._tmp.path().join("work").join(relative);
            std::fs::create_dir_all(&dir).unwrap();
            dir.canonicalize().unwrap()
        }

        fn discover(&self, args: Value) -> Result<Value, AppError> {
            let mut request = json!({ "action": "discover" });
            request
                .as_object_mut()
                .unwrap()
                .extend(args.as_object().unwrap().clone());
            dispatch(
                &self.ctx,
                "project_setup_request",
                &json!({ "request": request }),
            )
        }
    }

    #[test]
    fn an_exact_checkout_resolves_to_its_project() {
        let host = Host::new();
        let checkout = host.dir("app");
        // Stored through a path that is not canonical; asked by the real one.
        let stored = checkout.join("../app");
        host.repo
            .store
            .insert_project(&project("p1", &stored))
            .unwrap();

        let found = host.discover(json!({ "checkout_path": checkout })).unwrap();

        assert_eq!(found["checkout_path"], json!(checkout));
        assert_eq!(found["project"]["id"], "p1");
        assert_eq!(found["project"]["agent_keys"], json!(["codex"]));
        assert_eq!(found["candidates"], json!([]));
    }

    #[test]
    fn a_linked_worktree_never_resolves_to_its_main_checkout() {
        let host = Host::new();
        let main = host.dir("app");
        let worktree = host.dir("app/.worktrees/feature");
        host.repo
            .store
            .insert_project(&project("main", &main))
            .unwrap();

        let found = host.discover(json!({ "checkout_path": worktree })).unwrap();

        assert_eq!(found["project"], Value::Null);
        assert_eq!(found["saved_setup"], Value::Null);
        // Offered for the caller to tell apart, never chosen for it.
        assert_eq!(found["candidates"][0]["id"], "main");
        assert_eq!(found["candidates"][0]["path"], json!(main));
    }

    #[test]
    fn an_unregistered_checkout_is_reported_without_creating_records() {
        let host = Host::new();
        let checkout = host.dir("fresh");
        for n in 0..12 {
            let inside = host.dir(&format!("fresh/pkg{n}"));
            host.repo
                .store
                .insert_project(&project(&format!("p{n}"), &inside))
                .unwrap();
        }

        let found = host.discover(json!({ "checkout_path": checkout })).unwrap();

        assert_eq!(found["project"], Value::Null);
        assert_eq!(found["candidates"].as_array().unwrap().len(), 10);
        assert_eq!(host.repo.store.get_all_projects().unwrap().len(), 12);
        let entries: Vec<_> = std::fs::read_dir(&checkout).unwrap().collect();
        assert_eq!(entries.len(), 12, "discovery added files to the checkout");
    }

    #[test]
    fn a_registered_project_returns_its_saved_setup() {
        let host = Host::new();
        let checkout = host.dir("app");
        host.repo
            .store
            .insert_project(&project("p1", &checkout))
            .unwrap();
        let setup = json!({ "items": [{ "kind": "skill", "id": "s1", "agents": ["codex"] }] });
        host.repo
            .store
            .resource_state_put("project_setup", "p1", &setup)
            .unwrap();

        let found = host.discover(json!({ "checkout_path": checkout })).unwrap();

        assert_eq!(found["saved_setup"], setup);
        assert_eq!(found["sync"]["verified"], false);
        assert_eq!(found["sync"]["state"], "not_set_up");
    }

    #[test]
    fn library_options_come_in_pages_of_50() {
        let host = Host::new();
        let checkout = host.dir("app");
        for n in 0..55 {
            let name = format!("skill-{n:02}");
            let dir = write_skill_dir(&name);
            host.repo
                .store
                .insert_skill(&sample_skill(&format!("s{n:02}"), &name, &dir))
                .unwrap();
        }

        let first = host.discover(json!({ "checkout_path": checkout })).unwrap();
        let cursor = first["library"]["next_cursor"].clone();
        let second = host
            .discover(json!({ "checkout_path": checkout, "cursor": cursor }))
            .unwrap();

        assert_eq!(first["library"]["items"].as_array().unwrap().len(), 50);
        assert_eq!(
            first["library"]["items"][0],
            json!({ "kind": "skill", "id": "s00", "name": "skill-00" })
        );
        let rest = second["library"]["items"].as_array().unwrap();
        assert_eq!(rest.len(), 5);
        assert_eq!(rest[4]["id"], "s54");
        assert_eq!(second["library"]["next_cursor"], Value::Null);
    }

    #[test]
    fn a_library_cursor_stays_put_when_an_earlier_item_is_added() {
        let host = Host::new();
        let checkout = host.dir("app");
        for n in 0..55 {
            let name = format!("skill-{n:02}");
            let dir = write_skill_dir(&name);
            host.repo
                .store
                .insert_skill(&sample_skill(&format!("s{n:02}"), &name, &dir))
                .unwrap();
        }
        let first = host.discover(json!({ "checkout_path": checkout })).unwrap();
        let dir = write_skill_dir("early");
        host.repo
            .store
            .insert_skill(&sample_skill("a00", "early", &dir))
            .unwrap();

        let cursor = first["library"]["next_cursor"].clone();
        let second = host
            .discover(json!({ "checkout_path": checkout, "cursor": cursor }))
            .unwrap();

        let ids: Vec<&Value> = second["library"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["id"])
            .collect();
        assert_eq!(ids, ["s50", "s51", "s52", "s53", "s54"]);
    }

    #[test]
    fn a_relative_checkout_path_is_refused() {
        let host = Host::new();
        let error = host
            .discover(json!({ "checkout_path": "work/app" }))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn an_unknown_argument_is_refused_by_every_action() {
        let host = Host::new();
        let checkout = host.dir("app");
        let requests = [
            json!({ "action": "discover", "checkout_path": checkout, "cursr": "x" }),
            json!({ "action": "inspect", "checkout_path": checkout, "operation_id": "op-1",
                    "evidense": true }),
            json!({ "action": "apply", "checkout_path": checkout, "operation_id": "op-1",
                    "agent": ["codex"] }),
        ];
        for request in requests {
            let error = dispatch(
                &host.ctx,
                "project_setup_request",
                &json!({ "request": request }),
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidInput, "{request}");
            assert!(error.message.contains("unknown field"), "{}", error.message);
        }
    }

    #[test]
    fn a_library_page_reads_only_the_connections_on_it() {
        let host = Host::new();
        let checkout = host.dir("app");
        let mut ids: Vec<String> = ["docs", "search"]
            .iter()
            .map(|name| {
                let saved = dispatch(
                    &host.ctx,
                    "mcps_request",
                    &json!({ "request": { "action": "save", "definition": {
                        "name": name, "transport": "stdio",
                        "server": { "command": "npx", "args": ["-y", name] } } } }),
                )
                .unwrap();
                saved["definition"]["id"].as_str().unwrap().to_owned()
            })
            .collect();
        ids.sort();
        // A connection before the cursor, and so not on this page, is unreadable.
        let file = crate::core::resource_store::root().join(format!("mcps/{}.json", ids[0]));
        std::fs::write(file, "not json").unwrap();

        let page = host
            .discover(json!({ "checkout_path": checkout, "cursor": format!("mcp:{}", ids[0]) }))
            .unwrap();

        let found: Vec<&Value> = page["library"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["id"])
            .collect();
        assert_eq!(found, [&json!(ids[1])]);
    }
}
