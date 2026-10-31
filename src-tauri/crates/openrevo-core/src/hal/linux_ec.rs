//! Linux transport for the original modern Tongfang register contract.
use super::{
    fan_curve::FanCurveConfig,
    tongfang,
    vault::{ec::*, linux_fan as protocol, mask::*, tongfang as values},
};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::FileExt,
    time::{Duration, Instant},
};

pub trait EcIo {
    fn read(&self, address: u16) -> Result<u8, String>;
    fn write(&self, address: u16, value: u8) -> Result<(), String>;
}

impl EcIo for File {
    fn read(&self, address: u16) -> Result<u8, String> {
        let mut byte = [0];
        self.read_exact_at(&mut byte, u64::from(address))
            .map_err(|e| format!("EC 读取失败：{e}"))?;
        Ok(byte[0])
    }
    fn write(&self, address: u16, value: u8) -> Result<(), String> {
        self.write_all_at(&[value], u64::from(address))
            .map_err(|e| format!("EC 写入失败：{e}"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct CpuLimits {
    pub pl1: u8,
    pub pl2: u8,
    pub pl4: u8,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct OemSnapshot {
    pub available: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub mode: Option<u8>,
    pub custom_power: bool,
    pub fan_boost: bool,
    pub fan_rpm: Vec<u16>,
    pub dc_jack: bool,
    pub presets: Vec<CpuLimits>,
    pub limits: Option<CpuLimits>,
    pub fan_curve: CurveSnapshot,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CurveSnapshot {
    pub supported: bool,
    pub active: bool,
    pub fault: bool,
    pub applied: Option<FanCurveConfig>,
}

fn curve_frame(
    action: usize,
    config: Option<&FanCurveConfig>,
) -> Result<[u8; protocol::FRAME_SIZE], String> {
    let mut frame = [0; protocol::FRAME_SIZE];
    frame[protocol::VERSION_OFFSET] = protocol::VERSION as u8;
    frame[protocol::ACTION_OFFSET] = action as u8;
    if let Some(config) = config {
        let table = config.table()?;
        let mut offset = protocol::TABLE_OFFSET;
        for data in [
            table.cpu_upt,
            table.cpu_downt,
            table.cpu_duty,
            table.gpu_upt,
            table.gpu_downt,
            table.gpu_duty,
        ] {
            frame[offset..offset + protocol::TABLE_STEPS].copy_from_slice(&data);
            offset += protocol::TABLE_STEPS;
        }
        frame[protocol::RAMP_OFFSET] = protocol::RAMP_ENABLE as u8
            | if config.ramp_enabled {
                (config.ramp_ms / 100) as u8
            } else {
                1
            };
        frame[protocol::ISOLATED_OFFSET] = protocol::ISOLATED_BIT as u8;
    }
    Ok(frame)
}

fn curve_supported() -> bool {
    std::fs::read_to_string("/sys/module/openrevo_ec/parameters/fan_curve_abi")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        == Some(protocol::VERSION)
}

fn curve_flag(name: &str) -> bool {
    std::fs::read_to_string(format!("/sys/module/openrevo_ec/parameters/{name}"))
        .map(|s| s.trim() == "Y")
        .unwrap_or(false)
}

fn limits(io: &impl EcIo, addresses: [u16; 3]) -> Result<CpuLimits, String> {
    Ok(CpuLimits {
        pl1: io.read(addresses[0])?,
        pl2: io.read(addresses[1])?,
        pl4: io.read(addresses[2])?,
    })
}

fn presets(io: &impl EcIo) -> Result<Vec<CpuLimits>, String> {
    let result = vec![
        limits(io, [ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4])?,
        limits(io, [ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4])?,
        limits(io, [ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4])?,
    ];
    if result
        .iter()
        .any(|p| p.pl1 < 15 || p.pl2 < p.pl1 || p.pl4 < p.pl2 || p.pl4 == u8::MAX)
    {
        return Err("EC 出厂功耗表无效，停止硬件控制".into());
    }
    Ok(result)
}

fn rpm(io: &impl EcIo, high: u16, low: u16) -> Result<u16, String> {
    // Avoid a torn counter if the EC refreshes it between byte reads.
    for _ in 0..3 {
        let first = io.read(high)?;
        let second = io.read(low)?;
        if first == io.read(high)? {
            return Ok(u16::from_be_bytes([first, second]));
        }
    }
    Err("风扇转速正在更新，请重试".into())
}

pub fn read_snapshot(io: &impl EcIo) -> Result<OemSnapshot, String> {
    let factory = presets(io)?;
    let clamp = io.read(ADDR_FAN_BOOST_CLAMP)?;
    let mode = tongfang::decode_mode(clamp);
    let custom = io.read(ADDR_CUSTOM_MODE)? == values::CUSTOM_ON;
    let current = if custom {
        Some(limits(
            io,
            [ADDR_CUSTOM_PL1, ADDR_CUSTOM_PL2, ADDR_CUSTOM_PL4],
        )?)
    } else {
        mode.map(|m| factory[usize::from(m - 1)])
    };
    Ok(OemSnapshot {
        available: true,
        writable: false,
        error: None,
        mode,
        custom_power: custom,
        fan_boost: clamp & FAN_BOOST_CLAMP_BIT != 0,
        fan_rpm: vec![
            rpm(io, ADDR_MAIN_FAN_RPM_B1, ADDR_MAIN_FAN_RPM_B2)?,
            rpm(io, ADDR_SECOND_FAN_RPM_B1, ADDR_SECOND_FAN_RPM_B2)?,
        ],
        dc_jack: io.read(ADDR_POWER_STATUS)? & POWER_STATUS_DC_JACK != 0,
        presets: factory,
        limits: current,
        fan_curve: CurveSnapshot::default(),
    })
}

/// Capture every original value before the first write, verify each stable
/// register, and restore even the failing write (it may have reached firmware).
fn transaction(io: &impl EcIo, changes: &[(u16, u8)]) -> Result<(), String> {
    transaction_commit(io, changes, None)
}

fn transaction_commit(
    io: &impl EcIo,
    changes: &[(u16, u8)],
    trigger: Option<u8>,
) -> Result<(), String> {
    let originals: Vec<_> = changes
        .iter()
        .map(|(a, _)| io.read(*a))
        .collect::<Result<_, _>>()?;
    let original_trigger = if trigger.is_some() {
        Some(io.read(ADDR_TRIGGER_BYTE)?)
    } else {
        None
    };
    let rollback = |last: usize, error: String| -> Result<(), String> {
        let mut failed = false;
        for index in (0..=last).rev() {
            let address = changes[index].0;
            let expected = originals[index];
            failed |= io
                .write(address, expected)
                .and_then(|()| io.read(address))
                .map(|v| v != expected)
                .unwrap_or(true);
        }
        if let Some(old_trigger) = original_trigger {
            // Re-latch the restored mode, not the failed target. Trigger bits
            // self-clear, so restore their meaning instead of stale raw bits.
            if let Ok(clamp) = io.read(ADDR_FAN_BOOST_CLAMP) {
                if let Some(mode) = tongfang::decode_mode(clamp) {
                    failed |= io
                        .write(ADDR_TRIGGER_BYTE, tongfang::mode_trigger(old_trigger, mode))
                        .is_err();
                }
            } else {
                failed = true;
            }
        }
        Err(format!(
            "{error}；{}",
            if failed {
                "部分寄存器恢复失败，请退出控制并重启机器"
            } else {
                "已恢复操作前的寄存器"
            }
        ))
    };
    for (index, (address, value)) in changes.iter().enumerate() {
        if originals[index] == *value {
            continue;
        }
        let result = io.write(*address, *value).and_then(|()| {
            if io.read(*address)? == *value {
                Ok(())
            } else {
                Err("EC 写入后的读回不一致".into())
            }
        });
        if let Err(error) = result {
            return rollback(index, error);
        }
    }
    if let Some(value) = trigger {
        if let Err(error) = io.write(ADDR_TRIGGER_BYTE, value) {
            return rollback(changes.len() - 1, error);
        }
    }
    Ok(())
}

pub fn apply_mode(io: &impl EcIo, mode: u8, ac_online: bool) -> Result<(), String> {
    let base = tongfang::mode_clamp(mode).ok_or("无效的机身性能模式")?;
    let before = read_snapshot(io)?;
    if mode != 1 && !(ac_online && before.dc_jack) {
        return Err("均衡/狂暴模式需要连接原装电源".into());
    }
    let trigger = tongfang::mode_trigger(io.read(ADDR_TRIGGER_BYTE)?, mode);
    // Keep firmware factory power and fan tables. The Windows-only GPU/NVAPI
    // tuning and user RAM fan tables are separate capabilities.
    transaction_commit(
        io,
        &[
            (
                ADDR_AP_OEM_BYTE,
                io.read(ADDR_AP_OEM_BYTE)? | values::AP_PRESENT,
            ),
            (
                ADDR_AC_RECOVERY,
                io.read(ADDR_AC_RECOVERY)? & !MASK_QKEY_WHITE_1830,
            ),
            (
                ADDR_CAMERA_CUTOFF,
                io.read(ADDR_CAMERA_CUTOFF)? & !MASK_QKEY_WHITE_1831,
            ),
            (
                ADDR_FAN_BOOST_CLAMP,
                tongfang::fan_boost_value(base, before.fan_boost),
            ),
            (
                ADDR_MODE_INDEX,
                tongfang::mode_index(io.read(ADDR_MODE_INDEX)?, mode),
            ),
            (ADDR_TURBO_TRIGGER_1115, u8::from(mode == 1)),
            (ADDR_CUSTOM_MODE, values::CUSTOM_OFF),
            (
                ADDR_FAN_RAMP_RATE,
                if mode == 3 { 0 } else { values::RAMP_SMOOTH },
            ),
            (
                ADDR_FAN_ISOLATED,
                io.read(ADDR_FAN_ISOLATED)? & !values::FAN_ISOLATED,
            ),
        ],
        Some(trigger),
    )?;
    // Trigger bits are consumed by firmware; comparing them with the command
    // would wrongly report failure. Confirm stable mode flags afterwards.
    if tongfang::decode_mode(io.read(ADDR_FAN_BOOST_CLAMP)?) != Some(mode)
        || io.read(ADDR_CUSTOM_MODE)? != values::CUSTOM_OFF
    {
        return Err("机身模式未保持，可能被固件或另一个控制程序改写".into());
    }
    Ok(())
}

pub fn apply_limits(io: &impl EcIo, requested: CpuLimits, ac_online: bool) -> Result<(), String> {
    let before = read_snapshot(io)?;
    if !(ac_online && before.dc_jack) {
        return Err("CPU 功耗调节需要连接原装电源".into());
    }
    let ceiling = before.presets[2];
    let (pl1, pl2, pl4) = tongfang::safe_pl_limits(
        requested.pl1.into(),
        requested.pl2.into(),
        requested.pl4.into(),
        values::PL4_MARGIN,
    );
    if requested != (CpuLimits { pl1, pl2, pl4 })
        || pl1 > ceiling.pl1
        || pl2 > ceiling.pl2
        || pl4 > ceiling.pl4
    {
        return Err("功耗必须满足 PL1 ≤ PL2、PL4 ≥ PL2 + 20 W，且不超过 EC 出厂狂暴上限".into());
    }
    // Disable custom power before changing its table; firmware consumes the
    // complete validated tuple only after the final custom-mode switch.
    transaction(io, &[(ADDR_CUSTOM_MODE, values::CUSTOM_OFF)])?;
    let trigger = tongfang::mode_trigger(io.read(ADDR_TRIGGER_BYTE)?, 2);
    if let Err(error) = transaction_commit(
        io,
        &[
            (
                ADDR_AP_OEM_BYTE,
                io.read(ADDR_AP_OEM_BYTE)? | values::AP_PRESENT,
            ),
            (
                ADDR_MODE_INDEX,
                tongfang::mode_index(io.read(ADDR_MODE_INDEX)?, 3),
            ),
            (ADDR_TURBO_TRIGGER_1115, 0),
            (
                ADDR_AC_RECOVERY,
                io.read(ADDR_AC_RECOVERY)? | MASK_QKEY_WHITE_1830,
            ),
            (
                ADDR_CAMERA_CUTOFF,
                io.read(ADDR_CAMERA_CUTOFF)? | MASK_QKEY_WHITE_1831,
            ),
            (ADDR_CUSTOM_PL4, pl4),
            (ADDR_CUSTOM_PL2, pl2),
            (ADDR_CUSTOM_PL1, pl1),
            (ADDR_CUSTOM_MODE, values::CUSTOM_ON),
        ],
        Some(trigger),
    ) {
        return Err(format!("{error}；已请求停用自定义功耗，请检查当前读回"));
    }
    let after = read_snapshot(io)?;
    if !after.custom_power || after.limits != Some(requested) {
        return Err("CPU 功耗被固件改写，请检查当前读回".into());
    }
    Ok(())
}

#[derive(Default)]
pub struct LinuxEc {
    file: Option<File>,
    heartbeat: Option<Instant>,
    original_boost: Option<bool>,
    last_error: Option<String>,
    fan_config: Option<FanCurveConfig>,
}

impl LinuxEc {
    fn connect(&mut self) -> Result<&File, String> {
        if self.file.is_none() {
            self.file = Some(OpenOptions::new().read(true).write(true).open("/dev/openrevo_ec")
                .map_err(|e| format!("Uniwill EC 通道不可用：{e}；请安装 openrevo_ec 模块并授予当前会话访问权限"))?);
        }
        Ok(self.file.as_ref().unwrap())
    }
    pub fn snapshot(&mut self) -> OemSnapshot {
        self.heartbeat = Some(Instant::now());
        let active = curve_flag("fan_curve_active");
        let fault = curve_flag("fan_curve_fault");
        if !active {
            self.fan_config = None;
        }
        if active && !fault && self.fan_config.is_some() {
            if let Err(error) = self.send_curve(protocol::HEARTBEAT, None) {
                self.last_error = Some(error);
            }
        }
        match self.connect().and_then(read_snapshot) {
            Ok(mut result) => {
                // Permission on the device and the module's root-owned gate
                // are separate. Reading this parameter never enables writes.
                result.writable =
                    std::fs::read_to_string("/sys/module/openrevo_ec/parameters/allow_writes")
                        .map(|s| s.trim() == "Y")
                        .unwrap_or(false);
                result.error = self.last_error.clone();
                result.fan_curve = CurveSnapshot {
                    supported: curve_supported(),
                    active,
                    fault,
                    applied: self.fan_config.clone(),
                };
                if fault {
                    result.error =
                        Some("风扇曲线恢复失败，驱动正在请求强冷并重试恢复；请检查风扇转速".into());
                }
                result
            }
            Err(error) => OemSnapshot {
                error: Some(error),
                fan_curve: CurveSnapshot {
                    supported: curve_supported(),
                    active,
                    fault,
                    applied: self.fan_config.clone(),
                },
                ..Default::default()
            },
        }
    }
    pub fn mode(&mut self, mode: u8, ac: bool) -> Result<(), String> {
        if tongfang::mode_clamp(mode).is_none() {
            return Err("无效的机身性能模式".into());
        }
        if mode != 1 && !ac {
            return Err("均衡/狂暴模式需要连接原装电源".into());
        }
        self.restore_curve()?;
        apply_mode(self.connect()?, mode, ac)
    }
    fn send_curve(&mut self, action: usize, config: Option<&FanCurveConfig>) -> Result<(), String> {
        let frame = curve_frame(action, config)?;
        self.connect()?
            .write_all_at(&frame, protocol::REQUEST_OFFSET as u64)
            .map_err(|e| format!("风扇曲线操作失败：{e}；请检查驱动版本、供电和当前强冷状态"))
    }
    pub fn apply_curve(&mut self, config: FanCurveConfig, ac: bool) -> Result<(), String> {
        config.validate()?;
        if !ac {
            return Err("应用风扇曲线需要连接原装电源；当前可编辑和保存草稿".into());
        }
        if !curve_supported() {
            return Err("请先更新 openrevo_ec 驱动以启用整表曲线控制".into());
        }
        self.send_curve(protocol::APPLY, Some(&config))?;
        self.fan_config = Some(config);
        self.heartbeat = Some(Instant::now());
        self.last_error = None;
        Ok(())
    }
    pub fn restore_curve(&mut self) -> Result<(), String> {
        if curve_flag("fan_curve_active") {
            self.send_curve(protocol::RESTORE, None)?;
        }
        self.fan_config = None;
        self.last_error = None;
        Ok(())
    }
    pub fn power(&mut self, requested: CpuLimits, ac: bool) -> Result<(), String> {
        if curve_flag("fan_curve_active") {
            return Err("请先恢复原风扇策略，再调整 CPU 功耗".into());
        }
        apply_limits(self.connect()?, requested, ac)
    }
    pub fn boost(&mut self, enabled: bool) -> Result<(), String> {
        let file = self.connect()?;
        let current = file.read(ADDR_FAN_BOOST_CLAMP)?;
        transaction(
            file,
            &[(
                ADDR_FAN_BOOST_CLAMP,
                tongfang::fan_boost_value(current, enabled),
            )],
        )?;
        if self.original_boost.is_none() {
            self.original_boost = Some(current & FAN_BOOST_CLAMP_BIT != 0);
        }
        self.heartbeat = Some(Instant::now());
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), String> {
        self.restore_curve()?;
        if let Some(original) = self.original_boost {
            self.boost(original)?;
            self.original_boost = None;
        }
        self.file = None; // Kernel release also restores boost after a crash.
        Ok(())
    }
    pub fn watchdog(&mut self) {
        if (self.original_boost.is_some() || self.fan_config.is_some())
            && self
                .heartbeat
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
        {
            if let Err(error) = self.release() {
                self.last_error = Some(error);
            }
        }
    }
}

impl Drop for LinuxEc {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
    };

    struct Fake {
        registers: RefCell<HashMap<u16, u8>>,
        writes: Cell<usize>,
        fail_write: Cell<Option<usize>>,
    }
    impl Fake {
        fn new() -> Self {
            let mut registers = HashMap::new();
            for (addresses, watts) in [
                (
                    [ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4],
                    [35, 35, 70],
                ),
                (
                    [ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4],
                    [65, 65, 100],
                ),
                (
                    [ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4],
                    [90, 90, 100],
                ),
            ] {
                for (address, value) in addresses.into_iter().zip(watts) {
                    registers.insert(address, value);
                }
            }
            registers.insert(ADDR_FAN_BOOST_CLAMP, values::OFFICE_CLAMP);
            registers.insert(ADDR_CUSTOM_MODE, values::CUSTOM_OFF);
            registers.insert(ADDR_POWER_STATUS, POWER_STATUS_DC_JACK);
            Self {
                registers: RefCell::new(registers),
                writes: Cell::new(0),
                fail_write: Cell::new(None),
            }
        }
    }
    impl EcIo for Fake {
        fn read(&self, address: u16) -> Result<u8, String> {
            Ok(*self.registers.borrow().get(&address).unwrap_or(&0))
        }
        fn write(&self, address: u16, value: u8) -> Result<(), String> {
            let count = self.writes.get() + 1;
            self.writes.set(count);
            self.registers.borrow_mut().insert(address, value);
            if self.fail_write.get() == Some(count) {
                Err("injected post-write failure".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn battery_and_usb_c_cannot_enable_high_power() {
        let io = Fake::new();
        assert!(apply_mode(&io, 2, false).is_err());
        assert!(apply_mode(&io, 3, false).is_err());
        assert!(apply_limits(
            &io,
            CpuLimits {
                pl1: 35,
                pl2: 35,
                pl4: 70
            },
            false
        )
        .is_err());
        io.registers.borrow_mut().insert(ADDR_POWER_STATUS, 0);
        assert!(apply_mode(&io, 3, true).is_err());
        assert_eq!(io.writes.get(), 0);
    }
    #[test]
    fn malformed_limits_do_not_write_anything() {
        let io = Fake::new();
        for limits in [
            CpuLimits {
                pl1: 5,
                pl2: 35,
                pl4: 70,
            },
            CpuLimits {
                pl1: 65,
                pl2: 35,
                pl4: 70,
            },
            CpuLimits {
                pl1: 90,
                pl2: 90,
                pl4: 100,
            },
            CpuLimits {
                pl1: 65,
                pl2: 65,
                pl4: 120,
            },
        ] {
            assert!(apply_limits(&io, limits, true).is_err());
        }
        assert_eq!(io.writes.get(), 0);
    }
    #[test]
    fn fault_restores_even_write_that_reported_failure() {
        let io = Fake::new();
        let addresses = [ADDR_CUSTOM_PL4, ADDR_CUSTOM_PL2, ADDR_CUSTOM_PL1];
        for fail in 1..=3 {
            io.writes.set(0);
            io.fail_write.set(Some(fail));
            let changes: Vec<_> = addresses.iter().map(|a| (*a, 70)).collect();
            assert!(transaction(&io, &changes).is_err());
            for address in addresses {
                assert_eq!(io.read(address).unwrap(), 0);
            }
        }
    }
    #[test]
    fn mode_preserves_lighting_usb_and_fan_boost() {
        let io = Fake::new();
        io.registers
            .borrow_mut()
            .insert(ADDR_MODE_INDEX, values::MODE_PRESERVE);
        io.registers
            .borrow_mut()
            .insert(ADDR_TRIGGER_BYTE, TRIGGER_USB_CHARGE);
        io.registers.borrow_mut().insert(
            ADDR_FAN_BOOST_CLAMP,
            values::OFFICE_CLAMP | FAN_BOOST_CLAMP_BIT,
        );
        apply_mode(&io, 3, true).unwrap();
        assert_eq!(
            io.read(ADDR_MODE_INDEX).unwrap() & values::MODE_PRESERVE,
            values::MODE_PRESERVE
        );
        assert_ne!(io.read(ADDR_TRIGGER_BYTE).unwrap() & TRIGGER_USB_CHARGE, 0);
        assert!(read_snapshot(&io).unwrap().fan_boost);
        assert_eq!(read_snapshot(&io).unwrap().mode, Some(3));
    }
    #[test]
    fn complete_tuple_activates_custom_and_mode_restores_factory() {
        let io = Fake::new();
        let requested = CpuLimits {
            pl1: 45,
            pl2: 55,
            pl4: 80,
        };
        apply_limits(&io, requested, true).unwrap();
        let custom = read_snapshot(&io).unwrap();
        assert!(custom.custom_power);
        assert_eq!(custom.limits, Some(requested));
        apply_mode(&io, 1, false).unwrap();
        let factory = read_snapshot(&io).unwrap();
        assert!(!factory.custom_power);
        assert_eq!(factory.limits, Some(factory.presets[0]));
    }
    #[test]
    fn invalid_factory_data_stops_mode_before_any_write() {
        let io = Fake::new();
        io.registers.borrow_mut().insert(ADDR_TURBO_PL1, 0);
        assert!(apply_mode(&io, 1, false).is_err());
        assert_eq!(io.writes.get(), 0);
    }
    #[test]
    fn trigger_failure_restores_previous_mode() {
        let reference = Fake::new();
        apply_mode(&reference, 3, true).unwrap();
        let io = Fake::new();
        io.fail_write.set(Some(reference.writes.get()));
        assert!(apply_mode(&io, 3, true).is_err());
        assert_eq!(read_snapshot(&io).unwrap().mode, Some(1));
        assert_eq!(io.read(ADDR_CUSTOM_MODE).unwrap(), values::CUSTOM_OFF);
        assert_eq!(io.read(ADDR_MODE_INDEX).unwrap(), 0);
    }
}
