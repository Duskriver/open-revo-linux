//! App-owned settings; saving or importing a draft never applies hardware state.
use openrevo_core::config::AppConfig;
use openrevo_core::linux::FanCurveConfig;
use openrevo_core::runtime::{AppRuntime, RuntimeError};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

pub(super) type ConfigState = Arc<AppRuntime>;

pub(super) async fn with_runtime<T: Send + 'static>(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
    operation: impl FnOnce(&AppRuntime) -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    if window.label() != "main" {
        return Err(RuntimeError::new(
            "unauthorized_window",
            "只有主窗口可以访问应用配置与场景",
        ));
    }
    let runtime = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || operation(&runtime))
        .await
        .map_err(|error| {
            RuntimeError::new("config_task_failed", format!("配置操作未完成：{error}"))
        })?
}

#[tauri::command]
pub(super) async fn get_linux_config(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<Option<AppConfig>, RuntimeError> {
    with_runtime(window, state, AppRuntime::load_config).await
}

#[tauri::command]
pub(super) async fn save_linux_fan_curve_draft(
    draft: FanCurveConfig,
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<AppConfig, RuntimeError> {
    with_runtime(window, state, move |runtime| {
        runtime.save_fan_curve_draft(&draft)
    })
    .await
}

#[tauri::command]
pub(super) async fn export_linux_config(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<Option<String>, RuntimeError> {
    with_runtime(window, state, AppRuntime::export_config).await
}

#[tauri::command]
pub(super) async fn import_linux_config(
    json: String,
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<AppConfig, RuntimeError> {
    with_runtime(window, state, move |runtime| runtime.import_config(&json)).await
}
