// OpenRevo Baolongda (BLD) Hardware Driver Manager
// Manages JiaoLongDriver64 kernel service lifecycle and provides obfuscated EC RAM operations.
// Strictly adheres to Rule 4 (Asset Vault Obfuscation) and Rule 5 (Safe Native Invocations).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use libloading::{Library, Symbol};
use crate::hal::vault::{ec, driver, mask};

static BLD_FAN_CLAMP_RPM: AtomicU16 = AtomicU16::new(0);

#[derive(Clone, Debug, PartialEq)]
pub enum BldFanControlMode {
    Auto,
    ManualClamp {
        cpu_rpm: u16,
        gpu_rpm: u16,
    },
    Curve {
        cpu_curve: Vec<crate::system::config::FanCurvePoint>,
        gpu_curve: Vec<crate::system::config::FanCurvePoint>,
        ramp_enabled: bool,
        ramp_speed_ms: u16,
    },
}

static BLD_FAN_MODE: parking_lot::RwLock<BldFanControlMode> = parking_lot::RwLock::new(BldFanControlMode::Auto);
static HEARTBEAT_SPAWNED: AtomicBool = AtomicBool::new(false);

fn query_cpu_temp() -> u8 {
    if let Ok(t) = crate::hal::get_hardware_driver().read_cpu_temp() {
        if t > 0 && t < 120 {
            return t;
        }
    }
    let monitor_temp = crate::commands::system_cmd::get_cpu_monitor().get_temp();
    if monitor_temp > 0 && monitor_temp < 120 {
        return monitor_temp as u8;
    }
    45
}

fn query_gpu_temp(fallback_cpu_temp: u8) -> u8 {
    let gpu = crate::driver::gpu_bridge::get_global_gpu();
    let t = gpu.lock().get_temp(0);
    if t > 0 && t < 120 {
        t as u8
    } else {
        fallback_cpu_temp
    }
}

fn interpolate_fan_curve(curve: &[crate::system::config::FanCurvePoint], temp: f32) -> f32 {
    if curve.is_empty() {
        return 22.0;
    }
    if temp <= curve[0].temp as f32 {
        return curve[0].duty as f32;
    }
    let last = curve.len() - 1;
    if temp >= curve[last].temp as f32 {
        return curve[last].duty as f32;
    }
    for i in 0..last {
        let t1 = curve[i].temp as f32;
        let t2 = curve[i + 1].temp as f32;
        if temp >= t1 && temp <= t2 {
            let d1 = curve[i].duty as f32;
            let d2 = curve[i + 1].duty as f32;
            if (t2 - t1).abs() < 0.001 {
                return d1;
            }
            let ratio = (temp - t1) / (t2 - t1);
            return d1 + ratio * (d2 - d1);
        }
    }
    curve[last].duty as f32
}

fn duty_to_rpm(duty: f32) -> f32 {
    let d = duty.clamp(22.0, 100.0);
    2200.0 + ((d - 22.0) / 78.0) * (5800.0 - 2200.0)
}

fn clamp_slew_rate(target: f32, current: f32, max_up: f32, max_down: f32) -> f32 {
    if target > current {
        let delta = target - current;
        if delta > max_up {
            current + max_up
        } else {
            target
        }
    } else {
        let delta = current - target;
        if delta > max_down {
            current - max_down
        } else {
            target
        }
    }
}

