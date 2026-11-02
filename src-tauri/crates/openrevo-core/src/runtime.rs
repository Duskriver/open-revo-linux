//! One desktop-owned backend shared by Tauri and the session-bus CLI.
//! The Rust worker owns control leases independently of all UI clients.

use crate::actions::{Action, PowerMode};
use crate::config::{AppConfig, ConfigError, ConfigStore};
use crate::linux::{FanCurveConfig, LinuxBackend, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl RuntimeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    fn hardware(message: String) -> Self {
        Self::new("control_failed", message)
    }
}

impl From<ConfigError> for RuntimeError {
    fn from(error: ConfigError) -> Self {
        Self::new(&error.code, error.message)
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    command: Command,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Status {},
    Config {},
    Action { action: Action },
    ReleaseControls {},
}

#[derive(Serialize)]
struct Response {
    version: u32,
    ok: bool,
    result: Option<Value>,
    error: Option<RuntimeError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub message: String,
    pub error: bool,
}

pub struct AppRuntime {
    backend: Mutex<LinuxBackend>,
    config: Result<ConfigStore, ConfigError>,
    closing: AtomicBool,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    control_error: Mutex<Option<String>>,
    hotkeys_enabled: AtomicBool,
    hotkey_error: Mutex<Option<String>>,
    notices: Mutex<Option<SyncSender<Notice>>>,
    last_mode_write: Mutex<Option<Instant>>,
}

impl Default for AppRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AppRuntime {
    pub fn new() -> Self {
        Self::with_config_store(ConfigStore::new())
    }

