//! Application-owned XDG settings. Loading or saving never applies hardware controls.
//!
//! This store is not a plugin filesystem: callers can only read or replace the
//! versioned application document. Directory descriptors anchor every operation;
//! symbolic links and non-regular files are rejected.

use crate::hal::fan_curve::FanCurveConfig;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const CONFIG_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
const CONFIG_FILE: &str = "config.json";
const BACKUP_FILE: &str = "config.json.bak";
// Linux open(2) flags; keeping the filesystem layer independent of extra crates.
const O_DIRECTORY: i32 = 0o200000;
const O_NOFOLLOW: i32 = 0o400000;
const O_NONBLOCK: i32 = 0o4000;
static STORE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub version: u32,
    pub fan_curve_draft: Option<FanCurveConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            fan_curve_draft: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigError {
    pub code: String,
    pub message: String,
}

impl ConfigError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn io(action: &str, error: std::io::Error) -> Self {
        Self::new("config_io", format!("{action}: {error}"))
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    directory: PathBuf,
}

impl ConfigStore {
    /// Resolve the location once. No files or directories are created here.
    pub fn new() -> Result<Self, ConfigError> {
        Self::from_environment(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    fn from_environment(
        xdg: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, ConfigError> {
        let base = if let Some(xdg) = xdg.filter(|value| !value.is_empty()) {
            PathBuf::from(xdg)
        } else if let Some(home) = home.filter(|value| !value.is_empty()) {
            let home = PathBuf::from(home);
            validate_absolute_path(&home)?;
            home.join(".config")
        } else {
            return Err(ConfigError::new(
                "config_environment",
                "Neither XDG_CONFIG_HOME nor HOME is set; configuration location is unavailable",
            ));
        };
        Self::from_config_home(base)
    }

    /// Explicit absolute XDG base for embedders and isolated tests, never an IPC path.
    pub fn from_config_home(base: impl AsRef<Path>) -> Result<Self, ConfigError> {
        validate_absolute_path(base.as_ref())?;
        Ok(Self {
            directory: base.as_ref().join("OpenRevo"),
        })
    }

    /// Missing settings return None. Invalid or newer documents return an error.
    pub fn load(&self) -> Result<Option<AppConfig>, ConfigError> {
        let _guard = store_lock()?;
        let Some(directory) = open_directory(&self.directory, false)? else {
            return Ok(None);
        };
        read_document(&directory, CONFIG_FILE).map(|document| document.map(|(_, config)| config))
    }

    pub fn save_fan_curve_draft(
        &self,
        draft: Option<&FanCurveConfig>,
    ) -> Result<AppConfig, ConfigError> {
        let config = AppConfig {
            version: CONFIG_VERSION,
            fan_curve_draft: draft.cloned(),
        };
        validate_config(&config)?;
        self.save(config)
    }

    /// Export validated JSON, not a path. Missing configuration remains missing.
    pub fn export_json(&self) -> Result<Option<String>, ConfigError> {
        self.load()?.map(|config| encode(&config)).transpose()
    }

    /// Import is an explicit settings write only; it never activates the draft.
    pub fn import_json(&self, json: &str) -> Result<AppConfig, ConfigError> {
        self.save(decode(json.as_bytes())?)
    }

    fn save(&self, config: AppConfig) -> Result<AppConfig, ConfigError> {
        let encoded = encode(&config)?;
        let _guard = store_lock()?;
        // Validate before creating anything, so damaged/future settings cannot be
        // overwritten and a missing application directory stays a distinct state.
        let directory = open_directory(&self.directory, true)?.ok_or_else(|| {
            ConfigError::new("config_io", "Could not open the configuration directory")
        })?;
        let previous = read_document(&directory, CONFIG_FILE)?;
        check_target(&directory, BACKUP_FILE)?;
        if let Some((bytes, _)) = previous {
            atomic_write(&directory, BACKUP_FILE, &bytes)?;
        }
        atomic_write(&directory, CONFIG_FILE, encoded.as_bytes())?;
        directory
            .sync_all()
            .map_err(|error| ConfigError::io("Sync configuration directory", error))?;
        Ok(config)
    }
}

fn store_lock() -> Result<std::sync::MutexGuard<'static, ()>, ConfigError> {
    STORE_LOCK
        .lock()
        .map_err(|_| ConfigError::new("config_lock", "Configuration access lock is unavailable"))
}

fn validate_absolute_path(path: &Path) -> Result<(), ConfigError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(ConfigError::new(
            "config_path",
            "Configuration base must be an absolute path without parent traversal",
        ));
    }
    Ok(())
}

fn anchored(directory: &File, name: &std::ffi::OsStr) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name)
}

