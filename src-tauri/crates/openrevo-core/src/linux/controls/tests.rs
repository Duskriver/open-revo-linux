use super::*;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

fn fixture() -> (TempDir, Controls) {
    let root = TempDir::new().unwrap();
    let controls = Controls::new(root.path().to_path_buf());
    (root, controls)
}

fn put(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{value}\n")).unwrap();
}

fn fan(root: &Path) {
    put(root, "class/hwmon/hwmon0/name", "test-fan");
    put(root, "class/hwmon/hwmon0/pwm1_enable", "3");
    put(root, "class/hwmon/hwmon0/fan1_input", "2500");
}

fn power(root: &Path) {
    put(root, "class/powercap/test:0/name", "package-0");
    put(root, "class/powercap/test:0/constraint_0_name", "long_term");
    put(
        root,
        "class/powercap/test:0/constraint_0_power_limit_uw",
        "45000000",
    );
    put(
        root,
        "class/powercap/test:0/constraint_0_min_power_uw",
        "10000000",
    );
    put(
        root,
        "class/powercap/test:0/constraint_0_max_power_uw",
        "65000000",
    );
}

#[test]
fn discovers_profiles_and_rejects_unadvertised_values_without_writing() {
    let (root, controls) = fixture();
    put(root.path(), "firmware/acpi/platform_profile", "balanced");
    put(
        root.path(),
        "firmware/acpi/platform_profile_choices",
        "low-power balanced performance",
    );
    assert_eq!(controls.platform().available.len(), 3);
    assert!(controls.set_platform("performance\nlow-power").is_err());
    assert!(controls.set_platform("../../etc/passwd").is_err());
    assert_eq!(controls.platform().active.as_deref(), Some("balanced"));
    controls.set_platform("performance").unwrap();
    assert_eq!(controls.platform().active.as_deref(), Some("performance"));
}

#[test]
fn class_only_profile_requires_a_single_handler() {
    let (root, controls) = fixture();
    put(
        root.path(),
        "class/platform-profile/profile-0/profile",
        "balanced",
    );
    put(
        root.path(),
        "class/platform-profile/profile-0/choices",
        "balanced performance",
    );
    put(
        root.path(),
        "class/platform-profile/profile-0/name",
        "test-driver",
    );
    assert_eq!(controls.platform().source.as_deref(), Some("test-driver"));
    put(
        root.path(),
        "class/platform-profile/profile-1/profile",
        "balanced",
    );
    assert!(controls.platform().available.is_empty());
}

#[test]
fn fan_boost_restores_exact_automatic_mode_and_is_idempotent() {
    let (root, mut controls) = fixture();
    fan(root.path());
    let id = "hwmon0/pwm1_enable";
    controls.set_fan_boost(id, true).unwrap();
    controls.set_fan_boost(id, true).unwrap();
    assert!(controls.snapshot().fans[0].boosted);
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(0)
    );
    controls.set_fan_boost(id, false).unwrap();
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(3)
    );
    assert!(controls.set_fan_boost(id, false).is_err());
}

#[test]
fn does_not_take_over_another_manual_fan_controller() {
    let (root, mut controls) = fixture();
    fan(root.path());
    put(root.path(), "class/hwmon/hwmon0/pwm1_enable", "1");
    assert!(!controls.snapshot().fans[0].writable);
    assert!(controls.set_fan_boost("hwmon0/pwm1_enable", true).is_err());
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(1)
    );
}

#[test]
fn watchdog_and_drop_restore_fans_without_frontend() {
    let (root, mut controls) = fixture();
    fan(root.path());
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    controls.deadline = Some(Instant::now() - Duration::from_secs(1));
    controls.watchdog();
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(3)
    );
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    drop(controls);
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(3)
    );
}

#[test]
fn vanished_fan_is_not_recreated_on_restore() {
    let (root, mut controls) = fixture();
    fan(root.path());
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    let path = root.path().join("class/hwmon/hwmon0/pwm1_enable");
    fs::rename(&path, root.path().join("removed-attribute")).unwrap();
    assert!(controls.restore_fans().is_err());
    assert!(!path.exists());
    assert!(!controls.boosted.is_empty());
}

#[test]
fn replaced_fan_device_is_not_modified_during_restore() {
    let (root, mut controls) = fixture();
    fan(root.path());
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    let path = root.path().join("class/hwmon/hwmon0/pwm1_enable");
    fs::rename(&path, root.path().join("old-attribute")).unwrap();
    put(root.path(), "class/hwmon/hwmon0/pwm1_enable", "0");
    assert!(controls.restore_fans().is_err());
    assert_eq!(number(&path), Some(0));
}

#[test]
fn respects_later_fan_policy_from_another_controller() {
    let (root, mut controls) = fixture();
    fan(root.path());
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    put(root.path(), "class/hwmon/hwmon0/pwm1_enable", "1");
    controls.restore_fans().unwrap();
    assert_eq!(
        number(root.path().join("class/hwmon/hwmon0/pwm1_enable")),
        Some(1)
    );
    assert!(controls.boosted.is_empty());
}

#[test]
fn rejects_invalid_ids_nonfinite_and_out_of_range_power_before_write() {
    let (root, controls) = fixture();
    power(root.path());
    let id = "powercap/test:0/constraint_0";
    for watts in [f64::NAN, f64::INFINITY, -10.0, 0.0, 9.0, 66.0] {
        assert!(controls.set_power_limit(id, watts).is_err());
    }
    assert!(controls.set_power_limit("../../etc/passwd", 30.0).is_err());
    assert_eq!(controls.snapshot().power_limits[0].watts, 45.0);
    controls.set_power_limit(id, 30.5).unwrap();
    assert_eq!(controls.snapshot().power_limits[0].watts, 30.5);
}

#[test]
fn missing_bounds_keep_power_read_only_and_suspended_devices_are_skipped() {
    let (root, controls) = fixture();
    put(root.path(), "class/hwmon/hwmon0/power1_cap", "45000000");
    assert!(!controls.snapshot().power_limits[0].writable);
    assert!(controls
        .set_power_limit("hwmon/hwmon0/power1_cap", 30.0)
        .is_err());
    put(root.path(), "class/hwmon/hwmon0/power1_cap_min", "10000000");
    put(root.path(), "class/hwmon/hwmon0/power1_cap_max", "65000000");
    controls
        .set_power_limit("hwmon/hwmon0/power1_cap", 35.0)
        .unwrap();
    assert_eq!(controls.snapshot().power_limits[0].watts, 35.0);
    put(
        root.path(),
        "class/hwmon/hwmon0/device/power/runtime_status",
        "suspended",
    );
    assert!(controls.snapshot().power_limits.is_empty());
}

#[test]
fn class_links_inside_sysfs_are_allowed_but_external_attributes_are_rejected() {
    let (root, mut controls) = fixture();
    let outside = TempDir::new().unwrap();
    put(root.path(), "devices/test/pwm1_enable", "3");
    fs::create_dir_all(root.path().join("class/hwmon")).unwrap();
    symlink(
        root.path().join("devices/test"),
        root.path().join("class/hwmon/hwmon0"),
    )
    .unwrap();
    controls.set_fan_boost("hwmon0/pwm1_enable", true).unwrap();
    controls.restore_fans().unwrap();
    put(outside.path(), "pwm2_enable", "3");
    symlink(
        outside.path().join("pwm2_enable"),
        root.path().join("devices/test/pwm2_enable"),
    )
    .unwrap();
    assert!(controls.set_fan_boost("hwmon0/pwm2_enable", true).is_err());
    assert_eq!(number(outside.path().join("pwm2_enable")), Some(3));
}
