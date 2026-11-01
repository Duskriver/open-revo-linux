//! Build-time validation and generation for the Linux hardware vault.
//! Kept in HAL so tests exercise exactly the generator used by Cargo.
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

type Constants = BTreeMap<String, u64>;

fn object<'a>(registry: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    registry
        .get(name)
        .and_then(Value::as_object)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| format!("{name}: expected a nonempty object"))
}

fn constants(registry: &Value, group: &str, min: u64, max: u64) -> Result<Constants, String> {
    object(registry, group)?
        .iter()
        .map(|(name, value)| {
            if !name.starts_with(|c: char| c.is_ascii_uppercase())
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            {
                return Err(format!(
                    "{group}.{name}: expected an uppercase constant name"
                ));
            }
            let number = value.as_u64().ok_or_else(|| {
                format!("{group}.{name}: expected an unsigned integer, got {value}")
            })?;
            if !(min..=max).contains(&number) {
                return Err(format!(
                    "{group}.{name}: value {number} outside {min}..={max}"
                ));
            }
            Ok((name.clone(), number))
        })
        .collect()
}

fn required(values: &Constants, group: &str, name: &str) -> Result<u64, String> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| format!("{group}.{name}: missing required constant"))
}

fn access_list(registry: &Value, name: &str, ec: &Constants) -> Result<BTreeSet<String>, String> {
    let list = registry
        .get(name)
        .and_then(Value::as_array)
        .filter(|list| name == "linux_ec_write" || !list.is_empty())
        .ok_or_else(|| {
            format!("{name}: expected a register-name array (read access must be nonempty)")
        })?;
    let mut seen = BTreeSet::new();
    for value in list {
        let register = value
            .as_str()
            .ok_or_else(|| format!("{name}: expected a register name"))?;
        if !ec.contains_key(register) {
            return Err(format!("{name}: unknown register {register}"));
        }
        if !seen.insert(register.to_owned()) {
            return Err(format!("{name}: duplicate register {register}"));
        }
    }
    Ok(seen)
}

fn validate_fan(values: &Constants) -> Result<(), String> {
    let get = |name| required(values, "linux_fan", name);
    let frame = get("FRAME_SIZE")?;
    let start = get("TABLE_OFFSET")?;
    let bytes = get("TABLE_BYTES")?;
    // These dimensions are the six fixed arrays of the firmware fan table.
    if get("TABLE_STEPS")? != 16 || bytes != get("TABLE_STEPS")? * 6 {
        return Err("linux_fan: table layout must contain six 16-byte arrays".into());
    }
    if frame == 0 || frame > u16::MAX.into() || start + bytes > frame {
        return Err("linux_fan: table is outside the request frame".into());
    }
    let mut occupied = BTreeSet::new();
    for name in [
        "VERSION_OFFSET",
        "ACTION_OFFSET",
        "RAMP_OFFSET",
        "ISOLATED_OFFSET",
    ] {
        let offset = get(name)?;
        if offset >= frame || (start..start + bytes).contains(&offset) || !occupied.insert(offset) {
            return Err(format!(
                "linux_fan.{name}: overlapping or out-of-frame offset"
            ));
        }
    }
    for name in [
        "VERSION",
        "APPLY",
        "RESTORE",
        "HEARTBEAT",
        "DUTY_MAX",
        "DUTY_MIN_ACTIVE",
        "CONTROL_MIN",
        "AP_ACTIVE_MASK",
        "ISOLATED_BIT",
        "RAMP_ENABLE",
        "CUSTOM_OFF",
        "CUSTOM_ON",
        "OFFICE_CLAMP",
        "BALANCED_CLAMP",
        "TURBO_CLAMP",
    ] {
        if get(name)? > u8::MAX.into() {
            return Err(format!("linux_fan.{name}: does not fit in a protocol byte"));
        }
    }
    let actions = [get("APPLY")?, get("RESTORE")?, get("HEARTBEAT")?];
    if actions.into_iter().collect::<BTreeSet<_>>().len() != actions.len() {
        return Err("linux_fan: action codes must be distinct".into());
    }
    if get("VERSION")? == 0 || get("LEASE_MS")? == 0 {
        return Err("linux_fan: version and lease must be nonzero".into());
    }
    if get("DUTY_MIN_ACTIVE")? > get("DUTY_MAX")? || get("CONTROL_MIN")? > get("DUTY_MAX")? {
        return Err("linux_fan: invalid duty limits".into());
    }
    get("REQUEST_OFFSET")?;
    Ok(())
}

