//! Linux hardware control and telemetry, independent of the desktop host.
#[cfg(not(target_os = "linux"))]
compile_error!("openrevo-core supports Linux only.");

pub mod config;
pub mod control_bus;
pub mod hal;
pub mod linux;
pub mod runtime;
pub mod actions;
