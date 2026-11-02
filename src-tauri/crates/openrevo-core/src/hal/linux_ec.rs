//! Linux transport for the original modern Tongfang register contract.
use super::device_controls::{
    self, BatteryRequest, DeviceControls, DeviceSwitch, DeviceTransaction,
};
use super::{
    control_transaction::{self, ControlTransaction, Setting},
    fan_curve::FanCurveConfig,
    tongfang,
    vault::{
        ec::*, linux_access, linux_fan as protocol, linux_transport as transport, mask::*,
        tongfang as values,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub trait EcIo {
    fn read(&self, address: u16) -> Result<u8, String>;
    fn write(&self, _address: u16, _value: u8) -> Result<(), String> {
        Err("EC 只读通道不支持写入".into())
    }
}

impl EcIo for File {
    fn read(&self, address: u16) -> Result<u8, String> {
        if !linux_access::READABLE.contains(&address) {
            return Err("EC 地址不在读取白名单中".into());
        }
        let mut byte = [0];
        self.read_exact_at(&mut byte, u64::from(address))
            .map_err(|e| format!("EC 读取失败：{e}"))?;
        Ok(byte[0])
    }
    fn write(&self, address: u16, value: u8) -> Result<(), String> {
        if !linux_access::WRITABLE.contains(&address) {
            return Err("EC 地址不在写入白名单中".into());
        }
        self.write_all_at(&[value], u64::from(address))
            .map_err(|e| format!("EC 写入失败：{e}"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
    pub devices: DeviceControls,
    pub water_cooling_supported: Option<bool>,
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

fn curve_supported(parameters: &Path) -> bool {
    std::fs::read_to_string(parameters.join("fan_curve_abi"))
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        == Some(protocol::VERSION)
}

fn curve_flag(parameters: &Path, name: &str) -> bool {
    std::fs::read_to_string(parameters.join(name))
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
    if version != transport::VERSION {
        return Err(format!(
            "EC 控制协议版本 {version} 与当前应用不兼容，请配套更新应用和驱动"
        ));
    }
    Ok(Some(version))
}

fn control_abi(parameters: &Path) -> Result<Option<usize>, String> {
    parse_control_abi(std::fs::read_to_string(parameters.join("transport_abi")))
}

fn require_control_version(version: Option<usize>) -> Result<(), String> {
    if version == Some(transport::VERSION) {
        Ok(())
    } else {
        Err("当前 EC 驱动仅支持只读监测；请将 openrevo_ec 模块与应用配套升级后启用硬件控制".into())
    }
}

fn require_control_abi(parameters: &Path) -> Result<(), String> {
    require_control_version(control_abi(parameters)?)
}

fn limits(io: &impl EcIo, addresses: [u16; 3]) -> Result<CpuLimits, String> {
    Ok(CpuLimits {
        pl1: io.read(addresses[0])?,
        pl2: io.read(addresses[1])?,
        pl4: io.read(addresses[2])?,
    })
}

pub(crate) fn presets(io: &impl EcIo) -> Result<Vec<CpuLimits>, String> {
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
        devices: DeviceControls::default(),
        water_cooling_supported: water_capability(io),
    })
}

fn water_capability(io: &impl EcIo) -> Option<bool> {
    let bios = io
        .read(ADDR_AP_BIOS_BYTE)
        .ok()
        .map(|v| v & MASK_LIQUID_COOLING_SUPPORT != 0);
    let status = io
        .read(ADDR_WATER_COOLER_FLAG)
        .ok()
        .map(|v| v & MASK_LIQUID_COOLING_STATUS_SUPPORT != 0);
    if bios == Some(true) || status == Some(true) {
        Some(true)
    } else if bios == Some(false) && status == Some(false) {
        Some(false)
    } else {
        None
    }
}

pub(crate) fn validate_limits(
    before: &OemSnapshot,
    requested: CpuLimits,
    ac_online: bool,
) -> Result<(), String> {
    if !(ac_online && before.dc_jack) {
        return Err("CPU 功耗调节需要连接原装电源".into());
    }
    let ceiling = before.presets.get(2).ok_or("缺少有效的 EC 出厂功耗上限")?;
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

pub struct LinuxEc {
    device_path: PathBuf,
    parameters_path: PathBuf,
    file: Option<File>,
    heartbeat: Option<Instant>,
    boost_owned: bool,
    last_error: Option<String>,
    fan_config: Option<FanCurveConfig>,
    transaction: ControlTransaction,
    devices: DeviceTransaction,
}

impl Default for LinuxEc {
    fn default() -> Self {
        Self {
            device_path: PathBuf::from("/dev/openrevo_ec"),
            parameters_path: PathBuf::from("/sys/module/openrevo_ec/parameters"),
            file: None,
            heartbeat: None,
            boost_owned: false,
            last_error: None,
            fan_config: None,
            transaction: ControlTransaction::default(),
            devices: DeviceTransaction::default(),
        }
    }
}

impl LinuxEc {
    fn require_device_writes(&self) -> Result<(), String> {
        self.require_writes()?;
        let abi = std::fs::read_to_string(self.parameters_path.join("device_controls_abi"))
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok());
        if abi != Some(super::vault::linux_device::VERSION) {
            return Err("请配套更新驱动以启用设备控制".into());
        }
        if curve_flag(&self.parameters_path, "fan_curve_active") {
            return Err("请先恢复原风扇策略，再调节设备".into());
        }
        Ok(())
    }
    pub fn battery(&mut self, request: BatteryRequest) -> Result<(), String> {
        request.thresholds()?;
        self.require_device_writes()?;
        self.connect()?;
        let result = self.devices.battery(self.file.as_ref().unwrap(), request);
        self.last_error = result.as_ref().err().cloned();
        result
    }
    pub fn device_switch(&mut self, id: DeviceSwitch, enabled: bool) -> Result<(), String> {
        self.require_device_writes()?;
        self.connect()?;
        let result = self
            .devices
            .switch(self.file.as_ref().unwrap(), id, enabled);
        self.last_error = result.as_ref().err().cloned();
        result
    }
    pub fn owns_boost(&self) -> bool {
        self.boost_owned
    }

    fn connect(&mut self) -> Result<&File, String> {
        if self.file.is_none() {
            self.file = Some(OpenOptions::new().read(true).write(true).open(&self.device_path)
                .map_err(|e| format!("Uniwill EC 通道不可用：{e}；请安装 openrevo_ec 模块并授予当前会话访问权限"))?);
        }
        Ok(self.file.as_ref().unwrap())
    }
    pub fn snapshot(&mut self) -> OemSnapshot {
        let abi = control_abi(&self.parameters_path);
        let control_backend = match &abi {
            Ok(Some(_)) => "whitelist",
            Ok(None) => "legacy",
            Err(_) => "incompatible",
        };
        let control_fault = self.transaction.faulted() || self.devices.faulted();
        let active = curve_flag(&self.parameters_path, "fan_curve_active");
        let fault = curve_flag(&self.parameters_path, "fan_curve_fault");
        if !active {
            self.fan_config = None;
        }
        match self.connect().and_then(read_snapshot) {
            Ok(mut result) => {
                // Permission on the device and the module's root-owned gate
                // are separate. Reading this parameter never enables writes.
                result.writable =
                    std::fs::read_to_string(self.parameters_path.join("allow_writes"))
                        .map(|s| s.trim() == "Y")
                        .unwrap_or(false)
                        && matches!(abi, Ok(Some(_)))
                        && !control_fault;
                let device_abi =
                    std::fs::read_to_string(self.parameters_path.join("device_controls_abi"))
                        .ok()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        == Some(super::vault::linux_device::VERSION);
                result.devices = device_controls::snapshot(
                    self.file.as_ref().unwrap(),
                    result.writable && device_abi && !active && !fault,
                );
                result.devices.fault = self.devices.faulted();
                if !device_abi {
                    result.devices.error = Some("更新配套驱动以启用设备控制".into());
                } else if active {
                    result.devices.error = Some("应用曲线时请先恢复原风扇策略再调节设备".into());
                }
                if self.devices.faulted() {
                    result.devices.error = self.last_error.clone();
                }
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
                    supported: matches!(abi, Ok(Some(_))) && curve_supported(&self.parameters_path),
                    active,
                    fault,
                    applied: self.fan_config.clone(),
                };
                if fault {
                    result.error =
                        Some("风扇曲线恢复失败，驱动正在请求强冷并重试恢复；请检查风扇转速".into());
                }
                if self.devices.faulted() {
                    result.error = Some("设备设置恢复失败，后台正在重试；恢复前暂停新调节".into());
                } else if control_fault {
                    result.error = Some("机身控制事务恢复失败，后台正在请求强冷并重试恢复；恢复前暂停功耗与模式调节".into());
                }
                result
            }
            Err(error) => OemSnapshot {
                error: Some(error),
                control_abi: abi.as_ref().ok().copied().flatten(),
                control_backend,
                control_fault,
                fan_curve: CurveSnapshot {
                    supported: matches!(abi, Ok(Some(_))) && curve_supported(&self.parameters_path),
                    active,
                    fault,
                    applied: self.fan_config.clone(),
                },
                ..Default::default()
            },
        }
    }
    pub fn mode(&mut self, mode: u8, mut ac: impl FnMut() -> bool) -> Result<(), String> {
        if tongfang::mode_clamp(mode).is_none() {
            return Err("无效的机身性能模式".into());
        }
        self.require_writes()?;
        // Check supply and mode encoding before releasing an active curve.
        let before = read_snapshot(self.connect()?)?;
        if before.mode.is_none() {
            return Err("未知的 EC 模式编码".into());
        }
        if mode != 1 && !(ac() && before.dc_jack) {
            return Err("均衡/狂暴模式需要连接原装电源".into());
        }
        self.restore_curve()?;
        self.connect()?;
        let result = self
            .transaction
            .apply(self.file.as_ref().unwrap(), Setting::Mode(mode), ac);
        self.last_error = result.as_ref().err().cloned();
        result
    }

    fn require_writes(&self) -> Result<(), String> {
        require_control_abi(&self.parameters_path)?;
        if !curve_flag(&self.parameters_path, "allow_writes") {
            return Err("EC 通道处于只读模式".into());
        }
        if self.transaction.faulted() || self.devices.faulted() {
            return Err("机身控制事务正在故障恢复".into());
        }
        Ok(())
    }
    fn send_curve(&mut self, action: usize, config: Option<&FanCurveConfig>) -> Result<(), String> {
        let frame = curve_frame(action, config)?;
        require_control_abi(&self.parameters_path)?;
        self.connect()?
            .write_all_at(&frame, protocol::REQUEST_OFFSET as u64)
            .map_err(|e| format!("风扇曲线操作失败：{e}；请检查驱动版本、供电和当前强冷状态"))
    }
    pub fn apply_curve(&mut self, config: FanCurveConfig, ac: bool) -> Result<(), String> {
        config.validate()?;
        self.require_writes()?;
        if !ac {
            return Err("应用风扇曲线需要连接原装电源；当前可编辑和保存草稿".into());
        }
        if !curve_supported(&self.parameters_path) {
            return Err("请先更新 openrevo_ec 驱动以启用整表曲线控制".into());
        }
        self.send_curve(protocol::APPLY, Some(&config))?;
        self.fan_config = Some(config);
        self.heartbeat = Some(Instant::now());
        self.last_error = None;
        Ok(())
    }
    pub fn restore_curve(&mut self) -> Result<(), String> {
        if curve_flag(&self.parameters_path, "fan_curve_active") {
            self.send_curve(protocol::RESTORE, None)?;
        }
        self.fan_config = None;
        self.last_error = None;
        Ok(())
    }
    pub fn power(
        &mut self,
        requested: CpuLimits,
        mut ac: impl FnMut() -> bool,
    ) -> Result<(), String> {
        if curve_flag(&self.parameters_path, "fan_curve_active") {
            return Err("请先恢复原风扇策略，再调整 CPU 功耗".into());
        }
        self.require_writes()?;
        let before = read_snapshot(self.connect()?)?;
        validate_limits(&before, requested, ac())?;
        let result =
            self.transaction
                .apply(self.file.as_ref().unwrap(), Setting::Power(requested), ac);
        self.last_error = result.as_ref().err().cloned();
        result
    }

    pub fn boost(&mut self, enabled: bool) -> Result<(), String> {
        self.require_writes()?;
        if !enabled && curve_flag(&self.parameters_path, "fan_curve_fault") {
            return Err("风扇恢复中，不能关闭保护强冷".into());
        }
        self.connect()?;
        // Kernel owns the original boost bit and compares its last write on
        // release. User space owns only the lease, never a stale restore value.
        self.boost_owned = true;
        self.heartbeat.get_or_insert_with(Instant::now);
        let result = control_transaction::boost(self.file.as_ref().unwrap(), enabled);
        if result.is_ok() {
            self.heartbeat = Some(Instant::now());
        }
        self.last_error = result.as_ref().err().cloned();
        result
    }
    pub fn release(&mut self) -> Result<(), String> {
        if self.file.is_none() {
            return Ok(());
        }
        self.transaction.recover(self.file.as_ref().unwrap())?;
        self.devices.recover(self.file.as_ref().unwrap())?;
        let result = self.restore_curve();
        // Always close after fan cleanup: release in the kernel is independent
        // of allow_writes and retains failed cooling restoration for its worker.
        self.file = None;
        self.boost_owned = false;
        self.fan_config = None;
        self.heartbeat = None;
        result
    }
    fn expire_control_lease(&mut self) -> Result<(), String> {
        if (self.boost_owned || self.fan_config.is_some())
            && self
                .heartbeat
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
        {
            if let Err(error) = self.release() {
                self.last_error = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn renew_control_lease(&mut self) -> Result<(), String> {
        self.expire_control_lease()?;
        if self.transaction.faulted() || self.devices.faulted() {
            return Err("机身控制事务正在故障恢复，无法续期硬件控制".into());
        }
        if !self.boost_owned && self.fan_config.is_none() {
            return Ok(());
        }
        // A recovery fault is not a successful heartbeat. Keep the previous
        // lease deadline so user-space release and kernel recovery can proceed.
        let fault = if curve_flag(&self.parameters_path, "fan_curve_fault") {
            Some("风扇曲线正在故障恢复，无法续期硬件控制")
        } else {
            None
        };
        if let Some(message) = fault {
            let error = message.to_string();
            self.last_error = Some(error.clone());
            return Err(error);
        }
        let active = curve_flag(&self.parameters_path, "fan_curve_active");
        if !active {
            self.fan_config = None;
        }
        if (active && self.fan_config.is_some()) || self.boost_owned {
            if let Err(error) = self.send_curve(protocol::HEARTBEAT, None) {
                self.last_error = Some(error.clone());
                return Err(error);
            }
        }
        if self.boost_owned || self.fan_config.is_some() {
            self.heartbeat = Some(Instant::now());
        }
        Ok(())
    }

    pub fn watchdog(&mut self) {
        if let Some(file) = &self.file {
            if self.transaction.faulted() {
                self.last_error = self.transaction.recover(file).err();
            }
        }
        if let Some(file) = &self.file {
            if self.devices.faulted() {
                self.last_error = self.devices.recover(file).err();
            }
        }
        let _ = self.expire_control_lease();
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
    fn old_unknown_and_unreadable_control_abi_never_allow_writes() {
        use std::io::{Error, ErrorKind};
        let missing = parse_control_abi(Err(Error::from(ErrorKind::NotFound))).unwrap();
        assert_eq!(missing, None);
        assert!(require_control_version(missing).is_err());
        assert_eq!(
            parse_control_abi(Ok(format!("{}\n", transport::VERSION))).unwrap(),
            Some(transport::VERSION)
        );
        assert!(require_control_version(Some(transport::VERSION)).is_ok());
        assert!(require_control_version(Some(transport::VERSION + 1)).is_err());
        assert!(parse_control_abi(Err(Error::from(ErrorKind::PermissionDenied))).is_err());
        assert!(parse_control_abi(Err(Error::from(ErrorKind::Interrupted))).is_err());
        assert!(parse_control_abi(Ok("invalid".into())).is_err());
        assert!(parse_control_abi(Ok("0".into())).is_err());
        assert!(parse_control_abi(Ok((transport::VERSION + 1).to_string())).is_err());
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

    fn transport_fixture() -> (tempfile::TempDir, LinuxEc) {
        let root = tempfile::tempdir().unwrap();
        let device_path = root.path().join("ec-device");
        let parameters_path = root.path().join("parameters");
        std::fs::create_dir(&parameters_path).unwrap();
        for (name, value) in [
            ("transport_abi", transport::VERSION.to_string()),
            ("fan_curve_abi", protocol::VERSION.to_string()),
            ("allow_writes", "Y".to_string()),
            ("fan_curve_active", "Y".to_string()),
            ("fan_curve_fault", "N".to_string()),
        ] {
            std::fs::write(parameters_path.join(name), value).unwrap();
        }
        let device = File::create(&device_path).unwrap();
        for (address, value) in fixture().0 {
            device.write_all_at(&[value], u64::from(address)).unwrap();
        }
        for address in [ADDR_SECOND_FAN_RPM_B1, ADDR_SECOND_FAN_RPM_B2] {
            device.write_all_at(&[0], u64::from(address)).unwrap();
        }
        let ec = LinuxEc {
            device_path,
            parameters_path,
            file: None,
            heartbeat: None,
            boost_owned: false,
            last_error: None,
            fan_config: None,
            transaction: ControlTransaction::default(),
            devices: DeviceTransaction::default(),
        };
        (root, ec)
    }

    fn curve_config() -> FanCurveConfig {
        let points: Vec<_> = [35, 45, 55, 65, 75, 85, 90, 95]
            .into_iter()
            .map(|temp| super::super::fan_curve::FanCurvePoint { temp, duty: 100 })
            .collect();
        FanCurveConfig {
            cpu: points.clone(),
            gpu: points,
            isolated: true,
            ramp_enabled: true,
            ramp_ms: 300,
        }
    }

    fn curve_request(ec: &LinuxEc) -> [u8; protocol::FRAME_SIZE] {
        let mut frame = [0; protocol::FRAME_SIZE];
        File::open(&ec.device_path)
            .unwrap()
            .read_exact_at(&mut frame, protocol::REQUEST_OFFSET as u64)
            .unwrap();
        frame
    }

    #[test]
    fn runtime_renews_without_ui_requests_and_restores_on_shutdown() {
        use crate::{linux::LinuxBackend, runtime::AppRuntime};
        use std::sync::Arc;
        let (root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        let device = File::open(&ec.device_path).unwrap();
        let runtime = Arc::new(AppRuntime::with_test_backend(LinuxBackend::with_test_ec(
            ec,
            root.path(),
        )));
        runtime.start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut frame = [0; protocol::FRAME_SIZE];
        loop {
            device
                .read_exact_at(&mut frame, protocol::REQUEST_OFFSET as u64)
                .unwrap();
            if frame[protocol::ACTION_OFFSET] == protocol::HEARTBEAT as u8 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background owner did not renew the curve"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        runtime.shutdown().unwrap();
        device
            .read_exact_at(&mut frame, protocol::REQUEST_OFFSET as u64)
            .unwrap();
        assert_eq!(frame[protocol::ACTION_OFFSET], protocol::RESTORE as u8);
    }

    #[test]
    fn oem_snapshots_neither_renew_lease_nor_send_curve_heartbeat() {
        let (_root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        let expired = Instant::now() - Duration::from_secs(11);
        ec.heartbeat = Some(expired);
        let before = std::fs::read(&ec.device_path).unwrap();
        for _ in 0..3 {
            assert!(ec.snapshot().available);
            assert_eq!(ec.heartbeat, Some(expired));
            assert_eq!(std::fs::read(&ec.device_path).unwrap(), before);
        }
        ec.watchdog();
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::RESTORE, None).unwrap()
        );
        assert!(ec.file.is_none());
        assert!(ec.fan_config.is_none());
        assert_eq!(ec.heartbeat, None);
    }

    #[test]
    fn explicit_oem_renewal_sends_heartbeat_for_a_live_owned_curve() {
        let (_root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        let earlier = Instant::now() - Duration::from_secs(5);
        ec.heartbeat = Some(earlier);
        ec.renew_control_lease().unwrap();
        assert!(ec.heartbeat.unwrap() > earlier);
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::HEARTBEAT, None).unwrap()
        );
        assert!(ec.fan_config.is_some());
    }

    #[test]
    fn late_oem_heartbeat_restores_instead_of_reviving_the_curve() {
        let (_root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        ec.heartbeat = Some(Instant::now() - Duration::from_secs(11));
        ec.renew_control_lease().unwrap();
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::RESTORE, None).unwrap()
        );
        assert!(ec.file.is_none());
        assert!(ec.fan_config.is_none());
        assert_eq!(ec.heartbeat, None);
        // Even a stale active flag cannot reapply a curve without ownership.
        ec.renew_control_lease().unwrap();
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::RESTORE, None).unwrap()
        );
        assert_eq!(ec.heartbeat, None);
    }

    #[test]
    fn failed_oem_heartbeat_does_not_extend_the_user_space_lease() {
        let (_root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        let earlier = Instant::now() - Duration::from_secs(5);
        ec.heartbeat = Some(earlier);
        std::fs::write(ec.parameters_path.join("transport_abi"), "invalid").unwrap();
        assert!(ec.renew_control_lease().is_err());
        assert_eq!(ec.heartbeat, Some(earlier));
        assert!(ec.last_error.is_some());
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::APPLY, Some(&curve_config())).unwrap()
        );
    }

    #[test]
    fn faulted_curve_cannot_renew_or_delay_expiry_restoration() {
        let (_root, mut ec) = transport_fixture();
        ec.apply_curve(curve_config(), true).unwrap();
        let earlier = Instant::now() - Duration::from_secs(5);
        ec.heartbeat = Some(earlier);
        std::fs::write(ec.parameters_path.join("fan_curve_fault"), "Y").unwrap();
        let before = std::fs::read(&ec.device_path).unwrap();
        for _ in 0..3 {
            let error = ec.renew_control_lease().unwrap_err();
            assert!(error.contains("风扇曲线"));
            assert_eq!(ec.last_error.as_ref(), Some(&error));
            assert_eq!(ec.heartbeat, Some(earlier));
            assert_eq!(std::fs::read(&ec.device_path).unwrap(), before);
        }
        ec.heartbeat = Some(Instant::now() - Duration::from_secs(11));
        ec.watchdog();
        assert_eq!(
            curve_request(&ec),
            curve_frame(protocol::RESTORE, None).unwrap()
        );
        assert!(ec.file.is_none());
        assert!(ec.fan_config.is_none());
        assert_eq!(ec.heartbeat, None);
    }

    #[test]
    fn late_boost_heartbeat_closes_the_kernel_owner_without_replaying_a_stale_value() {
        let (_root, mut ec) = transport_fixture();
        std::fs::write(ec.parameters_path.join("fan_curve_active"), "N").unwrap();
        ec.boost(true).unwrap();
        assert!(ec.boost_owned);
        ec.heartbeat = Some(Instant::now() - Duration::from_secs(11));
        ec.renew_control_lease().unwrap();
        // A regular file has no release callback; the C harness verifies
        // kernel restore. Rust must not overwrite an external mode/boost change.
        assert_eq!(
            File::open(&ec.device_path)
                .unwrap()
                .read(ADDR_FAN_BOOST_CLAMP)
                .unwrap()
                & FAN_BOOST_CLAMP_BIT,
            FAN_BOOST_CLAMP_BIT
        );
        assert!(!ec.boost_owned);
        assert!(ec.file.is_none());
        assert_eq!(ec.heartbeat, None);
    }

    #[test]
    fn device_writes_require_new_feature_abi_and_respect_curve_ownership() {
        let (_root, mut ec) = transport_fixture();
        let before = std::fs::read(&ec.device_path).unwrap();
        assert!(ec.device_switch(DeviceSwitch::FnLock, true).is_err());
        assert_eq!(std::fs::read(&ec.device_path).unwrap(), before);
        std::fs::write(
            ec.parameters_path.join("device_controls_abi"),
            super::super::vault::linux_device::VERSION.to_string(),
        )
        .unwrap();
        assert!(ec.device_switch(DeviceSwitch::FnLock, true).is_err());
        assert_eq!(std::fs::read(&ec.device_path).unwrap(), before);
        std::fs::write(ec.parameters_path.join("fan_curve_active"), "N").unwrap();
        ec.device_switch(DeviceSwitch::FnLock, true).unwrap();
        assert_ne!(
            File::open(&ec.device_path)
                .unwrap()
                .read(ADDR_FN_LOCK)
                .unwrap()
                & MASK_FN_LOCK,
            0
        );
    }

    #[test]
    fn water_capability_distinguishes_absent_flags_from_failed_reads() {
        struct Probe(Option<u8>, Option<u8>);
        impl EcIo for Probe {
            fn read(&self, address: u16) -> Result<u8, String> {
                match address {
                    ADDR_AP_BIOS_BYTE => self.0,
                    ADDR_WATER_COOLER_FLAG => self.1,
                    _ => None,
                }
                .ok_or("unavailable".into())
            }
        }
        assert_eq!(water_capability(&Probe(None, None)), None);
        assert_eq!(water_capability(&Probe(Some(0), None)), None);
        assert_eq!(water_capability(&Probe(Some(0), Some(0))), Some(false));
        assert_eq!(
            water_capability(&Probe(Some(MASK_LIQUID_COOLING_SUPPORT), None)),
            Some(true)
        );
        assert_eq!(
            water_capability(&Probe(None, Some(MASK_LIQUID_COOLING_STATUS_SUPPORT))),
            Some(true)
        );
    }

    #[test]
    fn releasing_boost_closes_owner_even_when_admin_disables_writes() {
        let (_root, mut ec) = transport_fixture();
        std::fs::write(ec.parameters_path.join("fan_curve_active"), "N").unwrap();
        ec.boost(true).unwrap();
        std::fs::write(ec.parameters_path.join("allow_writes"), "N").unwrap();
        ec.release().unwrap();
        assert!(ec.file.is_none());
        assert!(!ec.boost_owned);
    }

    #[test]
    fn physical_events_cycle_four_modes_through_runtime_and_persist() {
        use crate::{
            actions::{Action, PowerMode},
            config::ConfigStore,
            linux::LinuxBackend,
            runtime::AppRuntime,
        };
        use std::{
            ffi::CString,
            io::Write,
            os::unix::ffi::OsStrExt,
            sync::{mpsc, Arc},
        };
        let (root, ec) = transport_fixture();
        std::fs::write(ec.parameters_path.join("fan_curve_active"), "N").unwrap();
        let supply = root.path().join("sys/class/power_supply/AC");
        std::fs::create_dir_all(&supply).unwrap();
        std::fs::write(supply.join("type"), "Mains").unwrap();
        std::fs::write(supply.join("online"), "1").unwrap();
        let dev = root.path().join("dev");
        std::fs::create_dir(&dev).unwrap();
        let fifo = dev.join("openrevo_events");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut input = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let runtime = Arc::new(AppRuntime::with_test_parts(
            LinuxBackend::with_test_ec(ec, root.path()),
            ConfigStore::from_config_home(root.path()).unwrap(),
        ));
        let (sender, receiver) = mpsc::sync_channel(16);
        runtime.set_notice_sender(sender);
        runtime.execute(Action::Hotkeys { enabled: true }).unwrap();
        runtime.start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !runtime.snapshot().unwrap().wmi_events.available {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let send = |input: &mut File, sequence: u64, code: u32| {
            use super::super::vault::linux_events as wire;
            let mut frame = [0; wire::FRAME_SIZE];
            frame[wire::SEQUENCE_OFFSET..wire::SEQUENCE_OFFSET + 8]
                .copy_from_slice(&sequence.to_le_bytes());
            frame[wire::CODE_OFFSET..wire::CODE_OFFSET + 4].copy_from_slice(&code.to_le_bytes());
            frame[wire::VERSION_OFFSET..wire::VERSION_OFFSET + 4]
                .copy_from_slice(&(wire::VERSION as u32).to_le_bytes());
            input.write_all(&frame).unwrap();
        };
        send(&mut input, 1, u32::MAX);
        std::thread::sleep(Duration::from_millis(150));
        assert!(receiver.try_recv().is_err());
        for (index, mode) in [
            PowerMode::Balanced,
            PowerMode::Turbo,
            PowerMode::Custom,
            PowerMode::Office,
        ]
        .into_iter()
        .enumerate()
        {
            send(
                &mut input,
                index as u64 * 2 + 2,
                super::super::vault::wmi_hotkeys::MODE_KEY,
            );
            let notice = receiver.recv_timeout(Duration::from_secs(4)).unwrap();
            assert!(!notice.error, "{}", notice.message);
            assert_eq!(runtime.load_config().unwrap().unwrap().mode, Some(mode));
            send(
                &mut input,
                index as u64 * 2 + 3,
                super::super::vault::wmi_hotkeys::MODE_SWITCH,
            );
            std::thread::sleep(Duration::from_millis(600));
            assert!(receiver.try_recv().is_err(), "mode echo was replayed");
        }
        send(&mut input, 100, super::super::vault::wmi_hotkeys::MODE_KEY);
        std::thread::sleep(Duration::from_millis(200));
        assert!(receiver.try_recv().is_err(), "gapped batch was replayed");
        runtime.execute(Action::Hotkeys { enabled: false }).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        send(&mut input, 10, super::super::vault::wmi_hotkeys::MODE_KEY);
        std::thread::sleep(Duration::from_millis(150));
        assert!(receiver.try_recv().is_err());
        runtime.shutdown().unwrap();
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
