//! Hardware constants generated from the public registry.
//! Invalid or missing registry data stops the build; no zero-address fallback.
include!(concat!(env!("OUT_DIR"), "/generated_vault.rs"));

/// Verified Uniwill control values used by the Linux transport.
pub mod tongfang {
    use super::{linux_fan as fan, tongfang_control as control};

    // The generator checks byte ranges before these representation conversions.
    pub const OFFICE_CLAMP: u8 = fan::OFFICE_CLAMP as u8;
    pub const BALANCED_CLAMP: u8 = fan::BALANCED_CLAMP as u8;
    pub const TURBO_CLAMP: u8 = fan::TURBO_CLAMP as u8;
    pub const MODE_PRESERVE: u8 = control::MODE_PRESERVE;
    pub const MODE_TURBO: u8 = control::MODE_TURBO;
    pub const AP_PRESENT: u8 = control::AP_PRESENT;
    pub const CUSTOM_OFF: u8 = fan::CUSTOM_OFF as u8;
    pub const CUSTOM_ON: u8 = fan::CUSTOM_ON as u8;
    pub const RAMP_SMOOTH: u8 = control::RAMP_SMOOTH;
    pub const FAN_ISOLATED: u8 = fan::ISOLATED_BIT as u8;
    pub const PL4_MARGIN: u16 = control::PL4_MARGIN as u16;
}

#[cfg(test)]
#[path = "vault_codegen.rs"]
mod codegen;
