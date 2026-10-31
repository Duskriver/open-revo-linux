// OpenRevo - Single Atomic Actuator: Full-Power Architecture Pipeline
// Dedicated for Fixed Base TGP GPUs (e.g. RTX 4080L / 4090L @ 150W Base)
// Inviolable Rules:
// 1. ADDR_TGP_OFFSET MUST be 0x00 (Fixed Base TGP, zero cTGP offset)
// 2. ADDR_DB_FLAG1 MUST be 0xFF (Handshake kept alive, NEVER write 0x00!)
// 3. Strictly BANNED from writing mirror registers outside the declared GPU power contract
// 4. Super Performance (ADDR_FAN_BOOST_CLAMP Bit 4) is mutually exclusive with manual DB tuning:
//    - When Super Performance is ON, hardware clamps DB to 25W (175W total)
//    - When manually tuning DB (presets, sliders, step), automatically clear SuperPerf bit to ungate firmware
//    - When turning OFF Super Performance, restore user remembered DB state
// 5. Automatically remove factory cold-start 5W thermal margin (20W -> 25W full release)

use crate::driver::wmi_ec::{self, ADDR_DB_CTRL1, ADDR_DB_FLAG1, ADDR_DB_WATTS1, ADDR_FAN_BOOST_CLAMP, ADDR_TGP_OFFSET};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FullPowerStateTruth {
    pub super_perf: bool,
    pub db_enabled: bool,
    pub db_watts: u8,
    pub base_tgp: u16,
    pub expected_total_cap: u16,
}

pub struct FullPowerPipeline {
    base_tgp: u16,
    max_db_cap: u8,
    remembered_db_enabled: bool,
    remembered_db_watts: u8,
}

impl FullPowerPipeline {
    pub fn new(base_tgp: u16, max_db_cap: u8) -> Self {
        let base = if base_tgp > 0 { base_tgp } else { 150 };
        let max_db = if max_db_cap >= 5 && max_db_cap <= 35 { max_db_cap } else { 25 };

        // Read initial state from silicon to respect active hardware configuration
        let cur_ctrl = wmi_ec::wmi_read_ec(ADDR_DB_CTRL1).unwrap_or(0x07);
        let cur_target = wmi_ec::wmi_read_ec(ADDR_DB_WATTS1).unwrap_or(max_db);
        let db_en = (cur_ctrl & 0x02) != 0 && cur_target > 0;
        let db_val = if cur_target >= 5 && cur_target <= max_db { cur_target } else { max_db };

        Self {
            base_tgp: base,
            max_db_cap: max_db,
            remembered_db_enabled: db_en,
            remembered_db_watts: db_val,
        }
    }

    /// Read realtime ground truth directly from hardware registers
    pub fn read_live_truth(&self) -> FullPowerStateTruth {
        let clamp_raw = wmi_ec::wmi_read_ec(ADDR_FAN_BOOST_CLAMP).unwrap_or(0);
        let ctrl_raw = wmi_ec::wmi_read_ec(ADDR_DB_CTRL1).unwrap_or(0x07);
        let flag_raw = wmi_ec::wmi_read_ec(ADDR_DB_FLAG1).unwrap_or(0xFF);
        let watts_raw = wmi_ec::wmi_read_ec(ADDR_DB_WATTS1).unwrap_or(0);

        let sp_active = (clamp_raw & 0x10) != 0;
        let db_active = (ctrl_raw & 0x02) != 0 && flag_raw == 0xFF && watts_raw > 0;
        let eff_db = if sp_active { self.max_db_cap } else if db_active { watts_raw } else { 0 };

        FullPowerStateTruth {
            super_perf: sp_active,
            db_enabled: db_active || sp_active,
            db_watts: eff_db,
            base_tgp: self.base_tgp,
            expected_total_cap: self.base_tgp + (eff_db as u16),
        }
    }

    /// Execute atomic power actuation on Full-Power physical bus
    pub fn apply(
        &mut self,
        super_perf: Option<bool>,
        db_enable: Option<bool>,
        db_watts: Option<u8>,
        label: &str,
    ) -> Result<FullPowerStateTruth, String> {
        let clamp_prev = wmi_ec::wmi_read_ec(ADDR_FAN_BOOST_CLAMP).unwrap_or(0);
        let cur_sp = (clamp_prev & 0x10) != 0;

        // 1. Determine Target Super Performance State
        let target_sp = if let Some(sp) = super_perf {
            sp
        } else if db_enable.is_some() || db_watts.is_some() {
            // User manually touched DB controls: automatically ungate Super Performance (Bit 4 = 0)
            false
        } else {
            cur_sp
        };

        // 2. Update Super Performance bit if changed
        let clamp_next = if target_sp {
            clamp_prev | 0x10
        } else {
            clamp_prev & !0x10
        };
        if clamp_next != clamp_prev {
            wmi_ec::wmi_write_ec(ADDR_FAN_BOOST_CLAMP, clamp_next);
        }

        // 3. Determine Dynamic Boost Target
        let (eff_db_en, eff_db_val) = if target_sp {
            // Super Performance forces DB to maximum hardware ceiling (e.g. 25W -> 175W)
            (true, self.max_db_cap)
        } else {
            // Manual tuning mode: restore or update remembered setpoints
            if let Some(en) = db_enable {
                self.remembered_db_enabled = en;
            }
            if let Some(w) = db_watts {
                self.remembered_db_watts = w.clamp(5, self.max_db_cap);
            }

            let val = if self.remembered_db_enabled {
                self.remembered_db_watts
            } else {
                0
            };
            (self.remembered_db_enabled, val)
        };

        // 4. Physical Bus Atomic Writes
        // Control byte: 0x07 = Enable DB, 0x05 = Disable DB
        let db_ctrl = if target_sp || eff_db_en { 0x07 } else { 0x05 };

        wmi_ec::wmi_write_ec(ADDR_DB_CTRL1, db_ctrl);
        wmi_ec::wmi_write_ec(ADDR_TGP_OFFSET, 0x00); // ADDR_TGP_OFFSET MUST be 0x00 on full-power
        wmi_ec::wmi_write_ec(ADDR_DB_FLAG1, 0xFF);   // ADDR_DB_FLAG1 MUST be 0xFF, never clear to 0x00!
        wmi_ec::wmi_write_ec(ADDR_DB_WATTS1, eff_db_val);

        let total = self.base_tgp + (eff_db_val as u16);
        crate::log_info!(
            "ACTUATOR",
            "Applied [{}]: Full-Power Base={}W, DB={}W (En={}), SuperPerf={} -> Cap={}W",
            label, self.base_tgp, eff_db_val, eff_db_en, target_sp, total
        );

        Ok(FullPowerStateTruth {
            super_perf: target_sp,
            db_enabled: eff_db_en,
            db_watts: eff_db_val,
            base_tgp: self.base_tgp,
            expected_total_cap: total,
        })
    }

    /// Hard emergency unlock for Full-Power architecture (clears SuperPerf and re-arms 25W DB)
    pub fn emergency_unlock(&mut self) -> Result<FullPowerStateTruth, String> {
        self.remembered_db_enabled = true;
        self.remembered_db_watts = self.max_db_cap;
        self.apply(Some(false), Some(true), Some(self.max_db_cap), "Full-Power Emergency Unlock")
    }
}
