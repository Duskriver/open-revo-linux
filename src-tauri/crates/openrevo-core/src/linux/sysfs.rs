use super::{Backlight, Battery, PowerProfiles, Sensor, Snapshot};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) struct Sysfs {
    sys: PathBuf,
    proc: PathBuf,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    let value = fs::read_to_string(path).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn number(path: impl AsRef<Path>) -> Option<f64> {
    read(path)?.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn entries(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}

fn device_id(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
}

fn writable(path: &Path) -> bool {
    // Opening a sysfs attribute for writing does not alter it.
    OpenOptions::new().write(true).open(path).is_ok()
}

impl Sysfs {
    pub fn new(sys: PathBuf, proc: PathBuf) -> Self {
        Self { sys, proc }
    }

    pub fn snapshot(&self, previous_cpu: &mut Option<(u64, u64)>) -> Snapshot {
        let cpuinfo = read(self.proc.join("cpuinfo")).unwrap_or_default();
        let cpu = cpuinfo.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "model name").then(|| value.trim().to_owned())
        });
        let cpu_frequency_mhz = {
            let mhz: Vec<_> = cpuinfo
                .lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    (key.trim() == "cpu MHz")
                        .then(|| value.trim().parse::<f64>().ok())
                        .flatten()
                })
                .filter(|n| n.is_finite() && *n >= 0.0)
                .collect();
            (!mhz.is_empty()).then(|| mhz.iter().sum::<f64>() / mhz.len() as f64)
        };
        let cpu_usage = read(self.proc.join("stat")).and_then(|s| cpu_usage(&s, previous_cpu));
        let (memory_used_gib, memory_total_gib) =
            memory(&read(self.proc.join("meminfo")).unwrap_or_default());
        let dmi = self.sys.join("class/dmi/id");
        let ac_values: Vec<_> = entries(self.sys.join("class/power_supply"))
            .iter()
            .filter(|p| read(p.join("type")).as_deref() != Some("Battery"))
            .filter_map(|p| read(p.join("online")))
            .filter(|value| value == "0" || value == "1")
            .collect();
        Snapshot {
            platform: "linux",
            is_mock: false,
            timestamp_ms: super::now_ms(),
            model: read(dmi.join("product_name")),
            vendor: read(dmi.join("sys_vendor")),
            board: read(dmi.join("board_name")),
            bios: read(dmi.join("bios_version")),
            kernel: read(self.proc.join("sys/kernel/osrelease")),
            cpu,
            cpu_usage,
            cpu_frequency_mhz,
            memory_used_gib,
            memory_total_gib,
            ac_online: (!ac_values.is_empty()).then(|| ac_values.iter().any(|v| v == "1")),
            sensors: self.sensors(),
            batteries: self.batteries(),
            backlights: self.backlights(),
            power: PowerProfiles::default(),
            capabilities: Vec::new(),
            controls: Default::default(),
            oem: Default::default(),
        }
    }

    fn sensors(&self) -> Vec<Sensor> {
        let mut sensors = Vec::new();
        for device_path in entries(self.sys.join("class/hwmon")) {
            let Some(device) = read(device_path.join("name")) else {
                continue;
            };
            for path in entries(&device_path) {
                let name = device_id(&path);
                let Some(stem) = name.strip_suffix("_input") else {
                    continue;
                };
                let kind = if indexed(stem, "temp") {
                    "temperature"
                } else if indexed(stem, "fan") {
                    "fan"
                } else if indexed(stem, "power") {
                    "power"
                } else {
                    continue;
                };
                // GPU hwmon reads can resume a suspended dGPU. Leave it asleep.
                if device_path.join("device/power/runtime_status").exists()
                    && read(device_path.join("device/power/runtime_status")).as_deref()
                        == Some("suspended")
                {
                    continue;
                }
                let (divisor, unit) = match kind {
                    "temperature" => (1000.0, "°C"),
                    "power" => (1_000_000.0, "W"),
                    _ => (1.0, "RPM"),
                };
                let fault = read(device_path.join(format!("{stem}_fault"))).as_deref() == Some("1");
                let value = if fault {
                    None
                } else {
                    number(&path).map(|v| v / divisor).filter(|v| match kind {
                        "temperature" => (-100.0..=250.0).contains(v),
                        _ => *v >= 0.0,
                    })
                };
                let label =
                    read(device_path.join(format!("{stem}_label"))).unwrap_or_else(|| stem.into());
                sensors.push(Sensor {
                    id: format!("{}/{name}", device_id(&device_path)),
                    device: device.clone(),
                    label,
                    kind: kind.into(),
                    value,
                    unit: unit.into(),
                });
            }
        }
        sensors
    }

    fn batteries(&self) -> Vec<Battery> {
        entries(self.sys.join("class/power_supply"))
            .iter()
            .filter_map(|p| {
                if read(p.join("type")).as_deref() != Some("Battery") {
                    return None;
                }
                let full = number(p.join("energy_full"))
                    .zip(number(p.join("energy_full_design")))
                    .or_else(|| {
                        number(p.join("charge_full")).zip(number(p.join("charge_full_design")))
                    });
                let health_percent = full.and_then(|(full, design)| {
                    (design > 0.0 && full >= 0.0).then_some(full / design * 100.0)
                });
                let limit_path = p.join("charge_control_end_threshold");
                Some(Battery {
                    id: device_id(p),
                    capacity: number(p.join("capacity"))
                        .filter(|v| (0.0..=100.0).contains(v))
                        .map(|v| v as u8),
                    status: read(p.join("status")),
                    health_percent,
                    charge_limit: number(&limit_path)
                        .filter(|v| (0.0..=100.0).contains(v))
                        .map(|v| v as u8),
                    charge_limit_writable: writable(&limit_path),
                    power_w: number(p.join("power_now"))
                        .map(|v| v / 1_000_000.0)
                        .or_else(|| {
                            Some(
                                number(p.join("current_now"))? * number(p.join("voltage_now"))?
                                    / 1_000_000_000_000.0,
                            )
                        }),
                })
            })
            .collect()
    }

    fn backlights(&self) -> Vec<Backlight> {
        [("backlight", "class/backlight"), ("leds", "class/leds")]
            .iter()
            .flat_map(|(kind, subdir)| {
                entries(self.sys.join(subdir))
                    .into_iter()
                    .filter_map(|p| {
                        let id = device_id(&p);
                        if *kind == "leds" && !id.ends_with(":kbd_backlight") {
                            return None;
                        }
                        let max = number(p.join("max_brightness"))?;
                        if max <= 0.0 {
                            return None;
                        }
                        let percent = number(p.join("brightness"))
                            .map(|v| (v / max * 100.0).clamp(0.0, 100.0).round() as u8);
                        Some(Backlight {
                            id,
                            kind: (*kind).into(),
                            percent,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn backlight_target(&self, id: &str, percent: u8) -> Result<(&'static str, u32), String> {
        if !valid_id(id) || percent > 100 {
            return Err("无效亮度参数".into());
        }
        let device = self
            .backlights()
            .into_iter()
            .find(|d| d.id == id)
            .ok_or("未发现此亮度设备")?;
        let (subsystem, class) = if device.kind == "backlight" {
            ("backlight", "class/backlight")
        } else {
            ("leds", "class/leds")
        };
        let max = number(self.sys.join(class).join(id).join("max_brightness"))
            .ok_or("无法读取亮度范围")?;
        if max > u32::MAX as f64 {
            return Err("无效亮度范围".into());
        }
        Ok((subsystem, (max * percent as f64 / 100.0).round() as u32))
    }

    pub fn set_battery_limit(&self, id: &str, limit: u8) -> Result<(), String> {
        if !valid_id(id) || !(50..=100).contains(&limit) {
            return Err("限充参数必须为 50–100% 的有效电池设备".into());
        }
        let device = self
            .batteries()
            .into_iter()
            .find(|b| b.id == id)
            .ok_or("未发现此电池")?;
        if device.charge_limit.is_none() {
            return Err("内核未提供此电池的限充接口".into());
        }
        let dir = self.sys.join("class/power_supply").join(id);
        if number(dir.join("charge_control_start_threshold")).is_some_and(|v| v >= limit as f64) {
            return Err("结束阈值必须高于当前开始充电阈值".into());
        }
        let path = dir.join("charge_control_end_threshold");
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("无法写入限充接口（请检查设备权限）：{e}"))?;
        file.write_all(format!("{limit}\n").as_bytes())
            .map_err(|e| e.to_string())?;
        if read(&path).as_deref() != Some(&limit.to_string()) {
            return Err("电池限充读回与请求不符".into());
        }
        Ok(())
    }
}

fn indexed(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
}

fn cpu_usage(stat: &str, previous: &mut Option<(u64, u64)>) -> Option<f64> {
    let line = stat.lines().find(|s| s.starts_with("cpu "))?;
    // guest and guest_nice are already accounted for in user/nice.
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values
        .iter()
        .try_fold(0_u64, |sum, n| sum.checked_add(*n))?;
    let idle = values[3].checked_add(*values.get(4).unwrap_or(&0))?;
    let old = previous.replace((total, idle))?;
    let elapsed = total.checked_sub(old.0)?;
    let idle_elapsed = idle.checked_sub(old.1)?;
    (elapsed > 0 && idle_elapsed <= elapsed)
        .then(|| (elapsed - idle_elapsed) as f64 / elapsed as f64 * 100.0)
}

fn memory(info: &str) -> (Option<f64>, Option<f64>) {
    let get = |key: &str| {
        info.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k == key)
                .then(|| v.split_whitespace().next()?.parse::<u64>().ok())
                .flatten()
        })
    };
    let total = get("MemTotal");
    let available = get("MemAvailable");
    (
        total
            .zip(available)
            .and_then(|(t, a)| t.checked_sub(a))
            .map(|k| k as f64 / 1_048_576.0),
        total.map(|k| k as f64 / 1_048_576.0),
    )
}

#[cfg(test)]
mod tests;
