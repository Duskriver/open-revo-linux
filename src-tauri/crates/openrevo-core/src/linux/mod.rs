//! Native Linux backend: standard kernel telemetry, system power policy, and
//! the whitelisted Uniwill WMI transport for original OEM controls.
//! Missing measurements stay `None` and never fall back to simulated hardware.
mod controls;
mod domains;
use crate::actions::PowerMode;
pub use crate::hal::fan_curve::FanCurveConfig;
use crate::hal::linux_ec::{CpuLimits, LinuxEc, OemSnapshot};
use crate::hal::wmi_events::{EventSnapshot, WmiEvents};
pub use domains::{Domains, ModeOption, Performance};
mod power;
mod sysfs;
mod telemetry;
pub use controls::{ControlsSnapshot, FanControl, PlatformProfile, PowerLimit};
pub use telemetry::{ReadingMetadata, ReadingStatus};

use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct Sensor {
    pub id: String,
    pub device: String,
    pub label: String,
    pub kind: String,
    pub value: Option<f64>,
    pub unit: String,
    #[serde(flatten)]
    pub metadata: ReadingMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct Battery {
    pub id: String,
    pub capacity: Option<u8>,
    pub status: Option<String>,
    pub health_percent: Option<f64>,
    pub charge_limit: Option<u8>,
    pub charge_limit_writable: bool,
    pub power_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Backlight {
    pub id: String,
    pub kind: String,
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PowerProfiles {
    pub active: Option<String>,
    pub available: Vec<String>,
    pub source: Option<String>,
    pub can_set: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub platform: &'static str,
    pub is_mock: bool,
    pub timestamp_ms: u128,
    pub telemetry: std::collections::BTreeMap<String, ReadingMetadata>,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub board: Option<String>,
    pub bios: Option<String>,
    pub kernel: Option<String>,
    pub cpu: Option<String>,
    pub cpu_usage: Option<f64>,
    pub cpu_frequency_mhz: Option<f64>,
    pub memory_used_gib: Option<f64>,
    pub memory_total_gib: Option<f64>,
    pub ac_online: Option<bool>,
    pub sensors: Vec<Sensor>,
    pub batteries: Vec<Battery>,
    pub backlights: Vec<Backlight>,
    pub power: PowerProfiles,
    pub capabilities: Vec<Capability>,
    pub controls: ControlsSnapshot,
    pub oem: OemSnapshot,
    pub wmi_events: EventSnapshot,
    pub domains: Domains,
    pub control_error: Option<String>,
    pub hotkeys_enabled: bool,
    pub hotkey_error: Option<String>,
}

pub struct LinuxBackend {
    sysfs: sysfs::Sysfs,
    power: power::PowerService,
    previous_cpu: Option<(u64, u64)>,
    controls: controls::Controls,
    oem: LinuxEc,
    events: WmiEvents,
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxBackend {
    #[cfg(test)]
    pub(crate) fn with_test_ec(oem: LinuxEc, root: &std::path::Path) -> Self {
        Self {
            sysfs: sysfs::Sysfs::new(root.join("sys"), root.join("proc")),
            power: power::PowerService::new(),
            previous_cpu: None,
            oem,
            controls: controls::Controls::new(root.join("sys")),
            events: WmiEvents::new(root.join("dev/openrevo_events")),
        }
    }

    pub fn new() -> Self {
        Self {
            sysfs: sysfs::Sysfs::new(PathBuf::from("/sys"), PathBuf::from("/proc")),
            power: power::PowerService::new(),
            previous_cpu: None,
            oem: LinuxEc::default(),
            events: WmiEvents::default(),
            controls: controls::Controls::new(PathBuf::from("/sys")),
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let mut snapshot = self.sysfs.snapshot(&mut self.previous_cpu);
        snapshot.power = self.power.snapshot();
        snapshot.controls = self.controls.snapshot();
        snapshot.oem = self.oem.snapshot();
        snapshot.wmi_events = self.events.snapshot();
        snapshot.capabilities = capabilities(&snapshot);
        snapshot.domains = Domains::from_snapshot(&snapshot);
        snapshot
    }

    pub fn set_ec_battery(&mut self, policy: crate::actions::BatteryRequest) -> Result<(), String> {
        self.oem.battery(policy)
    }
    pub fn set_device_switch(
        &mut self,
        id: crate::actions::DeviceSwitch,
        enabled: bool,
    ) -> Result<(), String> {
        self.oem.device_switch(id, enabled)
    }
    pub fn poll_events(&mut self) -> Vec<crate::hal::wmi_events::RawEvent> {
        self.events.poll()
    }
    pub fn reset_events(&mut self) {
        self.events.reset();
    }
    pub fn set_mode(&mut self, mode: PowerMode, custom: Option<CpuLimits>) -> Result<(), String> {
        let snapshot = self.snapshot();
        if !snapshot
            .domains
            .performance
            .modes
            .iter()
            .any(|option| option.mode == mode && option.enabled)
        {
            return Err("当前设备或供电状态不支持此模式".into());
        }
        if snapshot.oem.available {
            if let Some(firmware) = mode.firmware() {
                self.set_oem_mode(firmware)
            } else {
                let limits = custom
                    .or(snapshot.oem.limits)
                    .ok_or("尚无可用的自定义功耗设置")?;
                // Reject an invalid saved tuple before changing the active curve.
                crate::hal::linux_ec::validate_limits(
                    &snapshot.oem,
                    limits,
                    snapshot.ac_online == Some(true),
                )?;
                self.restore_oem_fan_curve()?;
                self.set_oem_cpu_limits(limits.pl1, limits.pl2, limits.pl4)
            }
        } else {
            let profile = mode.profile().ok_or("当前设备不支持自定义模式")?;
            if !snapshot.power.available.is_empty() {
                self.set_power_profile(profile)
            } else {
                self.set_platform_profile(if mode == PowerMode::Office {
                    "low-power"
                } else {
                    profile
                })
            }
        }
    }

    pub fn set_cooling_boost(&mut self, enabled: bool) -> Result<(), String> {
        // A failed status read must not redirect an owned EC control to sysfs.
        if self.oem.owns_boost() || self.oem.snapshot().available {
            return self.set_oem_fan_boost(enabled);
        }
        if !enabled {
            return self.controls.restore_fans();
        }
        let fans = self.controls.snapshot().fans;
        if fans.is_empty() || fans.iter().any(|fan| !fan.writable) {
            return Err("未检测到可写的风扇控制接口".into());
        }
        for fan in fans {
            if let Err(error) = self.controls.set_fan_boost(&fan.id, true) {
                return match self.controls.restore_fans() {
                    Ok(()) => Err(error),
                    Err(restore) => Err(format!("{error}；恢复失败：{restore}")),
                };
            }
        }
        Ok(())
    }

    pub fn set_power_profile(&mut self, profile: &str) -> Result<(), String> {
        self.power.set(profile)
    }

    pub fn set_platform_profile(&mut self, profile: &str) -> Result<(), String> {
        if !self
            .controls
            .platform()
            .available
            .iter()
            .any(|p| p == profile)
        {
            return Err("内核未提供此平台性能模式".into());
        }
        let power = self.power.snapshot();
        if !power.available.is_empty() {
            // Keep the desktop service as the sole policy owner when present.
            let system_profile = match profile {
                "low-power" => "power-saver",
                "balanced" => "balanced",
                "performance" => "performance",
                _ => return Err("系统电源服务正在管理平台模式，无法独立切换此档位".into()),
            };
            self.power.set(system_profile)?;
            self.controls.verify_platform(profile)
        } else {
            self.controls.set_platform(profile)
        }
    }

    pub fn set_fan_boost(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        self.controls.set_fan_boost(id, enabled)
    }

    pub fn set_power_limit(&self, id: &str, watts: f64) -> Result<(), String> {
        self.controls.set_power_limit(id, watts)
    }

    pub fn control_watchdog(&mut self) {
        self.controls.watchdog();
        self.oem.watchdog();
    }

    /// Renew the runtime owner's control lease independently of telemetry.
    /// Each backend expires old controls before accepting a new heartbeat.
    pub fn renew_control_lease(&mut self) -> Result<(), String> {
        let generic = self.controls.renew_control_lease();
        let oem = self.oem.renew_control_lease();
        generic.and(oem)
    }

    pub fn release_controls(&mut self) -> Result<(), String> {
        let generic = self.controls.restore_fans();
        let oem = self.oem.release();
        generic.and(oem)
    }

    pub fn set_oem_mode(&mut self, mode: u8) -> Result<(), String> {
        let sysfs = &self.sysfs;
        let previous_cpu = &mut self.previous_cpu;
        self.oem.mode(mode, || {
            sysfs.snapshot(previous_cpu).ac_online == Some(true)
        })
    }

    pub fn set_oem_fan_boost(&mut self, enabled: bool) -> Result<(), String> {
        self.oem.boost(enabled)
    }

    pub fn set_oem_fan_curve(&mut self, config: FanCurveConfig) -> Result<(), String> {
        let ac = self.sysfs.snapshot(&mut self.previous_cpu).ac_online == Some(true);
        self.oem.apply_curve(config, ac)
    }

    pub fn restore_oem_fan_curve(&mut self) -> Result<(), String> {
        self.oem.restore_curve()
    }

    pub fn set_oem_cpu_limits(&mut self, pl1: u8, pl2: u8, pl4: u8) -> Result<(), String> {
        let sysfs = &self.sysfs;
        let previous_cpu = &mut self.previous_cpu;
        self.oem.power(CpuLimits { pl1, pl2, pl4 }, || {
            sysfs.snapshot(previous_cpu).ac_online == Some(true)
        })
    }

    pub fn set_battery_limit(&self, battery: &str, limit: u8) -> Result<(), String> {
        self.sysfs.set_battery_limit(battery, limit)
    }

    pub fn set_backlight(&self, id: &str, percent: u8) -> Result<(), String> {
        let (subsystem, value) = self.sysfs.backlight_target(id, percent)?;
        power::set_brightness(subsystem, id, value)
    }
}

fn capabilities(s: &Snapshot) -> Vec<Capability> {
    let cap = |id: &str, name: &str, available: bool, detail: String| Capability {
        id: id.into(),
        name: name.into(),
        available,
        detail,
    };
    vec![
        cap(
            "power_profile",
            "系统性能档位",
            !s.power.available.is_empty(),
            s.power.source.clone().unwrap_or_else(|| {
                s.power
                    .error
                    .clone()
                    .unwrap_or_else(|| "未检测到 power-profiles-daemon / tuned-ppd".into())
            }),
        ),
        cap(
            "sensors",
            "温度与功耗读取",
            !s.sensors.is_empty(),
            "读取 Linux hwmon；只显示内核实际提供的传感器".into(),
        ),
        cap(
            "fan_rpm",
            "风扇转速读取",
            s.oem.available || s.sensors.iter().any(|sensor| sensor.kind == "fan"),
            "读取 Uniwill EC 或内核 hwmon 风扇转速".into(),
        ),
        cap(
            "battery",
            "电池限充",
            s.oem.devices.battery.supported || s.batteries.iter().any(|b| b.charge_limit.is_some()),
            "EC 声明能力时提供三档与双阈值；否则使用标准 charge_control_end_threshold".into(),
        ),
        cap(
            "backlight",
            "屏幕 / 键盘亮度",
            !s.backlights.is_empty(),
            "通过 systemd-logind 设置亮度；权限由当前桌面会话决定".into(),
        ),
        cap(
            "fan_control",
            "风扇强冷 / 自动调速",
            s.oem.available || !s.controls.fans.is_empty(),
            "Uniwill EC / hwmon 强冷与自动模式恢复；原版 8 点曲线通过整表写入，退出后恢复原策略"
                .into(),
        ),
        cap(
            "platform_profile",
            "平台性能模式",
            !s.controls.platform.available.is_empty(),
            s.controls
                .platform
                .source
                .clone()
                .unwrap_or_else(|| "内核未公开平台性能接口".into()),
        ),
        cap(
            "power_limits",
            "硬件功耗上限",
            s.oem.available || !s.controls.power_limits.is_empty(),
            "读取 EC 出厂功耗表或 powercap / hwmon，写入后读回确认".into(),
        ),
        cap(
            "oem_mode",
            "机身性能模式",
            s.oem.available,
            s.oem
                .error
                .clone()
                .unwrap_or_else(|| "Uniwill EC 原生模式控制，读取机身档位".into()),
        ),
        cap("hardware_switches", "硬件快捷开关",s.oem.devices.switches.iter().any(|s|s.enabled.is_some()),"Fn/Win 锁、摄像头、关机 USB 与来电自启；触摸板项仅同步 EC 标记".into()),
        cap(
            "rgb",
            "RGB 灯效",
            false,
            "尚未移植 ITE / HID 灯控协议".into(),
        ),
        cap(
            "gpu",
            "显卡 MUX / 功耗调校",
            false,
            "尚未移植；不调用 Windows NVAPI".into(),
        ),
        cap(
            "water",
            "蓝牙水冷",
            false,
            match s.oem.water_cooling_supported {
                Some(true) => "EC 已报告水冷能力；当前仅检测能力位，BlueZ 控制未接入".into(),
                Some(false) => "EC 未报告水冷能力".into(),
                None => "无法读取 EC 水冷能力位".into(),
            },
        ),
        cap(
            "wmi_events",
            "机身按键事件读取",
            s.wmi_events.available,
            s.wmi_events
                .error
                .clone()
                .unwrap_or_else(|| "读取原始事件与丢失计数；可显式开启四档模式键联动".into()),
        ),
    ]
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
