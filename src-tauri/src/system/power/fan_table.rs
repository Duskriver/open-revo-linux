pub use openrevo_core::hal::fan_curve::{FanTableData, ModelFanTables, FAN_TABLE_OFFICE, FAN_TABLE_GAMING, FAN_TABLE_TURBO};

pub fn get_mode_fan_table(ui_mode: u8) -> FanTableData {
    let active_profile = crate::system::model_engine::get_active_profile();
    let fan_tables = active_profile.as_ref().and_then(|p| p.fan_tables.as_ref());
    match ui_mode {
        1 => fan_tables.and_then(|ft| ft.office.clone()).unwrap_or_else(|| FAN_TABLE_OFFICE.clone()),
        2 => fan_tables.and_then(|ft| ft.balanced.clone()).unwrap_or_else(|| FAN_TABLE_GAMING.clone()),
        3 => fan_tables.and_then(|ft| ft.turbo.clone()).unwrap_or_else(|| FAN_TABLE_TURBO.clone()),
        _ => fan_tables.and_then(|ft| ft.balanced.clone()).unwrap_or_else(|| FAN_TABLE_GAMING.clone()),
    }
}

pub fn apply_mode_fan_table(ui_mode: u8) {
    let cfg = crate::system::config::load_config();
    if cfg.fan_boost || crate::driver::wmi_ec::is_fan_boost_active() {
        crate::log_info!("POWER_MODE", "apply_mode_fan_table: fan boost is active, skipping fan table write to preserve full fan speed");
        return;
    }
    // Ground Truth: Legacy dual-mode chassis (e.g. GK5 / 10-series / 20-series) strictly uses
    // internal EC ROM fan curves. Injecting RAM fan tables corrupts the legacy fan state machine!
    if crate::driver::wmi_ec::is_legacy_dual_mode_ec() {
        crate::log_info!("POWER_MODE", "apply_mode_fan_table: Legacy Dual-Mode chassis detected, yielding to native EC firmware fan curve");
        return;
    }
    let table = get_mode_fan_table(ui_mode);
    apply_fan_table_to_ec(&table);
}

pub fn build_custom_fan_table(
    cpu_curve: &[crate::system::config::FanCurvePoint],
    gpu_curve: &[crate::system::config::FanCurvePoint],
) -> FanTableData {
    openrevo_core::hal::fan_curve::build_custom_fan_table_with_base(cpu_curve, gpu_curve, &get_mode_fan_table(3))
}

pub fn apply_fan_table_to_ec(table: &FanTableData) {
    use crate::driver::wmi_ec;

    if wmi_ec::is_legacy_dual_mode_ec() {
        crate::log_warn!("POWER_MODE", "apply_fan_table_to_ec: Legacy Dual-Mode chassis does not support RamFan1p5 table injection, skipping to prevent fan oscillation");
        return;
    }

    // 0. Ensure APExist is asserted so motherboard accepts table writes
    if let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_AP_OEM_BYTE) {
        if (val & 0x01) == 0 {
            let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_AP_OEM_BYTE, val | 0x01);
        }
    }

    // Official Mechrevo / Uniwill EC specification & Ground-Truth Live Sniffer Rules:
    // 1. Write full CPU & GPU temperature thresholds (UpT, DownT, GPU UpT, GPU DownT).
    //    Leaving temperature thresholds unwritten (all 0s) causes EC to evaluate any temp > 0 as exceeding Step 15,
    //    leading to the notorious 100% full-speed fan lockup bug!
    // 2. CPU Fan Duty and GPU Fan Duty (16 bytes).
    // 3. Step 0 is Zero-RPM Standby: MUST ALWAYS BE 0x00!
    // 4. RamFan1p5 status & control registers sync with maximum GPU duty.
    // 5. Hardware Latch: Pre-latch pulse, then write all tables, then commit!

    // Step A: Pre-latch reset pulse
    wmi_ec::latch_fan_table_ec(true);

    // Step B: Assert APExist & AP Active
    if let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_AP_OEM_BYTE) {
        let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_AP_OEM_BYTE, val | 0x81);
    }

    let mut entries = Vec::with_capacity(70);
    // 1. CPU Up Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_UPT_BASE + i as u16, table.cpu_upt[i]));
    }
    // 2. CPU Down Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_DOWNT_BASE + i as u16, table.cpu_downt[i]));
    }
    // 3. CPU Fan Duty (16 bytes, Step 0 = 0)
    for i in 0..16 {
        let duty = if i == 0 { 0 } else { table.cpu_duty[i] };
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_DUTY_BASE + i as u16, duty));
    }
    // 4. GPU Up Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_UPT_BASE + i as u16, table.gpu_upt[i]));
    }
    // 5. GPU Down Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_DOWNT_BASE + i as u16, table.gpu_downt[i]));
    }
    // 6. GPU Fan Duty (13 bytes, Step 0 = 0)
    for i in 0..13 {
        let duty = if i == 0 { 0 } else { table.gpu_duty[i] };
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_DUTY_BASE + i as u16, duty));
    }

    // 7. Sync RamFan1p5 status & control registers
    // CRITICAL GROUND-TRUTH RULES:
    // - Status/max-duty thresholds: sync to max GPU duty (table.gpu_duty[15]).
    // - ADDR_RAMFAN_TABLE_CTRL:
    //   ABSOLUTELY NEVER WRITE 0 TO TABLE_CTRL!
    //   Writing 0 signals a controller reset/disable to the EC firmware, which causes the EC
    //   fail-safe watchdog to panic and blast both fans to 5000+ RPM full-speed lockup!
    //   In official GCUService and live sniffer telemetry, active control byte is 0xA0 (160) or clamp to safe non-zero.
    let max_gpu = table.gpu_duty[15];
    let ctrl_val = if max_gpu > 0 { max_gpu.max(160) } else { 0xA0 };
    entries.push((wmi_ec::ADDR_RAMFAN_GPU_MAX_THRESHOLD1, max_gpu));
    entries.push((wmi_ec::ADDR_RAMFAN_GPU_MAX_THRESHOLD2, max_gpu));
    entries.push((wmi_ec::ADDR_RAMFAN_TABLE_CTRL, ctrl_val));

    wmi_ec::wmi_write_ec_batch(&entries);

    // Step C: Post-latch Commit: Register ADDR_FAN_TABLE_LATCH = 0x04!
    // Ground-truth reverse engineered from official GCUService:
    // Writing 0x04 latches and commits the RAM fan table into the EC firmware active controller!
    wmi_ec::commit_fan_table_latch_ec();

    if !wmi_ec::is_nextgen_oem_magic_ec() {
        if let Some(cur_trig) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_TRIGGER_BYTE) {
            let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_TRIGGER_BYTE, (cur_trig & 0x10) | 0x20);
        }
    }

    crate::log_info!(
        "POWER_MODE",
        "Full fan table committed to EC RAM (UpT/DownT thresholds + Duties, fan table latch committed):\n  CPU Duty: {:?}\n  GPU Duty: {:?}",
        table.cpu_duty,
        table.gpu_duty
    );
}
