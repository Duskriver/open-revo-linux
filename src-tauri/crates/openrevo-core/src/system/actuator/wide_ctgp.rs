// OpenRevo - Single Atomic Actuator: Wide cTGP Architecture Pipeline
// Dedicated for Configurable Floor GPUs (e.g. RTX 4050~4070 / 5060)
// Inviolable Rules:
// 1. Dedicated 3-register envelope: ONLY ADDR_DB_CTRL1, ADDR_TGP_OFFSET, ADDR_DB_WATTS1!
// 2. ADDR_DB_FLAG1 MUST be kept at 0xFF (Handshake kept alive, NEVER write 0x00!)
// 3. Strictly BANNED from writing ADDR_FAN_BOOST_CLAMP (SuperPerf is full-power only, writing breaks EC state machine)
// 4. Strictly BANNED from writing mirror registers outside the declared GPU power contract
// 5. Base TGP = Floor + ADDR_TGP_OFFSET. NEVER clear offset to 0 on mode changes!
// 6. Automatically remove factory cold-start 5W thermal margin (10W -> 15W full release)

use crate::driver::wmi_ec::{self, ADDR_DB_CTRL1, ADDR_DB_FLAG1, ADDR_DB_WATTS1, ADDR_TGP_OFFSET};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WideCtgpStateTruth {
    pub db_enabled: bool,
    pub db_watts: u8,
    pub ctgp_floor: u16,
    pub offset_watts: u8,
    pub effective_base_tgp: u16,
    pub expected_total_cap: u16,
}

pub struct WideCtgpPipeline {
    ctgp_floor: u16,
    max_offset: u8,
    max_db_cap: u8,
    remembered_offset: u8,
    remembered_db_enabled: bool,
    remembered_db_watts: u8,
}

impl WideCtgpPipeline {
    pub fn new(ctgp_floor: u16, max_base_tgp: u16, max_db_cap: u8) -> Self {
        let floor = if ctgp_floor > 0 { ctgp_floor } else { 50 };
        let max_db = if max_db_cap >= 5 && max_db_cap <= 35 { max_db_cap } else { 15 };
        let max_base = if max_base_tgp >= floor { max_base_tgp } else { floor + 50 };
        let max_off = (max_base - floor).min(100) as u8;

        // Snapshot current hardware configuration
        let cur_ctrl = wmi_ec::wmi_read_ec(ADDR_DB_CTRL1).unwrap_or(0x07);
        let cur_offset = wmi_ec::wmi_read_ec(ADDR_TGP_OFFSET).unwrap_or(max_off);
        let cur_watts = wmi_ec::wmi_read_ec(ADDR_DB_WATTS1).unwrap_or(max_db);

        let off = if cur_offset > 0 && cur_offset <= max_off { cur_offset } else { max_off };
        let db_en = (cur_ctrl & 0x02) != 0 && cur_watts > 0;
        let db_val = if cur_watts >= 5 && cur_watts <= max_db { cur_watts } else { max_db };

        Self {
            ctgp_floor: floor,
            max_offset: max_off,
            max_db_cap: max_db,
            remembered_offset: off,
            remembered_db_enabled: db_en,
            remembered_db_watts: db_val,
        }
    }

    /// Read realtime ground truth directly from hardware registers
    pub fn read_live_truth(&self) -> WideCtgpStateTruth {
        let ctrl_raw = wmi_ec::wmi_read_ec(ADDR_DB_CTRL1).unwrap_or(0x07);
        let offset_raw = wmi_ec::wmi_read_ec(ADDR_TGP_OFFSET).unwrap_or(0);
        let flag_raw = wmi_ec::wmi_read_ec(ADDR_DB_FLAG1).unwrap_or(0xFF);
        let watts_raw = wmi_ec::wmi_read_ec(ADDR_DB_WATTS1).unwrap_or(0);

        let db_active = (ctrl_raw & 0x02) != 0 && flag_raw == 0xFF && watts_raw > 0;
        let eff_base = self.ctgp_floor + (offset_raw as u16);
        let eff_db = if db_active { watts_raw } else { 0 };

        WideCtgpStateTruth {
            db_enabled: db_active,
            db_watts: eff_db,
            ctgp_floor: self.ctgp_floor,
            offset_watts: offset_raw,
            effective_base_tgp: eff_base,
            expected_total_cap: eff_base + (eff_db as u16),
        }
    }

    /// Execute atomic power actuation on Wide cTGP physical bus
    pub fn apply(
        &mut self,
        db_enable: Option<bool>,
        db_watts: Option<u8>,
        offset_watts: Option<u8>,
        label: &str,
    ) -> Result<WideCtgpStateTruth, String> {
        // 1. Update setpoints
        if let Some(off) = offset_watts {
            self.remembered_offset = off.min(self.max_offset);
        }
        if let Some(en) = db_enable {
            self.remembered_db_enabled = en;
        }
        if let Some(w) = db_watts {
            self.remembered_db_watts = w.clamp(5, self.max_db_cap);
        }

        let target_off = self.remembered_offset;
        let target_db = if self.remembered_db_enabled {
            self.remembered_db_watts
        } else {
            0
        };

        // 2. Control byte: 0x07 = Enable DB (Bit 0 + Bit 1 + Bit 2), 0x05 = Disable DB
        let db_ctrl = if self.remembered_db_enabled && target_db > 0 { 0x07 } else { 0x05 };

        // 3. Ensure ADDR_DB_FLAG1 driver handshake is kept alive (0xFF)
        let flag_prev = wmi_ec::wmi_read_ec(ADDR_DB_FLAG1).unwrap_or(0);
        if flag_prev != 0xFF {
            wmi_ec::wmi_write_ec(ADDR_DB_FLAG1, 0xFF);
        }

        // 4. Physical Bus Atomic Writes: Only ADDR_DB_CTRL1, ADDR_TGP_OFFSET, ADDR_DB_WATTS1!
        wmi_ec::wmi_write_ec(ADDR_DB_CTRL1, db_ctrl);
        wmi_ec::wmi_write_ec(ADDR_TGP_OFFSET, target_off);
        wmi_ec::wmi_write_ec(ADDR_DB_WATTS1, target_db);

        let eff_base = self.ctgp_floor + (target_off as u16);
        let total = eff_base + (target_db as u16);

        crate::log_info!(
            "ACTUATOR",
            "Applied [{}]: Wide Base={}W (Floor={}W + Off={}W), DB={}W (En={}) -> Cap={}W",
            label, eff_base, self.ctgp_floor, target_off, target_db, self.remembered_db_enabled, total
        );

        Ok(WideCtgpStateTruth {
            db_enabled: self.remembered_db_enabled,
            db_watts: target_db,
            ctgp_floor: self.ctgp_floor,
            offset_watts: target_off,
            effective_base_tgp: eff_base,
            expected_total_cap: total,
        })
    }

    /// Hard emergency unlock for Wide cTGP architecture (re-arms ADDR_DB_FLAG1=0xFF, MaxOffset, MaxDB)
    pub fn emergency_unlock(&mut self) -> Result<WideCtgpStateTruth, String> {
        self.remembered_offset = self.max_offset;
        self.remembered_db_enabled = true;
        self.remembered_db_watts = self.max_db_cap;
        self.apply(Some(true), Some(self.max_db_cap), Some(self.max_offset), "Wide cTGP Emergency Unlock")
    }
}
