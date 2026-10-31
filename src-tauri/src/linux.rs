//! Linux Tauri host. The GUI runs as the desktop user, never as root.
use openrevo_core::linux::{LinuxBackend, Snapshot};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, State,
};

type LinuxState = Arc<Mutex<LinuxBackend>>;

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
async fn get_linux_status(state: State<'_, LinuxState>) -> Result<Snapshot, String> {
    with_backend(&state, |backend| Ok(backend.snapshot())).await
}

#[tauri::command]
async fn set_linux_power_profile(
    profile: String,
    state: State<'_, LinuxState>,
) -> Result<(), String> {
    with_backend(&state, move |backend| backend.set_power_profile(&profile)).await
}

#[tauri::command]
async fn set_linux_battery_limit(
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
async fn set_linux_backlight(
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
fn get_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let launch = app.autolaunch();
    if enabled {
        launch.enable()
    } else {
        launch.disable()
    }
    .map_err(|e| e.to_string())
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn run() {
    if std::env::args().any(|a| a == "--linux-diagnostics") {
        let mut backend = LinuxBackend::new();
        println!(
            "{}",
            serde_json::to_string_pretty(&backend.snapshot()).expect("serialize Linux status")
        );
        return;
    }

    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(LinuxBackend::new())))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .invoke_handler(tauri::generate_handler![
            crate::platform::get_platform,
            get_linux_status,
            set_linux_power_profile,
            set_linux_battery_limit,
            set_linux_backlight,
            get_autostart_enabled,
            set_autostart_enabled
        ])
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "打开 OpenRevo", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("OpenRevo for Linux")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            // A missing indicator implementation must not prevent opening the GUI.
            if let Err(error) = tray.build(app) {
                eprintln!("System tray unavailable: {error}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run OpenRevo for Linux");
}
