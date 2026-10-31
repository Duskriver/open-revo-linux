#[cfg(windows)]
mod windows_probe {
use windows::core::GUID;
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};

const NUS_SERVICE_UUID: GUID = GUID::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);
const WATER_COOLER_MAC: u64 = 0xcacbc3029718;

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OpenRevo Native BLE Water Cooler Probe ===");
    println!("Resolving device CA:CB:C3:02:97:18...");

    let op = BluetoothLEDevice::FromBluetoothAddressAsync(WATER_COOLER_MAC)?;
    let dev = op.get()?;
    println!("Device: {} (Status: {:?})", dev.Name()?, dev.ConnectionStatus()?);

    // Try Uncached discovery of GATT services
    println!("Querying NUS service (Uncached)...");
    let svc_op = dev.GetGattServicesForUuidWithCacheModeAsync(NUS_SERVICE_UUID, BluetoothCacheMode::Uncached)?;
    let svc_res = svc_op.get()?;
    println!("Services query status: {:?}", svc_res.Status()?);

    let services = svc_res.Services()?;
    println!("Found {} services", services.Size()?);

    if services.Size()? > 0 {
        let service = services.GetAt(0)?;
        let chars_op = service.GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)?;
        let chars_res = chars_op.get()?;
        println!("Chars query status: {:?}", chars_res.Status()?);
        let chars = chars_res.Characteristics()?;
        println!("Chars found: {}", chars.Size()?);
        for i in 0..chars.Size()? {
            let c = chars.GetAt(i)?;
            println!("  Char: {:?}", c.Uuid()?);
        }
    } else {
        // Try Cached fallback
        println!("Trying Cached fallback...");
        let svc_op = dev.GetGattServicesForUuidWithCacheModeAsync(NUS_SERVICE_UUID, BluetoothCacheMode::Cached)?;
        let svc_res = svc_op.get()?;
        let services = svc_res.Services()?;
        println!("Cached services: {}", services.Size()?);
    }

    Ok(())
}

}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::main()?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This diagnostic tool is only available on Windows. Use openrevo-probe on Linux.");
    std::process::exit(1);
}
