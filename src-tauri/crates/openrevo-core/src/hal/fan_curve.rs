//! Shared original fan tables, interpolation, standby and hysteresis rules.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FanTableData {
    pub cpu_upt: [u8; 16],
    pub cpu_downt: [u8; 16],
    pub cpu_duty: [u8; 16],
    pub gpu_upt: [u8; 16],
    pub gpu_downt: [u8; 16],
    pub gpu_duty: [u8; 16],
    pub max_duty: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelFanTables {
    #[serde(default)]
    pub office: Option<FanTableData>,
    #[serde(default)]
    pub balanced: Option<FanTableData>,
    #[serde(default)]
    pub turbo: Option<FanTableData>,
}

pub const FAN_TABLE_OFFICE: FanTableData = FanTableData {
    cpu_upt: [
        0, 55, 57, 59, 61, 63, 65, 67, 69, 255, 255, 255, 255, 255, 255, 255,
    ],
    cpu_downt: [
        48, 58, 60, 62, 64, 66, 68, 70, 255, 255, 255, 255, 255, 255, 255, 255,
    ],
    cpu_duty: [
        0, 60, 60, 60, 60, 70, 70, 90, 110, 110, 110, 110, 110, 110, 110, 110,
    ], // 55% max duty
    gpu_upt: [
        0, 55, 55, 57, 57, 59, 62, 64, 66, 255, 255, 255, 255, 255, 255, 255,
    ],
    gpu_downt: [
        48, 56, 58, 58, 60, 63, 65, 67, 255, 255, 255, 255, 255, 255, 255, 255,
    ],
    gpu_duty: [
        0, 60, 60, 60, 60, 70, 70, 90, 110, 110, 110, 110, 110, 110, 110, 110,
    ],
    max_duty: 110,
};

pub const FAN_TABLE_GAMING: FanTableData = FanTableData {
    cpu_upt: [
        0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 255, 255, 255, 255, 255, 255,
    ],
    cpu_downt: [
        48, 58, 61, 65, 69, 73, 76, 79, 82, 255, 255, 255, 255, 255, 255, 255,
    ],
    cpu_duty: [
        0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 160, 160, 160, 160, 160, 160,
    ], // 80% max duty
    gpu_upt: [
        0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 255, 255, 255, 255, 255, 255,
    ],
    gpu_downt: [
        48, 58, 61, 65, 69, 73, 76, 79, 82, 255, 255, 255, 255, 255, 255, 255,
    ],
    gpu_duty: [
        0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 160, 160, 160, 160, 160, 160,
    ],
    max_duty: 160,
};

