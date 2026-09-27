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
