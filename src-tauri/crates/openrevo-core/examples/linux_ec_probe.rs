//! Explicit hardware smoke test. Default invocation is read-only.
#[cfg(target_os = "linux")]
fn main() -> Result<(), String> {
    use openrevo_core::linux::LinuxBackend;
    let mut backend = LinuxBackend::new();
    let before = backend.snapshot();
    println!("{}", serde_json::to_string_pretty(&before.oem).unwrap());
    match std::env::args().nth(1).as_deref() {
        Some("--fan-smoke") => {
            if !before.oem.available || before.oem.fan_boost {
                return Err("Need available EC and automatic fans".into());
            }
            backend.set_oem_fan_boost(true)?;
            std::thread::sleep(std::time::Duration::from_secs(5));
            let boosted = backend.snapshot();
            println!("Boosted: {}", serde_json::to_string(&boosted.oem).unwrap());
            backend.release_controls()?;
            let restored = backend.snapshot();
            if restored.oem.fan_boost {
                return Err("Fan boost did not restore".into());
            }
            println!(
                "Restored: {}",
                serde_json::to_string(&restored.oem).unwrap()
            );
        }
        Some("--office") => {
            backend.set_oem_mode(1)?;
            let after = backend.snapshot();
            if after.oem.mode != Some(1) {
                return Err("Office mode did not read back".into());
            }
            println!("Office: {}", serde_json::to_string(&after.oem).unwrap());
        }
        None => {}
        _ => return Err("Use no argument, --fan-smoke or --office".into()),
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {}
