use crate::core::{error::AppError, host::HostCtx, instructions, mcps};
use serde_json::Value;
use tauri::State;

#[tauri::command]
pub async fn instructions_request(
    ctx: State<'_, HostCtx>,
    request: Value,
) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || instructions::dispatch(&ctx, request)).await?
}

#[tauri::command]
pub async fn mcps_request(ctx: State<'_, HostCtx>, request: Value) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || mcps::dispatch(&ctx, request)).await?
}

/// Register this computer's CLI bridge as an MCP server for `agent_key`
/// (`codex` or `claude_code`). Always local: it is not host-scoped.
#[tauri::command]
pub async fn register_mcp_client(
    ctx: State<'_, HostCtx>,
    agent_key: mcps::Client,
) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        mcps::register_self(&ctx, agent_key, &crate::core::cli_bridge::bridge_path())
    })
    .await?
}

#[tauri::command]
pub async fn resource_sync_request(
    ctx: State<'_, HostCtx>,
    request: Value,
) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::core::resource_sync::dispatch(&ctx, request)
    })
    .await?
}

#[tauri::command]
pub async fn operation_request(
    ctx: State<'_, HostCtx>,
    boundary: State<'_, std::sync::Arc<crate::shutdown::Shutdown>>,
    request: Value,
) -> Result<Value, AppError> {
    let call = boundary.enter(!matches!(
        request["action"].as_str(),
        Some("read" | "list" | "reconcile")
    ))?;
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _call = call;
        crate::core::operations::dispatch(&ctx, request)
    })
    .await?
}

/// Always called locally by the desktop, including results from offline SSH hosts.
#[tauri::command]
pub async fn operation_results_request(
    ctx: State<'_, HostCtx>,
    request: Value,
) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::core::operations::results::dispatch(&ctx, request)
    })
    .await?
}
