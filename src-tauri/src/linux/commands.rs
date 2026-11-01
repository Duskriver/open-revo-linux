//! Stable IPC surface; hardware policies live in openrevo-core.
use openrevo_core::linux::{FanCurveConfig, LinuxBackend, Snapshot};
use std::sync::{Arc, Mutex};
use tauri::State;

pub(super) type LinuxState = Arc<Mutex<LinuxBackend>>;

async fn with_backend<T: Send + 'static>(
    state: &State<'_, LinuxState>,
    operation: impl FnOnce(&mut LinuxBackend) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let backend = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let mut backend = backend.lock().map_err(|e| e.to_string())?;
        operation(&mut backend)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn get_linux_status(state: State<'_, LinuxState>) -> Result<Snapshot, String> {
    with_backend(&state, |backend| Ok(backend.snapshot())).await
}

#[tauri::command]
pub(super) async fn renew_linux_control_lease(
    window: tauri::WebviewWindow,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    // This is a single-GUI lease, not a grant to diagnostic readers or future
    // secondary windows. The label comes from Tauri, never from IPC arguments.
    if window.label() != "main" {
        return Err("只有主窗口可以续期硬件控制".into());
    }
    with_backend(&state, LinuxBackend::renew_control_lease).await
}

#[tauri::command]
pub(super) async fn set_linux_power_profile(
    profile: String,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_power_profile(&profile)).await
}

#[tauri::command]
pub(super) async fn set_linux_platform_profile(
    profile: String,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| {
        backend.set_platform_profile(&profile)
    })
    .await
}

#[tauri::command]
pub(super) async fn set_linux_fan_boost(
    id: String,
    enabled: bool,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_fan_boost(&id, enabled)).await
}

#[tauri::command]
pub(super) async fn set_linux_power_limit(
    id: String,
    watts: f64,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_power_limit(&id, watts)).await
}

#[tauri::command]
pub(super) async fn set_linux_oem_mode(
    mode: u8,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_oem_mode(mode)).await
}

#[tauri::command]
pub(super) async fn set_linux_oem_fan_boost(
    enabled: bool,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_oem_fan_boost(enabled)).await
}

#[tauri::command]
pub(super) async fn set_linux_oem_cpu_limits(
    pl1: u8,
    pl2: u8,
    pl4: u8,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| {
        backend.set_oem_cpu_limits(pl1, pl2, pl4)
    })
    .await
}

#[tauri::command]
pub(super) async fn set_linux_oem_fan_curve(
    config: FanCurveConfig,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_oem_fan_curve(config)).await
}

#[tauri::command]
pub(super) async fn restore_linux_oem_fan_curve(
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, |backend| backend.restore_oem_fan_curve()).await
}

#[tauri::command]
pub(super) async fn set_linux_battery_limit(
    battery: String,
    limit: u8,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| {
        backend.set_battery_limit(&battery, limit)
    })
    .await
}

#[tauri::command]
pub(super) async fn set_linux_backlight(
    device: String,
    percent: u8,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| {
        backend.set_backlight(&device, percent)
    })
    .await
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
