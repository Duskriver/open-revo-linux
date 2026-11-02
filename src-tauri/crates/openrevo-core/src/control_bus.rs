//! Session-bus transport for the running desktop owner. No activation or retry.
use crate::runtime::{response_json, AppRuntime, RuntimeError, MAX_REQUEST_BYTES};
use std::sync::Arc;
use std::time::Duration;
use zbus::message::Header;

pub const BUS_NAME: &str = "com.openrevo.controlcenter";
pub const OBJECT_PATH: &str = "/com/openrevo/controlcenter";
pub const INTERFACE_NAME: &str = "com.openrevo.controlcenter.Control1";
pub const METHOD_TIMEOUT: Duration = Duration::from_secs(15);

struct ControlService {
    runtime: Arc<AppRuntime>,
}

#[zbus::interface(name = "com.openrevo.controlcenter.Control1")]
impl ControlService {
    async fn request(
        &self,
        json: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("Missing caller identity".into()))?;
        let own = connection
            .unique_name()
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("Missing host identity".into()))?;
        let bus = zbus::fdo::DBusProxy::new(connection).await?;
        let caller_uid = bus.get_connection_unix_user(sender.clone().into()).await?;
        let owner_uid = bus.get_connection_unix_user(own.clone().into()).await?;
        if caller_uid != owner_uid {
            return Err(zbus::fdo::Error::AccessDenied(
                "Control is limited to the desktop user".into(),
            ));
        }
        if json.len() > MAX_REQUEST_BYTES {
            return Ok(response_json(Err(RuntimeError::new(
                "request_too_large",
                "请求超过 128 KiB 限制",
            ))));
        }
        let runtime = Arc::clone(&self.runtime);
        let json = json.to_owned();
        // Backend methods include blocking system-bus calls. Never run them on
        // zbus's executor thread or hold an async interface lock across them.
        Ok(blocking::unblock(move || runtime.handle_request_json(&json)).await)
    }
}

/// Keep the returned connection alive for the desktop lifetime. Name collisions
/// fail instead of replacing or queuing behind another controller.
pub fn serve_control(runtime: Arc<AppRuntime>) -> Result<zbus::blocking::Connection, String> {
    let builder = zbus::blocking::connection::Builder::session()
        .map_err(|error| format!("无法连接桌面会话总线：{error}"))?;
    serve_with_builder(builder, runtime)
}

fn serve_with_builder(
    builder: zbus::blocking::connection::Builder<'_>,
    runtime: Arc<AppRuntime>,
) -> Result<zbus::blocking::Connection, String> {
    builder
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .method_timeout(METHOD_TIMEOUT)
        .serve_at(OBJECT_PATH, ControlService { runtime })
        .and_then(|builder| builder.name(BUS_NAME))
        .and_then(|builder| builder.build())
        .map_err(|error| format!("无法注册桌面控制服务（不会替换已有宿主）：{error}"))
}

/// Call the existing desktop once. A timeout/disconnect after submission cannot
/// establish whether a hardware operation ran, so callers must not auto-retry.
pub fn call_control(request: &str) -> Result<String, String> {
    if request.len() > MAX_REQUEST_BYTES {
        return Err("请求超过 128 KiB 限制".into());
    }
    let connection = zbus::blocking::connection::Builder::session()
        .map_err(|error| format!("无法连接当前桌面会话总线：{error}"))?
        .method_timeout(METHOD_TIMEOUT)
        .build()
        .map_err(|error| format!("无法连接当前桌面会话总线：{error}"))?;
    call_on_connection(&connection, request, METHOD_TIMEOUT)
}

