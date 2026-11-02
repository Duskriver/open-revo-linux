//! Public domain actions. Transport selection belongs to the backend.
use crate::hal::{fan_curve::FanCurveConfig, linux_ec::CpuLimits};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerMode {
    Office,
    Balanced,
    Turbo,
    Custom,
}

impl PowerMode {
    pub fn from_firmware(mode: u8) -> Option<Self> {
        match mode {
            1 => Some(Self::Office),
            2 => Some(Self::Balanced),
            3 => Some(Self::Turbo),
            _ => None,
        }
    }

    pub fn firmware(self) -> Option<u8> {
        match self {
            Self::Office => Some(1),
            Self::Balanced => Some(2),
            Self::Turbo => Some(3),
            Self::Custom => None,
        }
    }

    pub fn profile(self) -> Option<&'static str> {
        match self {
            Self::Office => Some("power-saver"),
            Self::Balanced => Some("balanced"),
            Self::Turbo => Some("performance"),
            Self::Custom => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Mode { mode: PowerMode },
    Fan { enabled: bool },
    CpuLimits { limits: CpuLimits },
    FanCurve { config: FanCurveConfig },
    RestoreFanCurve {},
    Battery { policy: BatteryRequest },
    Switch { switch: DeviceSwitch, enabled: bool },
    Hotkeys { enabled: bool },
    BatteryLimit { battery: String, limit: u8 },
    Backlight { device: String, percent: u8 },
    PowerLimit { id: String, watts: f64 },
    ReleaseControls {},
}

pub use crate::hal::device_controls::{BatteryMode, BatteryRequest, DeviceSwitch};
