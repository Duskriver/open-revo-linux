//! Linux transport for the original modern Tongfang register contract.
use super::{
    fan_curve::FanCurveConfig,
    tongfang,
    vault::{ec::*, linux_control as control, linux_fan as protocol, mask::*, tongfang as values},
};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::FileExt,
    time::{Duration, Instant},
};

pub trait EcIo {
    fn read(&self, address: u16) -> Result<u8, String>;
}

impl EcIo for File {
    fn read(&self, address: u16) -> Result<u8, String> {
        let mut byte = [0];
        self.read_exact_at(&mut byte, u64::from(address))
            .map_err(|e| format!("EC 读取失败：{e}"))?;
        Ok(byte[0])
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
    pub control_abi: Option<usize>,
    pub control_backend: &'static str,
    pub control_fault: bool,
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

fn parse_control_abi(result: std::io::Result<String>) -> Result<Option<usize>, String> {
    let text = match result {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 EC 控制协议版本，停止写入：{error}")),
    };
    let version = text
        .trim()
        .parse::<usize>()
        .map_err(|_| "EC 控制协议版本无效，停止写入".to_string())?;
    if version != control::VERSION {
        return Err(format!(
            "EC 控制协议版本 {version} 与当前应用不兼容，请配套更新应用和驱动"
        ));
    }
    Ok(Some(version))
}

fn control_abi() -> Result<Option<usize>, String> {
    parse_control_abi(std::fs::read_to_string(
        "/sys/module/openrevo_ec/parameters/control_abi",
    ))
}

fn require_control_version(version: Option<usize>) -> Result<(), String> {
    if version == Some(control::VERSION) {
        Ok(())
    } else {
        Err("当前 EC 驱动仅支持只读监测；请将 openrevo_ec 模块与应用配套升级后启用硬件控制".into())
    }
}

fn require_control_abi() -> Result<(), String> {
    require_control_version(control_abi()?)
}

fn control_frame(action: usize, arguments: [u8; 3]) -> Result<[u8; control::FRAME_SIZE], String> {
    let valid = match action {
        control::MODE => (1..=3).contains(&arguments[0]) && arguments[1..] == [0, 0],
        control::POWER => {
            usize::from(arguments[0]) >= control::MIN_POWER
                && arguments[0] <= arguments[1]
                && usize::from(arguments[2]) >= usize::from(arguments[1]) + control::PL4_MARGIN
        }
        control::BOOST => arguments[0] <= 1 && arguments[1..] == [0, 0],
        control::RECOVER => arguments == [0; 3],
        _ => false,
    };
    if !valid {
        return Err("无效的 EC 语义控制请求".into());
    }
    let mut frame = [0; control::FRAME_SIZE];
    frame[control::VERSION_OFFSET] = control::VERSION as u8;
    frame[control::ACTION_OFFSET] = action as u8;
    frame[control::ARGUMENT_OFFSET..control::RESERVED_OFFSET].copy_from_slice(&arguments);
    Ok(frame)
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
        control_abi: None,
        control_backend: "unknown",
        control_fault: false,
    })
}

