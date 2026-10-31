//! Kernel-owned controls. Never accept a filesystem path or raw EC value over IPC.
//! ABI references and hardware limitations are documented in docs/linux-port.md.
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize)]
pub struct PlatformProfile {
    pub active: Option<String>,
    pub available: Vec<String>,
    pub source: Option<String>,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FanControl {
    pub id: String,
    pub name: String,
    pub rpm: Option<u32>,
    pub mode: Option<u32>,
    pub boosted: bool,
    pub writable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerLimit {
    pub id: String,
    pub name: String,
    pub watts: f64,
    pub min_watts: f64,
    pub max_watts: f64,
    pub writable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ControlsSnapshot {
    pub platform: PlatformProfile,
    pub fans: Vec<FanControl>,
    pub power_limits: Vec<PowerLimit>,
    pub fan_error: Option<String>,
}

struct FanRestore {
    path: PathBuf,
    mode: u32,
    identity: (u64, u64),
}

pub(super) struct Controls {
    sys: PathBuf,
    boosted: BTreeMap<String, FanRestore>,
    deadline: Option<Instant>,
    fan_error: Option<String>,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|v| v.trim().to_owned())
}

fn number(path: impl AsRef<Path>) -> Option<u64> {
    read(path)?.parse().ok()
}

fn entries(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    entries
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn index<'a>(text: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let id = text.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

impl Controls {
    pub fn new(sys: PathBuf) -> Self {
        Self {
            sys,
            boosted: BTreeMap::new(),
            deadline: None,
            fan_error: None,
        }
    }

    // Class entries are symlinks into /sys/devices. Permit those but never an
    // attribute resolving outside sysfs (including in fixture-based tests).
    fn checked(&self, path: &Path) -> Result<PathBuf, String> {
        let root = self.sys.canonicalize().map_err(|e| e.to_string())?;
        let target = path.canonicalize().map_err(|e| e.to_string())?;
        if !target.starts_with(&root) || target == root || !target.is_file() {
            return Err("控制路径不在已发现的 sysfs 设备中".into());
        }
        Ok(target)
    }

    fn writable(&self, path: &Path) -> bool {
        self.checked(path)
            .ok()
            .is_some_and(|p| OpenOptions::new().write(true).open(p).is_ok())
    }

    fn write(&self, path: &Path, value: &str) -> Result<(), String> {
        let path = self.checked(path)?;
        // No create/truncate: a removed kernel attribute must never become a file.
        let mut file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| format!("设备控制权限不足或设备已断开：{e}"))?;
        file.write_all(format!("{value}\n").as_bytes())
            .map_err(|e| format!("内核拒绝控制请求：{e}"))
    }

    fn platform_path(&self) -> Option<(PathBuf, PathBuf, String)> {
        let legacy = self.sys.join("firmware/acpi/platform_profile");
        if legacy.exists() {
            let sources: Vec<_> = entries(self.sys.join("class/platform-profile"))
                .iter()
                .filter_map(|p| read(p.join("name")))
                .collect();
            return Some((
                legacy,
                self.sys.join("firmware/acpi/platform_profile_choices"),
                if sources.is_empty() {
                    "ACPI platform_profile".into()
                } else {
                    sources.join(" / ")
                },
            ));
        }
        let profiles = entries(self.sys.join("class/platform-profile"));
        // With multiple handlers, only the aggregate kernel interface can make
        // a consistent selection. Do not silently choose the first handler.
        if profiles.len() != 1 {
            return None;
        }
        let dir = &profiles[0];
        Some((
            dir.join("profile"),
            dir.join("choices"),
            read(dir.join("name")).unwrap_or_else(|| name(dir)),
        ))
    }

    pub fn platform(&self) -> PlatformProfile {
        let Some((path, choices, source)) = self.platform_path() else {
            return PlatformProfile::default();
        };
        PlatformProfile {
            active: read(&path),
            available: read(choices)
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            source: Some(source),
            writable: self.writable(&path),
        }
    }

    pub fn set_platform(&self, profile: &str) -> Result<(), String> {
        if !self.platform().available.iter().any(|p| p == profile) {
            return Err("内核未提供此平台性能模式".into());
        }
        let (path, _, _) = self.platform_path().ok_or("未发现平台性能接口")?;
        self.write(&path, profile)?;
        self.verify_platform(profile)
    }

    pub fn verify_platform(&self, profile: &str) -> Result<(), String> {
        let actual = self.platform().active;
        if actual.as_deref() != Some(profile) {
            return Err(format!(
                "平台模式读回不一致，当前为 {}",
                actual.as_deref().unwrap_or("未知")
            ));
        }
        Ok(())
    }

    fn fan_entries(&self) -> Vec<(FanControl, PathBuf)> {
        let mut fans = Vec::new();
        for device in entries(self.sys.join("class/hwmon")) {
            if read(device.join("device/power/runtime_status")).as_deref() == Some("suspended") {
                continue;
            }
            let device_name = read(device.join("name")).unwrap_or_else(|| name(&device));
            for path in entries(&device) {
                let filename = name(&path);
                let Some(channel) = index(&filename, "pwm", "_enable") else {
                    continue;
                };
                let id = format!("{}/{filename}", name(&device));
                let mode = number(&path).and_then(|n| u32::try_from(n).ok());
                let owned = self.boosted.contains_key(&id);
                let automatic = mode.is_some_and(|v| v >= 2);
                let reason = if !automatic && !owned {
                    Some("当前由其他手动策略控制；先恢复固件自动控制".into())
                } else if !self.writable(&path) {
                    Some("风扇控制接口只读，需要设备写入权限".into())
                } else {
                    None
                };
                fans.push((
                    FanControl {
                        id,
                        name: format!(
                            "{} · {}",
                            device_name,
                            read(device.join(format!("fan{channel}_label")))
                                .unwrap_or_else(|| format!("风扇 {channel}"))
                        ),
                        rpm: number(device.join(format!("fan{channel}_input")))
                            .and_then(|v| u32::try_from(v).ok()),
                        mode,
                        boosted: owned && mode == Some(0),
                        writable: reason.is_none(),
                        reason,
                    },
                    path,
                ));
            }
        }
        fans
    }

    pub fn set_fan_boost(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        self.fan_error = None;
        if !enabled {
            return self.restore_fan(id);
        }
        if let Some(state) = self.boosted.get(id) {
            self.check_fan_identity(state)?;
            if number(&state.path) == Some(0) {
                self.heartbeat();
                return Ok(());
            }
            // A different controller has taken over since our last request.
            self.boosted.remove(id);
        }
        let (fan, path) = self
            .fan_entries()
            .into_iter()
            .find(|(f, _)| f.id == id)
            .ok_or("未发现此风扇控制通道")?;
        if !fan.writable {
            return Err(fan.reason.unwrap_or_else(|| "风扇不可写".into()));
        }
        let mode = fan.mode.ok_or("无法读取风扇自动控制模式")?;
        let path = self.checked(&path)?;
        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
        // pwm_enable=0 means full speed in the standard hwmon ABI. Save the
        // exact automatic mode; drivers may use 2, 3 or another automatic mode.
        self.boosted.insert(
            id.into(),
            FanRestore {
                path: path.clone(),
                mode,
                identity: (metadata.dev(), metadata.ino()),
            },
        );
        self.heartbeat();
        if let Err(error) = self.write(&path, "0").and_then(|_| {
            if number(&path) == Some(0) {
                Ok(())
            } else {
                Err("风扇强冷状态读回失败".into())
            }
        }) {
            let restored = self.restore_fan(id);
            return Err(format!(
                "{error}{}",
                restored
                    .err()
                    .map(|e| format!("；恢复失败：{e}"))
                    .unwrap_or_default()
            ));
        }
        Ok(())
    }

    fn restore_fan(&mut self, id: &str) -> Result<(), String> {
        let state = self.boosted.get(id).ok_or("此风扇未由 OpenRevo 接管")?;
        self.check_fan_identity(state)?;
        if number(&state.path).is_some_and(|mode| mode != 0) {
            // Respect a later policy change made by firmware or another app.
            self.boosted.remove(id);
            return Ok(());
        }
        self.write(&state.path, &state.mode.to_string())?;
        if number(&state.path) != Some(u64::from(state.mode)) {
            return Err("恢复风扇自动控制后读回不一致".into());
        }
        self.boosted.remove(id);
        Ok(())
    }

    fn check_fan_identity(&self, state: &FanRestore) -> Result<(), String> {
        let path = self.checked(&state.path)?;
        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
        if path != state.path || (metadata.dev(), metadata.ino()) != state.identity {
            return Err("风扇设备已更换，停止对旧通道的写入".into());
        }
        Ok(())
    }

    pub fn restore_fans(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        for id in self.boosted.keys().cloned().collect::<Vec<_>>() {
            if let Err(e) = self.restore_fan(&id) {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            self.deadline = None;
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    pub fn heartbeat(&mut self) {
        if !self.boosted.is_empty() {
            self.deadline = Some(Instant::now() + Duration::from_secs(10));
        }
    }

    pub fn watchdog(&mut self) {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            if let Err(error) = self.restore_fans() {
                self.fan_error = Some(error);
            }
        }
    }

    fn power_entries(&self) -> Vec<(PowerLimit, PathBuf)> {
        let mut limits = Vec::new();
        // powercap class links enumerate the zones, including nested zones.
        for zone in entries(self.sys.join("class/powercap")) {
            for path in entries(&zone) {
                let filename = name(&path);
                let Some(channel) = index(&filename, "constraint_", "_power_limit_uw") else {
                    continue;
                };
                let stem = format!("constraint_{channel}");
                let label = format!(
                    "{} · {}",
                    read(zone.join("name")).unwrap_or_else(|| name(&zone)),
                    read(zone.join(format!("{stem}_name"))).unwrap_or_else(|| stem.clone())
                );
                self.add_limit(
                    &mut limits,
                    path,
                    format!("powercap/{}/{stem}", name(&zone)),
                    label,
                    zone.join(format!("{stem}_min_power_uw")),
                    zone.join(format!("{stem}_max_power_uw")),
                );
            }
        }
        for device in entries(self.sys.join("class/hwmon")) {
            if read(device.join("device/power/runtime_status")).as_deref() == Some("suspended") {
                continue;
            }
            for path in entries(&device) {
                let filename = name(&path);
                let Some(channel) = index(&filename, "power", "_cap") else {
                    continue;
                };
                let label = format!(
                    "{} · {}",
                    read(device.join("name")).unwrap_or_else(|| name(&device)),
                    read(device.join(format!("power{channel}_label")))
                        .unwrap_or_else(|| format!("功耗 {channel}"))
                );
                self.add_limit(
                    &mut limits,
                    path,
                    format!("hwmon/{}/{filename}", name(&device)),
                    label,
                    device.join(format!("power{channel}_cap_min")),
                    device.join(format!("power{channel}_cap_max")),
                );
            }
        }
        limits
    }

    fn add_limit(
        &self,
        limits: &mut Vec<(PowerLimit, PathBuf)>,
        path: PathBuf,
        id: String,
        label: String,
        min_path: PathBuf,
        max_path: PathBuf,
    ) {
        let Some(current) = number(&path) else {
            return;
        };
        let max = number(max_path).filter(|v| *v > 0);
        let min = number(min_path).unwrap_or(1_000_000).max(1);
        let reason = if max.is_none() || max.is_some_and(|v| v < min) {
            Some("驱动未提供有效的功耗调节范围，保持只读".into())
        } else if !self.writable(&path) {
            Some("功耗接口只读，需要设备写入权限".into())
        } else {
            None
        };
        limits.push((
            PowerLimit {
                id,
                name: label,
                watts: current as f64 / 1_000_000.0,
                min_watts: min as f64 / 1_000_000.0,
                max_watts: max.unwrap_or(current) as f64 / 1_000_000.0,
                writable: reason.is_none(),
                reason,
            },
            path,
        ));
    }

    pub fn set_power_limit(&self, id: &str, watts: f64) -> Result<(), String> {
        let (limit, path) = self
            .power_entries()
            .into_iter()
            .find(|(p, _)| p.id == id)
            .ok_or("未发现此功耗控制通道")?;
        if !limit.writable {
            return Err(limit.reason.unwrap_or_else(|| "功耗接口不可写".into()));
        }
        if !watts.is_finite() || watts < limit.min_watts || watts > limit.max_watts {
            return Err(format!(
                "功耗必须位于驱动允许的 {}–{} W",
                limit.min_watts, limit.max_watts
            ));
        }
        let microwatts = (watts * 1_000_000.0).round() as u64;
        self.write(&path, &microwatts.to_string())?;
        let actual = number(&path).ok_or("功耗写入后无法读回")?;
        if actual != microwatts {
            return Err(format!(
                "驱动调整了功耗目标，实际读回 {} W",
                actual as f64 / 1_000_000.0
            ));
        }
        Ok(())
    }

    pub fn snapshot(&self) -> ControlsSnapshot {
        ControlsSnapshot {
            platform: self.platform(),
            fans: self.fan_entries().into_iter().map(|(f, _)| f).collect(),
            power_limits: self.power_entries().into_iter().map(|(p, _)| p).collect(),
            fan_error: self.fan_error.clone(),
        }
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        let _ = self.restore_fans();
    }
}

#[cfg(test)]
mod tests;
