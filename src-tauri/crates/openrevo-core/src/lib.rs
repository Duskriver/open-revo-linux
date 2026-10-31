//! OpenRevo Core Engine (openrevo-core)
//! Ultra-lightweight, zero-UI-dependency hardware control, EC communications,
//! telemetry, and power orchestration engine for Mechrevo laptops.

pub mod event;
#[cfg(windows)]
pub mod hal;
#[cfg(windows)]
pub mod driver;
#[cfg(windows)]
pub mod system;

pub use event::{
    emit_global, fan_boost_changed_global, get_global_event_sink, osd_notify_global,
    power_mode_changed_global, set_global_event_sink, CoreEventSink, NoopEventSink,
    SharedEventSink,
};


#[cfg(target_os = "linux")]
pub mod linux;
