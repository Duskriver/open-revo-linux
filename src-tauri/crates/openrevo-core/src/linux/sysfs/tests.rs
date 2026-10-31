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
    assert_eq!(
        memory("MemTotal: 2097152 kB\nMemAvailable: 1048576 kB"),
        (Some(1.0), Some(2.0))
    );
    assert_eq!(
        memory("MemTotal: 20 kB\nMemAvailable: 40 kB"),
        (None, Some(20.0 / 1_048_576.0))
    );
    let f = Fixture::new();
    f.write("class/power_supply/AC0/type", "Mains\n");
    f.write("class/power_supply/AC0/online", "1\n");
    assert_eq!(f.sys.snapshot(&mut None).ac_online, Some(true));
}

#[test]
fn invalid_ac_online_is_unknown() {
    let f = Fixture::new();
    f.write("class/power_supply/AC0/type", "Mains\n");
    f.write("class/power_supply/AC0/online", "unknown\n");
    assert_eq!(f.sys.snapshot(&mut None).ac_online, None);
}