fn emit(output: &mut String, name: &str, ty: &str, values: &Constants) {
    writeln!(output, "pub mod {name} {{").unwrap();
    for (key, value) in values {
        writeln!(output, "    pub const {key}: {ty} = {value};").unwrap();
    }
    writeln!(output, "}}\n").unwrap();
}

fn validate_control(values: &Constants) -> Result<(), String> {
    let get = |name| required(values, "linux_control", name);
    for (name, expected) in [
        ("FRAME_SIZE", 8),
        ("VERSION_OFFSET", 0),
        ("ACTION_OFFSET", 1),
        ("ARGUMENT_OFFSET", 2),
        ("RESERVED_OFFSET", 5),
    ] {
        if get(name)? != expected {
            return Err(format!(
                "linux_control.{name}: incompatible control frame layout"
            ));
        }
    }
    for name in [
        "VERSION",
        "MODE",
        "POWER",
        "BOOST",
        "RECOVER",
        "AP_PRESENT",
        "MODE_PRESERVE",
        "MODE_TURBO",
        "RAMP_SMOOTH",
        "MIN_POWER",
        "POWER_INVALID",
        "PL4_MARGIN",
    ] {
        if get(name)? > u8::MAX.into() {
            return Err(format!(
                "linux_control.{name}: does not fit in a protocol byte"
            ));
        }
    }
    let actions = [get("MODE")?, get("POWER")?, get("BOOST")?, get("RECOVER")?];
    if actions.into_iter().collect::<BTreeSet<_>>().len() != actions.len() {
        return Err("linux_control: action codes must be distinct".into());
    }
    if get("VERSION")? == 0
        || get("MIN_POWER")? == 0
        || get("PL4_MARGIN")? == 0
        || get("MIN_POWER")? + get("PL4_MARGIN")? >= get("POWER_INVALID")?
    {
        return Err("linux_control: invalid version or power bounds".into());
    }
    get("REQUEST_OFFSET")?;
    Ok(())
}

