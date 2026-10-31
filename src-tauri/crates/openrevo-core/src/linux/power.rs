use super::PowerProfiles;
use std::collections::HashMap;
use std::time::Duration;
use zbus::{
    blocking::{connection::Builder, Connection, Proxy},
    zvariant::OwnedValue,
};

// Current upstream service name, followed by its older compatibility name.
const ENDPOINTS: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];

pub struct PowerService {
    connection: Option<Connection>,
}

fn system_connection() -> Result<Connection, String> {
    Builder::system()
        .map_err(|e| e.to_string())?
        .method_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())
}

impl PowerService {
    pub fn new() -> Self {
        Self { connection: None }
    }

    fn proxy(&mut self) -> Result<Proxy<'_>, String> {
        if self.connection.is_none() {
            self.connection = Some(system_connection()?);
        }
        let connection = self.connection.as_ref().unwrap();
        for (name, path) in ENDPOINTS {
            let proxy = (|| -> zbus::Result<Proxy<'_>> {
                zbus::blocking::proxy::Builder::new(connection)
                    .destination(name)?
                    .path(path)?
                    .interface(name)?
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build()
            })();
            if let Ok(proxy) = proxy {
                if proxy.get_property::<String>("ActiveProfile").is_ok() {
                    return Ok(proxy);
                }
            }
        }
        Err("无法连接 power-profiles-daemon / tuned-ppd；请检查系统电源服务".into())
    }

    pub fn snapshot(&mut self) -> PowerProfiles {
        let result = (|| -> Result<PowerProfiles, String> {
            let proxy = self.proxy()?;
            let active = proxy
                .get_property::<String>("ActiveProfile")
                .map_err(|e| e.to_string())?;
            let profiles = proxy
                .get_property::<Vec<HashMap<String, OwnedValue>>>("Profiles")
                .map_err(|e| e.to_string())?;
            let available = profiles
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("Profile")
                        .and_then(|v| <&str>::try_from(v).ok())
                        .map(str::to_owned)
                })
                .collect();
            let can_set = profile_authorized(proxy.connection(), proxy.destination().as_str());
            Ok(PowerProfiles {
                active: Some(active),
                available,
                source: Some(proxy.destination().to_string()),
                can_set,
                error: (can_set == Some(false))
                    .then(|| "当前进程未获得档位切换授权；请从活动桌面会话启动 OpenRevo".into()),
            })
        })();
        result.unwrap_or_else(|error| {
            self.connection = None;
            PowerProfiles {
                error: Some(error),
                ..Default::default()
            }
        })
    }

    pub fn set(&mut self, profile: &str) -> Result<(), String> {
        validate_profile(profile)?;
        let snapshot = self.snapshot();
        if !snapshot.available.iter().any(|p| p == profile) {
            return Err(format!("系统未提供性能档位：{profile}"));
        }
        let proxy = self.proxy()?;
        proxy
            .set_property("ActiveProfile", profile)
            .map_err(|e| format!("切换档位失败：{e}"))?;
        let actual: String = proxy
            .get_property("ActiveProfile")
            .map_err(|e| e.to_string())?;
        if actual != profile {
            return Err(format!("档位被系统或其他程序改为 {actual}"));
        }
        Ok(())
    }
}

fn profile_authorized(connection: &Connection, service: &str) -> Option<bool> {
    let authority = Proxy::new(
        connection,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .ok()?;
    let mut subject_details = HashMap::new();
    subject_details.insert(
        "name",
        zbus::zvariant::Value::from(connection.unique_name()?.as_str()),
    );
    let subject = ("system-bus-name", subject_details);
    let action = format!("{service}.switch-profile");
    let details = HashMap::<String, String>::new();
    // Flags = 0: query authorization without prompting or modifying policy.
    let (authorized, _challenge, _details): (bool, bool, HashMap<String, String>) = authority
        .call("CheckAuthorization", &(subject, action, details, 0_u32, ""))
        .ok()?;
    Some(authorized)
}

fn validate_profile(profile: &str) -> Result<(), String> {
    match profile {
        "power-saver" | "balanced" | "performance" => Ok(()),
        _ => Err("无效性能档位".into()),
    }
}

pub fn set_brightness(subsystem: &str, name: &str, brightness: u32) -> Result<(), String> {
    let connection = system_connection()?;
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
    )
    .map_err(|e| e.to_string())?;
    proxy
        .call::<_, _, ()>("SetBrightness", &(subsystem, name, brightness))
        .map_err(|e| format!("当前会话无法设置亮度：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_power_profiles() {
        assert!(validate_profile("performance").is_ok());
        assert!(validate_profile("custom").is_err());
        assert!(validate_profile("performance; reboot").is_err());
    }
}