    /// Supports isolated settings tests without changing process environment.
    pub fn with_config_store(config: Result<ConfigStore, ConfigError>) -> Self {
        let hotkeys_enabled = config
            .as_ref()
            .ok()
            .and_then(|s| s.load().ok().flatten())
            .is_some_and(|c| c.hotkeys_enabled);
        Self {
            hotkeys_enabled: AtomicBool::new(hotkeys_enabled),
            hotkey_error: Mutex::new(None),
            notices: Mutex::new(None),
            last_mode_write: Mutex::new(None),
            backend: Mutex::new(LinuxBackend::new()),
            config,
            closing: AtomicBool::new(false),
            worker: Mutex::new(None),
            control_error: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_backend(backend: LinuxBackend) -> Self {
        let mut runtime = Self::with_config_store(Err(ConfigError {
            code: "test_no_config".into(),
            message: "Fixture has no settings store".into(),
        }));
        runtime.backend = Mutex::new(backend);
        runtime
    }

    #[cfg(test)]
    pub(crate) fn with_test_parts(backend: LinuxBackend, store: ConfigStore) -> Self {
        let mut runtime = Self::with_config_store(Ok(store));
        runtime.backend = Mutex::new(backend);
        runtime
    }
    fn ensure_open(&self) -> Result<(), RuntimeError> {
        if self.closing.load(Ordering::Acquire) {
            Err(RuntimeError::new(
                "runtime_closing",
                "桌面程序正在退出，未开始新的操作",
            ))
        } else {
            Ok(())
        }
    }

    fn lock_backend(&self) -> Result<MutexGuard<'_, LinuxBackend>, RuntimeError> {
        self.ensure_open()?;
        let backend = self
            .backend
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock", "控制宿主状态锁不可用"))?;
        // Shutdown can start while this request is waiting for another operation.
        self.ensure_open()?;
        Ok(backend)
    }

    /// Existing semantic Tauri controls share exactly the same owner and lock.
    pub fn with_backend<T>(
        &self,
        operation: impl FnOnce(&mut LinuxBackend) -> Result<T, String>,
    ) -> Result<T, RuntimeError> {
        operation(&mut *self.lock_backend()?).map_err(RuntimeError::hardware)
    }

    fn config(&self) -> Result<&ConfigStore, RuntimeError> {
        self.config.as_ref().map_err(|error| error.clone().into())
    }

    fn with_config<T>(
        &self,
        operation: impl FnOnce(&ConfigStore) -> Result<T, ConfigError>,
    ) -> Result<T, RuntimeError> {
        let _backend = self.lock_backend()?;
        operation(self.config()?).map_err(Into::into)
    }

    pub fn snapshot(&self) -> Result<Snapshot, RuntimeError> {
        let mut snapshot = self.with_backend(|backend| Ok(backend.snapshot()))?;
        snapshot.hotkeys_enabled = self.hotkeys_enabled.load(Ordering::Acquire);
        snapshot.hotkey_error = self.hotkey_error.lock().ok().and_then(|e| e.clone());
        snapshot.control_error = self.control_error.lock().ok().and_then(|e| e.clone());
        Ok(snapshot)
    }

    pub fn load_config(&self) -> Result<Option<AppConfig>, RuntimeError> {
        self.with_config(ConfigStore::load)
    }

    pub fn save_fan_curve_draft(&self, draft: &FanCurveConfig) -> Result<AppConfig, RuntimeError> {
        self.with_config(|store| store.save_fan_curve_draft(Some(draft)))
    }

    pub fn export_config(&self) -> Result<Option<String>, RuntimeError> {
        self.with_config(ConfigStore::export_json)
    }

    pub fn import_config(&self, json: &str) -> Result<AppConfig, RuntimeError> {
        let mut backend = self.lock_backend()?;
        let config = self.config()?.import_json(json)?;
        backend.reset_events();
        self.hotkeys_enabled
            .store(config.hotkeys_enabled, Ordering::Release);
        Ok(config)
    }

    /// A single worker owns leases even while the WebView is hidden or suspended.
    pub fn start(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock", "后台线程锁不可用"))?;
        self.ensure_open()?;
        if worker.is_some() {
            return Ok(());
        }
        let runtime = Arc::downgrade(self);
        *worker = Some(
            std::thread::Builder::new()
                .name("openrevo-control".into())
                .spawn(move || {
                    let mut maintenance = Instant::now();
                    let mut previous = SystemTime::now();
                    let mut last_key = None;
                    loop {
                        std::thread::park_timeout(Duration::from_millis(100));
                        let Some(runtime) = runtime.upgrade() else {
                            break;
                        };
                        if runtime.closing.load(Ordering::Acquire) {
                            break;
                        }
                        let stalled = previous
                            .elapsed()
                            .map(|t| t > Duration::from_secs(1))
                            .unwrap_or(true);
                        previous = SystemTime::now();
                        if stalled {
                            let _ = runtime.with_backend(|b| {
                                b.reset_events();
                                Ok(())
                            });
                        }
                        if maintenance.elapsed() >= Duration::from_secs(2) {
                            maintenance = Instant::now();
                            let result = runtime.with_backend(|backend| {
                                backend.control_watchdog();
                                backend.renew_control_lease()
                            });
                            if let Ok(mut error) = runtime.control_error.lock() {
                                *error = result.err().map(|error| error.message);
                            }
                        }
                        runtime.dispatch_hotkeys(&mut last_key);
                    }
                })
                .map_err(|error| RuntimeError::new("runtime_start", error.to_string()))?,
        );
        Ok(())
    }

    pub fn set_notice_sender(&self, sender: SyncSender<Notice>) {
        if let Ok(mut target) = self.notices.lock() {
            *target = Some(sender);
        }
    }
    fn notify(&self, message: String, error: bool) {
        if let Ok(target) = self.notices.lock() {
            if let Some(sender) = target.as_ref() {
                let _ = sender.try_send(Notice { message, error });
            }
        }
    }
    fn dispatch_hotkeys(&self, last: &mut Option<Instant>) {
        let requested = Instant::now();
        let result = self.with_backend(|b| {
            if requested.elapsed() > Duration::from_secs(1) {
                b.reset_events();
                return Ok(None);
            }
            let events = b.poll_events();
            if !self.hotkeys_enabled.load(Ordering::Acquire)
                || !events.iter().any(|e| is_mode_key(e.code))
            {
                return Ok(None);
            }
            let now = Instant::now();
            if last.is_some_and(|time| {
                now.duration_since(time)
                    < Duration::from_millis(u64::from(crate::hal::vault::wmi_hotkeys::DEBOUNCE_MS))
            }) {
                return Ok(None);
            }
            if self
                .last_mode_write
                .lock()
                .ok()
                .and_then(|t| *t)
                .is_some_and(|time| {
                    now.duration_since(time)
                        < Duration::from_millis(u64::from(
                            crate::hal::vault::wmi_hotkeys::DEBOUNCE_MS,
                        ))
                })
            {
                return Ok(None);
            }
            *last = Some(now);
            let snapshot = b.snapshot();
            // Physical EC keys never redirect to a generic desktop power service.
            if !snapshot.oem.available {
                return Err("机身按键需要可用的 EC 通道".into());
            }
            next_mode(&snapshot.domains.performance).map(Some)
        });
        let result = match result {
            Ok(Some(mode)) => self.execute(Action::Mode { mode }),
            Ok(None) => return,
            Err(e) => Err(e),
        };
        let message = result.err().map(|e| e.message);
        if let Some(message) = &message {
            self.notify(format!("按键切换失败：{message}"), true);
        }
        if let Ok(mut error) = self.hotkey_error.lock() {
            *error = message;
        }
    }

    pub fn execute(&self, action: Action) -> Result<(), RuntimeError> {
        let mut backend = self.lock_backend()?;
        let persist = |mode, limits| {
            self.config()?
                .save_mode(mode, limits)
                .map(|_| ())
                .map_err(|error| {
                    RuntimeError::new(
                        "settings_not_saved",
                        format!("硬件已应用，但配置保存失败：{error}"),
                    )
                })
        };
        let notice = match &action {
            Action::Mode { mode } => Some(format!("性能模式：{}", mode_name(*mode))),
            _ => None,
        };
        let result = match action {
            Action::Battery { policy } => backend
                .set_ec_battery(policy)
                .map_err(RuntimeError::hardware),
            Action::Switch { switch, enabled } => backend
                .set_device_switch(switch, enabled)
                .map_err(RuntimeError::hardware),
            Action::Hotkeys { enabled } => {
                self.config()?.save_hotkeys(enabled)?;
                backend.reset_events();
                self.hotkeys_enabled.store(enabled, Ordering::Release);
                if let Ok(mut error) = self.hotkey_error.lock() {
                    *error = None;
                }
                Ok(())
            }
            Action::Mode { mode } => {
                if let Ok(mut time) = self.last_mode_write.lock() {
                    *time = Some(Instant::now());
                }
                let custom = if mode == PowerMode::Custom {
                    self.config()?
                        .load()?
                        .and_then(|c| c.custom_limits)
                        .or_else(|| backend.snapshot().oem.presets.first().copied())
                } else {
                    None
                };
                backend
                    .set_mode(mode, custom)
                    .map_err(RuntimeError::hardware)?;
                let limits = (mode == PowerMode::Custom)
                    .then(|| backend.snapshot().domains.performance.limits)
                    .flatten();
                persist(mode, limits)
            }
            Action::CpuLimits { limits } => {
                backend
                    .set_oem_cpu_limits(limits.pl1, limits.pl2, limits.pl4)
                    .map_err(RuntimeError::hardware)?;
                persist(PowerMode::Custom, Some(limits))
            }
            Action::Fan { enabled } => backend
                .set_cooling_boost(enabled)
                .map_err(RuntimeError::hardware),
            Action::FanCurve { config } => {
                backend
                    .set_oem_fan_curve(config.clone())
                    .map_err(RuntimeError::hardware)?;
                self.config()?
                    .save_fan_curve_draft(Some(&config))
                    .map(|_| ())
                    .map_err(|error| {
                        RuntimeError::new(
                            "settings_not_saved",
                            format!("曲线已应用，但配置保存失败：{error}"),
                        )
                    })
            }
            Action::RestoreFanCurve {} => backend
                .restore_oem_fan_curve()
                .map_err(RuntimeError::hardware),
            Action::BatteryLimit { battery, limit } => backend
                .set_battery_limit(&battery, limit)
                .map_err(RuntimeError::hardware),
            Action::Backlight { device, percent } => backend
                .set_backlight(&device, percent)
                .map_err(RuntimeError::hardware),
            Action::PowerLimit { id, watts } => backend
                .set_power_limit(&id, watts)
                .map_err(RuntimeError::hardware),
            Action::ReleaseControls {} => {
                backend.release_controls().map_err(RuntimeError::hardware)
            }
        };
        if result.is_ok() {
            if let Some(message) = notice {
                self.notify(message, false);
            }
        }
        result
    }

    pub fn release_controls(&self) -> Result<(), RuntimeError> {
        self.with_backend(LinuxBackend::release_controls)
    }

    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        // Reject queued writes before waiting for an in-flight operation.
        self.closing.store(true, Ordering::Release);
        if let Some(worker) = self
            .worker
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock", "后台线程锁不可用"))?
            .take()
        {
            worker.thread().unpark();
            let _ = worker.join();
        }
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock", "退出时无法取得控制宿主状态"))?;
        backend.release_controls().map_err(RuntimeError::hardware)
    }

