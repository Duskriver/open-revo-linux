use super::{Backlight, Battery, PowerProfiles, ReadingMetadata, ReadingStatus, Sensor, Snapshot};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const IDENTITY_RETRY_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Reading<T> {
    value: Option<T>,
    metadata: ReadingMetadata,
}

impl<T> Reading<T> {
    fn from_result(source: String, result: Result<T, ReadingStatus>) -> Self {
        let (value, status) = match result {
            Ok(value) => (Some(value), ReadingStatus::Ok),
            Err(status) => (None, status),
        };
        let sampled_at_ms = value.as_ref().map(|_| super::now_ms());
        Self {
            value,
            metadata: ReadingMetadata {
                source,
                sampled_at_ms,
                status,
            },
        }
    }

    fn derive<U>(&self, parse: impl FnOnce(&T) -> Result<U, ReadingStatus>) -> Reading<U> {
        let result = self
            .value
            .as_ref()
            .ok_or(self.metadata.status)
            .and_then(parse);
        let mut derived = Reading::from_result(self.metadata.source.clone(), result);
        if derived.value.is_some() {
            derived.metadata.sampled_at_ms = self.metadata.sampled_at_ms;
        }
        derived
    }
}

fn observed_text(path: impl AsRef<Path>, source: String) -> Reading<String> {
    let result = fs::read_to_string(path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => ReadingStatus::Unavailable,
            _ => ReadingStatus::Fault,
        })
        .and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() {
                Err(ReadingStatus::Unavailable)
            } else {
                Ok(value)
            }
        });
    Reading::from_result(source, result)
}

struct CachedIdentity {
    reading: Reading<String>,
    attempted_at: Instant,
}

