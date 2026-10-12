//! Pure three-way decision (design §3): object-level table, component-level
//! merge for skills, per-field merge for presets and memberships (SID-104),
//! whole-file newest-wins for residual paths, and the viewpoint-independent
//! path-collision pass.
//!
//! Everything here is a pure function of the three snapshots plus the
//! declared-pending pin set and the per-side last-touch info — no repository
//! access — so both devices merging the same pair of commits compute the
//! same plan (§10 convergence).

use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::snapshot::{attrs_eq, skill_identical, FileEntry, SkillObj, Snapshot, METADATA_DIR};
use crate::core::sync_metadata::{path_key, SkillMetaFile, ACTIVE_PRESET_FILE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// Last-touch stamp of a path on one side: committer time plus the touching
/// commit id as a viewpoint-independent tie-break.
pub type Touch = (i64, String);

pub struct DecisionInput<'a> {
    pub base: &'a Snapshot,
    pub ours: &'a Snapshot,
    pub theirs: &'a Snapshot,
    /// Declared-pending skills (from trailers, §4) → side whose version is
    /// pinned. These bypass the normal three-way decision entirely.
    pub pinned: &'a BTreeMap<String, Side>,
    /// Newest-first last-touch info per repo-relative path, per side, for
    /// whole-file newest-wins decisions. Only consulted for paths where both
    /// sides diverged from base.
    pub ours_touch: &'a BTreeMap<String, Touch>,
    pub theirs_touch: &'a BTreeMap<String, Touch>,
}

#[derive(Debug, Clone)]
pub struct PlannedSkill {
    pub meta: SkillMetaFile,
    pub content: Option<git2::Oid>,
    /// True when the collision pass had to move this skill to a fresh path.
    pub renamed_for_collision: bool,
    /// Where the surviving components came from, for the summary.
    pub changed_from_theirs: bool,
}

#[derive(Debug, Clone)]
pub struct NewConflict {
    pub skill_id: String,
    /// Path of the theirs version (for the pending projection / UI).
    pub theirs_path: Option<String>,
}

/// A preset or membership file in the merged tree: one side's blob, or a new
/// file merged field by field (SID-104).
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedFile {
    Blob(FileEntry),
    Merged(Value),
}

#[derive(Debug, Default)]
pub struct MergePlan {
    pub skills: BTreeMap<String, PlannedSkill>,
    pub new_conflicts: Vec<NewConflict>,
    /// Previously declared pendings still active after this merge (their
    /// conflict refs advance to the new theirs commit).
    pub still_pending: Vec<String>,
    pub scenarios: BTreeMap<String, PlannedFile>,
    pub memberships: BTreeMap<(String, String), PlannedFile>,
    pub residual: BTreeMap<String, FileEntry>,
    /// Changes the newer side replaced, by field, for the merge commit.
    pub replaced: Vec<String>,
    /// Presets renamed apart from a same-name preset → that preset (SID-104).
    pub same_name: BTreeMap<String, String>,
    /// skill ids adopted or partially adopted from theirs, for the summary.
    pub updated_from_theirs: Vec<String>,
    /// skill ids whose local version was kept over a conflicting remote one.
    pub kept_local: Vec<String>,
}

