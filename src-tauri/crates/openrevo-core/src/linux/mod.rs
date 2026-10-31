//! Native Linux backend. Read telemetry through documented kernel interfaces;
//! route power policy through the desktop's existing power-profiles service.
//! Missing measurements stay `None` and never fall back to simulated hardware.
mod power;
mod sysfs;

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
}

pub struct LinuxBackend {
    sysfs: sysfs::Sysfs,
    power: power::PowerService,
    previous_cpu: Option<(u64, u64)>,
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxBackend {
    pub fn new() -> Self {
        Self {
            sysfs: sysfs::Sysfs::new(PathBuf::from("/sys"), PathBuf::from("/proc")),
            power: power::PowerService::new(),
            previous_cpu: None,
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let mut snapshot = self.sysfs.snapshot(&mut self.previous_cpu);
        snapshot.power = self.power.snapshot();
        snapshot.capabilities = capabilities(&snapshot);
        snapshot
    }

    pub fn set_power_profile(&mut self, profile: &str) -> Result<(), String> {
        self.power.set(profile)
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
            s.sensors.iter().any(|sensor| sensor.kind == "fan"),
            "需要内核 hwmon 提供风扇转速接口".into(),
        ),
        cap(
            "battery",
            "电池限充",
            s.batteries.iter().any(|b| b.charge_limit.is_some()),
            "需要内核提供 charge_control_end_threshold；写入还需要设备访问权限".into(),
        ),
        cap(
            "backlight",
            "屏幕 / 键盘亮度",
            !s.backlights.is_empty(),
            "通过 systemd-logind 设置亮度；权限由当前桌面会话决定".into(),
        ),
        cap(
            "fan_control",
            "自定义风扇与强冷",
            false,
            "尚未移植机型专用风扇协议；当前由固件管理散热".into(),
        ),
        cap(
            "oem_mode",
            "机身性能键 / OEM 模式灯",
            false,
            "系统性能档位与机身 EC 档位是不同接口，尚未接入 EC 联动".into(),
        ),
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
        cap("water", "蓝牙水冷", false, "尚未移植 BlueZ 后端".into()),
    ]
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
