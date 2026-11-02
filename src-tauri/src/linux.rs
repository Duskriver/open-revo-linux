//! Linux Tauri host. The GUI runs as the desktop user, never as root.
mod commands;
mod config_commands;
mod notifications;

use commands::LinuxState;
use openrevo_core::actions::{Action, PowerMode};
use openrevo_core::linux::LinuxBackend;
use openrevo_core::runtime::AppRuntime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
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
        .manage(Arc::new(AppRuntime::new()))
        .manage(AtomicBool::new(false))
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
            commands::execute_linux_action,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled
        ])
        .setup(|app| {
            let runtime = Arc::clone(app.state::<LinuxState>().inner());
            let connection = openrevo_core::control_bus::serve_control(Arc::clone(&runtime))
                .map_err(std::io::Error::other)?;
            app.manage(connection);
            notifications::start(app.handle().clone(), &runtime)?;
            runtime.start()?;
            let show = MenuItem::with_id(app, "show", "打开 OpenRevo", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let office = MenuItem::with_id(app, "office", "办公", true, None::<&str>)?;
            let balanced = MenuItem::with_id(app, "balanced", "均衡", true, None::<&str>)?;
            let turbo = MenuItem::with_id(app, "turbo", "狂暴", true, None::<&str>)?;
            let custom = MenuItem::with_id(app, "custom", "自定义", true, None::<&str>)?;
            let boost = MenuItem::with_id(app, "boost", "一键强冷", true, None::<&str>)?;
            let automatic =
                MenuItem::with_id(app, "automatic", "恢复自动散热", true, None::<&str>)?;
            let full = MenuItem::with_id(app, "battery_full", "电池：满充", true, None::<&str>)?;
            let battery_balanced =
                MenuItem::with_id(app, "battery_balanced", "电池：均衡", true, None::<&str>)?;
            let longevity =
                MenuItem::with_id(app, "battery_longevity", "电池：长效", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show,
                    &office,
                    &balanced,
                    &turbo,
                    &custom,
                    &boost,
                    &automatic,
                    &full,
                    &battery_balanced,
                    &longevity,
                    &quit,
                ],
            )?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("OpenRevo for Linux")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "quit" => app.exit(0),
                    id => {
                        let action = match id {
                            "office" => Action::Mode {
                                mode: PowerMode::Office,
                            },
                            "balanced" => Action::Mode {
                                mode: PowerMode::Balanced,
                            },
                            "turbo" => Action::Mode {
                                mode: PowerMode::Turbo,
                            },
                            "custom" => Action::Mode {
                                mode: PowerMode::Custom,
                            },
                            "battery_full" | "battery_balanced" | "battery_longevity" => {
                                Action::Battery {
                                    policy: openrevo_core::actions::BatteryRequest {
                                        mode: match id {
                                            "battery_full" => {
                                                openrevo_core::actions::BatteryMode::Full
                                            }
                                            "battery_balanced" => {
                                                openrevo_core::actions::BatteryMode::Balanced
                                            }
                                            _ => openrevo_core::actions::BatteryMode::Longevity,
                                        },
                                        lower: None,
                                        upper: None,
                                    },
                                }
                            }
                            "boost" => Action::Fan { enabled: true },
                            "automatic" => Action::Fan { enabled: false },
                            _ => return,
                        };
                        let runtime = Arc::clone(app.state::<LinuxState>().inner());
                        let app = app.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            if let Err(error) = runtime.execute(action) {
                                eprintln!("Tray control failed: {error}");
                                let _ = app.emit("linux-control-error", &error.message);
                                show_window(&app);
                            }
                        });
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            // A missing indicator implementation must not prevent opening the GUI.
            match tray.build(app) {
                Ok(_) => app.state::<AtomicBool>().store(true, Ordering::Release),
                Err(error) => eprintln!("System tray unavailable: {error}"),
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if window
                        .app_handle()
                        .state::<AtomicBool>()
                        .load(Ordering::Acquire)
                        && window.hide().is_ok()
                    {
                        api.prevent_close();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build OpenRevo for Linux")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                if let Err(error) = app.state::<LinuxState>().shutdown() {
                    eprintln!("Could not restore automatic fan control: {error}");
                }
            }
        });
}
