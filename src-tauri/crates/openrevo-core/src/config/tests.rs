use super::*;
use crate::hal::fan_curve::FanCurvePoint;
use std::os::unix::fs::{symlink, PermissionsExt};

fn curve(ramp_ms: u16) -> FanCurveConfig {
    let points: Vec<_> = [35, 45, 55, 65, 75, 85, 90, 95]
        .into_iter()
        .zip([22, 30, 40, 50, 60, 70, 85, 100])
        .map(|(temp, duty)| FanCurvePoint { temp, duty })
        .collect();
    FanCurveConfig {
        cpu: points.clone(),
        gpu: points,
        isolated: true,
        ramp_enabled: true,
        ramp_ms,
    }
}

fn fixture() -> (tempfile::TempDir, ConfigStore, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().join("xdg");
    let store = ConfigStore::from_config_home(&base).unwrap();
    (temporary, store, base.join("OpenRevo"))
}

#[test]
fn resolves_xdg_and_home_without_cwd_fallback_or_environment_mutation() {
    let xdg =
        ConfigStore::from_environment(Some("/tmp/config-home".into()), Some("/unused".into()))
            .unwrap();
    assert_eq!(xdg.directory, Path::new("/tmp/config-home/OpenRevo"));
    let home = ConfigStore::from_environment(None, Some("/tmp/user-home".into())).unwrap();
    assert_eq!(home.directory, Path::new("/tmp/user-home/.config/OpenRevo"));
    let empty_xdg =
        ConfigStore::from_environment(Some("".into()), Some("/tmp/user-home".into())).unwrap();
    assert_eq!(empty_xdg.directory, home.directory);
    assert_eq!(
        ConfigStore::from_environment(None, None).unwrap_err().code,
        "config_environment"
    );
    for base in ["relative", "/tmp/../etc"] {
        assert_eq!(
            ConfigStore::from_config_home(base).unwrap_err().code,
            "config_path"
        );
    }
    assert_eq!(
        ConfigStore::from_environment(Some("relative".into()), Some("/tmp/home".into()))
            .unwrap_err()
            .code,
        "config_path"
    );
}

#[test]
fn missing_load_and_export_have_no_filesystem_side_effects() {
    let (_temporary, store, directory) = fixture();
    assert_eq!(store.load().unwrap(), None);
    assert_eq!(store.export_json().unwrap(), None);
    assert!(!directory.parent().unwrap().exists());
}