pub fn decide(input: &DecisionInput) -> Result<MergePlan> {
    let mut plan = MergePlan::default();

    // ── skills: pinned pendings first (§4 钉住) ──
    let mut pinned_ids: BTreeSet<&String> = BTreeSet::new();
    for (id, side) in input.pinned {
        let snap = match side {
            Side::Ours => input.ours,
            Side::Theirs => input.theirs,
        };
        match snap.skills.get(id) {
            Some(obj) => {
                plan.skills.insert(
                    id.clone(),
                    PlannedSkill {
                        meta: obj.meta.clone(),
                        content: obj.content,
                        renamed_for_collision: false,
                        changed_from_theirs: *side == Side::Theirs,
                    },
                );
                plan.still_pending.push(id.clone());
                pinned_ids.insert(id);
            }
            // The pinned side no longer has the skill (it was deleted there
            // after being pinned) — the pin has nothing to hold; fall back to
            // the normal three-way decision below.
            None => {
                plan.still_pending.push(id.clone());
            }
        }
    }

    // ── skills: object-level table (§3) ──
    let ids: BTreeSet<&String> = input
        .base
        .skills
        .keys()
        .chain(input.ours.skills.keys())
        .chain(input.theirs.skills.keys())
        .collect();
    for id in ids {
        if pinned_ids.contains(id) {
            continue;
        }
        let b = input.base.skills.get(id);
        let o = input.ours.skills.get(id);
        let t = input.theirs.skills.get(id);
        match (b, o, t) {
            (_, None, None) => {} // absent (or deleted on both) — stays absent
            (None, Some(o), None) => {
                plan.skills.insert(id.clone(), planned(o, false));
            }
            (None, None, Some(t)) => {
                plan.skills.insert(id.clone(), planned(t, true));
                plan.updated_from_theirs.push(id.clone());
            }
            (None, Some(o), Some(t)) => {
                if skill_identical(o, t) {
                    plan.skills.insert(id.clone(), planned(o, false));
                } else {
                    // 双新增分叉 → true conflict, keep ours.
                    plan.skills.insert(id.clone(), planned(o, false));
                    plan.new_conflicts.push(NewConflict {
                        skill_id: id.clone(),
                        theirs_path: Some(t.meta.path.clone()),
                    });
                    plan.kept_local.push(id.clone());
                }
            }
            (Some(b), None, Some(t)) => {
                if skill_identical(b, t) {
                    // our deletion propagates
                } else {
                    // 删 vs 改 → true conflict; ours (the deletion) is kept.
                    plan.new_conflicts.push(NewConflict {
                        skill_id: id.clone(),
                        theirs_path: Some(t.meta.path.clone()),
                    });
                    plan.kept_local.push(id.clone());
                }
            }
            (Some(b), Some(o), None) => {
                if skill_identical(b, o) {
                    // their deletion propagates
                    plan.updated_from_theirs.push(id.clone());
                } else {
                    // 改 vs 删 → true conflict, keep ours.
                    plan.skills.insert(id.clone(), planned(o, false));
                    plan.new_conflicts.push(NewConflict {
                        skill_id: id.clone(),
                        theirs_path: None,
                    });
                    plan.kept_local.push(id.clone());
                }
            }
            (Some(b), Some(o), Some(t)) => match merge_components(b, o, t) {
                ComponentOutcome::Merged { skill, from_theirs } => {
                    plan.skills.insert(id.clone(), skill);
                    if from_theirs {
                        plan.updated_from_theirs.push(id.clone());
                    }
                }
                ComponentOutcome::Conflict => {
                    plan.skills.insert(id.clone(), planned(o, false));
                    plan.new_conflicts.push(NewConflict {
                        skill_id: id.clone(),
                        theirs_path: Some(t.meta.path.clone()),
                    });
                    plan.kept_local.push(id.clone());
                }
            },
        }
    }

    // ── residual (§3), first: a preset someone switched to survives a delete ──
    plan.residual = merge_whole_files(
        &input.base.residual,
        &input.ours.residual,
        &input.theirs.residual,
        |path| path.clone(),
        input,
    );

    // ── presets and memberships, per field (SID-104) ──
    let mut replaced_presets = Vec::new();
    plan.scenarios = merge_objects(
        &input.base.scenarios,
        &input.ours.scenarios,
        &input.theirs.scenarios,
        |id| format!("{METADATA_DIR}/scenarios/{id}.json"),
        |id, kept, _| switched_to(kept, input).as_deref() == Some(id.as_str()),
        input,
        &mut replaced_presets,
    );
    for (id, field) in replaced_presets {
        let name = preset_name(&plan.scenarios, &id, input);
        plan.replaced
            .push(format!("{name} {}", field_label(&field)));
    }
    separate_same_name_presets(&mut plan, input);
    let mut replaced_members = Vec::new();
    plan.memberships = merge_objects(
        &input.base.memberships,
        &input.ours.memberships,
        &input.theirs.memberships,
        |(sid, skid)| format!("{METADATA_DIR}/scenario-skills/{sid}/{skid}.json"),
        // A deleted preset that came back keeps its skills.
        |(sid, _), _, deleted| {
            plan.scenarios.contains_key(sid) && !deleted.scenarios.contains_key(sid)
        },
        input,
        &mut replaced_members,
    );
    for ((sid, skid), field) in replaced_members {
        let skill = plan
            .skills
            .get(&skid)
            .map_or(skid.as_str(), |s| s.meta.path.as_str());
        let name = preset_name(&plan.scenarios, &sid, input);
        plan.replaced
            .push(format!("{name} › {skill} {}", field_label(&field)));
    }

    // ── orphan self-heal as tree-build input (§5 step 3 / v2-R2 finding 9):
    // memberships must reference a surviving scenario and skill.
    plan.memberships.retain(|(sid, skid), _| {
        plan.scenarios.contains_key(sid) && plan.skills.contains_key(skid)
    });

    // ── metadata-namespace junk drop (构树输入自愈): legacy trees carry
    // atomic-write leftovers (`x.json.tmp.<uuid>`) and OS noise inside the
    // metadata subdirectories — an old client committed them before disk
    // cleanup ran. The app only ever writes `.json` files there, so any
    // other residual is junk; merging it forward would trip the strict
    // validator on every device forever.
    plan.residual
        .retain(|path, _| !is_metadata_namespace_junk(path));

    resolve_path_collisions(&mut plan, input)?;
    Ok(plan)
}

fn planned(obj: &SkillObj, from_theirs: bool) -> PlannedSkill {
    PlannedSkill {
        meta: obj.meta.clone(),
        content: obj.content,
        renamed_for_collision: false,
        changed_from_theirs: from_theirs,
    }
}

// Short-lived, one per merged component, so the unboxed size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum ComponentOutcome {
    Merged {
        skill: PlannedSkill,
        from_theirs: bool,
    },
    Conflict,
}