pub(super) struct Sysfs {
    sys: PathBuf,
    proc: PathBuf,
    // This cache belongs to the process backend, never to a shared disk file.
    identity: RefCell<BTreeMap<&'static str, CachedIdentity>>,
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
        Self {
            sys,
            proc,
            identity: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn snapshot(&self, previous_cpu: &mut Option<(u64, u64)>) -> Snapshot {
        self.snapshot_at(previous_cpu, Instant::now())
    }

    fn cached_identity(
        &self,
        key: &'static str,
        now: Instant,
        read_value: impl FnOnce() -> Reading<String>,
    ) -> Reading<String> {
        let mut cache = self.identity.borrow_mut();
        if let Some(cached) = cache.get(key) {
            if cached.reading.value.is_some()
                || now.saturating_duration_since(cached.attempted_at) < IDENTITY_RETRY_INTERVAL
            {
                return cached.reading.clone();
            }
        }
        let reading = read_value();
        cache.insert(
            key,
            CachedIdentity {
                reading: reading.clone(),
                attempted_at: now,
            },
        );
        reading
    }

    fn snapshot_at(&self, previous_cpu: &mut Option<(u64, u64)>, now: Instant) -> Snapshot {
        // cpuinfo contains both static identity and changing frequencies. Only the
        // model name is cached; the source is still read on each live sample.
        let cpuinfo = observed_text(self.proc.join("cpuinfo"), "/proc/cpuinfo".into());
        let cpu = self.cached_identity("cpu", now, || {
            cpuinfo.derive(|info| {
                info.lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        (key.trim() == "model name" && !value.trim().is_empty())
                            .then(|| value.trim().to_owned())
                    })
                    .ok_or(ReadingStatus::Unavailable)
            })
        });
        let cpu_frequency_mhz = cpuinfo.derive(|info| cpu_frequency(info));
        let stat = observed_text(self.proc.join("stat"), "/proc/stat".into());
        let cpu_usage = stat.derive(|text| {
            if cpu_counters(text).is_none() {
                *previous_cpu = None;
                return Err(ReadingStatus::Fault);
            }
            cpu_usage(text, previous_cpu).ok_or(ReadingStatus::WarmingUp)
        });
        if stat.value.is_none() {
            *previous_cpu = None;
        }
        let meminfo = observed_text(self.proc.join("meminfo"), "/proc/meminfo".into());
        let memory_total_gib = meminfo
            .derive(|info| memory_value(info, "MemTotal").map(|value| value as f64 / 1_048_576.0));
        let memory_used_gib = meminfo.derive(|info| {
            let total = memory_value(info, "MemTotal")?;
            let available = memory_value(info, "MemAvailable")?;
            total
                .checked_sub(available)
                .map(|value| value as f64 / 1_048_576.0)
                .ok_or(ReadingStatus::Fault)
        });
        let dmi = self.sys.join("class/dmi/id");
        let dmi_field = |key, filename| {
            self.cached_identity(key, now, || {
                observed_text(dmi.join(filename), format!("/sys/class/dmi/id/{filename}"))
            })
        };
        let model = dmi_field("model", "product_name");
        let vendor = dmi_field("vendor", "sys_vendor");
        let board = dmi_field("board", "board_name");
        let bios = dmi_field("bios", "bios_version");
        let kernel = self.cached_identity("kernel", now, || {
            observed_text(
                self.proc.join("sys/kernel/osrelease"),
                "/proc/sys/kernel/osrelease".into(),
            )
        });
        let supplies: Vec<_> = entries(self.sys.join("class/power_supply"))
            .iter()
            .filter(|p| read(p.join("type")).as_deref() != Some("Battery"))
            .map(|p| {
                observed_text(
                    p.join("online"),
                    format!("/sys/class/power_supply/{}/online", device_id(p)),
                )
                .derive(|value| match value.as_str() {
                    "0" => Ok(false),
                    "1" => Ok(true),
                    _ => Err(ReadingStatus::Fault),
                })
            })
            .collect();
        let ac_values: Vec<_> = supplies
            .iter()
            .filter_map(|reading| reading.value)
            .collect();
        let ac_online = Reading::from_result(
            "/sys/class/power_supply/*/online".into(),
            if ac_values.is_empty() {
                Err(
                    if supplies
                        .iter()
                        .any(|reading| reading.metadata.status == ReadingStatus::Fault)
                    {
                        ReadingStatus::Fault
                    } else {
                        ReadingStatus::Unavailable
                    },
                )
            } else {
                Ok(ac_values.iter().any(|value| *value))
            },
        );
        let telemetry = [
            ("model", &model.metadata),
            ("vendor", &vendor.metadata),
            ("board", &board.metadata),
            ("bios", &bios.metadata),
            ("kernel", &kernel.metadata),
            ("cpu", &cpu.metadata),
            ("cpu_usage", &cpu_usage.metadata),
            ("cpu_frequency_mhz", &cpu_frequency_mhz.metadata),
            ("memory_used_gib", &memory_used_gib.metadata),
            ("memory_total_gib", &memory_total_gib.metadata),
            ("ac_online", &ac_online.metadata),
        ]
        .into_iter()
        .map(|(key, metadata)| (key.into(), metadata.clone()))
        .collect();
        Snapshot {
            platform: "linux",
            is_mock: false,
            timestamp_ms: super::now_ms(),
            model: model.value,
            vendor: vendor.value,
            board: board.value,
            bios: bios.value,
            kernel: kernel.value,
            cpu: cpu.value,
            cpu_usage: cpu_usage.value,
            cpu_frequency_mhz: cpu_frequency_mhz.value,
            memory_used_gib: memory_used_gib.value,
            memory_total_gib: memory_total_gib.value,
            ac_online: ac_online.value,
            telemetry,
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
                let reading = if fault {
                    Reading::from_result(
                        format!("/sys/class/hwmon/{}/{name}", device_id(&device_path)),
                        Err(ReadingStatus::Fault),
                    )
                } else {
                    observed_text(
                        &path,
                        format!("/sys/class/hwmon/{}/{name}", device_id(&device_path)),
                    )
                    .derive(|text| {
                        let value =
                            text.parse::<f64>().map_err(|_| ReadingStatus::Fault)? / divisor;
                        let valid = value.is_finite()
                            && match kind {
                                "temperature" => (-100.0..=250.0).contains(&value),
                                _ => value >= 0.0,
                            };
                        if valid {
                            Ok(value)
                        } else {
                            Err(ReadingStatus::Fault)
                        }
                    })
                };
                let label =
                    read(device_path.join(format!("{stem}_label"))).unwrap_or_else(|| stem.into());
                sensors.push(Sensor {
                    id: format!("{}/{name}", device_id(&device_path)),
                    device: device.clone(),
                    label,
                    kind: kind.into(),
                    value: reading.value,
                    unit: unit.into(),
                    metadata: reading.metadata,
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

fn cpu_counters(stat: &str) -> Option<(u64, u64)> {
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
    Some((total, idle))
}

fn cpu_usage(stat: &str, previous: &mut Option<(u64, u64)>) -> Option<f64> {
    let (total, idle) = cpu_counters(stat)?;
    let old = previous.replace((total, idle))?;
    let elapsed = total.checked_sub(old.0)?;
    let idle_elapsed = idle.checked_sub(old.1)?;
    (elapsed > 0 && idle_elapsed <= elapsed)
        .then(|| (elapsed - idle_elapsed) as f64 / elapsed as f64 * 100.0)
}

fn cpu_frequency(info: &str) -> Result<f64, ReadingStatus> {
    let mut found = false;
    let mhz: Vec<_> = info
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.trim() != "cpu MHz" {
                return None;
            }
            found = true;
            value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
        })
        .collect();
    if mhz.is_empty() {
        return Err(if found {
            ReadingStatus::Fault
        } else {
            ReadingStatus::Unavailable
        });
    }
    let mean = mhz
        .iter()
        .map(|value| value / mhz.len() as f64)
        .sum::<f64>();
    if mean.is_finite() {
        Ok(mean)
    } else {
        Err(ReadingStatus::Fault)
    }
}

fn memory_value(info: &str, key: &str) -> Result<u64, ReadingStatus> {
    let value = info
        .lines()
        .find_map(|line| {
            let (k, value) = line.split_once(':')?;
            (k == key).then_some(value)
        })
        .ok_or(ReadingStatus::Unavailable)?;
    value
        .split_whitespace()
        .next()
        .ok_or(ReadingStatus::Fault)?
        .parse()
        .map_err(|_| ReadingStatus::Fault)
}

#[cfg(test)]
mod tests;
