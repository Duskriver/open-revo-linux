//! Explicit hardware smoke test. Default invocation is read-only.
#[cfg(target_os = "linux")]
fn require_healthy(
    snapshot: &openrevo_core::hal::linux_ec::OemSnapshot,
    stage: &str,
) -> Result<(), String> {
    if !snapshot.available
        || !snapshot.writable
        || snapshot.error.is_some()
        || snapshot.control_fault
        || snapshot.fan_curve.fault
    {
        return Err(format!(
            "{stage}: EC is not ready: available={}, writable={}, error={:?}, control_fault={}, curve_fault={}",
            snapshot.available,
            snapshot.writable,
            snapshot.error,
            snapshot.control_fault,
            snapshot.fan_curve.fault,
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn require_unchanged_policy(
    before: &openrevo_core::hal::linux_ec::OemSnapshot,
    after: &openrevo_core::hal::linux_ec::OemSnapshot,
    stage: &str,
) -> Result<(), String> {
    if after.mode != before.mode
        || after.custom_power != before.custom_power
        || after.limits != before.limits
        || after.presets != before.presets
        || after.fan_curve.supported != before.fan_curve.supported
        || after.fan_curve.active != before.fan_curve.active
        || after.fan_curve.fault != before.fan_curve.fault
        || after.fan_curve.applied != before.fan_curve.applied
    {
        return Err(format!(
            "{stage}: mode, custom power, CPU limits, presets or curve state changed"
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn ac_smoke(
    backend: &mut openrevo_core::linux::LinuxBackend,
    before: &openrevo_core::linux::Snapshot,
) -> Result<(), String> {
    use openrevo_core::hal::linux_ec::CpuLimits;
    require_healthy(&before.oem, "Before AC smoke")?;
    if before.ac_online != Some(true)
        || !before.oem.dc_jack
        || before.oem.mode != Some(2)
        || before.oem.custom_power
        || before.oem.fan_boost
        || before.oem.fan_curve.active
        || before.oem.presets.len() != 3
        || before.oem.limits != before.oem.presets.get(1).copied()
    {
        return Err(
            "AC smoke requires barrel power, factory balanced mode and automatic fans".into(),
        );
    }
    let requested = CpuLimits {
        pl1: 35,
        pl2: 35,
        pl4: 70,
    };
    if before.oem.presets[0] != requested {
        return Err("AC smoke is limited to the verified 35/35/70 W office preset".into());
    }
    let result = (|| -> Result<(), String> {
        for mode in [2, 3] {
            backend.set_oem_mode(mode)?;
            let after = backend.snapshot();
            println!(
                "ACMode: {}",
                serde_json::to_string(&after.oem).map_err(|e| e.to_string())?
            );
            require_healthy(&after.oem, "AC mode")?;
            if after.oem.mode != Some(mode)
                || after.oem.custom_power
                || after.oem.limits != Some(before.oem.presets[usize::from(mode - 1)])
                || after.oem.presets != before.oem.presets
                || after.oem.fan_boost
                || after.oem.fan_curve.active
            {
                return Err("AC mode, factory limits or automatic fans did not read back".into());
            }
        }
        backend.set_oem_cpu_limits(requested.pl1, requested.pl2, requested.pl4)?;
        let after = backend.snapshot();
        println!(
            "ACPower: {}",
            serde_json::to_string(&after.oem).map_err(|e| e.to_string())?
        );
        require_healthy(&after.oem, "AC power")?;
        if !after.oem.custom_power
            || after.oem.limits != Some(requested)
            || after.oem.presets != before.oem.presets
            || after.oem.fan_boost
            || after.oem.fan_curve.active
        {
            return Err(
                "Custom CPU tuple or unchanged factory/fan policy did not read back".into(),
            );
        }
        Ok(())
    })();
    // Modes and power persist after Drop. Restore the original factory mode
    // explicitly on both success and failure, including a recovery attempt.
    let restore = (|| -> Result<(), String> {
        backend.release_controls()?;
        backend.set_oem_mode(2)?;
        backend.release_controls()?;
        let after = backend.snapshot();
        println!(
            "ACRestored: {}",
            serde_json::to_string(&after.oem).map_err(|e| e.to_string())?
        );
        require_healthy(&after.oem, "AC restored")?;
        require_unchanged_policy(&before.oem, &after.oem, "AC restored")?;
        if after.oem.fan_boost {
            return Err("AC restore left fan boost enabled".into());
        }
        Ok(())
    })();
    match (result, restore) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(format!("{error}; original balanced policy restored")),
        (result, Err(error)) => Err(format!(
            "AC test result: {result:?}; restoration failed: {error}"
        )),
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), String> {
    use openrevo_core::linux::LinuxBackend;
    let mut backend = LinuxBackend::new();
    let before = backend.snapshot();
    println!(
        "{}",
        serde_json::to_string_pretty(&before.oem).map_err(|error| error.to_string())?
    );
    match std::env::args().nth(1).as_deref() {
        Some("--ac-smoke") => ac_smoke(&mut backend, &before)?,
        Some("--fan-smoke") => {
            require_healthy(&before.oem, "Before fan smoke")?;
            if before.oem.fan_boost || before.oem.fan_curve.active {
                return Err("Need automatic fans and no active custom fan curve".into());
            }
            backend.set_oem_fan_boost(true)?;
            std::thread::sleep(std::time::Duration::from_secs(5));
            let boosted = backend.snapshot();
            println!(
                "Boosted: {}",
                serde_json::to_string(&boosted.oem).map_err(|error| error.to_string())?
            );
            // Release before validating readback so a bad snapshot cannot keep
            // boost active for the rest of the smoke test. Drop covers errors.
            backend.release_controls()?;
            let restored = backend.snapshot();
            require_healthy(&boosted.oem, "Boosted")?;
            if !boosted.oem.fan_boost {
                return Err("Fan boost did not enable".into());
            }
            require_healthy(&restored.oem, "Restored")?;
            if restored.oem.fan_boost {
                return Err("Fan boost did not restore".into());
            }
            require_unchanged_policy(&before.oem, &boosted.oem, "Boosted")?;
            require_unchanged_policy(&before.oem, &restored.oem, "Restored")?;
            println!(
                "Restored: {}",
                serde_json::to_string(&restored.oem).map_err(|error| error.to_string())?
            );
        }
        Some("--office") => {
            require_healthy(&before.oem, "Before office mode")?;
            let office = before
                .oem
                .presets
                .first()
                .copied()
                .ok_or("Office factory preset is unavailable")?;
            backend.set_oem_mode(1)?;
            let after = backend.snapshot();
            require_healthy(&after.oem, "Office")?;
            if after.oem.mode != Some(1)
                || after.oem.custom_power
                || after.oem.limits != Some(office)
                || after.oem.presets != before.oem.presets
            {
                return Err(
                    "Office mode, disabled custom power and factory limits did not read back"
                        .into(),
                );
            }
            println!(
                "Office: {}",
                serde_json::to_string(&after.oem).map_err(|error| error.to_string())?
            );
        }
        None => {}
        _ => return Err("Use no argument, --fan-smoke, --office or --ac-smoke".into()),
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {}