pub fn generate(source: &str) -> Result<String, String> {
    let registry: Value = serde_json::from_str(source).map_err(|error| format!("JSON: {error}"))?;
    let ec = constants(&registry, "ec", 1, u16::MAX.into())?;
    let mask = constants(&registry, "mask", 0, u8::MAX.into())?;
    // Protocol values fit both supported userspace usize and kernel u32.
    let fan = constants(&registry, "linux_fan", 0, u32::MAX.into())?;
    validate_fan(&fan)?;
    let control = constants(&registry, "linux_control", 0, u32::MAX.into())?;
    validate_control(&control)?;
    let fan_request = required(&fan, "linux_fan", "REQUEST_OFFSET")?;
    let control_request = required(&control, "linux_control", "REQUEST_OFFSET")?;
    if fan_request == control_request
        || ec
            .values()
            .any(|address| [fan_request, control_request].contains(address))
    {
        return Err(
            "request offsets must be distinct from each other and physical registers".into(),
        );
    }
    let readable = access_list(&registry, "linux_ec_read", &ec)?;
    let writable = access_list(&registry, "linux_ec_write", &ec)?;
    if !writable.is_subset(&readable) {
        return Err("linux_ec_write: every writable register must permit readback".into());
    }
    if !writable.is_empty() {
        return Err("linux_ec_write: raw writes are disabled; use semantic control requests".into());
    }
    let mut output = String::from("// Generated from config/ec_registry.json. Do not edit.\n\n");
    emit(&mut output, "ec", "u16", &ec);
    emit(&mut output, "mask", "u8", &mask);
    emit(&mut output, "linux_fan", "usize", &fan);
    emit(&mut output, "linux_control", "usize", &control);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Value {
        serde_json::from_str(include_str!("../../../../config/ec_registry.json")).unwrap()
    }

    fn error(value: &Value) -> String {
        generate(&value.to_string()).expect_err("invalid registry must fail closed")
    }

    #[test]
    fn current_registry_generates_linux_values_without_mock_fallback() {
        let registry = registry();
        let output = generate(&registry.to_string()).unwrap();
        for (group, ty) in [
            ("ec", "u16"),
            ("mask", "u8"),
            ("linux_fan", "usize"),
            ("linux_control", "usize"),
        ] {
            for (name, value) in registry[group].as_object().unwrap() {
                assert!(output.contains(&format!("pub const {name}: {ty} = {value};")));
            }
        }
        assert!(!output.contains("pub mod driver"));
        assert!(!output.contains("deobf"));
    }

    #[test]
    fn invalid_ec_values_never_become_zero_or_truncated_addresses() {
        for invalid in [
            Value::Null,
            Value::Bool(true),
            Value::from("123"),
            Value::from(-1),
            Value::from(1.5),
            Value::from(0),
            Value::from(u64::from(u16::MAX) + 1),
        ] {
            let mut value = registry();
            value["ec"]["ADDR_PROJECT_ID"] = invalid;
            assert!(error(&value).contains("ec.ADDR_PROJECT_ID"));
        }
    }

    #[test]
    fn byte_masks_and_protocol_values_cannot_overflow() {
        let mut value = registry();
        value["mask"]["POWER_STATUS_DC_JACK"] = Value::from(u16::from(u8::MAX) + 1);
        assert!(error(&value).contains("mask.POWER_STATUS_DC_JACK"));
        value = registry();
        value["linux_fan"]["VERSION"] = Value::from(u16::from(u8::MAX) + 1);
        assert!(error(&value).contains("linux_fan.VERSION"));
        value = registry();
        value["linux_fan"]["LEASE_MS"] = Value::from(u64::from(u32::MAX) + 1);
        assert!(error(&value).contains("linux_fan.LEASE_MS"));
    }

    #[test]
    fn missing_or_malformed_sections_fail_closed() {
        for group in [
            "ec",
            "mask",
            "linux_fan",
            "linux_control",
            "linux_ec_read",
            "linux_ec_write",
        ] {
            let mut value = registry();
            value.as_object_mut().unwrap().remove(group);
            assert!(error(&value).contains(group));
        }
        assert!(generate("{").unwrap_err().starts_with("JSON:"));
        let mut value = registry();
        value["linux_fan"]
            .as_object_mut()
            .unwrap()
            .remove("RAMP_OFFSET");
        assert!(error(&value).contains("RAMP_OFFSET"));
    }

    #[test]
    fn unsafe_frame_layout_is_rejected() {
        for field in ["FRAME_SIZE", "TABLE_STEPS", "TABLE_BYTES"] {
            let mut value = registry();
            value["linux_fan"][field] = Value::from(0);
            assert!(error(&value).contains("linux_fan"));
        }
        let mut value = registry();
        value["linux_fan"]["RAMP_OFFSET"] = value["linux_fan"]["TABLE_OFFSET"].clone();
        assert!(error(&value).contains("RAMP_OFFSET"));
        value = registry();
        value["linux_fan"]["RESTORE"] = value["linux_fan"]["APPLY"].clone();
        assert!(error(&value).contains("distinct"));
    }

    #[test]
    fn access_lists_must_reference_unique_readable_registers() {
        let mut value = registry();
        value["linux_ec_write"] = serde_json::json!(["ADDR_NOT_REGISTERED"]);
        assert!(error(&value).contains("unknown register"));
        value = registry();
        let register = value["linux_ec_read"][0].clone();
        value["linux_ec_write"] = serde_json::json!([register, register]);
        assert!(error(&value).contains("duplicate register"));
        value = registry();
        let write = value["linux_ec_read"][0].clone();
        value["linux_ec_write"] = serde_json::json!([write]);
        value["linux_ec_read"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry != &write);
        assert!(error(&value).contains("permit readback"));
        value = registry();
        value["linux_ec_write"] = serde_json::json!([]);
        assert!(generate(&value.to_string()).is_ok());
        value["linux_ec_write"] = serde_json::json!([value["linux_ec_read"][0]]);
        assert!(error(&value).contains("raw writes are disabled"));
    }

    #[test]
    fn invalid_constant_identifiers_are_rejected() {
        let mut value = registry();
        value["ec"]["ADDR_BAD;CODE"] = Value::from(1);
        assert!(error(&value).contains("uppercase constant name"));
    }

    #[test]
    fn control_protocol_rejects_overflow_overlap_and_incompatible_layout() {
        let mut value = registry();
        value["linux_control"]["MIN_POWER"] = Value::from(u16::from(u8::MAX) + 1);
        assert!(error(&value).contains("MIN_POWER"));
        value = registry();
        value["linux_control"]["ARGUMENT_OFFSET"] = Value::from(0);
        assert!(error(&value).contains("ARGUMENT_OFFSET"));
        value = registry();
        value["linux_control"]["POWER"] = value["linux_control"]["MODE"].clone();
        assert!(error(&value).contains("distinct"));
        value = registry();
        value["linux_control"]["REQUEST_OFFSET"] = value["linux_fan"]["REQUEST_OFFSET"].clone();
        assert!(error(&value).contains("request offsets"));
    }
}
