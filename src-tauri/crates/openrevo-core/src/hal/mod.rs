// OpenRevo Hardware Abstraction Layer (HAL) Module

// pub mod baolongda_guard;
#[cfg(windows)]
pub mod declarative;
#[cfg(windows)]
pub mod driver_impl;
#[cfg(windows)]
pub mod mock;
#[cfg(windows)]
pub mod platform_probe;
#[cfg(windows)]
pub mod traits;
pub mod vault;
pub mod tongfang;
pub mod fan_curve;
#[cfg(target_os = "linux")]
pub mod linux_ec;

#[cfg(windows)]
pub use declarative::DeclarativeHardwareDriver;
#[cfg(windows)]
pub use driver_impl::MechrevoHALImpl;
#[cfg(windows)]
pub use mock::MockHAL;
#[cfg(windows)]
pub use traits::*;

#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
static DRIVER_INSTANCE: OnceLock<Box<dyn HardwareDriver>> = OnceLock::new();

/// Global accessor for the active Hardware Driver backend
#[cfg(windows)]
pub fn get_hardware_driver() -> &'static dyn HardwareDriver {
    DRIVER_INSTANCE
        .get_or_init(|| {
            // 1. Try declarative model driver first (e.g. Xingyao 14, thin & lights, external plugin drivers)
            if let Some(manifest) = declarative::load_matched_manifest() {
                crate::log_info!(
                    "HAL",
                    "Activated Declarative Hardware Driver backend: {} ({})",
                    manifest.name,
                    manifest.id
                );
                return Box::new(DeclarativeHardwareDriver::new(manifest));
            }

            // 2. Fall back to built-in Tongfang ACPIDriver for standard gaming laptops
            let real = MechrevoHALImpl::new();
            if real.probe() {
                crate::log_info!("HAL", "Activated Native Tongfang ACPI Hardware Driver backend");
                Box::new(real)
            } else {
                crate::log_warn!("HAL", "No physical hardware driver matched. Activating MockHAL backend.");
                Box::new(MockHAL::new())
            }
        })
        .as_ref()
}

/// Global accessor for the Hardware Abstraction Layer (backward compatibility)
#[cfg(windows)]
pub fn get_hardware_hal() -> &'static dyn MechrevoHardwareHAL {
    get_hardware_driver()
}

