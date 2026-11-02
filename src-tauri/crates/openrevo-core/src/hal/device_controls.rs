//! Persistent Uniwill controls; only declared bits are owned by each operation.
use super::{
    control_transaction::put,
    linux_ec::EcIo,
    vault::{ec::*, mask::*},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryMode {
    Full,
    Balanced,
    Longevity,
    Custom,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryRequest {
    pub mode: BatteryMode,
    pub lower: Option<u8>,
    pub upper: Option<u8>,
}
impl BatteryRequest {
    pub fn thresholds(self) -> Result<(u8, u8, u8), String> {
        let (lower, upper) = match self.mode {
            BatteryMode::Full => (95, 100),
            BatteryMode::Balanced => (75, 80),
            BatteryMode::Longevity => (55, 60),
            BatteryMode::Custom => (
                self.lower.ok_or("缺少充电下限")?,
                self.upper.ok_or("缺少充电上限")?,
            ),
        };
        if self.mode != BatteryMode::Custom && (self.lower.is_some() || self.upper.is_some()) {
            return Err("预设模式不能携带自定义阈值".into());
        }
        if lower == 0 || lower >= upper || upper > 100 {
            return Err("充电阈值要求 1 ≤ 下限 < 上限 ≤ 100".into());
        }
        let mode = if upper >= 95 {
            BATTERY_MODE_LONGLIFE
        } else if upper <= 65 {
            BATTERY_MODE_STATION
        } else {
            BATTERY_MODE_BALANCE
        };
        Ok((lower, upper, mode))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSwitch {
    WinLock,
    FnLock,
    Camera,
    UsbCharge,
    AcRecovery,
    Touchpad,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatteryControl {
    pub supported: bool,
    pub writable: bool,
    pub mode: Option<BatteryMode>,
    pub lower: Option<u8>,
    pub upper: Option<u8>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SwitchControl {
    pub id: DeviceSwitch,
    pub enabled: Option<bool>,
    pub writable: bool,
    pub detail: String,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceControls {
    pub battery: BatteryControl,
    pub switches: Vec<SwitchControl>,
    pub fault: bool,
    pub error: Option<String>,
}
fn byte(io: &impl EcIo, address: u16) -> Result<u8, String> {
    let value = io.read(address)?;
    if value == u8::MAX {
        Err("EC 返回无效控制值".into())
    } else {
        Ok(value)
    }
}
fn battery_values(io: &impl EcIo) -> Result<(u8, u8, u8), String> {
    if byte(io, ADDR_SUPPORT_BYTE2)? & MASK_BATTERY_LIMIT_SUPPORT == 0 {
        return Err("EC 未声明电池限充能力".into());
    }
    let lower = byte(io, ADDR_BATTERY_LIMIT_DOWN)?;
    let upper = byte(io, ADDR_BATTERY_LIMIT_UP)?;
    let mode = byte(io, ADDR_BATTERY_PROTECTION_MODE)? & MASK_BATTERY_MODE;
    if lower == 0
        || lower >= upper
        || upper > 100
        || ![
            BATTERY_MODE_LONGLIFE,
            BATTERY_MODE_BALANCE,
            BATTERY_MODE_STATION,
        ]
        .contains(&mode)
    {
        return Err("EC 电池策略无效，停止写入".into());
    }
    Ok((lower, upper, mode))
}
fn switch_field(id: DeviceSwitch) -> (u16, u8, bool) {
    match id {
        DeviceSwitch::WinLock => (ADDR_STATUS_BYTE, STATUS_WINLOCK, false),
        DeviceSwitch::FnLock => (ADDR_FN_LOCK, MASK_FN_LOCK, false),
        DeviceSwitch::Camera => (ADDR_CAMERA_CUTOFF, CAMERA_CUTOFF_BITMASK, true),
        DeviceSwitch::UsbCharge => (ADDR_TRIGGER_BYTE, TRIGGER_USB_CHARGE, false),
        DeviceSwitch::AcRecovery => (ADDR_AC_RECOVERY, AC_RECOVERY_BITMASK, false),
        DeviceSwitch::Touchpad => (ADDR_BATTERY_PROTECTION_MODE, TOUCHPAD_BITMASK, false),
    }
}
pub fn switch_value(io: &impl EcIo, id: DeviceSwitch) -> Result<bool, String> {
    if id == DeviceSwitch::WinLock && byte(io, ADDR_SUPPORT_BYTE1)? & MASK_WIN_LOCK_SUPPORT == 0 {
        return Err("EC 未声明 Win 锁能力".into());
    }
    let (address, mask, inverted) = switch_field(id);
    Ok((byte(io, address)? & mask != 0) ^ inverted)
}
pub fn snapshot(io: &impl EcIo, writable: bool) -> DeviceControls {
    let battery = match battery_values(io) {
        Ok((lower, upper, code)) => BatteryControl {
            supported: true,
            writable,
            lower: Some(lower),
            upper: Some(upper),
            mode: Some(match (lower, upper, code) {
                (95, 100, BATTERY_MODE_LONGLIFE) => BatteryMode::Full,
                (75, 80, BATTERY_MODE_BALANCE) => BatteryMode::Balanced,
                (55, 60, BATTERY_MODE_STATION) => BatteryMode::Longevity,
                _ => BatteryMode::Custom,
            }),
            error: None,
        },
        Err(error) => BatteryControl {
            error: Some(error),
            ..Default::default()
        },
    };
    let switches = [
        DeviceSwitch::WinLock,
        DeviceSwitch::FnLock,
        DeviceSwitch::Camera,
        DeviceSwitch::UsbCharge,
        DeviceSwitch::AcRecovery,
        DeviceSwitch::Touchpad,
    ]
    .into_iter()
    .map(|id| {
        let state = switch_value(io, id);
        SwitchControl {
            id,
            enabled: state.as_ref().ok().copied(),
            writable: writable && state.is_ok(),
            detail: state.err().unwrap_or_else(|| {
                if id == DeviceSwitch::Touchpad {
                    "仅同步 EC 触摸板标记；桌面输入开关由合成器管理".into()
                } else {
                    "EC 写入后读回确认".into()
                }
            }),
        }
    })
    .collect();
    DeviceControls {
        battery,
        switches,
        ..Default::default()
    }
}
#[derive(Clone, Copy)]
struct Field {
    address: u16,
    mask: u8,
    value: u8,
}
fn write_field(io: &impl EcIo, field: Field) -> Result<(), String> {
    let current = io.read(field.address)?;
    let value = if field.address == ADDR_TRIGGER_BYTE {
        field.value & TRIGGER_USB_CHARGE
    } else {
        (current & !field.mask) | (field.value & field.mask)
    };
    // Trigger command bits can be consumed by firmware; verify only persistent USB policy.
    if field.address == ADDR_TRIGGER_BYTE {
        io.write(field.address, value)?;
        if io.read(field.address)? & field.mask != value & field.mask {
            return Err("USB 策略读回不一致".into());
        }
        Ok(())
    } else {
        put(io, field.address, value)
    }
}
fn verify(io: &impl EcIo, fields: &[Field]) -> Result<(), String> {
    for field in fields {
        if io.read(field.address)? & field.mask != field.value & field.mask {
            return Err("设备设置提交后读回不一致".into());
        }
    }
    Ok(())
}
fn threshold_order(fields: &mut [Field], io: &impl EcIo) -> Result<(), String> {
    if fields.len() == 3 && fields[0].address == ADDR_BATTERY_LIMIT_DOWN {
        let upper = io.read(ADDR_BATTERY_LIMIT_UP)?;
        if fields[0].value >= upper {
            fields.swap(0, 1);
        }
    }
    Ok(())
}
#[derive(Default)]
pub struct DeviceTransaction {
    pending: Option<Vec<Field>>,
}
impl DeviceTransaction {
    pub fn faulted(&self) -> bool {
        self.pending.is_some()
    }
    fn apply(&mut self, io: &impl EcIo, mut fields: Vec<Field>) -> Result<(), String> {
        if self.faulted() {
            return Err("设备控制正在恢复，暂停新调节".into());
        }
        let saved = fields
            .iter()
            .map(|f| {
                Ok(Field {
                    value: io.read(f.address)?,
                    ..*f
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        threshold_order(&mut fields, io)?;
        self.pending = Some(saved);
        let result = fields
            .iter()
            .try_for_each(|f| write_field(io, *f))
            .and_then(|()| verify(io, &fields));
        match result {
            Ok(()) => {
                self.pending = None;
                Ok(())
            }
            Err(error) => match self.recover(io) {
                Ok(()) => Err(format!("{error}；已恢复原设置")),
                Err(restore) => Err(format!("{error}；恢复待重试：{restore}")),
            },
        }
    }
    pub fn recover(&mut self, io: &impl EcIo) -> Result<(), String> {
        let Some(mut fields) = self.pending.clone() else {
            return Ok(());
        };
        threshold_order(&mut fields, io)?;
        for field in fields {
            write_field(io, field)?;
        }
        verify(io, self.pending.as_ref().unwrap())?;
        self.pending = None;
        Ok(())
    }
    pub fn battery(&mut self, io: &impl EcIo, request: BatteryRequest) -> Result<(), String> {
        let (lower, upper, mode) = request.thresholds()?;
        battery_values(io)?;
        self.apply(
            io,
            vec![
                Field {
                    address: ADDR_BATTERY_LIMIT_DOWN,
                    mask: u8::MAX,
                    value: lower,
                },
                Field {
                    address: ADDR_BATTERY_LIMIT_UP,
                    mask: u8::MAX,
                    value: upper,
                },
                Field {
                    address: ADDR_BATTERY_PROTECTION_MODE,
                    mask: MASK_BATTERY_MODE,
                    value: mode,
                },
            ],
        )
    }
    pub fn switch(
        &mut self,
        io: &impl EcIo,
        id: DeviceSwitch,
        enabled: bool,
    ) -> Result<(), String> {
        let current = switch_value(io, id)?;
        if current == enabled {
            return Ok(());
        }
        if self.faulted() {
            return Err("设备控制正在恢复".into());
        }
        if id == DeviceSwitch::WinLock {
            let trigger = io.read(ADDR_TRIGGER_BYTE)? & TRIGGER_USB_CHARGE;
            let sent = io.write(ADDR_TRIGGER_BYTE, trigger | TRIGGER_WINLOCK);
            // Never automatically repeat a toggle pulse after a lost reply.
            for _ in 0..5 {
                if switch_value(io, id) == Ok(enabled) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            return Err(format!(
                "Win 锁切换未确认，请刷新后再操作{}",
                sent.err().map(|e| format!("：{e}")).unwrap_or_default()
            ));
        }
        let (address, mask, inverted) = switch_field(id);
        self.apply(
            io,
            vec![Field {
                address,
                mask,
                value: if enabled ^ inverted { mask } else { 0 },
            }],
        )
    }
}
#[cfg(test)]
mod tests;