pub fn start_bld_fan_heartbeat() {
    if HEARTBEAT_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::Builder::new()
        .name("openrevo-bld-fan-heartbeat".to_string())
        .spawn(move || {
            crate::log_info!("BLD_FAN", "Baolongda 1Hz fan heartbeat daemon started.");

            let mut last_was_auto = true;
            let mut smoothed_cpu_temp = 45.0f32;
            let mut smoothed_gpu_temp = 45.0f32;
            let mut last_cpu_rpm = 2200.0f32;
            let mut last_gpu_rpm = 2200.0f32;
            let mut tick_counter = 0u64;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                tick_counter = tick_counter.wrapping_add(1);

                let mode = BLD_FAN_MODE.read().clone();
                match mode {
                    BldFanControlMode::Auto => {
                        if !last_was_auto {
                            match BldDriverManager::get_or_init() {
                                Ok(mgr) => {
                                    mgr.release_fan_clamp();
                                }
                                Err(e) => {
                                    crate::log_error!("BLD_FAN", "Heartbeat Auto: failed to get driver manager: {}", e);
                                }
                            }
                            BLD_FAN_CLAMP_RPM.store(0, Ordering::SeqCst);
                            last_was_auto = true;
                            crate::log_info!("BLD_FAN", "Heartbeat transitioned to Auto mode (clamp released)");
                        }
                    }
                    BldFanControlMode::ManualClamp { cpu_rpm, gpu_rpm } => {
                        last_was_auto = false;
                        match BldDriverManager::get_or_init() {
                            Ok(mgr) => {
                                let c_val = ((cpu_rpm / 100) as u8).clamp(22, 58);
                                let g_val = ((gpu_rpm / 100) as u8).clamp(22, 58);
                                mgr.set_fan_clamp(c_val, g_val);
                                BLD_FAN_CLAMP_RPM.store((c_val as u16) * 100, Ordering::SeqCst);
                                if tick_counter % 5 == 1 {
                                    let (live_c, live_g) = mgr.read_dual_fan_rpm();
                                    crate::log_info!(
                                        "BLD_FAN",
                                        "Heartbeat tick #{} [ManualClamp]: Target CPU={} RPM, GPU={} RPM | Live tachometer: CPU={} RPM, GPU={} RPM",
                                        tick_counter, cpu_rpm, gpu_rpm, live_c, live_g
                                    );
                                }
                            }
                            Err(e) => {
                                if tick_counter % 5 == 1 {
                                    crate::log_error!("BLD_FAN", "Heartbeat tick #{} [ManualClamp] failed: driver manager error: {}", tick_counter, e);
                                }
                            }
                        }
                    }
                    BldFanControlMode::Curve {
                        ref cpu_curve,
                        ref gpu_curve,
                        ramp_enabled,
                        ..
                    } => {
                        last_was_auto = false;
                        match BldDriverManager::get_or_init() {
                            Ok(mgr) => {
                                let raw_cpu = query_cpu_temp();
                                let raw_gpu = query_gpu_temp(raw_cpu);

                                let a_cpu = if (raw_cpu as f32) > smoothed_cpu_temp { 0.35 } else { 0.05 };
                                smoothed_cpu_temp = a_cpu * (raw_cpu as f32) + (1.0 - a_cpu) * smoothed_cpu_temp;

                                let a_gpu = if (raw_gpu as f32) > smoothed_gpu_temp { 0.35 } else { 0.05 };
                                smoothed_gpu_temp = a_gpu * (raw_gpu as f32) + (1.0 - a_gpu) * smoothed_gpu_temp;

                                let cpu_duty = interpolate_fan_curve(cpu_curve, smoothed_cpu_temp);
                                let gpu_duty = interpolate_fan_curve(gpu_curve, smoothed_gpu_temp);

                                let mut target_cpu = duty_to_rpm(cpu_duty);
                                let mut target_gpu = duty_to_rpm(gpu_duty);

                                // Independent 85% heat pipe cross-cooling sync
                                let co_cpu = target_gpu * 0.85;
                                let co_gpu = target_cpu * 0.85;
                                if co_cpu > target_cpu { target_cpu = co_cpu; }
                                if co_gpu > target_gpu { target_gpu = co_gpu; }

                                if ramp_enabled {
                                    target_cpu = clamp_slew_rate(target_cpu, last_cpu_rpm, 800.0, 150.0);
                                    target_gpu = clamp_slew_rate(target_gpu, last_gpu_rpm, 800.0, 150.0);
                                }
                                last_cpu_rpm = target_cpu;
                                last_gpu_rpm = target_gpu;

                                let c_val = ((target_cpu / 100.0).round() as u8).clamp(22, 58);
                                let g_val = ((target_gpu / 100.0).round() as u8).clamp(22, 58);

                                mgr.set_fan_clamp(c_val, g_val);
                                BLD_FAN_CLAMP_RPM.store((c_val as u16) * 100, Ordering::SeqCst);

                                if tick_counter % 5 == 1 {
                                    let (live_c, live_g) = mgr.read_dual_fan_rpm();
                                    crate::log_info!(
                                        "BLD_FAN",
                                        "Heartbeat tick #{} [Curve]: Temp CPU={}C GPU={}C | Target CPU={} RPM, GPU={} RPM | Live tachometer: CPU={} RPM, GPU={} RPM",
                                        tick_counter, raw_cpu, raw_gpu, (c_val as u16) * 100, (g_val as u16) * 100, live_c, live_g
                                    );
                                }
                            }
                            Err(e) => {
                                if tick_counter % 5 == 1 {
                                    crate::log_error!("BLD_FAN", "Heartbeat tick #{} [Curve] failed: driver manager error: {}", tick_counter, e);
                                }
                            }
                        }
                    }
                }
            }
        })
        .expect("Failed to spawn bld fan heartbeat thread");
}

pub fn set_bld_fan_manual_clamp(cpu_rpm: u16, gpu_rpm: u16) {
    crate::log_info!("BLD_FAN", "set_bld_fan_manual_clamp requested: CPU={} RPM, GPU={} RPM", cpu_rpm, gpu_rpm);
    *BLD_FAN_MODE.write() = BldFanControlMode::ManualClamp { cpu_rpm, gpu_rpm };
    start_bld_fan_heartbeat();
    match BldDriverManager::get_or_init() {
        Ok(mgr) => {
            let c_val = ((cpu_rpm / 100) as u8).clamp(22, 58);
            let g_val = ((gpu_rpm / 100) as u8).clamp(22, 58);
            let ok = mgr.set_fan_clamp(c_val, g_val);
            BLD_FAN_CLAMP_RPM.store((c_val as u16) * 100, Ordering::SeqCst);
            crate::log_info!(
                "BLD_FAN",
                "Manual fan clamp applied immediately: CPU={} ({} RPM), GPU={} ({} RPM), success={}",
                c_val, (c_val as u16) * 100, g_val, (g_val as u16) * 100, ok
            );
        }
        Err(e) => {
            crate::log_error!("BLD_FAN", "set_bld_fan_manual_clamp failed: cannot initialize driver manager: {}", e);
        }
    }
}