#[test]
fn saves_roundtrips_private_settings_and_preserves_previous_good_document() {
    let (_temporary, store, directory) = fixture();
    let first = store.save_fan_curve_draft(Some(&curve(300))).unwrap();
    assert_eq!(first.version, CONFIG_VERSION);
    assert_eq!(store.load().unwrap(), Some(first.clone()));
    assert!(!directory.join(BACKUP_FILE).exists());
    let first_bytes = fs::read(directory.join(CONFIG_FILE)).unwrap();
    let second = store.save_fan_curve_draft(Some(&curve(500))).unwrap();
    assert_eq!(store.load().unwrap(), Some(second));
    assert_eq!(fs::read(directory.join(BACKUP_FILE)).unwrap(), first_bytes);
    assert_eq!(decode(&first_bytes).unwrap(), first);
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in [CONFIG_FILE, BACKUP_FILE] {
        assert_eq!(
            fs::metadata(directory.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        store.save_fan_curve_draft(None).unwrap().fan_curve_draft,
        None
    );
    assert!(store.load().unwrap().is_some());
}

#[test]
fn damaged_and_future_documents_are_distinct_from_missing_and_never_overwritten() {
    let (_temporary, store, directory) = fixture();
    store.save_fan_curve_draft(Some(&curve(300))).unwrap();
    store.save_fan_curve_draft(Some(&curve(500))).unwrap();
    let backup = fs::read(directory.join(BACKUP_FILE)).unwrap();
    for (bytes, expected) in [
        (b"{broken".as_slice(), "config_invalid"),
        (
            b"{\"version\":2,\"new_setting\":true}".as_slice(),
            "config_version",
        ),
    ] {
        fs::write(directory.join(CONFIG_FILE), bytes).unwrap();
        assert_eq!(store.load().unwrap_err().code, expected);
        assert_eq!(
            store
                .save_fan_curve_draft(Some(&curve(100)))
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(
            store
                .import_json(&encode(&AppConfig::default()).unwrap())
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(fs::read(directory.join(CONFIG_FILE)).unwrap(), bytes);
        assert_eq!(fs::read(directory.join(BACKUP_FILE)).unwrap(), backup);
    }
}

#[test]
fn rejects_invalid_curve_and_unknown_settings_before_creating_files() {
    let (_temporary, store, directory) = fixture();
    let mut bad = curve(300);
    bad.cpu[7].duty = 90;
    assert_eq!(
        store.save_fan_curve_draft(Some(&bad)).unwrap_err().code,
        "config_invalid"
    );
    bad = curve(300);
    bad.isolated = false;
    assert_eq!(
        store.save_fan_curve_draft(Some(&bad)).unwrap_err().code,
        "config_invalid"
    );
    let mut value = serde_json::to_value(AppConfig {
        version: 1,
        fan_curve_draft: Some(curve(300)),
    })
    .unwrap();
    value["fan_curve_draft"]["cpu"][0]["address"] = 42.into();
    assert_eq!(
        store.import_json(&value.to_string()).unwrap_err().code,
        "config_invalid"
    );
    assert_eq!(
        store
            .import_json(r#"{"version":1,"extra":true}"#)
            .unwrap_err()
            .code,
        "config_invalid"
    );
    assert_eq!(
        store
            .import_json(r#"{"fan_curve_draft":null}"#)
            .unwrap_err()
            .code,
        "config_invalid"
    );
    assert_eq!(
        store
            .import_json(r#"{"version":1,"version":1}"#)
            .unwrap_err()
            .code,
        "config_invalid"
    );
    assert!(!directory.exists());
}

#[test]
fn import_export_validate_json_without_accepting_paths() {
    let (_temporary, store, _directory) = fixture();
    let original = AppConfig {
        version: CONFIG_VERSION,
        fan_curve_draft: Some(curve(1000)),
    };
    assert_eq!(
        store.import_json(&encode(&original).unwrap()).unwrap(),
        original
    );
    assert_eq!(
        decode(store.export_json().unwrap().unwrap().as_bytes()).unwrap(),
        original
    );
    assert_eq!(
        store
            .import_json("/tmp/another-config.json")
            .unwrap_err()
            .code,
        "config_invalid"
    );
    assert_eq!(store.load().unwrap(), Some(original));
}

#[test]
fn rejects_oversized_imports_and_existing_documents_without_replacing_them() {
    let (_temporary, store, directory) = fixture();
    let oversized = " ".repeat(MAX_CONFIG_BYTES + 1);
    assert_eq!(
        store.import_json(&oversized).unwrap_err().code,
        "config_too_large"
    );
    assert!(!directory.exists());
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(CONFIG_FILE), &oversized).unwrap();
    assert_eq!(store.load().unwrap_err().code, "config_too_large");
    assert_eq!(
        store.save_fan_curve_draft(None).unwrap_err().code,
        "config_too_large"
    );
    assert_eq!(
        fs::read_to_string(directory.join(CONFIG_FILE)).unwrap(),
        oversized
    );
    assert!(!directory.join(BACKUP_FILE).exists());
}

#[test]
fn rejects_symlinks_in_parent_or_application_directory() {
    for link_app_directory in [false, true] {
        let (temporary, store, directory) = fixture();
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).unwrap();
        if link_app_directory {
            fs::create_dir(directory.parent().unwrap()).unwrap();
            symlink(&outside, &directory).unwrap();
        } else {
            symlink(&outside, directory.parent().unwrap()).unwrap();
        }
        assert_eq!(store.load().unwrap_err().code, "config_path");
        assert_eq!(
            store.save_fan_curve_draft(None).unwrap_err().code,
            "config_path"
        );
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}

#[test]
fn rejects_symlink_hardlink_and_directory_config_targets() {
    for kind in ["symlink", "hardlink", "directory"] {
        let (temporary, store, directory) = fixture();
        fs::create_dir_all(&directory).unwrap();
        let outside = temporary.path().join("outside.json");
        let original = encode(&AppConfig::default()).unwrap();
        fs::write(&outside, &original).unwrap();
        let target = directory.join(CONFIG_FILE);
        match kind {
            "symlink" => symlink(&outside, &target).unwrap(),
            "hardlink" => fs::hard_link(&outside, &target).unwrap(),
            _ => fs::create_dir(&target).unwrap(),
        }
        assert_eq!(store.load().unwrap_err().code, "config_path");
        assert_eq!(
            store.save_fan_curve_draft(None).unwrap_err().code,
            "config_path"
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), original);
    }
}

#[test]
fn rejects_backup_symlink_without_touching_current_or_external_data() {
    let (temporary, store, directory) = fixture();
    store.save_fan_curve_draft(Some(&curve(300))).unwrap();
    let original = fs::read(directory.join(CONFIG_FILE)).unwrap();
    let outside = temporary.path().join("outside.json");
    fs::write(&outside, "external data").unwrap();
    symlink(&outside, directory.join(BACKUP_FILE)).unwrap();
    assert_eq!(
        store.save_fan_curve_draft(None).unwrap_err().code,
        "config_path"
    );
    assert_eq!(fs::read(directory.join(CONFIG_FILE)).unwrap(), original);
    assert_eq!(fs::read_to_string(outside).unwrap(), "external data");
}

#[test]
fn independent_instances_serialize_writes_and_never_publish_partial_documents() {
    let (_temporary, store, directory) = fixture();
    store.save_fan_curve_draft(Some(&curve(100))).unwrap();
    let start = std::sync::Arc::new(std::sync::Barrier::new(9));
    std::thread::scope(|scope| {
        for index in 0..8 {
            let start = start.clone();
            let independent = ConfigStore::from_config_home(directory.parent().unwrap()).unwrap();
            scope.spawn(move || {
                start.wait();
                for _ in 0..3 {
                    independent
                        .save_fan_curve_draft(Some(&curve([100, 300, 500, 1000][index % 4])))
                        .unwrap();
                }
            });
        }
        start.wait();
        for _ in 0..100 {
            let bytes = fs::read(directory.join(CONFIG_FILE)).unwrap();
            assert!(decode(&bytes).unwrap().fan_curve_draft.is_some());
            std::thread::yield_now();
        }
    });
    assert!(store.load().unwrap().unwrap().fan_curve_draft.is_some());
    assert!(decode(&fs::read(directory.join(BACKUP_FILE)).unwrap())
        .unwrap()
        .fan_curve_draft
        .is_some());
    let mut names: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![OsString::from(CONFIG_FILE), OsString::from(BACKUP_FILE)]
    );
}
