//! Portable MCP definitions and deployment management.
//!
//! This module deliberately stores references to credentials, never resolved
//! credential values. Native configuration files are treated as user-owned;
//! adapters patch only the MCP entry they manage.

mod adapters;
mod catalog;
mod model;
mod store;

pub use model::{Definition, DefinitionInput, McpsRequest};

use crate::core::{error::AppError, host::HostCtx};
use serde_json::{json, Value};

/// Validate one portable library file before backup/import code exposes it.
/// Invalid entries fail closed, so an accidentally committed credential is
/// never copied to the database or emitted over IPC as ordinary MCP data.
pub fn validate_portable(value: &Value) -> Result<(), AppError> {
    let definition: Definition = serde_json::from_value(value.clone()).map_err(|_| {
        AppError::invalid_input("MCP portable definition has an unsupported or malformed shape")
    })?;
    crate::core::resource_store::validate_id(&definition.id)
        .map_err(|_| AppError::invalid_input("MCP id must be a UUID"))?;
    crate::core::resource_store::validate_id(&definition.revision)
        .map_err(|_| AppError::invalid_input("MCP revision must be a UUID"))?;
    let input = model::DefinitionInput {
        id: Some(definition.id),
        name: definition.name,
        transport: definition.transport,
        server: definition.server,
        auth: definition.auth,
        source: definition.source,
    };
    store::validate_portable_input(&input)
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    let request: McpsRequest = serde_json::from_value(request)
        .map_err(|_| AppError::invalid_input("Invalid MCP request shape"))?;
    match request {
        McpsRequest::List {} => Ok(json!({ "definitions": store::list(ctx)? })),
        McpsRequest::Get { id } => Ok(json!({ "definition": store::get(ctx, &id)? })),
        McpsRequest::Save {
            definition,
            expected_revision,
        } => Ok(
            json!({ "definition": store::save(ctx, *definition, expected_revision.as_deref())? }),
        ),
        McpsRequest::Remove { id, detach } => {
            store::remove(ctx, &id, detach)?;
            Ok(json!({ "removed": true }))
        }
        McpsRequest::Capabilities {} => Ok(adapters::capabilities()),
        McpsRequest::Inspect { target } => adapters::inspect(ctx, &target),
        McpsRequest::Import {
            target,
            name,
            conflict,
            new_name,
        } => adapters::import(
            ctx,
            &target,
            &name,
            conflict.as_deref(),
            new_name.as_deref(),
        ),
        McpsRequest::Catalog {
            query,
            cursor,
            server_id,
            dry_run,
        } => catalog::search(
            ctx,
            query.as_deref(),
            cursor.as_deref(),
            server_id.as_deref(),
            dry_run,
        ),
        McpsRequest::Preview {
            target,
            operations,
            dry_run,
        } => adapters::preview(ctx, &target, &operations, dry_run),
        McpsRequest::Apply { preview_id } => adapters::apply(ctx, &preview_id),
        McpsRequest::Recover { recovery_id } => adapters::recover(ctx, &recovery_id),
        McpsRequest::Deployments { target } => adapters::deployments(ctx, target.as_ref()),
        McpsRequest::Undeploy {
            target,
            definition_id,
            server_name,
        } => adapters::undeploy(ctx, &target, &definition_id, server_name.as_deref()),
    }
}
