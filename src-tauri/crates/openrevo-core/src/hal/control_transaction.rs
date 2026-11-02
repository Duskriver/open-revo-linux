//! Tongfang mode/power policy and rollback. The kernel only transports bytes.
use super::{
    linux_ec::{presets, validate_limits, CpuLimits, EcIo, OemSnapshot},
    tongfang,
    vault::{ec::*, mask::*, tongfang as values, tongfang_control as policy},
};

const AP: usize = 0;
const LIGHT_A: usize = 1;
const LIGHT_B: usize = 2;
const INDEX: usize = 3;
const REQUEST: usize = 4;
const RAMP: usize = 5;
const ISOLATED: usize = 6;
const PL4: usize = 7;
const PL2: usize = 8;
const PL1: usize = 9;
const CLAMP: usize = 10;
const CUSTOM: usize = 11;
const FIELDS: [u16; 12] = [
    ADDR_AP_OEM_BYTE,
    ADDR_AC_RECOVERY,
    ADDR_CAMERA_CUTOFF,
    ADDR_MODE_INDEX,
    ADDR_MODE_REQUEST,
    ADDR_FAN_RAMP_RATE,
    ADDR_FAN_ISOLATED,
    ADDR_CUSTOM_PL4,
    ADDR_CUSTOM_PL2,
    ADDR_CUSTOM_PL1,
    ADDR_FAN_BOOST_CLAMP,
    ADDR_CUSTOM_MODE,
];

#[derive(Clone, Copy)]
struct Saved {
    values: [u8; FIELDS.len()],
    trigger: u8,
}

impl Saved {
    fn capture(io: &impl EcIo) -> Result<Self, String> {
        let mut values = [0; FIELDS.len()];
        for (index, address) in FIELDS.iter().enumerate() {
            values[index] = io.read(*address)?;
        }
        Ok(Self {
            values,
            trigger: io.read(ADDR_TRIGGER_BYTE)?,
        })
    }

    fn verify(&self, io: &impl EcIo) -> Result<(), String> {
        for (index, address) in FIELDS.iter().enumerate() {
            let actual = io.read(*address)?;
            // Firmware consumes only the silent request at commit. Immediate
            // write/readback is still exact, including this field.
            if actual != self.values[index]
                && !(index == REQUEST
                    && self.values[index] == policy::MODE_STATUS_REQUEST
                    && actual == policy::MODE_STATUS_IDLE)
            {
                return Err(format!("EC 模式/功耗提交读回不一致（字段 {index}）"));
            }
        }
        Ok(())
    }

    fn trigger(&self) -> u8 {
        tongfang::mode_trigger(
            self.trigger,
            if self.values[CUSTOM] == values::CUSTOM_ON {
                2
            } else {
                tongfang::decode_mode(self.values[CLAMP]).unwrap_or(2)
            },
        )
    }
}

pub fn put(io: &impl EcIo, address: u16, value: u8) -> Result<(), String> {
    io.write(address, value)?;
    if io.read(address)? != value {
        return Err("EC 写入后读回不一致".into());
    }
    Ok(())
}