fn call_on_connection(
    connection: &zbus::blocking::Connection,
    request: &str,
    timeout: Duration,
) -> Result<String, String> {
    let proxy: zbus::blocking::Proxy<'_> = zbus::blocking::proxy::Builder::new(connection)
        .destination(BUS_NAME)
        .and_then(|builder| builder.path(OBJECT_PATH))
        .and_then(|builder| builder.interface(INTERFACE_NAME))
        .and_then(|builder| {
            builder
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
        })
        .map_err(|error| format!("无法建立桌面控制连接：{error}"))?;
    // zbus 5.19 call_with_flags uses call_method_raw and bypasses the connection's
    // method_timeout. Race the whole request against an explicit timer.
    let result: zbus::Result<Option<String>> = async_io::block_on(futures_lite::future::race(
        async {
            proxy
                .inner()
                .call_with_flags(
                    "Request",
                    zbus::proxy::MethodFlags::NoAutoStart.into(),
                    &(request,),
                )
                .await
        },
        async {
            async_io::Timer::after(timeout).await;
            Err(zbus::Error::InputOutput(
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Desktop control reply timed out",
                )
                .into(),
            ))
        },
    ));
    match result {
        Ok(Some(response)) => Ok(response),
        Ok(None) => Err("桌面控制未返回结果；执行结果未知，请查询 status 和 status，勿自动重试应用".into()),
        Err(zbus::Error::MethodError(name, message, _)) if matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner") => {
            Err(format!("桌面控制程序未运行；请先打开 OpenRevo。{}", message.unwrap_or_default()))
        }
        Err(zbus::Error::MethodError(name, message, _)) if matches!(name.as_str(), "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" | "org.freedesktop.DBus.Error.TimedOut" | "org.freedesktop.DBus.Error.Disconnected") => {
            Err(format!("桌面控制未能返回结果（{name}）：{}；执行结果未知，请查询 status 和 status，勿自动重试应用", message.unwrap_or_default()))
        }
        Err(zbus::Error::MethodError(name, message, _)) => Err(format!("桌面控制请求被拒绝（{name}）：{}", message.unwrap_or_default())),
        Err(error) => Err(format!("桌面控制连接超时或断开：{error}；执行结果未知，请查询 status 和 status，勿自动重试应用")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigStore;
    use std::io::BufRead;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    struct PrivateBus {
        child: Child,
        address: String,
    }
    impl PrivateBus {
        fn new() -> Self {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon for isolated transport tests");
            let mut address = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            assert!(!address.trim().is_empty());
            Self {
                child,
                address: address.trim().into(),
            }
        }
        fn builder(&self) -> zbus::blocking::connection::Builder<'_> {
            zbus::blocking::connection::Builder::address(self.address.as_str()).unwrap()
        }
    }
    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct SlowService {
        finished: Arc<AtomicBool>,
    }
    #[zbus::interface(name = "com.openrevo.controlcenter.Control1")]
    impl SlowService {
        async fn request(&self, request: &str) -> zbus::fdo::Result<String> {
            if request == "no_reply" {
                return Err(zbus::fdo::Error::NoReply("Fixture no reply".into()));
            }
            async_io::Timer::after(Duration::from_millis(200)).await;
            self.finished.store(true, Ordering::Release);
            Ok("finished".into())
        }
    }

    #[test]
    fn explicit_timeout_returns_unknown_outcome_without_waiting_for_service() {
        let bus = PrivateBus::new();
        let finished = Arc::new(AtomicBool::new(false));
        let _service = bus
            .builder()
            .serve_at(
                OBJECT_PATH,
                SlowService {
                    finished: finished.clone(),
                },
            )
            .unwrap()
            .name(BUS_NAME)
            .unwrap()
            .build()
            .unwrap();
        let client = bus.builder().build().unwrap();
        let started = Instant::now();
        let error = call_on_connection(&client, "delay", Duration::from_millis(30)).unwrap_err();
        assert!(error.contains("执行结果未知"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!finished.load(Ordering::Acquire));
        // A timeout does not cancel an already dispatched operation.
        std::thread::sleep(Duration::from_millis(250));
        assert!(finished.load(Ordering::Acquire));
        let error = call_on_connection(&client, "no_reply", Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("执行结果未知"));
    }

    #[test]
    fn private_bus_serves_same_user_and_rejects_second_owner_without_hardware() {
        let bus = PrivateBus::new();
        let root = tempfile::tempdir().unwrap();
        let runtime = Arc::new(AppRuntime::with_config_store(
            ConfigStore::from_config_home(root.path()),
        ));
        let client = bus.builder().build().unwrap();
        let request = r#"{"version":1,"command":{"kind":"config"}}"#;
        assert!(call_on_connection(&client, request, Duration::from_secs(1))
            .unwrap_err()
            .contains("未运行"));
        let _service = serve_with_builder(bus.builder(), runtime.clone()).unwrap();
        assert!(serve_with_builder(bus.builder(), runtime.clone()).is_err());
        let reply = call_on_connection(&client, request, Duration::from_secs(1)).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["result"], serde_json::Value::Null);
        runtime.shutdown().unwrap();
        let reply = call_on_connection(&client, request, Duration::from_secs(1)).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["error"]["code"], "runtime_closing");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
