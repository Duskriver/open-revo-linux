//! Linux desktop host. Hardware policy and transport live in openrevo-core.
#[cfg(not(target_os = "linux"))]
compile_error!("OpenRevo supports Linux only.");

pub use openrevo_core;

mod linux;
pub use linux::run;

mod platform {
    // Retained for existing IPC clients and native-window tests.
    #[tauri::command]
    pub fn get_platform() -> &'static str {
        "linux"
    }
}
