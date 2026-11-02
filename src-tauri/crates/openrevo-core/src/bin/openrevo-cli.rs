//! A session-bus client of the running desktop owner. Never opens hardware.
use openrevo_core::{
    actions::{Action, BatteryMode, BatteryRequest, DeviceSwitch, PowerMode},
    control_bus::call_control,
};
use serde_json::{json, Value};
use std::process::ExitCode;

const HELP: &str = "OpenRevo CLI — controls the running desktop runtime\n\n\
Usage: openrevo-cli [--json] COMMAND\n\n\
  status                         Live hardware status\n\
  config                         Saved settings\n\
  mode office|balanced|turbo|custom\n\
  fan auto|boost                 Automatic cooling or maximum cooling\n\
  battery full|balanced|longevity or battery custom LOWER UPPER\n\
  switch win_lock|fn_lock|camera|usb_charge|ac_recovery|touchpad on|off\n\
  hotkeys on|off                 Physical mode key linkage\n\
  battery DEVICE LIMIT           Charge limit percentage\n\
  brightness DEVICE PERCENT      Display or keyboard brightness\n\
  cpu PL1 PL2 PL4                 Custom CPU limits in watts\n\
  release-controls               Restore temporary fan controls\n\
  --help / --version\n\n\
The desktop may be hidden in the tray; its Rust runtime owns control leases.\n\
Commands never start a second hardware owner. After a timeout, inspect status.\n\
Exit codes: 0 success; 1 operation rejected; 2 invalid input; 3 transport error.\n";

#[derive(Debug, PartialEq)]
enum Input {
    Help,
    Version,
    Request(Value),
}