pub const FAN_TABLE_TURBO: FanTableData = FanTableData {
    cpu_upt: [
        0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 84, 255, 255, 255, 255, 255,
    ],
    cpu_downt: [
        48, 58, 61, 65, 69, 73, 76, 79, 82, 85, 255, 255, 255, 255, 255, 255,
    ],
    cpu_duty: [
        0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 180, 180, 180, 180, 180, 180,
    ], // 90% max duty
    gpu_upt: [
        0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 84, 255, 255, 255, 255, 255,
    ],
    gpu_downt: [
        48, 58, 61, 65, 69, 73, 76, 79, 82, 85, 255, 255, 255, 255, 255, 255,
    ],
    gpu_duty: [
        0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 180, 180, 180, 180, 180, 180,
    ],
    max_duty: 180,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FanCurvePoint {
    pub temp: u8,
    pub duty: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FanCurveConfig {
    pub cpu: Vec<FanCurvePoint>,
    pub gpu: Vec<FanCurvePoint>,
    pub isolated: bool,
    pub ramp_enabled: bool,
    pub ramp_ms: u16,
}

impl FanCurveConfig {
    pub fn validate(&self) -> Result<(), String> {
        if ![100, 300, 500, 1000].contains(&self.ramp_ms) {
            return Err("风扇过渡时间必须为 100 / 300 / 500 / 1000 ms".into());
        }
        for curve in [&self.cpu, &self.gpu] {
            if curve.len() != 8
                || curve
                    .iter()
                    .any(|p| !(30..=95).contains(&p.temp) || !(22..=100).contains(&p.duty))
                || curve
                    .windows(2)
                    .any(|p| p[0].temp >= p[1].temp || p[0].duty > p[1].duty)
                || curve[0].temp > 40
                || curve[7].temp != 95
                || curve[7].duty != 100
            {
                return Err("曲线需要 8 个温度递增、转速不下降的节点；转速为 22–100%，起点不高于 40°C，95°C 必须达到 100%".into());
            }
        }
        if !self.isolated {
            return Err("请开启风扇独立输出后应用自定义曲线".into());
        }
        Ok(())
    }
    pub fn table(&self) -> Result<FanTableData, String> {
        self.validate()?;
        Ok(build_custom_fan_table_with_base(
            &self.cpu,
            &self.gpu,
            &FAN_TABLE_TURBO,
        ))
    }
}

pub fn build_custom_fan_table_with_base(
    cpu_curve: &[FanCurvePoint],
    gpu_curve: &[FanCurvePoint],
    turbo_table: &FanTableData,
) -> FanTableData {
    // Standardized safe linear temperature progression (30°C ~ 100°C in 5°C increments)
    // Eliminates factory mute freeze (all 65s) and hysteresis deadlocks (DownT >= UpT)
    let mut custom_upt = [0u8; 16];
    let mut custom_downt = [0u8; 16];
    custom_upt[0] = 0;
    custom_downt[0] = 0;
    for i in 1..16 {
        let t = (30 + (i - 1) * 5) as u8; // Step 1: 30°C ... Step 15: 100°C
        custom_upt[i] = t;
        // Anti-Hunting Hysteresis: Widen gap to 8°C for idle/low-load (<= 55°C) to prevent start-stop oscillation!
        let hyst = if t <= 55 { 8 } else { 5 };
        custom_downt[i] = t.saturating_sub(hyst).max(custom_downt[i - 1]);
    }

    let (effective_cpu_upt, effective_cpu_downt) = if !cpu_curve.is_empty() {
        (custom_upt, custom_downt)
    } else {
        (turbo_table.cpu_upt, turbo_table.cpu_downt)
    };

    let (effective_gpu_upt, effective_gpu_downt) = if !gpu_curve.is_empty() {
        (custom_upt, custom_downt)
    } else {
        let mut upt = turbo_table.gpu_upt;
        let mut downt = turbo_table.gpu_downt;
        if upt[1] == 65 && upt[5] == 65 {
            upt = custom_upt;
            downt = custom_downt;
        } else {
            for i in 1..16 {
                if upt[i] != 255 && downt[i] >= upt[i] {
                    downt[i] = upt[i].saturating_sub(4);
                }
            }
        }
        (upt, downt)
    };

    let interpolate_duty =
        |curve: &[FanCurvePoint], base_upt: &[u8; 16], fallback_duty: &[u8; 16]| -> [u8; 16] {
            let mut duty = [0u8; 16];
            if curve.is_empty() {
                return *fallback_duty;
            }

            for step in 0..16 {
                let t = base_upt[step];
                if t == 255 {
                    duty[step] = duty[step.saturating_sub(1)];
                    continue;
                }
                if t <= curve[0].temp {
                    duty[step] = ((curve[0].duty as u16 * 200) / 100).min(200) as u8;
                    continue;
                }
                let last_idx = curve.len() - 1;
                if t >= curve[last_idx].temp {
                    duty[step] = ((curve[last_idx].duty as u16 * 200) / 100).min(200) as u8;
                    continue;
                }
                // Linear interpolation between adjacent curve points
                let mut found = false;
                for i in 0..last_idx {
                    let p1 = &curve[i];
                    let p2 = &curve[i + 1];
                    if t >= p1.temp && t <= p2.temp {
                        let span_t = (p2.temp - p1.temp) as f32;
                        let factor = if span_t > 0.0 {
                            (t - p1.temp) as f32 / span_t
                        } else {
                            0.0
                        };
                        let d = p1.duty as f32 + factor * (p2.duty as f32 - p1.duty as f32);
                        duty[step] = ((d * 2.0).round() as u16).min(200) as u8;
                        found = true;
                        break;
                    }
                }
                if !found {
                    duty[step] = ((curve[last_idx].duty as u16 * 200) / 100).min(200) as u8;
                }
            }

            // Hardware Safety Guard: Enforce strict monotonic non-decreasing progression
            // EC firmware rejects fan tables with inverted/decreasing duty steps!
            let mut running_max = 0u8;
            for value in &mut duty {
                if *value < running_max {
                    *value = running_max;
                } else {
                    running_max = *value;
                }
            }

            // CRITICAL MECHREVO / UNIWILL EC FIRMWARE RULE:
            // Step 0 in the CPU/GPU fan tables is the Zero-RPM standby threshold (0~50°C idle).
            // It MUST ALWAYS be 0x00! If written with non-zero, the EC fan FSM state machine
            // treats it as invalid standby configuration and completely locks up fan control!
            duty[0] = 0;

            duty
        };

    let cpu_duty = interpolate_duty(cpu_curve, &effective_cpu_upt, &turbo_table.cpu_duty);
    let mut gpu_duty = interpolate_duty(gpu_curve, &effective_gpu_upt, &turbo_table.gpu_duty);

    // Live hardware sniffer rule: GPU tail steps (RamFan1p5) must sync to max GPU duty
    let max_gpu = gpu_duty[12].max(gpu_duty[15]);
    gpu_duty[13] = gpu_duty[13].max(max_gpu);
    gpu_duty[14] = gpu_duty[14].max(max_gpu);
    gpu_duty[15] = max_gpu;

    let max_duty = cpu_duty[15].max(gpu_duty[15]).max(160);

    FanTableData {
        cpu_upt: effective_cpu_upt,
        cpu_downt: effective_cpu_downt,
        cpu_duty,
        gpu_upt: effective_gpu_upt,
        gpu_downt: effective_gpu_downt,
        gpu_duty,
        max_duty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn config() -> FanCurveConfig {
        let points = [
            (40, 22),
            (50, 30),
            (60, 45),
            (68, 55),
            (75, 65),
            (82, 75),
            (88, 85),
            (95, 100),
        ]
        .into_iter()
        .map(|(temp, duty)| FanCurvePoint { temp, duty })
        .collect::<Vec<_>>();
        FanCurveConfig {
            cpu: points.clone(),
            gpu: points,
            isolated: true,
            ramp_enabled: true,
            ramp_ms: 300,
        }
    }
    #[test]
    fn original_default_curve_regression() {
        let table = config().table().unwrap();
        assert_eq!(
            table.cpu_duty,
            [0, 44, 44, 44, 52, 60, 75, 90, 103, 116, 130, 144, 160, 179, 200, 200]
        );
        assert_eq!(
            table.gpu_duty,
            [0, 44, 44, 44, 52, 60, 75, 90, 103, 116, 130, 144, 160, 200, 200, 200]
        );
        assert_eq!(
            table.cpu_upt,
            [0, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100]
        );
        assert_eq!(
            table.cpu_downt,
            [0, 22, 27, 32, 37, 42, 47, 55, 60, 65, 70, 75, 80, 85, 90, 95]
        );
    }
    #[test]
    fn windows_empty_curve_still_uses_model_fallback() {
        let table = build_custom_fan_table_with_base(&[], &[], &FAN_TABLE_TURBO);
        assert_eq!(table.cpu_duty, FAN_TABLE_TURBO.cpu_duty);
        assert_eq!(table.cpu_upt, FAN_TABLE_TURBO.cpu_upt);
        assert_eq!(table.cpu_downt, FAN_TABLE_TURBO.cpu_downt);
    }
    #[test]
    fn rejects_malformed_and_unsafe_curves() {
        let base = config();
        let mut broken = base.clone();
        broken.cpu.pop();
        assert!(broken.validate().is_err());
        let mut broken = base.clone();
        broken.gpu[1].temp = broken.gpu[0].temp;
        assert!(broken.validate().is_err());
        let mut broken = base.clone();
        broken.cpu[4].duty = 10;
        assert!(broken.validate().is_err());
        let mut broken = base.clone();
        broken.cpu[6].duty = 101;
        assert!(broken.validate().is_err());
        let mut broken = base.clone();
        broken.gpu[7].duty = 95;
        assert!(broken.validate().is_err());
        let mut broken = base.clone();
        broken.isolated = false;
        assert!(broken.validate().is_err());
        let mut broken = base;
        broken.ramp_ms = 0;
        assert!(broken.validate().is_err());
    }
}
