//! App-owned settings; saving or importing a draft never applies hardware state.
use openrevo_core::config::{AppConfig, ConfigError, ConfigStore};
use openrevo_core::linux::FanCurveConfig;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

pub(super) type ConfigState = Arc<Result<ConfigStore, ConfigError>>;

async fn with_config<T: Send + 'static>(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
    operation: impl FnOnce(&ConfigStore) -> Result<T, ConfigError> + Send + 'static,
) -> Result<T, ConfigError> {
    if window.label() != "main" {
        return Err(ConfigError {
            code: "unauthorized_window".into(),
            message: "只有主窗口可以访问应用配置".into(),
        });
    }
    let store = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || match store.as_ref() {
        Ok(store) => operation(store),
        Err(error) => Err(error.clone()),
    })
    .await
    .map_err(|error| ConfigError {
        code: "config_task_failed".into(),
        message: format!("配置操作未完成：{error}"),
    })?
}

#[tauri::command]
pub(super) async fn get_linux_config(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<Option<AppConfig>, ConfigError> {
    with_config(window, state, ConfigStore::load).await
}

#[tauri::command]
pub(super) async fn save_linux_fan_curve_draft(
    draft: FanCurveConfig,
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<AppConfig, ConfigError> {
    with_config(window, state, move |store| {
        store.save_fan_curve_draft(Some(&draft))
    })
    .await
}

#[tauri::command]
pub(super) async fn export_linux_config(
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<Option<String>, ConfigError> {
    with_config(window, state, ConfigStore::export_json).await
}

#[tauri::command]
pub(super) async fn import_linux_config(
    json: String,
    window: WebviewWindow,
    state: State<'_, ConfigState>,
) -> Result<AppConfig, ConfigError> {
    with_config(window, state, move |store| store.import_json(&json)).await
}