fn parse(args: &[String]) -> Result<(bool, Input), String> {
    let (json_output, args) = if args.first().is_some_and(|arg| arg == "--json") {
        (true, &args[1..])
    } else {
        (false, args)
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let action = |action: Action| Input::Request(json!({"kind":"action", "action": action}));
    let byte = |text: &str| {
        text.parse::<u8>()
            .map_err(|_| "Expected an integer from 0 to 255".to_string())
    };
    let percent = |text: &str| -> Result<u8, String> {
        let n = byte(text)?;
        if n > 100 {
            Err("Percentage exceeds 100".into())
        } else {
            Ok(n)
        }
    };
    let input = match words.as_slice() {
        [] | ["--help"] | ["-h"] => Input::Help,
        ["--version"] => Input::Version,
        ["status"] => Input::Request(json!({"kind":"status"})),
        ["config"] => Input::Request(json!({"kind":"config"})),
        ["release-controls"] => action(Action::ReleaseControls {}),
        ["mode", mode] => action(Action::Mode {
            mode: match *mode {
                "office" => PowerMode::Office,
                "balanced" => PowerMode::Balanced,
                "turbo" => PowerMode::Turbo,
                "custom" => PowerMode::Custom,
                _ => return Err("Expected office, balanced, turbo or custom".into()),
            },
        }),
        ["fan", mode @ ("auto" | "boost")] => action(Action::Fan {
            enabled: *mode == "boost",
        }),
        ["battery", preset @ ("full" | "balanced" | "longevity")] => action(Action::Battery {
            policy: BatteryRequest {
                mode: match *preset {
                    "full" => BatteryMode::Full,
                    "balanced" => BatteryMode::Balanced,
                    _ => BatteryMode::Longevity,
                },
                lower: None,
                upper: None,
            },
        }),
        ["battery", "custom", lower, upper] => {
            let policy = BatteryRequest {
                mode: BatteryMode::Custom,
                lower: Some(percent(lower)?),
                upper: Some(percent(upper)?),
            };
            policy.thresholds()?;
            action(Action::Battery { policy })
        }
        ["switch", id, state @ ("on" | "off")] => action(Action::Switch {
            switch: match *id {
                "win_lock" => DeviceSwitch::WinLock,
                "fn_lock" => DeviceSwitch::FnLock,
                "camera" => DeviceSwitch::Camera,
                "usb_charge" => DeviceSwitch::UsbCharge,
                "ac_recovery" => DeviceSwitch::AcRecovery,
                "touchpad" => DeviceSwitch::Touchpad,
                _ => return Err("Unknown device switch".into()),
            },
            enabled: *state == "on",
        }),
        ["hotkeys", state @ ("on" | "off")] => action(Action::Hotkeys {
            enabled: *state == "on",
        }),
        ["battery", device, limit] => action(Action::BatteryLimit {
            battery: (*device).into(),
            limit: percent(limit)?,
        }),
        ["brightness", device, value] => action(Action::Backlight {
            device: (*device).into(),
            percent: percent(value)?,
        }),
        ["cpu", pl1, pl2, pl4] => {
            let limits = openrevo_core::hal::linux_ec::CpuLimits {
                pl1: byte(pl1)?,
                pl2: byte(pl2)?,
                pl4: byte(pl4)?,
            };
            if limits.pl1 < 15
                || limits.pl1 > limits.pl2
                || u16::from(limits.pl4) < u16::from(limits.pl2) + 20
            {
                return Err("Expected PL1 >= 15, PL1 <= PL2 and PL4 >= PL2 + 20".into());
            }
            action(Action::CpuLimits { limits })
        }
        _ => return Err("Unknown command or arguments; run openrevo-cli --help".into()),
    };
    Ok((json_output, input))
}

fn print_error(json_output: bool, code: &str, message: &str) {
    if json_output {
        println!(
            "{}",
            json!({"version":1,"ok":false,"result":null,
            "error":{"code":code,"message":message,"details":null}})
        );
    } else {
        eprintln!("{code}: {message}");
    }
}

fn show_response(response: &str, json_output: bool) -> Result<u8, String> {
    let value: Value = serde_json::from_str(response)
        .map_err(|error| format!("Invalid desktop response: {error}"))?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("ok").and_then(Value::as_bool).is_none()
    {
        return Err("Unsupported desktop response; update the desktop and CLI together".into());
    }
    let ok = value["ok"] == true;
    if json_output {
        println!("{}", serde_json::to_string(&value).unwrap());
    } else if ok {
        println!(
            "{}",
            serde_json::to_string_pretty(&value["result"]).unwrap()
        );
    } else {
        eprintln!(
            "{}: {}",
            value["error"]["code"]
                .as_str()
                .unwrap_or("operation_failed"),
            value["error"]["message"]
                .as_str()
                .unwrap_or("Desktop rejected the operation")
        );
        if !value["error"]["details"].is_null() {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&value["error"]["details"]).unwrap()
            );
        }
    }
    Ok(u8::from(!ok))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json_requested = args.first().is_some_and(|arg| arg == "--json");
    let (json_output, input) = match parse(&args) {
        Ok(input) => input,
        Err(message) => {
            print_error(json_requested, "invalid_input", &message);
            return ExitCode::from(2);
        }
    };
    let command = match input {
        Input::Help => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Input::Version => {
            println!("openrevo-cli {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Input::Request(command) => command,
    };
    let request = json!({"version":1,"command":command}).to_string();
    match call_control(&request).and_then(|response| show_response(&response, json_output)) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            print_error(json_output, "transport_error", &message);
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn direct_commands_are_typed_and_reject_invalid_inputs() {
        assert_eq!(
            parse(&args(&["--json", "mode", "custom"])).unwrap(),
            (
                true,
                Input::Request(json!({"kind":"action","action":{"kind":"mode","mode":"custom"}}))
            )
        );
        for command in [
            vec!["scene", "list"],
            vec!["fan", "invalid"],
            vec!["battery", "BAT0", "101"],
            vec!["mode", "turbo", "extra"],
            vec!["cpu", "40", "50", "60"],
            vec!["cpu", "-1", "50", "90"],
        ] {
            assert!(parse(&args(&command)).is_err());
        }
        for command in [
            vec!["battery", "full"],
            vec!["battery", "custom", "70", "80"],
            vec!["switch", "fn_lock", "on"],
            vec!["hotkeys", "off"],
        ] {
            assert!(parse(&args(&command)).is_ok());
        }
        for command in [
            vec!["battery", "custom", "80", "70"],
            vec!["switch", "unknown", "on"],
            vec!["hotkeys", "maybe"],
        ] {
            assert!(parse(&args(&command)).is_err());
        }
        assert!(parse(&args(&["cpu", "35", "45", "70"])).is_ok());
        assert_eq!(
            show_response(
                r#"{"version":1,"ok":false,"error":{"code":"control_failed"}}"#,
                true
            )
            .unwrap(),
            1
        );
        assert!(show_response(r#"{"version":2,"ok":true}"#, true).is_err());
    }
}