    pub fn handle_request_json(&self, json: &str) -> String {
        let result = (|| {
            if json.len() > MAX_REQUEST_BYTES {
                return Err(RuntimeError::new(
                    "request_too_large",
                    "请求超过 128 KiB 限制",
                ));
            }
            let request: Request = serde_json::from_str(json).map_err(|error| {
                RuntimeError::new("invalid_request", format!("无效的控制请求：{error}"))
            })?;
            if request.version != PROTOCOL_VERSION {
                return Err(RuntimeError::new(
                    "protocol_version",
                    "控制协议版本不兼容，请配套更新桌面程序和 CLI",
                ));
            }
            match request.command {
                Command::Status {} => value(self.snapshot()?),
                Command::Config {} => value(self.load_config()?),
                Command::Action { action } => value(self.execute(action)?),
                Command::ReleaseControls {} => value(self.release_controls()?),
            }
        })();
        response_json(result)
    }
}

fn is_mode_key(code: u32) -> bool {
    use crate::hal::vault::wmi_hotkeys as keys;
    [keys::MODE_KEY, keys::MODE_SWITCH, keys::CUSTOM_KEY].contains(&code)
}
fn mode_name(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Office => "办公",
        PowerMode::Balanced => "均衡",
        PowerMode::Turbo => "狂暴",
        PowerMode::Custom => "自定义",
    }
}
fn next_mode(performance: &crate::linux::Performance) -> Result<PowerMode, String> {
    let cycle = [
        PowerMode::Office,
        PowerMode::Balanced,
        PowerMode::Turbo,
        PowerMode::Custom,
    ];
    let start = performance
        .active
        .and_then(|active| cycle.iter().position(|m| *m == active))
        .map(|i| (i + 1) % cycle.len())
        .unwrap_or(0);
    (0..cycle.len())
        .map(|offset| cycle[(start + offset) % cycle.len()])
        .find(|mode| {
            performance
                .modes
                .iter()
                .any(|m| m.mode == *mode && m.enabled)
        })
        .ok_or("当前供电或权限不允许按键切换模式".into())
}