pub fn set_bld_fan_curve(
    cpu_curve: Vec<crate::system::config::FanCurvePoint>,
    gpu_curve: Vec<crate::system::config::FanCurvePoint>,
    ramp_enabled: bool,
    ramp_speed_ms: u16,
) {
    crate::log_info!(
        "BLD_FAN",
        "set_bld_fan_curve requested: CPU points={}, GPU points={}, ramp_enabled={}, ramp_speed_ms={}",
        cpu_curve.len(), gpu_curve.len(), ramp_enabled, ramp_speed_ms
    );
    *BLD_FAN_MODE.write() = BldFanControlMode::Curve {
        cpu_curve,
        gpu_curve,
        ramp_enabled,
        ramp_speed_ms,
    };
    start_bld_fan_heartbeat();
}

pub fn restore_bld_fan_auto() {
    crate::log_info!("BLD_FAN", "restore_bld_fan_auto requested: releasing clamp and restoring BIOS auto policy");
    *BLD_FAN_MODE.write() = BldFanControlMode::Auto;
    start_bld_fan_heartbeat();
    match BldDriverManager::get_or_init() {
        Ok(mgr) => {
            let ok = mgr.release_fan_clamp();
            crate::log_info!("BLD_FAN", "Fan clamp released immediately: success={}", ok);
        }
        Err(e) => {
            crate::log_error!("BLD_FAN", "restore_bld_fan_auto failed: cannot initialize driver manager: {}", e);
        }
    }
    BLD_FAN_CLAMP_RPM.store(0, Ordering::SeqCst);
}

pub fn is_bld_fan_clamped() -> bool {
    *BLD_FAN_MODE.read() != BldFanControlMode::Auto
}

pub fn get_bld_fan_clamp_rpm() -> u16 {
    BLD_FAN_CLAMP_RPM.load(Ordering::SeqCst)
}

pub fn set_bld_fan_clamp_rpm(rpm: u16) {
    BLD_FAN_CLAMP_RPM.store(rpm, Ordering::SeqCst);
}

