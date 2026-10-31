//! Transport-independent rules extracted from the original Windows controller.
use super::vault::{mask, tongfang as values};

pub fn safe_pl_limits(pl1: u16, pl2: u16, pl4: u16, margin: u16) -> (u8, u8, u8) {
    let sustained = pl1.clamp(15, 255);
    let boost = pl2.clamp(sustained, 255);
    let peak = pl4.clamp(boost.saturating_add(margin).min(255), 255);
    (sustained as u8, boost as u8, peak as u8)
}

pub fn fan_boost_value(current: u8, enabled: bool) -> u8 {
    if enabled {
        current | mask::FAN_BOOST_CLAMP_BIT
    } else {
        current & !mask::FAN_BOOST_CLAMP_BIT
    }
}

pub fn mode_clamp(mode: u8) -> Option<u8> {
    match mode {
        1 => Some(values::OFFICE_CLAMP),
        2 => Some(values::BALANCED_CLAMP),
        3 => Some(values::TURBO_CLAMP),
        _ => None,
    }
}

pub fn decode_mode(clamp: u8) -> Option<u8> {
    (1..=3).find(|mode| mode_clamp(*mode) == Some(fan_boost_value(clamp, false)))
}

pub fn mode_index(current: u8, mode: u8) -> u8 {
    (current & values::MODE_PRESERVE) | if mode == 3 { values::MODE_TURBO } else { 0 }
}

pub fn mode_trigger(current: u8, mode: u8) -> u8 {
    (current & mask::TRIGGER_USB_CHARGE)
        | mask::TRIGGER_RGB_KB
        | if mode == 1 {
            mask::TRIGGER_SILENT_MODE
        } else {
            0
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_limits_keep_transient_headroom() {
        assert_eq!(safe_pl_limits(130, 100, 135, 20), (130, 130, 150));
        assert_eq!(safe_pl_limits(240, 250, 240, 20), (240, 250, 255));
    }
    #[test]
    fn shared_modes_preserve_unrelated_bits() {
        for mode in 1..=3 {
            let clamp = fan_boost_value(mode_clamp(mode).unwrap(), true);
            assert_eq!(decode_mode(clamp), Some(mode));
            assert_eq!(
                mode_index(0xff, mode) & values::MODE_PRESERVE,
                values::MODE_PRESERVE
            );
            assert_ne!(
                mode_trigger(mask::TRIGGER_USB_CHARGE, mode) & mask::TRIGGER_USB_CHARGE,
                0
            );
        }
    }
}
