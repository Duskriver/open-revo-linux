//! Resolve device-specific observations into a single UI/CLI contract.
use super::{PowerLimit, Snapshot};
use crate::actions::PowerMode;
use crate::hal::linux_ec::{CpuLimits, CurveSnapshot};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ModeOption {
    pub mode: PowerMode,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Performance {
    pub active: Option<PowerMode>,
    pub modes: Vec<ModeOption>,
    pub limits: Option<CpuLimits>,
    pub ceiling: Option<CpuLimits>,
    pub limits_writable: bool,
    pub power_limits: Vec<PowerLimit>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Cooling {
    pub rpm: Vec<Option<u32>>,
    pub boosted: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub curve: CurveSnapshot,
    pub curve_writable: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Domains {
    pub performance: Performance,
    pub cooling: Cooling,
    pub devices: crate::hal::device_controls::DeviceControls,
}

impl Domains {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        let oem = &s.oem;
        let on_adapter = s.ac_online == Some(true) && oem.dc_jack;
        let modes = [
            PowerMode::Office,
            PowerMode::Balanced,
            PowerMode::Turbo,
            PowerMode::Custom,
        ]
        .into_iter()
        .map(|mode| {
            let enabled = if oem.available {
                oem.writable && (mode == PowerMode::Office || on_adapter)
            } else if let Some(profile) = mode.profile() {
                if !s.power.available.is_empty() {
                    s.power.can_set != Some(false) && s.power.available.iter().any(|p| p == profile)
                } else {
                    let profile = if mode == PowerMode::Office {
                        "low-power"
                    } else {
                        profile
                    };
                    s.controls.platform.writable
                        && s.controls.platform.available.iter().any(|p| p == profile)
                }
            } else {
                false
            };
            ModeOption {
                mode,
                enabled,
                reason: (!enabled).then(|| {
                    if oem.available && oem.writable && !on_adapter {
                        "需要连接原装电源".into()
                    } else {
                        "当前设备未提供此档位或没有控制权限".into()
                    }
                }),
            }
        })
        .collect();
        let active = if oem.available {
            if oem.custom_power {
                Some(PowerMode::Custom)
            } else {
                oem.mode.and_then(PowerMode::from_firmware)
            }
        } else {
            match s
                .power
                .active
                .as_deref()
                .or(s.controls.platform.active.as_deref())
            {
                Some("power-saver" | "low-power") => Some(PowerMode::Office),
                Some("balanced") => Some(PowerMode::Balanced),
                Some("performance") => Some(PowerMode::Turbo),
                _ => None,
            }
        };
        Self {
            devices: oem.devices.clone(),
            performance: Performance {
                active,
                modes,
                limits: oem.limits,
                ceiling: oem.presets.get(2).copied(),
                limits_writable: oem.writable && on_adapter && !oem.fan_curve.active,
                power_limits: if oem.available {
                    Vec::new()
                } else {
                    s.controls.power_limits.clone()
                },
                error: if oem.available {
                    oem.error.clone()
                } else {
                    s.power.error.clone()
                },
            },
            cooling: Cooling {
                rpm: if oem.available {
                    oem.fan_rpm.iter().map(|r| Some(u32::from(*r))).collect()
                } else {
                    s.sensors
                        .iter()
                        .filter(|s| s.kind == "fan")
                        .map(|s| s.value.map(|v| v as u32))
                        .collect()
                },
                boosted: if oem.available {
                    oem.fan_boost
                } else {
                    s.controls.fans.iter().any(|f| f.boosted)
                },
                writable: if oem.available {
                    oem.writable && !oem.fan_curve.fault
                } else {
                    !s.controls.fans.is_empty() && s.controls.fans.iter().all(|f| f.writable)
                },
                error: if oem.available {
                    oem.error.clone()
                } else {
                    s.controls.fan_error.clone()
                },
                curve: oem.fan_curve.clone(),
                curve_writable: oem.writable
                    && oem.fan_curve.supported
                    && !oem.fan_curve.fault
                    && on_adapter
                    && !oem.fan_boost,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &std::path::Path) -> Snapshot {
        super::super::sysfs::Sysfs::new(root.join("sys"), root.join("proc")).snapshot(&mut None)
    }

    #[test]
    fn system_service_resolves_three_modes_without_advertising_custom_power() {
        let root = tempfile::tempdir().unwrap();
        let mut s = fixture(root.path());
        s.power.active = Some("balanced".into());
        s.power.available = vec![
            "power-saver".into(),
            "balanced".into(),
            "performance".into(),
        ];
        s.power.can_set = Some(true);
        let domains = Domains::from_snapshot(&s);
        assert_eq!(domains.performance.active, Some(PowerMode::Balanced));
        assert_eq!(
            domains
                .performance
                .modes
                .iter()
                .map(|m| m.enabled)
                .collect::<Vec<_>>(),
            vec![true, true, true, false]
        );
        assert!(!domains.cooling.curve_writable);
        s.power.can_set = Some(false);
        assert!(Domains::from_snapshot(&s)
            .performance
            .modes
            .iter()
            .all(|m| !m.enabled));
    }

    #[test]
    fn firmware_custom_and_cooling_capabilities_follow_supply_and_faults() {
        let root = tempfile::tempdir().unwrap();
        let mut s = fixture(root.path());
        s.oem.available = true;
        s.oem.writable = true;
        s.oem.custom_power = true;
        s.oem.fan_curve.supported = true;
        let domains = Domains::from_snapshot(&s);
        assert_eq!(domains.performance.active, Some(PowerMode::Custom));
        assert_eq!(
            domains
                .performance
                .modes
                .iter()
                .map(|m| m.enabled)
                .collect::<Vec<_>>(),
            vec![true, false, false, false]
        );
        assert!(!domains.performance.limits_writable);
        s.ac_online = Some(true);
        s.oem.dc_jack = true;
        assert!(Domains::from_snapshot(&s).cooling.curve_writable);
        s.oem.fan_curve.fault = true;
        let domains = Domains::from_snapshot(&s);
        assert!(!domains.cooling.writable);
        assert!(!domains.cooling.curve_writable);
    }
}