/// Component-level merge (§3): content / path / attrs each decide
/// independently; any per-component two-sided divergence conflicts the whole
/// skill.
fn merge_components(b: &SkillObj, o: &SkillObj, t: &SkillObj) -> ComponentOutcome {
    fn pick<'x, V: PartialEq>(bv: &V, ov: &'x V, tv: &'x V) -> Option<(&'x V, bool)> {
        let o_changed = ov != bv;
        let t_changed = tv != bv;
        match (o_changed, t_changed) {
            (false, false) => Some((ov, false)),
            (true, false) => Some((ov, false)),
            (false, true) => Some((tv, true)),
            (true, true) => {
                if ov == tv {
                    Some((ov, false))
                } else {
                    None
                }
            }
        }
    }

    let content = pick(&b.content, &o.content, &t.content);
    let path = pick(&b.meta.path, &o.meta.path, &t.meta.path);
    // attrs are one component: enabled + tags + source move together.
    let attrs_changed_o = !attrs_eq(&b.meta, &o.meta);
    let attrs_changed_t = !attrs_eq(&b.meta, &t.meta);
    let attrs: Option<(&SkillMetaFile, bool)> = match (attrs_changed_o, attrs_changed_t) {
        (false, false) | (true, false) => Some((&o.meta, false)),
        (false, true) => Some((&t.meta, true)),
        (true, true) => {
            if attrs_eq(&o.meta, &t.meta) {
                Some((&o.meta, false))
            } else {
                None
            }
        }
    };

    match (content, path, attrs) {
        (Some((content, c_t)), Some((path, p_t)), Some((attrs_src, a_t))) => {
            // Canonical rebuild (§2.1): attrs winner's values verbatim, the
            // winning path, path_key recomputed by the shared helper.
            let meta = SkillMetaFile {
                schema_version: attrs_src.schema_version,
                skill_id: o.meta.skill_id.clone(),
                path: path.clone(),
                path_key: path_key(path),
                enabled: attrs_src.enabled,
                tags: attrs_src.tags.clone(),
                source: attrs_src.source.clone(),
            };
            let from_theirs = c_t || p_t || a_t;
            ComponentOutcome::Merged {
                skill: PlannedSkill {
                    meta,
                    content: *content,
                    renamed_for_collision: false,
                    changed_from_theirs: from_theirs,
                },
                from_theirs,
            }
        }
        _ => ComponentOutcome::Conflict,
    }
}

/// Whole-file object merge: three-way per key; both-changed-unequal (and
/// delete-vs-modify) resolve by newest committer time of the last commit
/// touching the path on each side, with the touching commit id as a
/// deterministic tie-break (§3 新者胜).
fn merge_whole_files<K: Ord + Clone>(
    base: &BTreeMap<K, FileEntry>,
    ours: &BTreeMap<K, FileEntry>,
    theirs: &BTreeMap<K, FileEntry>,
    to_path: impl Fn(&K) -> String,
    input: &DecisionInput,
) -> BTreeMap<K, FileEntry> {
    let keys: BTreeSet<&K> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut out = BTreeMap::new();
    for key in keys {
        let b = base.get(key);
        let o = ours.get(key);
        let t = theirs.get(key);
        let winner: Option<FileEntry> = match (b, o, t) {
            (_, None, None) => None,
            (None, Some(o), None) => Some(*o),
            (None, None, Some(t)) => Some(*t),
            (Some(b), Some(o), None) => {
                if o == b {
                    None // their deletion propagates
                } else {
                    newest_wins(key, Some(*o), None, &to_path, input)
                }
            }
            (Some(b), None, Some(t)) => {
                if t == b {
                    None // our deletion propagates
                } else {
                    newest_wins(key, None, Some(*t), &to_path, input)
                }
            }
            (None, Some(o), Some(t)) | (Some(_), Some(o), Some(t)) => {
                let b_entry = b.copied();
                if o == t || Some(*o) != b_entry && Some(*t) == b_entry {
                    Some(*o)
                } else if Some(*o) == b_entry {
                    Some(*t)
                } else {
                    newest_wins(key, Some(*o), Some(*t), &to_path, input)
                }
            }
        };
        if let Some(entry) = winner {
            out.insert(key.clone(), entry);
        }
    }
    out
}

/// Per-field object merge for presets and memberships (SID-104): a field
/// changed on one side is taken, and a field changed on both takes the newest
/// side's value and is reported in `replaced`. A delete loses to a concurrent
/// edit, and to `keep(key, side that kept it, side that deleted it)` for an
/// unchanged object.
fn merge_objects<K: Ord + Clone>(
    base: &BTreeMap<K, FileEntry>,
    ours: &BTreeMap<K, FileEntry>,
    theirs: &BTreeMap<K, FileEntry>,
    to_path: impl Fn(&K) -> String,
    keep: impl Fn(&K, &Snapshot, &Snapshot) -> bool,
    input: &DecisionInput,
    replaced: &mut Vec<(K, String)>,
) -> BTreeMap<K, PlannedFile> {
    let keys: BTreeSet<&K> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut out = BTreeMap::new();
    for key in keys {
        let b = base.get(key);
        let planned = match (b, ours.get(key), theirs.get(key)) {
            (_, None, None) => None,
            (Some(b), Some(o), None) if o == b && !keep(key, input.ours, input.theirs) => None,
            (Some(b), None, Some(t)) if t == b && !keep(key, input.theirs, input.ours) => None,
            (_, Some(o), None) => Some(PlannedFile::Blob(*o)),
            (_, None, Some(t)) => Some(PlannedFile::Blob(*t)),
            (_, Some(o), Some(t)) if o == t || b == Some(t) => Some(PlannedFile::Blob(*o)),
            (_, Some(_), Some(t)) if b == ours.get(key) => Some(PlannedFile::Blob(*t)),
            (_, Some(o), Some(t)) => {
                let theirs_newer = theirs_is_newer(&to_path(key), input);
                let values = (input.ours.json.get(&o.oid), input.theirs.json.get(&t.oid));
                Some(match values {
                    (Some(ov), Some(tv)) => {
                        let bv = b.and_then(|b| input.base.json.get(&b.oid));
                        let mut fields = Vec::new();
                        let merged =
                            merge_fields(bv, Some(ov), Some(tv), theirs_newer, "", &mut fields);
                        replaced.extend(fields.into_iter().map(|f| (key.clone(), f)));
                        PlannedFile::Merged(merged.unwrap_or(Value::Null))
                    }
                    // Not JSON: the newest file wins whole.
                    _ => PlannedFile::Blob(if theirs_newer { *t } else { *o }),
                })
            }
        };
        if let Some(planned) = planned {
            out.insert(key.clone(), planned);
        }
    }
    out
}

