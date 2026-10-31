#[cfg(target_os = "linux")]
fn main() {
    let mut backend = openrevo_core::linux::LinuxBackend::new();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.is_empty() && args != ["--verify-current-profile"] {
        eprintln!("Usage: openrevo-probe [--verify-current-profile]");
        std::process::exit(2);
    }
    // The first sample establishes CPU accounting; no hardware writes occur.
    let initial = backend.snapshot();
    if !args.is_empty() {
        let result = initial
            .power
            .active
            .as_deref()
            .ok_or_else(|| "No active profile".to_owned())
            .and_then(|profile| backend.set_power_profile(profile));
        if let Err(error) = result {
            eprintln!("Profile verification failed: {error}");
            std::process::exit(1);
        }
        eprintln!("Current power profile write and readback verified.");
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    println!(
        "{}",
        serde_json::to_string_pretty(&backend.snapshot()).unwrap()
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("openrevo-probe is available on Linux only");
    std::process::exit(1);
}
