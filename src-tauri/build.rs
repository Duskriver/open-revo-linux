use std::env;

fn main() {
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux"),
        "OpenRevo supports Linux only"
    );
    tauri_build::build();
}