/// Three-way merge of one JSON value, recursing into objects such as a
/// membership's per-agent `tools`. A field changed on both sides takes the
/// newest side's value, and its dotted name goes into `replaced`.
fn merge_fields(
    b: Option<&Value>,
    o: Option<&Value>,
    t: Option<&Value>,
    theirs_newer: bool,
    field: &str,
    replaced: &mut Vec<String>,
) -> Option<Value> {
    if o == t || t == b {
        return o.cloned();
    }
    if o == b {
        return t.cloned();
    }
    if let (Some(Value::Object(om)), Some(Value::Object(tm))) = (o, t) {
        let bm = b.and_then(Value::as_object);
        let keys: BTreeSet<&String> = om
            .keys()
            .chain(tm.keys())
            .chain(bm.into_iter().flat_map(|m| m.keys()))
            .collect();
        let merged = keys
            .into_iter()
            .filter_map(|k| {
                let name = if field.is_empty() {
                    k.clone()
                } else {
                    format!("{field}.{k}")
                };
                let bv = bm.and_then(|m| m.get(k));
                merge_fields(bv, om.get(k), tm.get(k), theirs_newer, &name, replaced)
                    .map(|v| (k.clone(), v))
            })
            .collect();
        return Some(Value::Object(merged));
    }
    replaced.push(field.to_string());
    if theirs_newer {
        t.cloned()
    } else {
        o.cloned()
    }
}

/// The preset a side switched to since the base (SID-104).
fn switched_to(snap: &Snapshot, input: &DecisionInput) -> Option<String> {
    let active = |snap: &Snapshot| {
        let entry = snap
            .residual
            .get(&format!("{METADATA_DIR}/{ACTIVE_PRESET_FILE}"))?;
        snap.json.get(&entry.oid)?["scenario_id"]
            .as_str()
            .map(str::to_owned)
    };
    active(snap).filter(|id| active(input.base).as_ref() != Some(id))
}

fn planned_value<'a>(file: &'a PlannedFile, input: &'a DecisionInput) -> Option<&'a Value> {
    match file {
        PlannedFile::Merged(value) => Some(value),
        PlannedFile::Blob(entry) => [input.ours, input.theirs, input.base]
            .into_iter()
            .find_map(|snap| snap.json.get(&entry.oid)),
    }
}

fn preset_name(
    scenarios: &BTreeMap<String, PlannedFile>,
    id: &str,
    input: &DecisionInput,
) -> String {
    scenarios
        .get(id)
        .and_then(|file| planned_value(file, input)?["name"].as_str())
        .unwrap_or(id)
        .to_string()
}

fn field_label(field: &str) -> String {
    match field {
        "sort_order" => "order".to_string(),
        _ => match field.strip_prefix("tools.") {
            Some(agent) => format!("on {agent}"),
            None => field.to_string(),
        },
    }
}

fn fold_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Presets with the same name and different ids stay apart (SID-104): one
/// keeps the name and the others become "Name (2)", "Name (3)"…, marked for
/// review. The preset that had the name in the base keeps it, else the
/// smallest id. A clash either side already had is left alone.
fn separate_same_name_presets(plan: &mut MergePlan, input: &DecisionInput) {
    let name_on = |snap: &Snapshot, id: &str| {
        let entry = snap.scenarios.get(id)?;
        snap.json.get(&entry.oid)?["name"].as_str().map(fold_name)
    };
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, file) in &plan.scenarios {
        if let Some(name) = planned_value(file, input).and_then(|v| v["name"].as_str()) {
            groups.entry(fold_name(name)).or_default().push(id.clone());
        }
    }
    let mut taken: BTreeSet<String> = groups.keys().cloned().collect();
    for (name, ids) in groups {
        let had_it = |snap: &Snapshot| {
            ids.iter()
                .all(|id| name_on(snap, id).as_ref() == Some(&name))
        };
        if ids.len() < 2 || had_it(input.ours) || had_it(input.theirs) {
            continue;
        }
        let keeper = ids
            .iter()
            .find(|id| name_on(input.base, id).as_ref() == Some(&name))
            .unwrap_or(&ids[0])
            .clone();
        for id in ids.iter().filter(|id| **id != keeper) {
            let Some(mut value) = planned_value(&plan.scenarios[id], input).cloned() else {
                continue;
            };
            let original = value["name"].as_str().unwrap_or_default().to_string();
            let renamed = (2..)
                .map(|n| format!("{original} ({n})"))
                .find(|candidate| taken.insert(fold_name(candidate)))
                .unwrap_or(original);
            value["name"] = Value::String(renamed);
            plan.scenarios
                .insert(id.clone(), PlannedFile::Merged(value));
            plan.same_name.insert(id.clone(), keeper.clone());
        }
    }
}

/// Whether theirs touched `path` last: committer time, then commit id (§3 新者胜).
fn theirs_is_newer(path: &str, input: &DecisionInput) -> bool {
    match (input.ours_touch.get(path), input.theirs_touch.get(path)) {
        (Some(o), Some(t)) => t > o,
        (None, Some(_)) => true,
        // No touch info for a diverged path shouldn't happen: keep ours.
        _ => false,
    }
}

fn newest_wins<K>(
    key: &K,
    ours: Option<FileEntry>,
    theirs: Option<FileEntry>,
    to_path: &impl Fn(&K) -> String,
    input: &DecisionInput,
) -> Option<FileEntry> {
    if theirs_is_newer(&to_path(key), input) {
        theirs
    } else {
        ours
    }
}

