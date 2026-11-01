//! Installation work uses the coordinator's library writer, never a project reader.
use super::*;
use crate::core::{
    central_repo, installer,
    skill_install::{store_installed_skill_unlocked, InstallSourceMetadata},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallSource {
    Local {
        path: String,
        name: Option<String>,
        origin: Option<LocalOrigin>,
    },
    LocalBatch {
        path: String,
    },
    Git {
        url: String,
    },
    Marketplace {
        source: String,
        skill_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalOrigin {
    Discovered,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct InstallWork {
    source: InstallSource,
    pub prepared: Option<Prepared>,
    preview: Vec<(crate::commands::skills::GitSkillPreview, Prepared)>,
    #[serde(default)]
    staging: Vec<std::path::PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Prepared {
    path: std::path::PathBuf,
    name: String,
    source_content: String,
    source_identity: Option<std::path::PathBuf>,
    content: String,
    metadata: InstallSourceMetadata,
    existing: Option<(String, String, String)>,
    #[serde(default)]
    destination: Option<std::path::PathBuf>,
}

pub(super) fn submit(
    ctx: &HostCtx,
    operation_id: String,
    host_id: Option<String>,
    mut sources: Vec<InstallSource>,
) -> Result<Value, AppError> {
    if sources
        .iter()
        .any(|source| matches!(source, InstallSource::Git { .. }))
        && sources.len() != 1
    {
        return Err(AppError::invalid_input(
            "Prepare each Git repository separately before selecting skills",
        ));
    }
    for source in &mut sources {
        sanitize(source)?;
    }
    let mut state = ctx.operations.0.lock().unwrap();
    if let Some(record) = state
        .records
        .iter()
        .find(|r| r.snapshot.operation_id == operation_id)
    {
        return Ok(json!(record.snapshot));
    }
    if let Some(value) = ctx
        .store
        .resource_state_get(RECEIPT, &operation_id)
        .map_err(AppError::db)?
    {
        let record: Record = serde_json::from_value(value).map_err(AppError::db)?;
        return Ok(json!(unowned_snapshot(record.snapshot)));
    }
    if sources.is_empty() {
        return Err(AppError::invalid_input("Select a source"));
    }
    let intent = sources.clone();
    let mut expanded = vec![];
    for source in sources {
        if let InstallSource::LocalBatch { path } = source {
            for dir in crate::commands::skills::collect_local_batch(&path)? {
                let name = crate::core::skill_metadata::infer_skill_name(&dir);
                let prospective = central_repo::skills_dir().join(&name);
                if ctx
                    .store
                    .get_skill_by_central_path(&prospective.to_string_lossy())
                    .map_err(AppError::db)?
                    .is_none()
                {
                    expanded.push(InstallSource::Local {
                        path: dir.to_string_lossy().into(),
                        name: Some(name),
                        origin: None,
                    });
                }
            }
        } else {
            expanded.push(source);
        }
    }
    let mut sources = vec![];
    for source in expanded {
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    if let Some(record) = state.records.iter().find(|r| {
        !r.snapshot.settled
            && sources
                .iter()
                .all(|s| r.installs.iter().any(|i| &i.source == s))
    }) {
        return Ok(json!(record.snapshot));
    }
    // An uncertain result is not permission to apply that source again.
    if let Some(record) = state.records.iter().find(|record| {
        record
            .snapshot
            .items
            .iter()
            .zip(&record.installs)
            .any(|(item, work)| {
                item.phase == Phase::OutcomeUnknown && sources.contains(&work.source)
            })
    }) {
        return Ok(json!(record.snapshot));
    }
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let mut record = Record {
        snapshot: OperationSnapshot {
            operation_id,
            attempt_id: attempt_id.clone(),
            host_id,
            work: OperationWork::Installation,
            project_id: None,
            project_path: None,
            destination: Some(Destination::Library {
                path: central_repo::skills_dir()
                    .canonicalize()?
                    .to_string_lossy()
                    .into(),
            }),
            preview: None,
            sources: intent,
            revision: 0,
            settled: false,
            cancel_requested: false,
            items: sources
                .iter()
                .map(|source| OperationItem {
                    id: uuid::Uuid::new_v4().to_string(),
                    attempt_id: attempt_id.clone(),
                    relative_path: match source {
                        InstallSource::Local { path, .. } | InstallSource::LocalBatch { path } => {
                            path.clone()
                        }
                        InstallSource::Git { url } => url.clone(),
                        InstallSource::Marketplace { source, skill_id } => {
                            format!("{source}/{skill_id}")
                        }
                    },
                    agent: None,
                    source: Some(source.clone()),
                    phase: Phase::Queued,
                    message: None,
                })
                .collect(),
        },
        updates: vec![],
        installs: sources
            .into_iter()
            .map(|source| InstallWork {
                source,
                prepared: None,
                preview: vec![],
                staging: vec![],
            })
            .collect(),
        attempts: vec![attempt_id],
    };
    save(ctx, &mut record)?;
    let snapshot = record.snapshot.clone();
    state.records.push(record);
    if !state.driving {
        state.driving = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || drive(ctx));
    }
    Ok(json!(snapshot))
}

fn local_metadata(path: &str, origin: &Option<LocalOrigin>) -> InstallSourceMetadata {
    InstallSourceMetadata {
        source_type: if origin.is_some() { "import" } else { "local" }.into(),
        source_ref: Some(path.into()),
        source_ref_resolved: None,
        source_subpath: None,
        source_branch: None,
        source_revision: None,
        remote_revision: None,
        update_status: "local_only".into(),
    }
}

pub(super) fn execute(
    ctx: &HostCtx,
    id: &str,
    index: usize,
    work: InstallWork,
) -> Result<Phase, AppError> {
    if ctx.operations.cancel_requested(id) {
        return Ok(Phase::Canceled);
    }
    let destination = {
        let state = ctx.operations.0.lock().unwrap();
        state
            .records
            .iter()
            .find(|r| r.snapshot.operation_id == id)
            .and_then(|r| r.snapshot.destination.clone())
    };
    if !matches!(destination, Some(Destination::Library {path}) if central_repo::skills_dir().canonicalize().ok().as_ref() == Some(&std::path::PathBuf::from(&path)))
    {
        return Ok(Phase::NeedsReview);
    }

    let Some(prepared) = work.prepared else {
        match &work.source {
            InstallSource::LocalBatch { .. } => {
                unreachable!("batch sources expand before admission")
            }
            InstallSource::Local { path, name, origin } => {
                let source = std::path::Path::new(path);
                let source_identity = source.canonicalize()?;
                let source_content = installer::hash_local_source(source).map_err(AppError::io)?;
                let stage = tempfile::Builder::new()
                    .prefix("skills-manager-install-")
                    .tempdir()?;
                let result = installer::install_from_local_to_destination(
                    source,
                    name.as_deref(),
                    &stage.path().join("skill"),
                )
                .map_err(AppError::io)?;
                if installer::hash_local_source(source).map_err(AppError::io)? != source_content {
                    return Ok(Phase::NeedsReview);
                }
                let root = stage.keep();
                let path = root.join("skill");
                remember_staging(ctx, id, index, root);
                let mut prepared = Prepared {
                    path,
                    name: result.name,
                    content: result.content_hash,
                    source_content,
                    source_identity: Some(source_identity),
                    metadata: local_metadata(
                        match &work.source {
                            InstallSource::Local { path, .. } => path,
                            _ => unreachable!(),
                        },
                        origin,
                    ),
                    existing: None,
                    destination: None,
                };
                capture_destination(&mut prepared)?;
                let mut state = ctx.operations.0.lock().unwrap();
                let record = state
                    .records
                    .iter_mut()
                    .find(|r| r.snapshot.operation_id == id)
                    .unwrap();
                record.installs[index].prepared = Some(prepared);
                return Ok(if record.snapshot.cancel_requested {
                    Phase::Canceled
                } else {
                    Phase::Waiting
                });
            }
            InstallSource::Marketplace { source, skill_id } => {
                let url = format!("https://github.com/{source}.git");
                let cancel = ctx.cancel.register(id);
                if ctx.operations.cancel_requested(id) {
                    cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let result = crate::core::git_fetcher::try_clone_repo_ref_scoped(
                    &url,
                    None,
                    None,
                    Some(&cancel),
                    ctx.store.proxy_url().as_deref(),
                    None,
                )
                .map_err(AppError::classify_git_error);
                ctx.cancel.remove(id);
                let temp = result?;
                remember_staging(ctx, id, index, temp.clone());
                let dir =
                    crate::core::skill_install::resolve_skill_dir(&temp, None, Some(skill_id))?;
                let revision =
                    crate::core::git_fetcher::get_head_revision(&temp).map_err(AppError::git)?;
                let source_ref = format!("{source}/{skill_id}");
                let existing = ctx
                    .store
                    .get_skill_by_source_ref("skillssh", &source_ref)
                    .map_err(AppError::db)?;
                let evidence = if let Some(existing) = &existing {
                    let actual =
                        installer::hash_local_source(std::path::Path::new(&existing.central_path))
                            .map_err(AppError::io)?;
                    if existing.content_hash.as_deref() != Some(actual.as_str()) {
                        return Ok(Phase::NeedsReview);
                    }
                    Some((existing.id.clone(), existing.central_path.clone(), actual))
                } else {
                    None
                };
                let content = installer::hash_local_source(&dir).map_err(AppError::io)?;
                let metadata = InstallSourceMetadata {
                    source_type: "skillssh".into(),
                    source_ref: Some(source_ref),
                    source_ref_resolved: Some(url),
                    source_subpath: crate::core::git_fetcher::relative_subpath(&temp, &dir),
                    source_branch: None,
                    source_revision: Some(revision.clone()),
                    remote_revision: Some(revision),
                    update_status: "up_to_date".into(),
                };
                let mut prepared = Prepared {
                    path: dir,
                    name: existing.map(|s| s.name).unwrap_or_else(|| skill_id.clone()),
                    source_identity: None,
                    source_content: content.clone(),
                    content,
                    metadata,
                    existing: evidence,
                    destination: None,
                };
                capture_destination(&mut prepared)?;
                let mut state = ctx.operations.0.lock().unwrap();
                let record = state
                    .records
                    .iter_mut()
                    .find(|r| r.snapshot.operation_id == id)
                    .unwrap();
                record.installs[index].prepared = Some(prepared);
                return Ok(if record.snapshot.cancel_requested {
                    Phase::Canceled
                } else {
                    Phase::Waiting
                });
            }
            InstallSource::Git { url } => {
                let preview = crate::commands::skills::preview_git_install_operation(
                    ctx,
                    url.clone(),
                    id.into(),
                )?;
                let temp = std::path::Path::new(&preview.temp_dir);
                remember_staging(ctx, id, index, temp.to_path_buf());
                let parsed = crate::core::git_fetcher::parse_git_source(url);
                let root = crate::core::skill_install::resolve_skill_dir(
                    temp,
                    parsed.subpath.as_deref(),
                    None,
                )?;
                let revision =
                    crate::core::git_fetcher::get_head_revision(temp).map_err(AppError::git)?;
                let mut entries = vec![];
                for skill in &preview.skills {
                    let dir = crate::commands::skills::collect_git_skill_dirs(&root)
                        .into_iter()
                        .find(|dir| {
                            crate::commands::skills::skill_rel_key(&root, dir) == skill.rel_path
                        })
                        .unwrap();
                    let content = installer::hash_local_source(&dir).map_err(AppError::io)?;
                    let metadata = InstallSourceMetadata {
                        source_type: "git".into(),
                        source_ref: Some(url.clone()),
                        source_ref_resolved: Some(parsed.clone_url.clone()),
                        source_subpath: crate::core::git_fetcher::relative_subpath(temp, &dir),
                        source_branch: parsed.branch.clone(),
                        source_revision: Some(revision.clone()),
                        remote_revision: Some(revision.clone()),
                        update_status: "up_to_date".into(),
                    };
                    entries.push((
                        skill.clone(),
                        Prepared {
                            path: dir,
                            name: skill.name.clone(),
                            content: content.clone(),
                            source_identity: None,
                            source_content: content,
                            metadata,
                            existing: None,
                            destination: None,
                        },
                    ));
                }
                let mut state = ctx.operations.0.lock().unwrap();
                let record = state
                    .records
                    .iter_mut()
                    .find(|r| r.snapshot.operation_id == id)
                    .unwrap();
                record.snapshot.preview = Some(preview.skills);
                record.installs[index].preview = entries;
                return Ok(if record.snapshot.cancel_requested {
                    Phase::Canceled
                } else {
                    Phase::NeedsReview
                });
            }
        }
    };
    if let InstallSource::Local { path, .. } = &work.source {
        if Some(&std::path::Path::new(path).canonicalize()?) != prepared.source_identity.as_ref()
            || installer::hash_local_source(std::path::Path::new(path)).map_err(AppError::io)?
                != prepared.source_content
        {
            return Ok(Phase::NeedsReview);
        }
    }
    if !prepared.path.exists() {
        return Ok(Phase::NeedsReview);
    }
    if installer::hash_local_source(&prepared.path).map_err(AppError::io)? != prepared.content {
        return Ok(Phase::NeedsReview);
    }
    if ctx.operations.cancel_requested(id) {
        return Ok(Phase::Canceled);
    }
    let Some(destination) = &prepared.destination else {
        return Ok(Phase::NeedsReview);
    };
    if destination.parent().and_then(|p| p.canonicalize().ok())
        != central_repo::skills_dir().canonicalize().ok()
        || std::fs::symlink_metadata(destination).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(Phase::NeedsReview);
    }
    if let Some((id, path, content)) = &prepared.existing {
        let Some(current) = ctx.store.get_skill_by_id(id).map_err(AppError::db)? else {
            return Ok(Phase::NeedsReview);
        };
        if current.central_path != *path
            || current.source_ref != prepared.metadata.source_ref
            || current.source_type != prepared.metadata.source_type
            || current.content_hash.as_deref() != Some(content.as_str())
            || !destination.exists()
            || installer::hash_local_source(destination).map_err(AppError::io)? != *content
        {
            return Ok(Phase::NeedsReview);
        }
    } else if destination.exists()
        && installer::hash_local_source(destination).map_err(AppError::io)? != prepared.content
    {
        return Ok(Phase::NeedsReview);
    }
    if destination.exists()
        && installer::hash_local_source(destination).map_err(AppError::io)? == prepared.content
        && ctx
            .store
            .get_skill_by_central_path(&destination.to_string_lossy())
            .map_err(AppError::db)?
            .is_some()
    {
        return Ok(Phase::Unchanged);
    }
    if ctx.operations.cancel_requested(id) {
        return Ok(Phase::Canceled);
    }
    let result = installer::install_from_local_to_destination(
        &prepared.path,
        Some(&prepared.name),
        destination,
    )
    .map_err(AppError::io)?;
    store_installed_skill_unlocked(&ctx.store, &result, &prepared.metadata, None)?;
    Ok(Phase::Succeeded)
}

pub(super) fn confirm(
    ctx: &HostCtx,
    id: &str,
    attempt_id: String,
    items: Vec<crate::commands::skills::SkillInstallItem>,
) -> Result<Value, AppError> {
    let mut state = ctx.operations.0.lock().unwrap();
    let record = state
        .records
        .iter_mut()
        .find(|r| r.snapshot.operation_id == id)
        .ok_or_else(|| AppError::not_found("Operation not found"))?;
    if record.attempts.contains(&attempt_id) {
        return Ok(json!(record.snapshot));
    }
    if !record.snapshot.settled
        || record.snapshot.cancel_requested
        || record.snapshot.preview.is_none()
        || items.is_empty()
    {
        return Err(AppError::invalid_input(
            "Prepare and review this source again",
        ));
    }
    let original = record.clone();
    let work = &record.installs[0];
    let mut installs = vec![];
    let mut operation_items = vec![];
    for item in items {
        let (_, prepared) = work
            .preview
            .iter()
            .find(|(s, _)| s.rel_path == item.rel_path)
            .ok_or_else(|| AppError::invalid_input("Selection is outside this preview"))?;
        let mut prepared = prepared.clone();
        prepared.name = crate::core::skill_metadata::sanitize_skill_name(&item.name)
            .ok_or_else(|| AppError::invalid_input("Invalid skill name"))?;
        capture_destination(&mut prepared)?;
        installs.push(InstallWork {
            source: work.source.clone(),
            prepared: Some(prepared),
            preview: vec![],
            staging: work.staging.clone(),
        });
        operation_items.push(OperationItem {
            id: uuid::Uuid::new_v4().to_string(),
            attempt_id: attempt_id.clone(),
            relative_path: item.rel_path,
            agent: None,
            source: Some(work.source.clone()),
            phase: Phase::Queued,
            message: None,
        });
    }
    ctx.store
        .resource_state_put(
            "operation_attempt",
            &format!("{id}/{}", record.snapshot.attempt_id),
            &json!(record),
        )
        .map_err(AppError::db)?;
    record.installs = installs;
    record.snapshot.items = operation_items;
    record.snapshot.preview = None;
    record.snapshot.attempt_id = attempt_id.clone();
    record.attempts.push(attempt_id);
    if let Err(e) = save(ctx, record) {
        *record = original;
        return Err(e);
    }
    let snapshot = record.snapshot.clone();
    if !state.driving {
        state.driving = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || drive(ctx));
    }
    Ok(json!(snapshot))
}

fn sanitize(source: &mut InstallSource) -> Result<(), AppError> {
    match source {
        InstallSource::Git { url } => {
            if let Ok(mut parsed) = tauri::Url::parse(url) {
                if parsed.scheme() == "http" || parsed.scheme() == "https" {
                    let _ = parsed.set_username("");
                }
                let _ = parsed.set_password(None);
                parsed.set_query(None);
                parsed.set_fragment(None);
                *url = parsed.to_string();
            }
            crate::core::git_fetcher::validate_git_url(url)
                .map_err(|_| AppError::invalid_input("Use an HTTPS or SSH Git URL"))?;
        }
        InstallSource::Marketplace { source, skill_id } => {
            if source.split('/').count() != 2
                || !source
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_/ .".contains(c) && c != ' ')
                || crate::core::skill_metadata::sanitize_skill_name(skill_id).as_deref()
                    != Some(skill_id.as_str())
            {
                return Err(AppError::invalid_input(
                    "Invalid marketplace source or skill",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn remember_staging(ctx: &HostCtx, id: &str, index: usize, path: std::path::PathBuf) {
    let mut state = ctx.operations.0.lock().unwrap();
    let record = state
        .records
        .iter_mut()
        .find(|r| r.snapshot.operation_id == id)
        .unwrap();
    record.installs[index].staging.push(path);
}

pub(super) fn cleanup(record: &Record) {
    let temp = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    for path in record.installs.iter().flat_map(|work| &work.staging) {
        let owned_name = path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("skills-manager-install-")
                || name.starts_with(crate::core::git_fetcher::CLONE_TEMP_PREFIX)
        });
        if owned_name && path.parent().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&temp) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

fn capture_destination(prepared: &mut Prepared) -> Result<(), AppError> {
    prepared.name = crate::core::skill_metadata::sanitize_skill_name(&prepared.name)
        .ok_or_else(|| AppError::invalid_input("Invalid skill name"))?;
    prepared.destination = Some(if let Some((_, path, _)) = &prepared.existing {
        std::path::PathBuf::from(path)
    } else {
        installer::unique_skill_dest(&central_repo::skills_dir(), &prepared.name, &prepared.path)
            .map_err(AppError::io)?
    });
    Ok(())
}