// Software Hue Cycle Rainbow Gradient Engine (10 FPS, 60 Steps per 360 deg cycle)
static BLD_RAINBOW_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn start_bld_rainbow(anchor_rgb: Option<(u8, u8, u8)>) {
    if BLD_RAINBOW_ACTIVE.swap(true, Ordering::SeqCst) {
        return; // Already running
    }

    // Ensure mode 2 (static fixed custom mode) is set so custom RGB frames render
    let _ = crate::hal::declarative::invoke_raw_mailbox(&[0, 251, 0, 16, 2]);

    let (r, g, b) = anchor_rgb.unwrap_or((138, 43, 226));
    let (base_hue, _sat, val) = rgb_to_hsv(r, g, b);
    let eff_val = val.max(0.35); // Ensure minimum visible luminance

    std::thread::Builder::new()
        .name("openrevo-bld-rainbow".to_string())
        .spawn(move || {
            crate::log_info!("BLD_RGB", "Baolongda 10 FPS Hue Cycle Rainbow started.");
            let steps_per_cycle = 60.0f32;
            let mut step = 0u32;
            let mut last_rgb = (255u8, 255u8, 255u8);

            while BLD_RAINBOW_ACTIVE.load(Ordering::SeqCst) {
                let hue = (base_hue + (360.0 * (step as f32) / steps_per_cycle)) % 360.0;
                let (cur_r, cur_g, cur_b) = hsv_to_rgb(hue, 1.0, eff_val);

                if (cur_r, cur_g, cur_b) != last_rgb {
                    let _ = crate::hal::declarative::invoke_raw_mailbox(&[0, 251, 0, 17, cur_r, cur_g, cur_b]);
                    last_rgb = (cur_r, cur_g, cur_b);
                }

                step = (step + 1) % (steps_per_cycle as u32);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            crate::log_info!("BLD_RGB", "Baolongda Hue Cycle Rainbow stopped.");
        })
        .expect("Failed to spawn bld rainbow thread");
}

pub fn stop_bld_rainbow() {
    if BLD_RAINBOW_ACTIVE.swap(false, Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub fn is_bld_rainbow_active() -> bool {
    BLD_RAINBOW_ACTIVE.load(Ordering::SeqCst)
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf.max(bf));
    let min = rf.min(gf.min(bf));
    let delta = max - min;
    let mut hue = 0.0;
    if delta > 0.0 {
        if (max - rf).abs() < f32::EPSILON {
            hue = 60.0 * (((gf - bf) / delta) % 6.0);
        } else if (max - gf).abs() < f32::EPSILON {
            hue = 60.0 * (((bf - rf) / delta) + 2.0);
        } else {
            hue = 60.0 * (((rf - gf) / delta) + 4.0);
        }
    }
    if hue < 0.0 {
        hue += 360.0;
    }
    let sat = if max == 0.0 { 0.0 } else { delta / max };
    (hue, sat, max)
}

fn hsv_to_rgb(hue: f32, sat: f32, val: f32) -> (u8, u8, u8) {
    let c = val * sat;
    let h_prime = (hue / 60.0) % 6.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let m = val - c;
    let (rf, gf, bf) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((rf + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((gf + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((bf + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

// Windows SCM Constants & Official Driver Service Identifiers
const SERVICE_NAME: &str = "BLDRING";
const DLL_NAME: &str = "Bldring64.dll";
const SYS_NAME: &str = "bldring64.sys";
const LEGACY_DLL_NAME: &str = "JiaoLongDriver64.dll";
const LEGACY_SYS_NAME: &str = "JiaoLongDriver64.sys";

// Embedded driver assets compiled directly into the binary for 100% standalone single-file distribution
const EMBEDDED_BLD_DLL: &[u8] = include_bytes!("../../resources/driver/bld/Bldring64.dll");
const EMBEDDED_BLD_SYS: &[u8] = include_bytes!("../../resources/driver/bld/bldring64.sys");

#[allow(dead_code)]
const SC_MANAGER_ALL_ACCESS: u32 = 0xF003F;
#[allow(dead_code)]
const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
#[allow(dead_code)]
const SERVICE_KERNEL_DRIVER: u32 = 0x00000001;
#[allow(dead_code)]
const SERVICE_DEMAND_START: u32 = 0x00000003;
#[allow(dead_code)]
const SERVICE_ERROR_NORMAL: u32 = 0x00000001;
#[allow(dead_code)]
const SERVICE_CONTROL_STOP: u32 = 0x00000001;

#[link(name = "advapi32")]
extern "system" {
    fn OpenSCManagerW(
        lpMachineName: *const u16,
        lpDatabaseName: *const u16,
        dwDesiredAccess: u32,
    ) -> isize;

    fn OpenServiceW(
        hSCManager: isize,
        lpServiceName: *const u16,
        dwDesiredAccess: u32,
    ) -> isize;

    fn CreateServiceW(
        hSCManager: isize,
        lpServiceName: *const u16,
        lpDisplayName: *const u16,
        dwDesiredAccess: u32,
        dwServiceType: u32,
        dwStartType: u32,
        dwErrorControl: u32,
        lpBinaryPathName: *const u16,
        lpLoadOrderGroup: *const u16,
        lpdwTagId: *mut u32,
        lpDependencies: *const u16,
        lpServiceStartName: *const u16,
        lpPassword: *const u16,
    ) -> isize;

    fn ChangeServiceConfigW(
        hService: isize,
        dwServiceType: u32,
        dwStartType: u32,
        dwErrorControl: u32,
        lpBinaryPathName: *const u16,
        lpLoadOrderGroup: *const u16,
        lpdwTagId: *mut u32,
        lpDependencies: *const u16,
        lpServiceStartName: *const u16,
        lpPassword: *const u16,
        lpDisplayName: *const u16,
    ) -> i32;

    fn StartServiceW(
        hService: isize,
        dwNumServiceArgs: u32,
        lpServiceArgVectors: *const *const u16,
    ) -> i32;

    #[allow(dead_code)]
    fn ControlService(
        hService: isize,
        dwControl: u32,
        lpServiceStatus: *mut u8,
    ) -> i32;

    fn DeleteService(hService: isize) -> i32;

    fn CloseServiceHandle(hSCObject: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

type FnInitializeBldring = unsafe extern "system" fn() -> bool;
type FnShutdownBldring = unsafe extern "system" fn();
type FnGetBLDPortVal = unsafe extern "system" fn(port: u16, val: *mut u32, size: u8) -> bool;
type FnSetBLDPortVal = unsafe extern "system" fn(port: u16, val: u32, size: u8) -> bool;

pub struct BldDriverManager {
    _lib: Library,
    #[allow(dead_code)]
    init_bldring: FnInitializeBldring,
    shutdown_bldring: FnShutdownBldring,
    get_port_val: FnGetBLDPortVal,
    set_port_val: FnSetBLDPortVal,
    io_lock: Mutex<()>,
    initialized: AtomicBool,
}

static INSTANCE: Mutex<Option<Arc<BldDriverManager>>> = Mutex::new(None);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl BldDriverManager {
    pub fn get_or_init() -> Result<Arc<Self>, String> {
        let mut guard = INSTANCE.lock();
        if let Some(ref inst) = *guard {
            return Ok(inst.clone());
        }

        match Self::init_internal() {
            Ok(mgr) => {
                let inst = Arc::new(mgr);
                *guard = Some(inst.clone());
                Ok(inst)
            }
            Err(e) => {
                crate::log_error!("BLD_DRV", "Failed to initialize Baolongda driver manager: {}", e);
                Err(e)
            }
        }
    }

    fn clean_win_path(p: PathBuf) -> PathBuf {
        let s = p.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            p
        }
    }

    fn locate_driver_files() -> Result<(PathBuf, PathBuf), String> {
        let exe_dir = std::env::current_exe()
            .map(|p| p.parent().unwrap_or(Path::new("")).to_path_buf())
            .unwrap_or_else(|_| PathBuf::from("."));

        let mut candidate_dirs = vec![
            exe_dir.clone(), // Same directory as open-revo.exe
            exe_dir.join("driver"),
            exe_dir.join("driver").join("bld"),
            exe_dir.join("resources").join("driver").join("bld"),
            exe_dir.join("resources").join("drivers").join("bld"),
            PathBuf::from("."),
            PathBuf::from("driver"),
            PathBuf::from("resources/driver/bld"),
            PathBuf::from("src-tauri/resources/driver/bld"),
            PathBuf::from("src-tauri/resources/drivers/bld"),
        ];

        if let Ok(progdata) = std::env::var("ProgramData") {
            let base = PathBuf::from(progdata);
            candidate_dirs.push(base.join("OpenRevo").join("driver").join("bld"));
            candidate_dirs.push(base.join("OpenRevo").join("driver"));
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            let base = PathBuf::from(appdata);
            candidate_dirs.push(base.join("OpenRevo").join("driver").join("bld"));
            candidate_dirs.push(base.join("OpenRevo").join("driver"));
        }

        // Also search up parent directory trees (e.g. running from target/release or target/debug)
        let mut cur = exe_dir.as_path();
        for _ in 0..4 {
            if let Some(parent) = cur.parent() {
                candidate_dirs.push(parent.join("resources").join("driver").join("bld"));
                candidate_dirs.push(parent.join("src-tauri").join("resources").join("driver").join("bld"));
                cur = parent;
            } else {
                break;
            }
        }

        // 1. Primary search: official Bldring64 files
        for dir in &candidate_dirs {
            let dll = dir.join(DLL_NAME);
            let sys = dir.join(SYS_NAME);
            if dll.exists() && sys.exists() {
                let abs_dll = Self::clean_win_path(std::fs::canonicalize(&dll).unwrap_or(dll));
                let abs_sys = Self::clean_win_path(std::fs::canonicalize(&sys).unwrap_or(sys));
                return Ok((abs_dll, abs_sys));
            }
        }

        // 2. Fallback search: legacy renamed files
        for dir in &candidate_dirs {
            let dll = dir.join(LEGACY_DLL_NAME);
            let sys = dir.join(LEGACY_SYS_NAME);
            if dll.exists() && sys.exists() {
                let abs_dll = Self::clean_win_path(std::fs::canonicalize(&dll).unwrap_or(dll));
                let abs_sys = Self::clean_win_path(std::fs::canonicalize(&sys).unwrap_or(sys));
                return Ok((abs_dll, abs_sys));
            }
        }

        // 3. Fallback extraction: Self-extract embedded driver assets into %ProgramData%\OpenRevo\driver\bld\ (Pure ASCII)
        let target_dir = if let Ok(progdata) = std::env::var("ProgramData") {
            PathBuf::from(progdata).join("OpenRevo").join("driver").join("bld")
        } else if let Ok(appdata) = std::env::var("APPDATA") {
            PathBuf::from(appdata).join("OpenRevo").join("driver").join("bld")
        } else {
            exe_dir.join("driver").join("bld")
        };

        let _ = std::fs::create_dir_all(&target_dir);
        let dll_path = target_dir.join(DLL_NAME);
        let sys_path = target_dir.join(SYS_NAME);

        let need_write_dll = match std::fs::metadata(&dll_path) {
            Ok(m) => m.len() != EMBEDDED_BLD_DLL.len() as u64,
            Err(_) => true,
        };
        if need_write_dll {
            if let Err(e) = std::fs::write(&dll_path, EMBEDDED_BLD_DLL) {
                crate::log_warn!("BLD_DRV", "Failed to write embedded {}: {:?}", DLL_NAME, e);
            } else {
                crate::log_info!("BLD_DRV", "Extracted embedded {} to {:?}", DLL_NAME, dll_path);
            }
        }

        let need_write_sys = match std::fs::metadata(&sys_path) {
            Ok(m) => m.len() != EMBEDDED_BLD_SYS.len() as u64,
            Err(_) => true,
        };
        if need_write_sys {
            if let Err(e) = std::fs::write(&sys_path, EMBEDDED_BLD_SYS) {
                crate::log_warn!("BLD_DRV", "Failed to write embedded {}: {:?}", SYS_NAME, e);
            } else {
                crate::log_info!("BLD_DRV", "Extracted embedded {} to {:?}", SYS_NAME, sys_path);
            }
        }

        if dll_path.exists() && sys_path.exists() {
            let abs_dll = Self::clean_win_path(std::fs::canonicalize(&dll_path).unwrap_or(dll_path));
            let abs_sys = Self::clean_win_path(std::fs::canonicalize(&sys_path).unwrap_or(sys_path));
            crate::log_info!("BLD_DRV", "Using self-extracted driver assets: DLL={:?}, SYS={:?}", abs_dll, abs_sys);
            return Ok((abs_dll, abs_sys));
        }

        Err(format!(
            "Baolongda vendor driver files not found ({} / {}) in search paths",
            DLL_NAME, SYS_NAME
        ))
    }

    fn ensure_service_started(sys_path: &Path) -> Result<(), String> {
        // If legacy JiaoLongDriver64 is running from a third-party tool, stop it first to prevent NT device collision
        Self::stop_and_delete_service("JiaoLongDriver64");

        let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm == 0 {
            let err = unsafe { GetLastError() };
            crate::log_error!("BLD_DRV", "OpenSCManager failed with Win32 {}. Administrator elevation required.", err);
            return Err(format!("OpenSCManager failed with error: {}. Administrator elevation required.", err));
        }

        let svc_name_w = to_wide(SERVICE_NAME);
        let mut svc = unsafe { OpenServiceW(scm, svc_name_w.as_ptr(), SERVICE_ALL_ACCESS) };

        // CRITICAL: For SERVICE_KERNEL_DRIVER (.sys), NtLoadDriver resolves the path directly.
        // It must NOT be wrapped in quotes, otherwise Windows kernel rejects it with STATUS_OBJECT_NAME_INVALID (Win32 123).
        let clean_sys = Self::clean_win_path(sys_path.to_path_buf());
        let sys_path_str = clean_sys.to_string_lossy();
        let sys_path_w = to_wide(&sys_path_str);

        if svc != 0 {
            crate::log_info!("BLD_DRV", "Kernel service '{}' found, ensuring demand start & updating binary path: {}", SERVICE_NAME, sys_path_str);
            unsafe {
                ChangeServiceConfigW(
                    svc,
                    0xFFFFFFFF,
                    SERVICE_DEMAND_START,
                    0xFFFFFFFF,
                    sys_path_w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                );
            }
        } else {
            crate::log_info!("BLD_DRV", "Kernel service '{}' not found, creating with binary path: {}", SERVICE_NAME, sys_path_str);
            svc = unsafe {
                CreateServiceW(
                    scm,
                    svc_name_w.as_ptr(),
                    svc_name_w.as_ptr(),
                    SERVICE_ALL_ACCESS,
                    SERVICE_KERNEL_DRIVER,
                    SERVICE_DEMAND_START,
                    SERVICE_ERROR_NORMAL,
                    sys_path_w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };

            if svc == 0 {
                let err = unsafe { GetLastError() };
                unsafe { CloseServiceHandle(scm) };
                crate::log_error!("BLD_DRV", "CreateService ({}) failed with Win32 {}. Path: {}", SERVICE_NAME, err, sys_path_str);
                return Err(format!("CreateService ({}) failed with error: {}", SERVICE_NAME, err));
            }
        }

        // Start service
        let start_res = unsafe { StartServiceW(svc, 0, std::ptr::null()) };
        if start_res == 0 {
            let err = unsafe { GetLastError() };
            if err == 1056 {
                crate::log_info!("BLD_DRV", "StartService: service '{}' is already running (Win32 1056).", SERVICE_NAME);
            } else if err == 1058 {
                crate::log_warn!("BLD_DRV", "StartService returned Win32 1058 (service disabled). Reconfiguring to DEMAND_START and retrying...");
                unsafe {
                    ChangeServiceConfigW(
                        svc,
                        0xFFFFFFFF,
                        SERVICE_DEMAND_START,
                        0xFFFFFFFF,
                        sys_path_w.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                    );
                    let retry_res = StartServiceW(svc, 0, std::ptr::null());
                    if retry_res == 0 {
                        let retry_err = GetLastError();
                        crate::log_error!("BLD_DRV", "StartService retry failed with Win32 {}.", retry_err);
                    } else {
                        crate::log_info!("BLD_DRV", "StartService retry succeeded after DEMAND_START reconfig.");
                    }
                }
            } else if err == 1275 {
                crate::log_warn!(
                    "BLD_DRV",
                    "StartService driver blocked (Win32 1275). Core Isolation / Memory Integrity may block kernel driver."
                );
            } else {
                crate::log_warn!("BLD_DRV", "StartService returned Win32 error {}. Path: {}", err, sys_path_str);
            }
        } else {
            crate::log_info!("BLD_DRV", "StartService successfully started kernel service '{}'.", SERVICE_NAME);
        }

        unsafe {
            CloseServiceHandle(svc);
            CloseServiceHandle(scm);
        }

        Ok(())
    }

    fn init_internal() -> Result<Self, String> {
        let (dll_path, sys_path) = Self::locate_driver_files()?;
        crate::log_info!("BLD_DRV", "Found driver assets: DLL={:?}, SYS={:?}", dll_path, sys_path);

        Self::ensure_service_started(&sys_path)?;

        let lib = unsafe { Library::new(&dll_path) }
            .map_err(|e| format!("Failed to load {}: {:?}", dll_path.display(), e))?;

        let init_bldring: FnInitializeBldring = unsafe {
            let sym: Symbol<FnInitializeBldring> = lib
                .get(b"InitializeBldring\0")
                .map_err(|e| format!("Symbol InitializeBldring not found: {:?}", e))?;
            *sym
        };

        let shutdown_bldring: FnShutdownBldring = unsafe {
            let sym: Symbol<FnShutdownBldring> = lib
                .get(b"ShutdownBldring\0")
                .map_err(|e| format!("Symbol ShutdownBldring not found: {:?}", e))?;
            *sym
        };

        let get_port_val: FnGetBLDPortVal = unsafe {
            let sym: Symbol<FnGetBLDPortVal> = lib
                .get(b"GetBLDPortVal\0")
                .map_err(|e| format!("Symbol GetBLDPortVal not found: {:?}", e))?;
            *sym
        };

        let set_port_val: FnSetBLDPortVal = unsafe {
            let sym: Symbol<FnSetBLDPortVal> = lib
                .get(b"SetBLDPortVal\0")
                .map_err(|e| format!("Symbol SetBLDPortVal not found: {:?}", e))?;
            *sym
        };

        let ok = unsafe { init_bldring() };
        if !ok {
            crate::log_error!("BLD_DRV", "InitializeBldring() returned false!");
            return Err("InitializeBldring returned false".into());
        }
        crate::log_info!("BLD_DRV", "InitializeBldring() succeeded (Ring0 port connection active).");

        let mgr = Self {
            _lib: lib,
            init_bldring,
            shutdown_bldring,
            get_port_val,
            set_port_val,
            io_lock: Mutex::new(()),
            initialized: AtomicBool::new(true),
        };

        // 1. EC hardware identification and unlock through registered vault aliases.
        mgr.hardware_ec_init();

        // 2. Disarm official Mechrevo Control Center MaxFanSpeed override lock if engaged (WMI MIFS Method 47)
        // Spawn asynchronously so any ACPI/WMI delay never blocks low-level driver initialization
        std::thread::spawn(|| {
            let _ = crate::hal::declarative::invoke_raw_mailbox(&[0, 251, 0, 47, 0]);
        });

        crate::log_info!("BLD_DRV", "Baolongda hardware driver initialized successfully and port active.");

        Ok(mgr)
    }

    fn write_port(&self, port: u16, val: u8) -> bool {
        unsafe { (self.set_port_val)(port, val as u32, 1) }
    }

    fn read_port(&self, port: u16) -> Option<u8> {
        let mut val = 0u32;
        if unsafe { (self.get_port_val)(port, &mut val, 1) } {
            Some((val & 0xFF) as u8)
        } else {
            None
        }
    }

    pub fn write_ec_ram(&self, address: u16, data: u8) -> bool {
        let _lock = self.io_lock.lock();
        let high = (address >> 8) as u8;
        let low = (address & 0xFF) as u8;

        let p_addr = driver::PORT_BLD_EC_ADDR;
        let p_data = driver::PORT_BLD_EC_DATA;

        if !self.write_port(p_addr, 0x2E) { return false; }
        if !self.write_port(p_data, 0x11) { return false; }
        if !self.write_port(p_addr, 0x2F) { return false; }
        if !self.write_port(p_data, high) { return false; }

        if !self.write_port(p_addr, 0x2E) { return false; }
        if !self.write_port(p_data, 0x10) { return false; }
        if !self.write_port(p_addr, 0x2F) { return false; }
        if !self.write_port(p_data, low) { return false; }

        if !self.write_port(p_addr, 0x2E) { return false; }
        if !self.write_port(p_data, 0x12) { return false; }
        if !self.write_port(p_addr, 0x2F) { return false; }

        self.write_port(p_data, data)
    }

    pub fn read_ec_ram(&self, address: u16) -> Option<u8> {
        let _lock = self.io_lock.lock();
        let high = (address >> 8) as u8;
        let low = (address & 0xFF) as u8;

        let p_addr = driver::PORT_BLD_EC_ADDR;
        let p_data = driver::PORT_BLD_EC_DATA;

        self.write_port(p_addr, 0x2E);
        self.write_port(p_data, 0x11);
        self.write_port(p_addr, 0x2F);
        self.write_port(p_data, high);

        self.write_port(p_addr, 0x2E);
        self.write_port(p_data, 0x10);
        self.write_port(p_addr, 0x2F);
        self.write_port(p_data, low);

        self.write_port(p_addr, 0x2E);
        self.write_port(p_data, 0x12);
        self.write_port(p_addr, 0x2F);

        self.read_port(p_data)
    }

    fn hardware_ec_init(&self) {
        if let Some(chip_id) = self.read_ec_ram(ec::ADDR_BLD_CHIP_ID) {
            crate::log_info!("BLD_DRV", "EC Hardware chip identification queried: 0x{:02X}", chip_id);
            if chip_id == 0x55 {
                if let Some(init_reg) = self.read_ec_ram(ec::ADDR_BLD_EC_INIT) {
                    let new_val = init_reg | 0x80;
                    let write_ok = self.write_ec_ram(ec::ADDR_BLD_EC_INIT, new_val);
                    let verify_val = self.read_ec_ram(ec::ADDR_BLD_EC_INIT).unwrap_or(0);
                    crate::log_info!(
                        "BLD_DRV",
                        "EC Hardware RAM unlocked: before=0x{:02X}, write=0x{:02X} (ok={}), after=0x{:02X}",
                        init_reg, new_val, write_ok, verify_val
                    );
                } else {
                    crate::log_warn!("BLD_DRV", "Failed to read EC RAM init register (ADDR_BLD_EC_INIT)");
                }
            } else {
                crate::log_warn!("BLD_DRV", "EC Hardware chip ID is not 0x55 (got 0x{:02X}), skipping RAM unlock", chip_id);
            }
        } else {
            crate::log_warn!("BLD_DRV", "Failed to read EC chip identification register!");
        }
    }

    /// Clamps CPU and GPU fans to specified target speeds (e.g. 58 = 5800 RPM)
    /// Overrides BIOS curve and locks hardware PWM output.
    pub fn set_fan_clamp(&self, cpu_speed: u8, gpu_speed: u8) -> bool {
        let ok1 = self.write_ec_ram(ec::ADDR_BLD_FAN1_RPM_SET, cpu_speed);
        let ok2 = self.write_ec_ram(ec::ADDR_BLD_FAN2_RPM_SET, gpu_speed);

        let cur_mode = self.read_ec_ram(ec::ADDR_BLD_FAN_CTRL_MODE).unwrap_or(0);
        let mut target_mode = cur_mode;
        if cpu_speed > 0 {
            target_mode |= mask::BLD_FAN_CPU_MANUAL_MASK; // Bit 1
        } else {
            target_mode &= !mask::BLD_FAN_CPU_MANUAL_MASK;
        }
        if gpu_speed > 0 {
            target_mode |= mask::BLD_FAN_GPU_MANUAL_MASK; // Bit 3
        } else {
            target_mode &= !mask::BLD_FAN_GPU_MANUAL_MASK;
        }

        let ok3 = self.write_ec_ram(ec::ADDR_BLD_FAN_CTRL_MODE, target_mode);
        let verify_mode = self.read_ec_ram(ec::ADDR_BLD_FAN_CTRL_MODE).unwrap_or(0);
        if ok1 && ok2 && ok3 {
            set_bld_fan_clamp_rpm((cpu_speed as u16) * 100);
        }
        crate::log_info!(
            "BLD_FAN",
            "Hardware fan clamp written: CPU target={} ({} RPM, success={}), GPU target={} ({} RPM, success={}) | Mode register: before=0x{:02X}, target=0x{:02X}, after=0x{:02X} (success={})",
            cpu_speed, (cpu_speed as u16) * 100, ok1, gpu_speed, (gpu_speed as u16) * 100, ok2, cur_mode, target_mode, verify_mode, ok3
        );
        ok1 && ok2 && ok3
    }

    /// Releases manual override and restores BIOS adaptive fan curve.
    pub fn release_fan_clamp(&self) -> bool {
        let ok1 = self.write_ec_ram(ec::ADDR_BLD_FAN1_RPM_SET, 0);
        let ok2 = self.write_ec_ram(ec::ADDR_BLD_FAN2_RPM_SET, 0);
        let ok3 = self.write_ec_ram(ec::ADDR_BLD_FAN_CTRL_MODE, mask::BLD_FAN_RELEASE_MASK);
        let verify_mode = self.read_ec_ram(ec::ADDR_BLD_FAN_CTRL_MODE).unwrap_or(0);

        set_bld_fan_clamp_rpm(0);
        std::thread::sleep(std::time::Duration::from_millis(200));
        crate::log_info!(
            "BLD_FAN",
            "Hardware fan clamp released: CPU=0, GPU=0, Mode register=0x{:02X} (returned to firmware thermal curve, success: c={}, g={}, mode={})",
            verify_mode, ok1, ok2, ok3
        );
        ok1 && ok2 && ok3
    }

    /// Direct query of live fan RPM from EC RAM
    /// Note: In Baolongda hardware layout, FAN1 is GPU tachometer, FAN2 is CPU tachometer
    pub fn read_dual_fan_rpm(&self) -> (u16, u16) {
        let gpu_raw = self.read_ec_ram(ec::ADDR_BLD_FAN1_RPM_READ).unwrap_or(0);
        let cpu_raw = self.read_ec_ram(ec::ADDR_BLD_FAN2_RPM_READ).unwrap_or(0);
        ((cpu_raw as u16) * 100, (gpu_raw as u16) * 100)
    }

    /// Battery charging limit via raw ACPI EC port 0x66/0x62 is permanently disarmed.
    /// In Jiaolong 16 Pro / Baolongda EC firmware, writing 0x10/0x0D via ports 0x66/0x62
    /// triggers active discharge 2DE2(R7=1) LEARN mode when battery SOC > limit,
    /// and collides with Windows kernel acpiec.sys, causing hardware power-rail cutoff shutdown.
    pub fn set_battery_charge_limit(&self, _limit: u8) -> Result<(), String> {
        crate::log_warn!(
            "BLD_DRV",
            "set_battery_charge_limit disarmed for hardware power-rail safety (raw 0x66/0x62 port access suppressed)"
        );
        Ok(())
    }

    /// Safely stops and deletes the specified kernel driver service from Windows SCM.
    pub fn stop_and_delete_service(service_name: &str) {
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS);
            if scm == 0 {
                return;
            }
            let svc_name_w = to_wide(service_name);
            let svc = OpenServiceW(scm, svc_name_w.as_ptr(), SERVICE_ALL_ACCESS);
            if svc != 0 {
                let mut status = [0u8; 36];
                let stop_res = ControlService(svc, SERVICE_CONTROL_STOP, status.as_mut_ptr());
                let del_res = DeleteService(svc);
                crate::log_info!(
                    "BLD_DRV",
                    "Kernel service '{}' teardown: stopped={}, deleted={}",
                    service_name,
                    stop_res != 0,
                    del_res != 0
                );
                CloseServiceHandle(svc);
            }
            CloseServiceHandle(scm);
        }
    }

    /// Fully shuts down the Baolongda driver manager, restores firmware fan curve,
    /// disconnects Ring0 handles, and stops & unloads the kernel service so third-party
    /// tools (such as JiaolongControl) can open cleanly without device name collision.
    pub fn shutdown_and_unload() {
        let opt_inst = INSTANCE.lock().take();
        if let Some(mgr) = opt_inst {
            crate::log_info!("BLD_DRV", "Shutting down Baolongda driver manager on application exit...");
            // 1. Release fan manual clamps and restore BIOS auto mode
            let _ = mgr.release_fan_clamp();
            std::thread::sleep(std::time::Duration::from_millis(50));

            // 2. Disconnect Ring0 port session
            if mgr.initialized.swap(false, Ordering::SeqCst) {
                unsafe {
                    (mgr.shutdown_bldring)();
                }
            }
            // mgr is dropped here, safely releasing the loaded Library handle
        }

        // 3. Stop and delete kernel services (both official BLDRING and legacy JiaoLongDriver64)
        Self::stop_and_delete_service(SERVICE_NAME);
        Self::stop_and_delete_service("JiaoLongDriver64");
        crate::log_info!("BLD_DRV", "Baolongda kernel driver services stopped and cleaned up successfully.");
    }
}

/// Global clean shutdown entrypoint called when OpenRevo exits
pub fn shutdown_bld_driver_on_exit() {
    BldDriverManager::shutdown_and_unload();
}

impl Drop for BldDriverManager {
    fn drop(&mut self) {
        if self.initialized.swap(false, Ordering::SeqCst) {
            let _ = self.release_fan_clamp();
            unsafe {
                (self.shutdown_bldring)();
            }
            crate::log_info!("BLD_DRV", "Baolongda driver shutdown and released");
        }
    }
}
