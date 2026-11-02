//! Desktop feedback runs off the hardware worker; a missing server is harmless.
use openrevo_core::runtime::{AppRuntime, Notice};
use std::{
    collections::HashMap,
    sync::{mpsc, Arc},
    time::Duration,
};
use tauri::Emitter;
use zbus::{
    blocking::{connection::Builder, Proxy},
    zvariant::Value,
};

pub fn start(app: tauri::AppHandle, runtime: &Arc<AppRuntime>) -> std::io::Result<()> {
    let (sender, receiver) = mpsc::sync_channel::<Notice>(32);
    runtime.set_notice_sender(sender);
    std::thread::Builder::new()
        .name("openrevo-notices".into())
        .spawn(move || {
            let mut connection = None;
            let mut replacement = 0u32;
            while let Ok(notice) = receiver.recv() {
                let _ = app.emit("linux-state-changed", ());
                if notice.error {
                    let _ = app.emit("linux-control-error", &notice.message);
                }
                let result = (|| -> zbus::Result<u32> {
                    if connection.is_none() {
                        connection = Some(
                            Builder::session()?
                                .method_timeout(Duration::from_secs(2))
                                .build()?,
                        );
                    }
                    let proxy = Proxy::new(
                        connection.as_ref().unwrap(),
                        "org.freedesktop.Notifications",
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                    )?;
                    let hints = HashMap::<&str, Value<'_>>::new();
                    proxy.call(
                        "Notify",
                        &(
                            "OpenRevo",
                            replacement,
                            "open-revo",
                            &notice.message,
                            "",
                            Vec::<String>::new(),
                            hints,
                            2500i32,
                        ),
                    )
                })();
                match result {
                    Ok(id) => replacement = id,
                    Err(error) => {
                        eprintln!("Desktop notification unavailable: {error}");
                        connection = None;
                        replacement = 0;
                    }
                }
            }
        })?;
    Ok(())
}
