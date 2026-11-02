//! Stable IPC surface; hardware policies live in openrevo-core.
use openrevo_core::linux::Snapshot;
use openrevo_core::runtime::AppRuntime;
use std::sync::Arc;
use tauri::State;

pub(super) type LinuxState = Arc<AppRuntime>;

#[tauri::command]
pub(super) async fn get_linux_status(state: State<'_, LinuxState>) -> Result<Snapshot, String> {
    let runtime = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || runtime.snapshot().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn execute_linux_action(
    action: openrevo_core::actions::Action,
    state: State<'_, LinuxState>,
) -> Result<(), openrevo_core::runtime::RuntimeError> {
    let runtime = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || runtime.execute(action))
        .await
        .map_err(|error| {
            openrevo_core::runtime::RuntimeError::new("runtime_task", error.to_string())
        })?
}

#[tauri::command]
pub(super) fn get_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub(super) fn set_autostart_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let launch = app.autolaunch();
    if enabled {
        launch.enable()
    } else {
        launch.disable()
    }
    .map_err(|e| e.to_string())
}