fn value(value: impl Serialize) -> Result<Value, RuntimeError> {
    serde_json::to_value(value).map_err(|error| {
        RuntimeError::new("response_invalid", format!("无法编码操作结果：{error}"))
    })
}

pub(crate) fn response_json(result: Result<Value, RuntimeError>) -> String {
    let response = match result {
        Ok(result) => Response {
            version: PROTOCOL_VERSION,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => Response {
            version: PROTOCOL_VERSION,
            ok: false,
            result: None,
            error: Some(error),
        },
    };
    serde_json::to_string(&response).expect("JSON values and strings serialize infallibly")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_retired_and_raw_commands_without_io() {
        let root = tempfile::tempdir().unwrap();
        let runtime = AppRuntime::with_config_store(ConfigStore::from_config_home(root.path()));
        for command in [
            serde_json::json!({"kind":"scene_list"}),
            serde_json::json!({"kind":"renew_control_lease"}),
            serde_json::json!({"kind":"raw_write"}),
            serde_json::json!({"kind":"action","action":{"kind":"mode","mode":"invalid"}}),
            serde_json::json!({"kind":"action","action":{"kind":"fan","enabled":true,"address":42}}),
        ] {
            let reply: Value = serde_json::from_str(&runtime.handle_request_json(
                &serde_json::json!({"version":1,"command":command}).to_string(),
            ))
            .unwrap();
            assert_eq!(reply["error"]["code"], "invalid_request");
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn mode_key_mapping_and_cycle_respect_live_capabilities() {
        use crate::hal::vault::wmi_hotkeys as keys;
        for code in [keys::MODE_KEY, keys::MODE_SWITCH, keys::CUSTOM_KEY] {
            assert!(is_mode_key(code));
        }
        assert!(!is_mode_key(u32::MAX));
        let mut performance = crate::linux::Performance {
            active: Some(PowerMode::Office),
            modes: [
                PowerMode::Office,
                PowerMode::Balanced,
                PowerMode::Turbo,
                PowerMode::Custom,
            ]
            .into_iter()
            .map(|mode| crate::linux::ModeOption {
                mode,
                enabled: true,
                reason: None,
            })
            .collect(),
            ..Default::default()
        };
        for target in [
            PowerMode::Balanced,
            PowerMode::Turbo,
            PowerMode::Custom,
            PowerMode::Office,
        ] {
            assert_eq!(next_mode(&performance).unwrap(), target);
            performance.active = Some(target);
        }
        for mode in &mut performance.modes {
            mode.enabled = mode.mode == PowerMode::Office;
        }
        assert_eq!(next_mode(&performance).unwrap(), PowerMode::Office);
        performance.modes.clear();
        assert!(next_mode(&performance).is_err());
    }

    #[test]
    fn worker_is_unique_and_shutdown_rejects_late_requests() {
        let root = tempfile::tempdir().unwrap();
        let runtime = Arc::new(AppRuntime::with_config_store(
            ConfigStore::from_config_home(root.path()),
        ));
        runtime.start().unwrap();
        let id = runtime
            .worker
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .thread()
            .id();
        runtime.start().unwrap();
        assert_eq!(
            runtime
                .worker
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .thread()
                .id(),
            id
        );
        runtime.shutdown().unwrap();
        assert!(runtime.worker.lock().unwrap().is_none());
        assert_eq!(runtime.start().unwrap_err().code, "runtime_closing");
        assert_eq!(
            runtime
                .execute(Action::Fan { enabled: true })
                .unwrap_err()
                .code,
            "runtime_closing"
        );
        runtime.shutdown().unwrap();
    }
}