fn open_directory(path: &Path, create: bool) -> Result<Option<File>, ConfigError> {
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY | O_NOFOLLOW)
        .open("/")
        .map_err(|error| {
            ConfigError::io("Open filesystem root for configuration traversal", error)
        })?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let target = anchored(&directory, name);
        if create {
            match fs::DirBuilder::new().mode(0o700).create(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(ConfigError::io("Create configuration directory", error)),
            }
        }
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ConfigError::new(
                    "config_path",
                    "Configuration directory contains a symbolic link or non-directory entry",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
                return Ok(None)
            }
            Err(error) => return Err(ConfigError::io("Inspect configuration directory", error)),
        }
        directory = OpenOptions::new()
            .read(true)
            .custom_flags(O_DIRECTORY | O_NOFOLLOW)
            .open(target)
            .map_err(|error| {
                ConfigError::io(
                    "Open configuration directory without following links",
                    error,
                )
            })?;
    }
    Ok(Some(directory))
}

fn check_target(directory: &File, name: &str) -> Result<(), ConfigError> {
    match fs::symlink_metadata(anchored(directory, name.as_ref())) {
        Ok(metadata) if !metadata.is_file() || metadata.nlink() != 1 => Err(ConfigError::new(
            "config_path",
            format!("{name} must be a regular file with no symbolic or hard links"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ConfigError::io("Inspect configuration file", error)),
    }
}

fn read_document(
    directory: &File,
    name: &str,
) -> Result<Option<(Vec<u8>, AppConfig)>, ConfigError> {
    check_target(directory, name)?;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(anchored(directory, name.as_ref()))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ConfigError::io("Open configuration file", error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| ConfigError::io("Inspect open configuration file", error))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(ConfigError::new(
            "config_path",
            "Configuration must be a regular file with no hard links",
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(ConfigError::new(
            "config_too_large",
            "Configuration exceeds the 64 KiB limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ConfigError::io("Read configuration file", error))?;
    let config = decode(&bytes)?;
    Ok(Some((bytes, config)))
}

fn decode(bytes: &[u8]) -> Result<AppConfig, ConfigError> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::new(
            "config_too_large",
            "Configuration exceeds the 64 KiB limit",
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        ConfigError::new(
            "config_invalid",
            format!("Configuration JSON is invalid; existing data was preserved: {error}"),
        )
    })?;
    if let Some(version) = value.get("version").and_then(serde_json::Value::as_u64) {
        if version != u64::from(CONFIG_VERSION) {
            return Err(ConfigError::new("config_version", format!("Configuration version {version} is unsupported (expected {CONFIG_VERSION}); existing data was preserved")));
        }
    }
    // FanCurvePoint is also used by hardware IPC. Apply strict document keys here
    // without changing that existing protocol's deserializer.
    if let Some(draft) = value
        .get("fan_curve_draft")
        .filter(|draft| !draft.is_null())
    {
        for curve in ["cpu", "gpu"] {
            if let Some(points) = draft.get(curve).and_then(serde_json::Value::as_array) {
                for point in points {
                    if let Some(fields) = point.as_object() {
                        if fields.keys().any(|key| key != "temp" && key != "duty") {
                            return Err(ConfigError::new(
                                "config_invalid",
                                "Curve points contain unsupported settings",
                            ));
                        }
                    }
                }
            }
        }
    }
    let config: AppConfig = serde_json::from_slice(bytes).map_err(|error| {
        ConfigError::new(
            "config_invalid",
            format!("Configuration schema is invalid; existing data was preserved: {error}"),
        )
    })?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    if config.version != CONFIG_VERSION {
        return Err(ConfigError::new(
            "config_version",
            "Unsupported configuration version",
        ));
    }
    if let Some(draft) = &config.fan_curve_draft {
        draft.validate().map_err(|error| {
            ConfigError::new(
                "config_invalid",
                format!("Invalid fan curve draft: {error}"),
            )
        })?;
    }
    Ok(())
}

fn encode(config: &AppConfig) -> Result<String, ConfigError> {
    validate_config(config)?;
    let mut json = serde_json::to_string_pretty(config).map_err(|error| {
        ConfigError::new(
            "config_invalid",
            format!("Cannot encode configuration: {error}"),
        )
    })?;
    json.push('\n');
    Ok(json)
}

fn atomic_write(directory: &File, name: &str, bytes: &[u8]) -> Result<(), ConfigError> {
    check_target(directory, name)?;
    let mut temporary = None;
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_name = format!(".{name}.tmp.{}.{sequence}", std::process::id());
        let path = anchored(directory, temporary_name.as_ref());
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ConfigError::io(
                    "Create unique configuration temporary file",
                    error,
                ))
            }
        }
    }
    let Some((temporary_path, mut file)) = temporary else {
        return Err(ConfigError::new(
            "config_io",
            "Could not allocate a unique configuration temporary file",
        ));
    };
    let result = (|| {
        file.write_all(bytes)
            .map_err(|error| ConfigError::io("Write configuration temporary file", error))?;
        file.sync_all()
            .map_err(|error| ConfigError::io("Sync configuration temporary file", error))?;
        check_target(directory, name)?;
        fs::rename(&temporary_path, anchored(directory, name.as_ref()))
            .map_err(|error| ConfigError::io("Atomically replace configuration file", error))?;
        // Persist the backup's directory entry before replacing the main file.
        directory
            .sync_all()
            .map_err(|error| ConfigError::io("Sync configuration directory entry", error))?;
        Ok(())
    })();
    if result.is_err() {
        // Only this newly created file, anchored to its still-open directory.
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(test)]
mod tests;
