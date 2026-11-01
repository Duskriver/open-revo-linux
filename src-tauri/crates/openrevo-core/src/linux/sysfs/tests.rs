use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    sys: Sysfs,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sys = Sysfs::new(dir.path().join("sys"), dir.path().join("proc"));
        Self { _dir: dir, sys }
    }
    fn write(&self, path: &str, value: &str) {
        let path = self.sys.sys.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn write_proc(&self, path: &str, value: &str) {
        let path = self.sys.proc.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn battery(&self) {
        self.write("class/power_supply/BAT0/type", "Battery\n");
        self.write("class/power_supply/BAT0/capacity", "76\n");
        self.write("class/power_supply/BAT0/charge_full", "4500000\n");
        self.write("class/power_supply/BAT0/charge_full_design", "5000000\n");
        self.write("class/power_supply/BAT0/current_now", "1000000\n");
        self.write("class/power_supply/BAT0/voltage_now", "12000000\n");
    }
}

#[test]
fn absent_hardware_stays_unknown_instead_of_zero_or_mock() {
    let f = Fixture::new();
    let s = f.sys.snapshot(&mut None);
    assert!(!s.is_mock);
    assert_eq!(s.cpu_usage, None);
    assert_eq!(s.ac_online, None);
    assert!(s.batteries.is_empty());
    assert!(s.sensors.is_empty());
    assert_eq!(s.model, None);
}

#[test]
fn hwmon_converts_units_and_preserves_labels_and_faults() {
    let f = Fixture::new();
    f.write("class/hwmon/hwmon6/name", "k10temp\n");
    f.write("class/hwmon/hwmon6/temp1_input", "52375\n");
    f.write("class/hwmon/hwmon6/temp1_label", "Tctl\n");
    f.write("class/hwmon/hwmon6/temp2_input", "56000\n");
    f.write("class/hwmon/hwmon6/temp2_fault", "1\n");
    f.write("class/hwmon/hwmon6/fan1_input", "2310\n");
    f.write("class/hwmon/hwmon6/power1_input", "13250000\n");
    f.write("class/hwmon/hwmon6/temp3_input", "garbage\n");
    let sensors = f.sys.sensors();
    assert_eq!(
        sensors.iter().find(|s| s.label == "Tctl").unwrap().value,
        Some(52.375)
    );
    assert_eq!(
        sensors.iter().find(|s| s.label == "temp2").unwrap().value,
        None
    );
    assert_eq!(
        sensors.iter().find(|s| s.label == "temp3").unwrap().value,
        None
    );
    assert_eq!(
        sensors.iter().find(|s| s.kind == "fan").unwrap().value,
        Some(2310.0)
    );
    assert_eq!(
        sensors.iter().find(|s| s.kind == "power").unwrap().value,
        Some(13.25)
    );
}

#[test]
fn suspended_gpu_is_not_polled() {
    let f = Fixture::new();
    f.write("class/hwmon/hwmon0/name", "amdgpu\n");
    f.write("class/hwmon/hwmon0/temp1_input", "47000\n");
    f.write(
        "class/hwmon/hwmon0/device/power/runtime_status",
        "suspended\n",
    );
    assert!(f.sys.sensors().is_empty());
}

#[test]
fn battery_health_and_current_voltage_power_use_correct_units() {
    let f = Fixture::new();
    f.battery();
    let b = f.sys.batteries().remove(0);
    assert_eq!(b.capacity, Some(76));
    assert_eq!(b.health_percent, Some(90.0));
    assert_eq!(b.power_w, Some(12.0));
    assert_eq!(b.charge_limit, None);
    assert!(!b.charge_limit_writable);
}

#[test]
fn invalid_battery_values_remain_unknown() {
    let f = Fixture::new();
    f.battery();
    f.write("class/power_supply/BAT0/capacity", "255\n");
    f.write("class/power_supply/BAT0/charge_full_design", "0\n");
    let b = f.sys.batteries().remove(0);
    assert_eq!(b.capacity, None);
    assert_eq!(b.health_percent, None);
}

#[test]
fn battery_writes_are_bounded_and_require_a_discovered_battery() {
    let f = Fixture::new();
    f.battery();
    f.write(
        "class/power_supply/BAT0/charge_control_end_threshold",
        "100\n",
    );
    assert!(f.sys.set_battery_limit("../../test", 80).is_err());
    assert!(f.sys.set_battery_limit("/tmp/test", 80).is_err());
    assert!(f.sys.set_battery_limit("BAT1", 80).is_err());
    assert!(f.sys.set_battery_limit("BAT0", 49).is_err());
    assert!(f.sys.set_battery_limit("BAT0", 101).is_err());
    assert_eq!(
        read(
            f.sys
                .sys
                .join("class/power_supply/BAT0/charge_control_end_threshold")
        )
        .as_deref(),
        Some("100")
    );
    f.sys.set_battery_limit("BAT0", 80).unwrap();
    assert_eq!(f.sys.batteries()[0].charge_limit, Some(80));
}

#[test]
fn battery_end_threshold_must_exceed_start_threshold() {
    let f = Fixture::new();
    f.battery();
    f.write(
        "class/power_supply/BAT0/charge_control_end_threshold",
        "100\n",
    );
    f.write(
        "class/power_supply/BAT0/charge_control_start_threshold",
        "70\n",
    );
    assert!(f.sys.set_battery_limit("BAT0", 60).is_err());
    assert_eq!(f.sys.batteries()[0].charge_limit, Some(100));
}

#[test]
fn unsupported_battery_does_not_gain_a_charge_control_file() {
    let f = Fixture::new();
    f.battery();
    assert!(f.sys.set_battery_limit("BAT0", 80).is_err());
    assert!(!f
        .sys
        .sys
        .join("class/power_supply/BAT0/charge_control_end_threshold")
        .exists());
}

#[test]
fn backlight_targets_exclude_unrelated_leds_and_traversal() {
    let f = Fixture::new();
    f.write("class/backlight/amdgpu_bl1/max_brightness", "255\n");
    f.write("class/backlight/amdgpu_bl1/brightness", "128\n");
    f.write("class/leds/input0::capslock/max_brightness", "1\n");
    f.write("class/leds/input0::capslock/brightness", "1\n");
    assert_eq!(f.sys.backlights().len(), 1);
    assert_eq!(f.sys.backlights()[0].percent, Some(50));
    assert_eq!(
        f.sys.backlight_target("amdgpu_bl1", 50).unwrap(),
        ("backlight", 128)
    );
    assert!(f.sys.backlight_target("input0::capslock", 100).is_err());
    assert!(f.sys.backlight_target("../../foo", 50).is_err());
    assert!(f.sys.backlight_target("amdgpu_bl1", 101).is_err());
}

#[test]
fn cpu_utilisation_uses_deltas_ignores_guest_and_handles_reset() {
    let mut old = None;
    assert_eq!(cpu_usage("cpu 100 0 0 100 0 0 0 0 80 0", &mut old), None);
    assert_eq!(
        cpu_usage("cpu 150 0 0 150 0 0 0 0 90 0", &mut old),
        Some(50.0)
    );
    assert_eq!(cpu_usage("cpu 150 0 0 150 0 0 0 0", &mut old), None);
    assert_eq!(cpu_usage("cpu 1 0 0 1 0 0 0 0", &mut old), None);
}

#[test]
fn memory_and_unknown_supply_handle_missing_fields() {
    let f = Fixture::new();
    f.write_proc("meminfo", "MemTotal: 2097152 kB\nMemAvailable: 1048576 kB");
    let snapshot = f.sys.snapshot(&mut None);
    assert_eq!(
        (snapshot.memory_used_gib, snapshot.memory_total_gib),
        (Some(1.0), Some(2.0))
    );
    f.write_proc("meminfo", "MemTotal: 20 kB\nMemAvailable: 40 kB");
    let snapshot = f.sys.snapshot(&mut None);
    assert_eq!(
        (snapshot.memory_used_gib, snapshot.memory_total_gib),
        (None, Some(20.0 / 1_048_576.0))
    );
    assert_eq!(
        snapshot.telemetry["memory_used_gib"].status,
        ReadingStatus::Fault
    );
    f.write("class/power_supply/AC0/type", "Mains\n");
    f.write("class/power_supply/AC0/online", "1\n");
    assert_eq!(f.sys.snapshot(&mut None).ac_online, Some(true));
}

#[test]
fn invalid_ac_online_is_unknown() {
    let f = Fixture::new();
    f.write("class/power_supply/AC0/type", "Mains\n");
    f.write("class/power_supply/AC0/online", "unknown\n");
    let snapshot = f.sys.snapshot(&mut None);
    assert_eq!(snapshot.ac_online, None);
    assert_eq!(snapshot.telemetry["ac_online"].status, ReadingStatus::Fault);
}

#[test]
fn identity_cache_preserves_values_and_original_sample_times() {
    let f = Fixture::new();
    for (name, value) in [
        ("product_name", "Test laptop"),
        ("sys_vendor", "Test vendor"),
        ("board_name", "Test board"),
        ("bios_version", "Test BIOS"),
    ] {
        f.write(&format!("class/dmi/id/{name}"), value);
    }
    f.write_proc("sys/kernel/osrelease", "test-kernel");
    f.write_proc("cpuinfo", "model name : Test CPU\ncpu MHz : 1000\n");
    let now = Instant::now();
    let first = f.sys.snapshot_at(&mut None, now);
    for name in ["product_name", "sys_vendor", "board_name", "bios_version"] {
        f.write(&format!("class/dmi/id/{name}"), "Changed");
    }
    f.write_proc("sys/kernel/osrelease", "changed-kernel");
    f.write_proc("cpuinfo", "model name : Changed CPU\ncpu MHz : 2500\n");
    let second = f
        .sys
        .snapshot_at(&mut None, now + IDENTITY_RETRY_INTERVAL * 2);
    assert_eq!(first.model, second.model);
    assert_eq!(first.vendor, second.vendor);
    assert_eq!(first.board, second.board);
    assert_eq!(first.bios, second.bios);
    assert_eq!(first.kernel, second.kernel);
    assert_eq!(first.cpu, second.cpu);
    assert_eq!(second.cpu_frequency_mhz, Some(2500.0));
    for key in ["model", "vendor", "board", "bios", "kernel", "cpu"] {
        assert_eq!(first.telemetry[key].status, ReadingStatus::Ok);
        assert!(first.telemetry[key].sampled_at_ms.is_some());
        assert_eq!(
            first.telemetry[key].sampled_at_ms,
            second.telemetry[key].sampled_at_ms
        );
    }
}

#[test]
fn missing_identity_is_retried_without_caching_dynamic_values() {
    let f = Fixture::new();
    let now = Instant::now();
    let first = f.sys.snapshot_at(&mut None, now);
    assert_eq!(first.telemetry["board"].status, ReadingStatus::Unavailable);
    assert_eq!(first.telemetry["board"].sampled_at_ms, None);
    f.write("class/dmi/id/board_name", "Appeared board");
    f.write_proc("cpuinfo", "model name : Appeared CPU\ncpu MHz : 1800\n");
    let second = f.sys.snapshot_at(&mut None, now + Duration::from_secs(1));
    assert_eq!(second.board, None);
    assert_eq!(second.cpu, None);
    assert_eq!(second.cpu_frequency_mhz, Some(1800.0));
    let third = f.sys.snapshot_at(&mut None, now + IDENTITY_RETRY_INTERVAL);
    assert_eq!(third.board.as_deref(), Some("Appeared board"));
    assert_eq!(third.cpu.as_deref(), Some("Appeared CPU"));
    assert_eq!(third.telemetry["board"].status, ReadingStatus::Ok);
    assert!(third.telemetry["board"].sampled_at_ms.is_some());
}

#[test]
fn dynamic_measurements_and_control_availability_are_never_identity_cached() {
    let f = Fixture::new();
    f.battery();
    f.write("class/power_supply/AC0/type", "Mains");
    f.write("class/power_supply/AC0/online", "1");
    f.write("class/hwmon/hwmon0/name", "k10temp");
    f.write("class/hwmon/hwmon0/temp1_input", "41000");
    f.write_proc("stat", "cpu 100 0 0 100 0 0 0 0");
    f.write_proc("meminfo", "MemTotal: 2097152 kB\nMemAvailable: 1048576 kB");
    let mut previous = None;
    let first = f.sys.snapshot(&mut previous);
    assert_eq!(
        first.telemetry["cpu_usage"].status,
        ReadingStatus::WarmingUp
    );
    assert!(!first.batteries[0].charge_limit_writable);
    f.write("class/power_supply/AC0/online", "0");
    f.write("class/power_supply/BAT0/capacity", "75");
    f.write("class/power_supply/BAT0/charge_control_end_threshold", "80");
    f.write("class/hwmon/hwmon0/temp1_input", "55000");
    f.write_proc("stat", "cpu 150 0 0 150 0 0 0 0");
    f.write_proc("meminfo", "MemTotal: 2097152 kB\nMemAvailable: 524288 kB");
    let second = f.sys.snapshot(&mut previous);
    assert_eq!(second.ac_online, Some(false));
    assert_eq!(second.batteries[0].capacity, Some(75));
    assert_eq!(second.batteries[0].charge_limit, Some(80));
    assert!(second.batteries[0].charge_limit_writable);
    assert_eq!(second.sensors[0].value, Some(55.0));
    assert_eq!(second.cpu_usage, Some(50.0));
    assert_eq!(second.memory_used_gib, Some(1.5));
    assert_eq!(second.telemetry["cpu_usage"].status, ReadingStatus::Ok);
}

#[test]
fn metadata_distinguishes_missing_invalid_and_first_differential_samples() {
    let f = Fixture::new();
    let missing = f.sys.snapshot(&mut None);
    for key in [
        "cpu_usage",
        "cpu_frequency_mhz",
        "memory_used_gib",
        "memory_total_gib",
    ] {
        assert_eq!(missing.telemetry[key].status, ReadingStatus::Unavailable);
        assert_eq!(missing.telemetry[key].sampled_at_ms, None);
    }
    f.write_proc("stat", "cpu invalid");
    f.write_proc("cpuinfo", "cpu MHz : NaN");
    f.write_proc("meminfo", "MemTotal: invalid kB");
    let mut previous = Some((200, 100));
    let invalid = f.sys.snapshot(&mut previous);
    for key in [
        "cpu_usage",
        "cpu_frequency_mhz",
        "memory_used_gib",
        "memory_total_gib",
    ] {
        assert_eq!(invalid.telemetry[key].status, ReadingStatus::Fault);
        assert_eq!(invalid.telemetry[key].sampled_at_ms, None);
    }
    assert_eq!(previous, None);
    f.write_proc("stat", "cpu 200 0 0 200 0 0 0 0");
    f.write_proc("meminfo", "MemTotal: 2097152 kB");
    let recovering = f.sys.snapshot(&mut previous);
    assert_eq!(recovering.cpu_usage, None);
    assert_eq!(
        recovering.telemetry["cpu_usage"].status,
        ReadingStatus::WarmingUp
    );
    assert_eq!(recovering.telemetry["cpu_usage"].sampled_at_ms, None);
    assert_eq!(
        recovering.telemetry["memory_total_gib"].status,
        ReadingStatus::Ok
    );
    assert_eq!(
        recovering.telemetry["memory_used_gib"].status,
        ReadingStatus::Unavailable
    );
}

#[test]
fn sensor_metadata_has_stable_source_and_does_not_timestamp_faults_as_samples() {
    let f = Fixture::new();
    f.write("class/hwmon/hwmon0/name", "k10temp");
    f.write("class/hwmon/hwmon0/temp1_input", "41000");
    f.write("class/hwmon/hwmon0/temp2_input", "41000");
    f.write("class/hwmon/hwmon0/temp2_fault", "1");
    f.write("class/hwmon/hwmon0/temp3_input", "NaN");
    f.write("class/hwmon/hwmon0/temp4_input", "");
    let sensors = f.sys.sensors();
    assert_eq!(
        sensors[0].metadata.source,
        "/sys/class/hwmon/hwmon0/temp1_input"
    );
    assert_eq!(sensors[0].metadata.status, ReadingStatus::Ok);
    assert!(sensors[0].metadata.sampled_at_ms.is_some());
    assert_eq!(sensors[1].metadata.status, ReadingStatus::Fault);
    assert_eq!(sensors[2].metadata.status, ReadingStatus::Fault);
    assert_eq!(sensors[3].metadata.status, ReadingStatus::Unavailable);
    for sensor in &sensors[1..] {
        assert_eq!(sensor.value, None);
        assert_eq!(sensor.metadata.sampled_at_ms, None);
    }
    let encoded = serde_json::to_value(&sensors[0]).unwrap();
    assert_eq!(encoded["status"], "ok");
    assert!(encoded.get("source").is_some());
    assert!(encoded.get("metadata").is_none());
}