/// Path-collision pass (§3): group the FINAL skill set by folded path key;
/// pendings are immovable placeholders, then the base holder, then the
/// smallest skill_id; everyone else moves to `path (2)`, `(3)`, … in
/// skill_id order.
fn resolve_path_collisions(plan: &mut MergePlan, input: &DecisionInput) -> Result<()> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, skill) in &plan.skills {
        groups
            .entry(skill.meta.path_key.clone())
            .or_default()
            .push(id.clone());
    }

    let mut occupied: BTreeSet<String> = groups.keys().cloned().collect();
    let base_holder_by_key: BTreeMap<String, String> = input
        .base
        .skills
        .iter()
        .map(|(id, s)| (path_key(&s.meta.path), id.clone()))
        .collect();
    let pending_ids: BTreeSet<&String> = input.pinned.keys().collect();

    for (key, mut ids) in groups {
        if ids.len() < 2 {
            continue;
        }
        ids.sort();

        let pending_holders: Vec<&String> =
            ids.iter().filter(|id| pending_ids.contains(id)).collect();
        if pending_holders.len() > 1 {
            // §3-4: two immovable placeholders on one key is an invariant
            // violation — abort with zero changes.
            bail!(
                "path collision on '{key}' between two pending skills ({}) — resolve the pending conflicts first",
                pending_holders
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let placeholder: Option<String> = pending_holders
            .first()
            .map(|s| (*s).clone())
            .or_else(|| {
                base_holder_by_key
                    .get(&key)
                    .filter(|holder| ids.contains(holder))
                    .cloned()
            })
            .or_else(|| ids.first().cloned());

        for id in &ids {
            if Some(id) == placeholder.as_ref() {
                continue;
            }
            let skill = plan.skills.get_mut(id).expect("grouped id exists");
            let mut n = 2u32;
            let (new_path, new_key) = loop {
                let candidate = suffixed_path(&skill.meta.path, n);
                let candidate_key = path_key(&candidate);
                if !occupied.contains(&candidate_key) {
                    break (candidate, candidate_key);
                }
                n += 1;
            };
            occupied.insert(new_key.clone());
            skill.meta.path = new_path;
            skill.meta.path_key = new_key;
            skill.renamed_for_collision = true;
        }
    }
    Ok(())
}

/// Residual files inside the managed metadata subdirectories that the app
/// never writes: anything non-`.json` under skills/scenarios/scenario-skills,
/// plus atomic-write temp leftovers anywhere under `.skills-manager/`.
pub(crate) fn is_metadata_namespace_junk(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(".skills-manager/") else {
        return false;
    };
    if rest.contains(".tmp.") {
        return true;
    }
    (rest.starts_with("skills/")
        || rest.starts_with("scenarios/")
        || rest.starts_with("scenario-skills/"))
        && !rest.ends_with(".json")
}

fn suffixed_path(path: &str, n: u32) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/{name} ({n})"),
        None => format!("{path} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sync_metadata::SourceMeta;

    fn oid(n: u8) -> git2::Oid {
        git2::Oid::from_str(&format!("{:040x}", n)).unwrap()
    }

    fn skill(id: &str, path: &str, content: u8, enabled: bool, tags: &[&str]) -> SkillObj {
        SkillObj {
            meta_entry: FileEntry {
                oid: oid(0),
                mode: 0o100644,
            },
            meta: SkillMetaFile {
                schema_version: 1,
                skill_id: id.to_string(),
                path: path.to_string(),
                path_key: path_key(path),
                enabled,
                tags: tags.iter().map(|s| s.to_string()).collect(),
                source: SourceMeta {
                    source_type: "import".to_string(),
                    ref_: None,
                    subpath: None,
                    branch: None,
                },
            },
            content: Some(oid(content)),
        }
    }

    fn snap(skills: Vec<SkillObj>) -> Snapshot {
        let mut s = Snapshot::default();
        for obj in skills {
            s.skills.insert(obj.meta.skill_id.clone(), obj);
        }
        s
    }

    fn decide_simple(base: &Snapshot, ours: &Snapshot, theirs: &Snapshot) -> MergePlan {
        let pinned = BTreeMap::new();
        let empty = BTreeMap::new();
        decide(&DecisionInput {
            base,
            ours,
            theirs,
            pinned: &pinned,
            ours_touch: &empty,
            theirs_touch: &empty,
        })
        .unwrap()
    }

    // ── object-level table (§3, all nine populated branches) ──

    #[test]
    fn object_level_add_and_delete_branches() {
        let base = snap(vec![skill("del-both", "a", 1, true, &[])]);
        let ours = snap(vec![skill("only-ours", "b", 2, true, &[])]);
        let theirs = snap(vec![skill("only-theirs", "c", 3, true, &[])]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(plan.skills.contains_key("only-ours"));
        assert!(plan.skills.contains_key("only-theirs"));
        assert!(!plan.skills.contains_key("del-both"));
        assert!(plan.new_conflicts.is_empty());
        assert_eq!(plan.updated_from_theirs, vec!["only-theirs"]);
    }

    #[test]
    fn double_add_identical_takes_one_divergent_conflicts() {
        let base = snap(vec![]);
        let same_o = skill("same", "s", 1, true, &[]);
        let same_t = skill("same", "s", 1, true, &[]);
        let div_o = skill("div", "d", 1, true, &[]);
        let div_t = skill("div", "d", 2, true, &[]);
        let ours = snap(vec![same_o, div_o]);
        let theirs = snap(vec![same_t, div_t]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(plan.skills.contains_key("same"));
        // divergent double-add keeps ours and declares a conflict
        assert_eq!(plan.skills["div"].content, Some(oid(1)));
        assert_eq!(plan.new_conflicts.len(), 1);
        assert_eq!(plan.new_conflicts[0].skill_id, "div");
        assert_eq!(plan.kept_local, vec!["div"]);
    }

    #[test]
    fn deletion_propagates_only_when_other_side_unchanged() {
        let base = snap(vec![
            skill("clean-del", "a", 1, true, &[]),
            skill("del-vs-edit", "b", 2, true, &[]),
            skill("edit-vs-del", "c", 3, true, &[]),
        ]);
        // ours deleted clean-del and del-vs-edit; edited edit-vs-del
        let ours = snap(vec![skill("edit-vs-del", "c", 4, true, &[])]);
        // theirs kept clean-del as base (deletion propagates), edited
        // del-vs-edit (conflict), deleted edit-vs-del (conflict)
        let theirs = snap(vec![
            skill("clean-del", "a", 1, true, &[]),
            skill("del-vs-edit", "b", 9, true, &[]),
        ]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(!plan.skills.contains_key("clean-del"));
        // del-vs-edit: ours deletion kept, conflict declared with theirs path
        assert!(!plan.skills.contains_key("del-vs-edit"));
        // edit-vs-del: ours edit kept, conflict declared
        assert_eq!(plan.skills["edit-vs-del"].content, Some(oid(4)));
        let mut conflicted: Vec<&str> = plan
            .new_conflicts
            .iter()
            .map(|c| c.skill_id.as_str())
            .collect();
        conflicted.sort();
        assert_eq!(conflicted, vec!["del-vs-edit", "edit-vs-del"]);
    }

    // ── component-level (§3 组件级) ──

    #[test]
    fn content_edit_plus_rename_compose() {
        let base = snap(vec![skill("s", "old-name", 1, true, &["x"])]);
        let ours = snap(vec![skill("s", "old-name", 2, true, &["x"])]); // content edit
        let theirs = snap(vec![skill("s", "new-name", 1, true, &["x"])]); // rename
        let plan = decide_simple(&base, &ours, &theirs);
        let s = &plan.skills["s"];
        assert_eq!(s.content, Some(oid(2)));
        assert_eq!(s.meta.path, "new-name");
        assert_eq!(s.meta.path_key, path_key("new-name"));
        assert!(plan.new_conflicts.is_empty());
        assert_eq!(plan.updated_from_theirs, vec!["s"]);
    }

    #[test]
    fn attrs_move_as_one_component() {
        let base = snap(vec![skill("s", "p", 1, true, &["a"])]);
        // ours toggles enabled; theirs edits tags → same component both
        // changed, unequal → whole-skill conflict, ours kept.
        let ours = snap(vec![skill("s", "p", 1, false, &["a"])]);
        let theirs = snap(vec![skill("s", "p", 1, true, &["a", "b"])]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(!plan.skills["s"].meta.enabled);
        assert_eq!(plan.new_conflicts.len(), 1);
    }

    #[test]
    fn same_component_equal_change_is_not_conflict() {
        let base = snap(vec![skill("s", "p", 1, true, &[])]);
        let ours = snap(vec![skill("s", "p", 2, true, &[])]);
        let theirs = snap(vec![skill("s", "p", 2, true, &[])]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.skills["s"].content, Some(oid(2)));
        assert!(plan.new_conflicts.is_empty());
        assert!(plan.updated_from_theirs.is_empty());
    }

    #[test]
    fn both_content_changed_unequal_conflicts_and_keeps_ours() {
        let base = snap(vec![skill("s", "p", 1, true, &[])]);
        let ours = snap(vec![skill("s", "p", 2, true, &[])]);
        let theirs = snap(vec![skill("s", "p", 3, true, &[])]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.skills["s"].content, Some(oid(2)));
        assert_eq!(plan.new_conflicts.len(), 1);
        assert_eq!(plan.new_conflicts[0].theirs_path.as_deref(), Some("p"));
    }

    // ── pinned pendings (§4) ──

    #[test]
    fn pinned_skill_bypasses_decision_and_keeps_pin_side() {
        let base = snap(vec![skill("s", "p", 1, true, &[])]);
        let ours = snap(vec![skill("s", "p", 2, true, &[])]);
        let theirs = snap(vec![skill("s", "p", 3, true, &[])]);
        let mut pinned = BTreeMap::new();
        pinned.insert("s".to_string(), Side::Theirs);
        let empty = BTreeMap::new();
        let plan = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &pinned,
            ours_touch: &empty,
            theirs_touch: &empty,
        })
        .unwrap();
        assert_eq!(plan.skills["s"].content, Some(oid(3)));
        assert!(plan.new_conflicts.is_empty());
        assert_eq!(plan.still_pending, vec!["s"]);
    }

    // ── whole-file newest-wins ──

    #[test]
    fn whole_file_double_edit_newest_wins() {
        let entry = |n: u8| FileEntry {
            oid: oid(n),
            mode: 0o100644,
        };
        let mut base = Snapshot::default();
        let mut ours = Snapshot::default();
        let mut theirs = Snapshot::default();
        base.scenarios.insert("sc".into(), entry(1));
        ours.scenarios.insert("sc".into(), entry(2));
        theirs.scenarios.insert("sc".into(), entry(3));

        let path = format!("{}/scenarios/sc.json", super::super::snapshot::METADATA_DIR);
        let mut ours_touch = BTreeMap::new();
        let mut theirs_touch = BTreeMap::new();
        ours_touch.insert(path.clone(), (100, "aaa".to_string()));
        theirs_touch.insert(path.clone(), (200, "bbb".to_string()));

        let pinned = BTreeMap::new();
        let plan = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &pinned,
            ours_touch: &ours_touch,
            theirs_touch: &theirs_touch,
        })
        .unwrap();
        assert_eq!(plan.scenarios["sc"], PlannedFile::Blob(entry(3)));

        // Flip the clock: ours newer → ours wins.
        ours_touch.insert(path.clone(), (300, "aaa".to_string()));
        let plan = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &pinned,
            ours_touch: &ours_touch,
            theirs_touch: &theirs_touch,
        })
        .unwrap();
        assert_eq!(plan.scenarios["sc"], PlannedFile::Blob(entry(2)));
    }

    #[test]
    fn membership_orphans_are_dropped() {
        let entry = FileEntry {
            oid: oid(9),
            mode: 0o100644,
        };
        let mut theirs = Snapshot::default();
        theirs
            .memberships
            .insert(("sc".to_string(), "missing-skill".to_string()), entry);
        let base = Snapshot::default();
        let ours = Snapshot::default();
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(plan.memberships.is_empty());
    }

    // ── presets and memberships per field (SID-104) ──

    fn preset(id: &str, name: &str, icon: &str) -> Value {
        serde_json::json!({
            "schema_version": 1, "scenario_id": id, "name": name,
            "description": null, "icon": icon, "sort_order": 0,
        })
    }

    /// Put a parsed file into `snap` under blob id `n`.
    fn file(snap: &mut Snapshot, n: u8, value: Value) -> FileEntry {
        let entry = FileEntry {
            oid: oid(n),
            mode: 0o100644,
        };
        snap.json.insert(entry.oid, value);
        entry
    }

    fn put_preset(snap: &mut Snapshot, n: u8, id: &str, name: &str, icon: &str) {
        let entry = file(snap, n, preset(id, name, icon));
        snap.scenarios.insert(id.to_string(), entry);
    }

    fn put_member(snap: &mut Snapshot, n: u8, tools: Value) {
        let value = serde_json::json!({
            "schema_version": 1, "scenario_id": "p", "skill_id": "s",
            "sort_order": 0, "tools": tools,
        });
        let entry = file(snap, n, value);
        snap.memberships
            .insert(("p".to_string(), "s".to_string()), entry);
        snap.skills
            .insert("s".to_string(), skill("s", "s", 1, true, &[]));
    }

    fn switch_to(snap: &mut Snapshot, n: u8, id: &str) {
        let entry = file(
            snap,
            n,
            serde_json::json!({"schema_version": 1, "scenario_id": id}),
        );
        snap.residual
            .insert(format!("{METADATA_DIR}/{ACTIVE_PRESET_FILE}"), entry);
    }

    #[test]
    fn a_rename_and_an_icon_change_both_survive() {
        let (mut base, mut ours, mut theirs) = Default::default();
        put_preset(&mut base, 1, "p", "Web", "a");
        put_preset(&mut ours, 2, "p", "Frontend", "a");
        put_preset(&mut theirs, 3, "p", "Web", "globe");
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(
            plan.scenarios["p"],
            PlannedFile::Merged(preset("p", "Frontend", "globe"))
        );
        assert!(plan.replaced.is_empty());
    }

    #[test]
    fn the_same_field_changed_on_both_keeps_the_newest_and_says_so() {
        let (mut base, mut ours, mut theirs) = Default::default();
        put_preset(&mut base, 1, "p", "Web", "a");
        put_preset(&mut ours, 2, "p", "Frontend", "a");
        put_preset(&mut theirs, 3, "p", "Site", "globe");
        let path = format!("{METADATA_DIR}/scenarios/p.json");
        let ours_touch = BTreeMap::from([(path.clone(), (100, "aaa".to_string()))]);
        let theirs_touch = BTreeMap::from([(path, (200, "bbb".to_string()))]);
        let plan = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &BTreeMap::new(),
            ours_touch: &ours_touch,
            theirs_touch: &theirs_touch,
        })
        .unwrap();
        assert_eq!(
            plan.scenarios["p"],
            PlannedFile::Merged(preset("p", "Site", "globe"))
        );
        assert_eq!(plan.replaced, ["Site name"]);
    }

    #[test]
    fn a_preset_delete_loses_to_a_switch_or_an_edit() {
        let (mut base, mut ours, mut theirs) = Default::default();
        for (snap, n) in [(&mut base, 1), (&mut ours, 1), (&mut theirs, 1)] {
            put_preset(snap, n, "p", "Web", "a");
            put_member(snap, n + 10, serde_json::json!({}));
            switch_to(snap, n + 20, "q");
        }
        // Ours deletes the preset and its skills.
        ours.scenarios.clear();
        ours.memberships.clear();
        assert!(decide_simple(&base, &ours, &theirs).scenarios.is_empty());

        // Theirs switched to it meanwhile: it comes back with its skills.
        let mut switched = Snapshot::default();
        put_preset(&mut switched, 1, "p", "Web", "a");
        put_member(&mut switched, 11, serde_json::json!({}));
        switch_to(&mut switched, 22, "p");
        let plan = decide_simple(&base, &ours, &switched);
        assert_eq!(plan.scenarios["p"], PlannedFile::Blob(base.scenarios["p"]));
        assert_eq!(plan.memberships.len(), 1);

        // Theirs renamed it meanwhile: the rename stays.
        let mut edited = Snapshot::default();
        put_preset(&mut edited, 2, "p", "Frontend", "a");
        put_member(&mut edited, 11, serde_json::json!({}));
        switch_to(&mut edited, 21, "q");
        let plan = decide_simple(&base, &ours, &edited);
        assert_eq!(
            plan.scenarios["p"],
            PlannedFile::Blob(edited.scenarios["p"])
        );
    }

    #[test]
    fn toggles_merge_per_agent_and_a_removal_loses_to_a_toggle() {
        let (mut base, mut ours, mut theirs) = Default::default();
        for snap in [&mut base, &mut ours, &mut theirs] {
            put_preset(snap, 1, "p", "Web", "a");
        }
        put_member(&mut base, 11, serde_json::json!({}));
        put_member(&mut ours, 12, serde_json::json!({"claude": false}));
        put_member(&mut theirs, 13, serde_json::json!({"codex": false}));
        let plan = decide_simple(&base, &ours, &theirs);
        let PlannedFile::Merged(member) = &plan.memberships[&("p".into(), "s".into())] else {
            panic!("expected a merged membership");
        };
        assert_eq!(
            member["tools"],
            serde_json::json!({"claude": false, "codex": false})
        );

        // Ours took the skill out of the preset; theirs turned an agent off.
        ours.memberships.clear();
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(
            plan.memberships[&("p".into(), "s".into())],
            PlannedFile::Blob(theirs.memberships[&("p".into(), "s".into())])
        );
    }

    #[test]
    fn same_name_presets_from_two_hosts_stay_apart() {
        let (base, mut ours, mut theirs) = Default::default();
        put_preset(&mut ours, 1, "a", "Web", "x");
        put_preset(&mut theirs, 2, "b", "web", "y");
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.scenarios["a"], PlannedFile::Blob(ours.scenarios["a"]));
        assert_eq!(
            plan.scenarios["b"],
            PlannedFile::Merged(preset("b", "web (2)", "y"))
        );
        assert_eq!(plan.same_name, BTreeMap::from([("b".into(), "a".into())]));

        // A clash a side already had is left alone.
        put_preset(&mut ours, 2, "b", "web", "y");
        let plan = decide_simple(&base, &ours, &theirs);
        assert!(plan.same_name.is_empty());
    }

    // ── path collisions (§3 碰撞) ──

    #[test]
    fn collision_base_holder_stays_migrant_moves() {
        // base holder keeps the key even with a larger skill_id.
        let base = snap(vec![skill("z-holder", "spot", 1, true, &[])]);
        let ours = snap(vec![skill("z-holder", "spot", 1, true, &[])]);
        let theirs = snap(vec![
            skill("z-holder", "spot", 1, true, &[]),
            skill("a-migrant", "Spot", 2, true, &[]), // folds to same key
        ]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.skills["z-holder"].meta.path, "spot");
        assert_eq!(plan.skills["a-migrant"].meta.path, "Spot (2)");
        assert!(plan.skills["a-migrant"].renamed_for_collision);
        assert_eq!(plan.skills["a-migrant"].meta.path_key, path_key("Spot (2)"));
    }

    #[test]
    fn collision_double_add_smallest_id_stays() {
        let base = snap(vec![]);
        let ours = snap(vec![skill("bbb", "dir/nAme", 1, true, &[])]);
        let theirs = snap(vec![skill("aaa", "dir/name", 2, true, &[])]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.skills["aaa"].meta.path, "dir/name");
        assert_eq!(plan.skills["bbb"].meta.path, "dir/nAme (2)");
    }

    #[test]
    fn collision_pending_placeholder_beats_base_holder() {
        // Pending skill (declared, pinned) occupies the key; even the base
        // holder must yield (§3 rule 1 — pending outranks base).
        let base = snap(vec![skill("holder", "spot", 1, true, &[])]);
        let ours = snap(vec![skill("holder", "spot", 1, true, &[])]);
        let theirs = snap(vec![
            skill("holder", "spot", 1, true, &[]),
            skill("pending-skill", "SPOT", 5, true, &[]),
        ]);
        let mut pinned = BTreeMap::new();
        pinned.insert("pending-skill".to_string(), Side::Theirs);
        let empty = BTreeMap::new();
        let plan = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &pinned,
            ours_touch: &empty,
            theirs_touch: &empty,
        })
        .unwrap();
        assert_eq!(plan.skills["pending-skill"].meta.path, "SPOT");
        assert_eq!(plan.skills["holder"].meta.path, "spot (2)");
    }

    #[test]
    fn collision_two_pendings_abort() {
        let base = snap(vec![]);
        let ours = snap(vec![skill("p1", "spot", 1, true, &[])]);
        let theirs = snap(vec![skill("p2", "SPOT", 2, true, &[])]);
        let mut pinned = BTreeMap::new();
        pinned.insert("p1".to_string(), Side::Ours);
        pinned.insert("p2".to_string(), Side::Theirs);
        let empty = BTreeMap::new();
        let err = decide(&DecisionInput {
            base: &base,
            ours: &ours,
            theirs: &theirs,
            pinned: &pinned,
            ours_touch: &empty,
            theirs_touch: &empty,
        })
        .unwrap_err();
        assert!(err.to_string().contains("pending"));
    }

    #[test]
    fn collision_rename_chain_finds_free_slot() {
        let base = snap(vec![skill("keeper", "n", 1, true, &[])]);
        let ours = snap(vec![
            skill("keeper", "n", 1, true, &[]),
            skill("aaa", "n (2)", 2, true, &[]),
        ]);
        let theirs = snap(vec![
            skill("keeper", "n", 1, true, &[]),
            skill("zzz", "N", 3, true, &[]),
        ]);
        let plan = decide_simple(&base, &ours, &theirs);
        assert_eq!(plan.skills["keeper"].meta.path, "n");
        assert_eq!(plan.skills["aaa"].meta.path, "n (2)");
        // "n (2)" is taken, so the migrant lands on "N (3)"... wait — suffix
        // applies to its own name "N": "N (2)" folds to occupied "n (2)" →
        // next free is "N (3)".
        assert_eq!(plan.skills["zzz"].meta.path, "N (3)");
    }
}
