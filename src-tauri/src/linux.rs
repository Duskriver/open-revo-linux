//! Linux Tauri host. The GUI runs as the desktop user, never as root.
mod commands;
mod config_commands;

use commands::LinuxState;
use openrevo_core::linux::LinuxBackend;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

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
        .manage(Arc::new(openrevo_core::config::ConfigStore::new()) as config_commands::ConfigState)
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .invoke_handler(tauri::generate_handler![
            crate::platform::get_platform,
            commands::get_linux_status,
            config_commands::get_linux_config,
            config_commands::save_linux_fan_curve_draft,
            config_commands::export_linux_config,
            config_commands::import_linux_config,
            commands::renew_linux_control_lease,
            commands::set_linux_power_profile,
            commands::set_linux_platform_profile,
            commands::set_linux_fan_boost,
            commands::set_linux_power_limit,
            commands::set_linux_oem_mode,
            commands::set_linux_oem_fan_boost,
            commands::set_linux_oem_cpu_limits,
            commands::set_linux_oem_fan_curve,
            commands::restore_linux_oem_fan_curve,
            commands::set_linux_battery_limit,
            commands::set_linux_backlight,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled
        ])
        .setup(|app| {
            let backend = Arc::clone(app.state::<LinuxState>().inner());
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Ok(mut state) = backend.lock() {
                    state.control_watchdog();
                }
            });
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
        .build(tauri::generate_context!())
        .expect("failed to build OpenRevo for Linux")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                if let Ok(mut state) = app.state::<LinuxState>().lock() {
                    if let Err(error) = state.release_controls() {
                        eprintln!("Could not restore automatic fan control: {error}");
                    }
                }
            }
        });
}
