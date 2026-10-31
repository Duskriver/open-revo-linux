#[cfg(windows)]
mod windows_probe {
use open_revo_lib::driver::ite_lighting::IteLightingManager;

pub fn main() {
    println!("============================================================");
    println!("     OpenRevo ITE RGB Lighting Native HID Hardware Probe    ");
    println!("============================================================");

    let mgr = IteLightingManager::new();
    println!("[*] Scanning for ITE HID devices (VID=0x048D)...");
    mgr.refresh_devices();

    println!("[*] Applying Keyboard Backlight: Wave effect, Brightness 3, Speed 2, Direction 1...");
    let kb_res = mgr.apply_keyboard(true, 3, 3, 2, 0, "#6366F1");
    println!("    -> Keyboard Result: {}", if kb_res { "SUCCESS [✓]" } else { "NOT FOUND / FAILED [X]" });

    println!("[*] Applying Coastline Lightbar: Wave effect, Brightness 3, Speed 2...");
    let lb_res = mgr.apply_lightbar(true, 3, 3, 2, "#00FFFF");
    println!("    -> Lightbar Result: {}", if lb_res { "SUCCESS [✓]" } else { "NOT FOUND / FAILED [X]" });

    println!("[*] Applying A-Cover Logo: Static Cyan #00FFFF, Brightness 3...");
    let logo_res = mgr.apply_logo(true, 1, 3, "#00FFFF");
    println!("    -> A-Cover Logo Result: {}", if logo_res { "SUCCESS [✓]" } else { "NOT FOUND / FAILED [X]" });

    println!("============================================================");
    println!("Probe finished.");
}

}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::main();
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This diagnostic tool is only available on Windows. Use openrevo-probe on Linux.");
    std::process::exit(1);
}