fn require_power(io: &impl EcIo, system_power: &mut impl FnMut() -> bool) -> Result<(), String> {
    if !system_power() || io.read(ADDR_POWER_STATUS)? & POWER_STATUS_DC_JACK == 0 {
        return Err("此调节需要连接原装电源".into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub enum Setting {
    Mode(u8),
    Power(CpuLimits),
}

#[derive(Default)]
pub struct ControlTransaction {
    pending: Option<Saved>,
}

impl ControlTransaction {
    pub fn faulted(&self) -> bool {
        self.pending.is_some()
    }

    pub fn apply(
        &mut self,
        io: &impl EcIo,
        setting: Setting,
        mut system_power: impl FnMut() -> bool,
    ) -> Result<(), String> {
        if self.faulted() {
            return Err("机身控制事务正在故障恢复".into());
        }
        let needs_power = !matches!(setting, Setting::Mode(1));
        if let Setting::Mode(mode) = setting {
            if tongfang::mode_clamp(mode).is_none() {
                return Err("无效的机身性能模式".into());
            }
        }
        if needs_power {
            require_power(io, &mut system_power)?;
        }
        let factory = presets(io)?;
        if let Setting::Power(limits) = setting {
            validate_limits(
                &OemSnapshot {
                    presets: factory,
                    dc_jack: true,
                    ..Default::default()
                },
                limits,
                true,
            )?;
        }
        let original = Saved::capture(io)?;
        if tongfang::decode_mode(original.values[CLAMP]).is_none() {
            return Err("未知的 EC 模式编码".into());
        }
        let mut next = original;
        next.values[AP] |= values::AP_PRESENT;
        next.values[INDEX] &= values::MODE_PRESERVE;
        match setting {
            Setting::Mode(mode) => {
                next.values[LIGHT_A] &= !MASK_MODE_INDICATOR_A;
                next.values[LIGHT_B] &= !MASK_MODE_INDICATOR_B;
                next.values[CLAMP] = tongfang::mode_clamp(mode).unwrap()
                    | (next.values[CLAMP] & FAN_BOOST_CLAMP_BIT);
                if mode == 3 {
                    next.values[INDEX] |= values::MODE_TURBO;
                }
                next.values[REQUEST] = if mode == 1 {
                    policy::MODE_STATUS_REQUEST
                } else {
                    policy::MODE_STATUS_IDLE
                };
                next.values[CUSTOM] = values::CUSTOM_OFF;
                next.values[RAMP] = if mode == 3 { 0 } else { values::RAMP_SMOOTH };
                next.values[ISOLATED] &= !values::FAN_ISOLATED;
            }
            Setting::Power(limits) => {
                next.values[INDEX] |= values::MODE_TURBO;
                next.values[REQUEST] = policy::MODE_STATUS_IDLE;
                next.values[LIGHT_A] |= MASK_MODE_INDICATOR_A;
                next.values[LIGHT_B] |= MASK_MODE_INDICATOR_B;
                next.values[PL1] = limits.pl1;
                next.values[PL2] = limits.pl2;
                next.values[PL4] = limits.pl4;
                next.values[CUSTOM] = values::CUSTOM_ON;
            }
        }
        // Save before the first write, including writes that reach firmware but
        // fail to return an acknowledgement. Inactive tuples are staged PL4 first.
        self.pending = Some(original);
        let applied: Result<(), String> = (|| {
            put(io, ADDR_CUSTOM_MODE, values::CUSTOM_OFF)?;
            for (index, address) in FIELDS.iter().enumerate() {
                if index != CUSTOM && next.values[index] != original.values[index] {
                    put(io, *address, next.values[index])?;
                }
            }
            if needs_power {
                require_power(io, &mut system_power)?;
            }
            put(io, ADDR_CUSTOM_MODE, next.values[CUSTOM])?;
            io.write(ADDR_TRIGGER_BYTE, next.trigger())?;
            next.verify(io)?;
            if needs_power {
                require_power(io, &mut system_power)?;
            }
            Ok(())
        })();
        match applied {
            Ok(()) => {
                self.pending = None;
                Ok(())
            }
            Err(error) => match self.recover(io) {
                Ok(()) => Err(format!("{error}；已恢复操作前状态")),
                Err(restore) => Err(format!("{error}；恢复待重试：{restore}")),
            },
        }
    }

    pub fn recover(&mut self, io: &impl EcIo) -> Result<(), String> {
        let Some(original) = self.pending else {
            return Ok(());
        };
        let result = (|| {
            put(io, ADDR_CUSTOM_MODE, values::CUSTOM_OFF)?;
            let mut failure = None;
            for (index, address) in FIELDS.iter().enumerate() {
                if index != CUSTOM && index != CLAMP {
                    if let Err(error) = put(io, *address, original.values[index]) {
                        failure = Some(error);
                    }
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            put(io, ADDR_CUSTOM_MODE, original.values[CUSTOM])?;
            put(io, ADDR_FAN_BOOST_CLAMP, original.values[CLAMP])?;
            io.write(ADDR_TRIGGER_BYTE, original.trigger())?;
            original.verify(io)
        })();
        if result.is_ok() {
            self.pending = None;
        } else if let Ok(clamp) = io.read(ADDR_FAN_BOOST_CLAMP) {
            let _ = io.write(ADDR_FAN_BOOST_CLAMP, clamp | FAN_BOOST_CLAMP_BIT);
        }
        result
    }
}

pub fn boost(io: &impl EcIo, enabled: bool) -> Result<(), String> {
    let before = io.read(ADDR_FAN_BOOST_CLAMP)?;
    match put(
        io,
        ADDR_FAN_BOOST_CLAMP,
        tongfang::fan_boost_value(before, enabled),
    ) {
        Ok(()) => Ok(()),
        Err(error) => match put(io, ADDR_FAN_BOOST_CLAMP, before) {
            Ok(()) => Err(error),
            Err(restore) => Err(format!("{error}；强冷恢复待重试：{restore}")),
        },
    }
}

#[cfg(test)]
mod tests;