fn validate_limits(
    before: &OemSnapshot,
    requested: CpuLimits,
    ac_online: bool,
) -> Result<(), String> {
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
        let abi = control_abi();
        let control_backend = match &abi {
            Ok(Some(_)) => "semantic",
            Ok(None) => "legacy",
            Err(_) => "incompatible",
        };
        let control_fault = curve_flag("control_fault");
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
                        .unwrap_or(false)
                        && matches!(abi, Ok(Some(_)))
                        && !control_fault;
                result.control_abi = abi.as_ref().ok().copied().flatten();
                result.control_backend = control_backend;
                result.control_fault = control_fault;
                result.error = self
                    .last_error
                    .clone()
                    .or_else(|| abi.as_ref().err().cloned());
                if control_backend == "legacy" && result.error.is_none() {
                    result.error = Some("当前 EC 驱动仅支持只读监测；请将 openrevo_ec 模块与应用配套升级后启用硬件控制".into());
                }
                result.fan_curve = CurveSnapshot {
                    supported: matches!(abi, Ok(Some(_))) && curve_supported(),
                    active,
                    fault,
                    applied: self.fan_config.clone(),
                };
                if fault {
                    result.error =
                        Some("风扇曲线恢复失败，驱动正在请求强冷并重试恢复；请检查风扇转速".into());
                }
                if control_fault {
                    result.error = Some("机身控制事务恢复失败，驱动正在请求强冷并重试恢复；恢复前暂停功耗与模式调节".into());
                }
                result
            }
            Err(error) => OemSnapshot {
                error: Some(error),
                control_abi: abi.as_ref().ok().copied().flatten(),
                control_backend,
                control_fault,
                fan_curve: CurveSnapshot {
                    supported: matches!(abi, Ok(Some(_))) && curve_supported(),
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
        require_control_abi()?;
        self.restore_curve()?;
        self.send_control(control::MODE, [mode, 0, 0])
    }
    fn send_control(&mut self, action: usize, arguments: [u8; 3]) -> Result<(), String> {
        let frame = control_frame(action, arguments)?;
        require_control_abi()?;
        self.connect()?
            .write_all_at(&frame, control::REQUEST_OFFSET as u64)
            .map_err(|e| format!("EC 语义控制失败：{e}；请检查供电、驱动状态和当前控制策略"))
    }
    fn send_curve(&mut self, action: usize, config: Option<&FanCurveConfig>) -> Result<(), String> {
        let frame = curve_frame(action, config)?;
        require_control_abi()?;
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
        require_control_abi()?;
        let before = read_snapshot(self.connect()?)?;
        validate_limits(&before, requested, ac)?;
        self.send_control(
            control::POWER,
            [requested.pl1, requested.pl2, requested.pl4],
        )
    }
    pub fn boost(&mut self, enabled: bool) -> Result<(), String> {
        require_control_abi()?;
        let file = self.connect()?;
        let current = file.read(ADDR_FAN_BOOST_CLAMP)?;
        self.send_control(control::BOOST, [u8::from(enabled), 0, 0])?;
        if self.original_boost.is_none() {
            self.original_boost = Some(current & FAN_BOOST_CLAMP_BIT != 0);
        }
        self.heartbeat = Some(Instant::now());
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), String> {
        if self.file.is_none() {
            return Ok(());
        }
        let result = (|| {
            if curve_flag("control_fault") {
                self.send_control(control::RECOVER, [0; 3])?;
            }
            self.restore_curve()?;
            if let Some(original) = self.original_boost {
                self.boost(original)?;
            }
            Ok(())
        })();
        // Always close, even if a status read or restoration failed. Kernel
        // release retains fault backups and provides the final restoration.
        self.file = None;
        self.original_boost = None;
        self.fan_config = None;
        self.heartbeat = None;
        result
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
    use std::collections::HashMap;

    #[test]
    fn semantic_frames_have_fixed_layout_and_reject_invalid_arguments() {
        let frame = control_frame(control::POWER, [45, 55, 80]).unwrap();
        assert_eq!(frame[control::VERSION_OFFSET], control::VERSION as u8);
        assert_eq!(frame[control::ACTION_OFFSET], control::POWER as u8);
        assert_eq!(
            &frame[control::ARGUMENT_OFFSET..control::RESERVED_OFFSET],
            &[45, 55, 80]
        );
        assert!(frame[control::RESERVED_OFFSET..]
            .iter()
            .all(|byte| *byte == 0));
        assert!(control_frame(control::POWER, [65, 35, 80]).is_err());
        assert!(control_frame(control::POWER, [45, 55, 60]).is_err());
        assert!(control_frame(control::POWER, [240, 250, 255]).is_err());
        assert!(control_frame(control::MODE, [0, 0, 0]).is_err());
        assert!(control_frame(control::MODE, [3, 1, 0]).is_err());
        assert!(control_frame(control::BOOST, [2, 0, 0]).is_err());
        assert!(control_frame(control::RECOVER, [1, 0, 0]).is_err());
        assert!(control_frame(control::BOOST, [1, 0, 0]).is_ok());
        assert!(control_frame(control::RECOVER, [0; 3]).is_ok());
    }

    #[test]
    fn old_unknown_and_unreadable_control_abi_never_allow_writes() {
        use std::io::{Error, ErrorKind};
        let missing = parse_control_abi(Err(Error::from(ErrorKind::NotFound))).unwrap();
        assert_eq!(missing, None);
        assert!(require_control_version(missing).is_err());
        assert_eq!(
            parse_control_abi(Ok(format!("{}\n", control::VERSION))).unwrap(),
            Some(control::VERSION)
        );
        assert!(require_control_version(Some(control::VERSION)).is_ok());
        assert!(require_control_version(Some(control::VERSION + 1)).is_err());
        assert!(parse_control_abi(Err(Error::from(ErrorKind::PermissionDenied))).is_err());
        assert!(parse_control_abi(Err(Error::from(ErrorKind::Interrupted))).is_err());
        assert!(parse_control_abi(Ok("invalid".into())).is_err());
        assert!(parse_control_abi(Ok("0".into())).is_err());
        assert!(parse_control_abi(Ok((control::VERSION + 1).to_string())).is_err());
    }

    struct ReadOnlyEc(HashMap<u16, u8>);
    impl EcIo for ReadOnlyEc {
        fn read(&self, address: u16) -> Result<u8, String> {
            Ok(*self.0.get(&address).unwrap_or(&0))
        }
    }
    fn fixture() -> ReadOnlyEc {
        let mut registers = HashMap::new();
        for (addresses, values) in [
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
            registers.extend(addresses.into_iter().zip(values));
        }
        registers.insert(ADDR_FAN_BOOST_CLAMP, values::OFFICE_CLAMP);
        registers.insert(ADDR_CUSTOM_MODE, values::CUSTOM_OFF);
        registers.insert(ADDR_POWER_STATUS, POWER_STATUS_DC_JACK);
        registers.insert(ADDR_MAIN_FAN_RPM_B1, 10);
        registers.insert(ADDR_MAIN_FAN_RPM_B2, 20);
        ReadOnlyEc(registers)
    }

    #[test]
    fn telemetry_and_factory_limits_are_available_without_any_write_primitive() {
        let mut io = fixture();
        let status = read_snapshot(&io).unwrap();
        assert_eq!(status.mode, Some(1));
        assert_eq!(status.fan_rpm[0], u16::from_be_bytes([10, 20]));
        assert_eq!(
            status.limits,
            Some(CpuLimits {
                pl1: 35,
                pl2: 35,
                pl4: 70
            })
        );
        assert!(!status.writable);
        io.0.insert(ADDR_CUSTOM_MODE, values::CUSTOM_ON);
        io.0.insert(ADDR_CUSTOM_PL1, 45);
        io.0.insert(ADDR_CUSTOM_PL2, 55);
        io.0.insert(ADDR_CUSTOM_PL4, 80);
        let custom = read_snapshot(&io).unwrap();
        assert!(custom.custom_power);
        assert_eq!(
            custom.limits,
            Some(CpuLimits {
                pl1: 45,
                pl2: 55,
                pl4: 80
            })
        );
        io.0.insert(ADDR_TURBO_PL1, 0);
        assert!(read_snapshot(&io).is_err());
    }

    #[test]
    fn frontend_validation_keeps_factory_ceiling_and_supply_rules() {
        let mut status = read_snapshot(&fixture()).unwrap();
        let requested = CpuLimits {
            pl1: 45,
            pl2: 55,
            pl4: 80,
        };
        assert!(validate_limits(&status, requested, true).is_ok());
        assert!(validate_limits(&status, requested, false).is_err());
        status.dc_jack = false;
        assert!(validate_limits(&status, requested, true).is_err());
        status.dc_jack = true;
        assert!(validate_limits(
            &status,
            CpuLimits {
                pl1: 90,
                pl2: 90,
                pl4: 100
            },
            true
        )
        .is_err());
        assert!(validate_limits(
            &status,
            CpuLimits {
                pl1: 45,
                pl2: 55,
                pl4: 120
            },
            true
        )
        .is_err());
    }
}
