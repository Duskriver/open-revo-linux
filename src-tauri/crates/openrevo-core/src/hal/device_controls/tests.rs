use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};
struct Fake {
    bytes: RefCell<BTreeMap<u16, u8>>,
    calls: Cell<usize>,
    fail: Cell<usize>,
    outage: Cell<bool>,
    before: Cell<bool>,
    writes: Cell<usize>,
    pulses: Cell<usize>,
}
impl Fake {
    fn new() -> Self {
        Self {
            bytes: RefCell::new(BTreeMap::from([
                (ADDR_SUPPORT_BYTE1, MASK_WIN_LOCK_SUPPORT),
                (ADDR_SUPPORT_BYTE2, MASK_BATTERY_LIMIT_SUPPORT),
                (ADDR_BATTERY_LIMIT_DOWN, 75),
                (ADDR_BATTERY_LIMIT_UP, 80),
                (
                    ADDR_BATTERY_PROTECTION_MODE,
                    BATTERY_MODE_BALANCE | TOUCHPAD_BITMASK,
                ),
                (ADDR_FN_LOCK, 1),
                (ADDR_STATUS_BYTE, 0),
                (ADDR_TRIGGER_BYTE, TRIGGER_USB_CHARGE),
                (ADDR_CAMERA_CUTOFF, MASK_MODE_INDICATOR_B),
                (ADDR_AC_RECOVERY, MASK_MODE_INDICATOR_A),
            ])),
            calls: Cell::new(0),
            fail: Cell::new(0),
            outage: Cell::new(false),
            before: Cell::new(false),
            writes: Cell::new(0),
            pulses: Cell::new(0),
        }
    }
    fn tick(&self) -> bool {
        self.calls.set(self.calls.get() + 1);
        self.outage.get() || self.calls.get() == self.fail.get()
    }
}
impl EcIo for Fake {
    fn read(&self, address: u16) -> Result<u8, String> {
        if self.tick() {
            Err("lost read".into())
        } else {
            self.bytes
                .borrow()
                .get(&address)
                .copied()
                .ok_or("unknown register".into())
        }
    }
    fn write(&self, address: u16, value: u8) -> Result<(), String> {
        let fail = self.tick();
        if fail && self.before.get() {
            return Err("before write".into());
        }
        self.writes.set(self.writes.get() + 1);
        let mut bytes = self.bytes.borrow_mut();
        bytes.insert(address, value);
        if address == ADDR_TRIGGER_BYTE && value & TRIGGER_WINLOCK != 0 {
            self.pulses.set(self.pulses.get() + 1);
            *bytes.get_mut(&ADDR_STATUS_BYTE).unwrap() ^= STATUS_WINLOCK;
            bytes.insert(address, value & TRIGGER_USB_CHARGE);
        }
        assert!(
            bytes[&ADDR_BATTERY_LIMIT_DOWN] < bytes[&ADDR_BATTERY_LIMIT_UP],
            "unsafe intermediate thresholds"
        );
        if fail {
            Err("lost reply".into())
        } else {
            Ok(())
        }
    }
}
fn request(mode: BatteryMode) -> BatteryRequest {
    BatteryRequest {
        mode,
        lower: None,
        upper: None,
    }
}
#[test]
fn preset_and_custom_policy_preserve_touchpad_and_unrelated_bits() {
    for policy in [
        request(BatteryMode::Full),
        request(BatteryMode::Longevity),
        BatteryRequest {
            mode: BatteryMode::Custom,
            lower: Some(85),
            upper: Some(90),
        },
    ] {
        let io = Fake::new();
        DeviceTransaction::default().battery(&io, policy).unwrap();
        let (lower, upper, code) = policy.thresholds().unwrap();
        assert_eq!(battery_values(&io).unwrap(), (lower, upper, code));
        assert_ne!(
            io.bytes.borrow()[&ADDR_BATTERY_PROTECTION_MODE] & TOUCHPAD_BITMASK,
            0
        );
    }
}
#[test]
fn failures_before_or_after_each_transfer_restore_battery() {
    let io = Fake::new();
    DeviceTransaction::default()
        .battery(&io, request(BatteryMode::Longevity))
        .unwrap();
    let total = io.calls.get();
    for fail in 1..=total {
        for before in [true, false] {
            let io = Fake::new();
            let original = io.bytes.borrow().clone();
            io.fail.set(fail);
            io.before.set(before);
            let mut txn = DeviceTransaction::default();
            assert!(
                txn.battery(&io, request(BatteryMode::Longevity)).is_err(),
                "{fail}"
            );
            assert!(!txn.faulted());
            assert_eq!(*io.bytes.borrow(), original, "{fail}");
        }
    }
}
#[test]
fn persistent_failure_retains_backup_and_preserves_external_shared_bits() {
    let io = Fake::new();
    let mut txn = DeviceTransaction {
        pending: Some(vec![Field {
            address: ADDR_CAMERA_CUTOFF,
            mask: CAMERA_CUTOFF_BITMASK,
            value: 0,
        }]),
    };
    io.outage.set(true);
    assert!(txn.recover(&io).is_err());
    assert!(txn.faulted());
    io.outage.set(false);
    io.bytes.borrow_mut().insert(
        ADDR_CAMERA_CUTOFF,
        MASK_MODE_INDICATOR_B | CAMERA_CUTOFF_BITMASK,
    );
    txn.recover(&io).unwrap();
    assert_eq!(
        io.bytes.borrow()[&ADDR_CAMERA_CUTOFF],
        MASK_MODE_INDICATOR_B
    );
}
#[test]
fn invalid_or_undeclared_battery_never_writes() {
    for policy in [
        BatteryRequest {
            mode: BatteryMode::Custom,
            lower: Some(80),
            upper: Some(80),
        },
        BatteryRequest {
            mode: BatteryMode::Custom,
            lower: Some(0),
            upper: Some(101),
        },
        BatteryRequest {
            mode: BatteryMode::Full,
            lower: Some(50),
            upper: None,
        },
    ] {
        let io = Fake::new();
        assert!(DeviceTransaction::default().battery(&io, policy).is_err());
        assert_eq!(io.writes.get(), 0);
    }
    let io = Fake::new();
    io.bytes.borrow_mut().insert(ADDR_SUPPORT_BYTE2, 0);
    assert!(!snapshot(&io, true).battery.supported);
    assert!(DeviceTransaction::default()
        .battery(&io, request(BatteryMode::Full))
        .is_err());
    assert_eq!(io.writes.get(), 0);
}
#[test]
fn switches_preserve_mode_indicators_and_battery_policy() {
    let io = Fake::new();
    let mut txn = DeviceTransaction::default();
    for id in [
        DeviceSwitch::Camera,
        DeviceSwitch::FnLock,
        DeviceSwitch::AcRecovery,
        DeviceSwitch::UsbCharge,
        DeviceSwitch::Touchpad,
    ] {
        txn.switch(&io, id, false).unwrap();
        assert!(!switch_value(&io, id).unwrap());
        txn.switch(&io, id, true).unwrap();
        assert!(switch_value(&io, id).unwrap());
    }
    assert_ne!(
        io.bytes.borrow()[&ADDR_AC_RECOVERY] & MASK_MODE_INDICATOR_A,
        0
    );
    assert_ne!(
        io.bytes.borrow()[&ADDR_CAMERA_CUTOFF] & MASK_MODE_INDICATOR_B,
        0
    );
    assert_eq!(
        io.bytes.borrow()[&ADDR_BATTERY_PROTECTION_MODE] & MASK_BATTERY_MODE,
        BATTERY_MODE_BALANCE
    );
}
#[test]
fn win_lock_lost_reply_is_confirmed_without_duplicate_pulse() {
    let io = Fake::new();
    io.fail.set(4);
    DeviceTransaction::default()
        .switch(&io, DeviceSwitch::WinLock, true)
        .unwrap();
    assert_eq!(io.pulses.get(), 1);
    DeviceTransaction::default()
        .switch(&io, DeviceSwitch::WinLock, true)
        .unwrap();
    assert_eq!(io.pulses.get(), 1);
}

#[test]
fn switch_transfer_failures_restore_owned_bits() {
    for id in [
        DeviceSwitch::FnLock,
        DeviceSwitch::Camera,
        DeviceSwitch::AcRecovery,
        DeviceSwitch::UsbCharge,
        DeviceSwitch::Touchpad,
    ] {
        let io = Fake::new();
        let target = !switch_value(&io, id).unwrap();
        io.calls.set(0);
        DeviceTransaction::default()
            .switch(&io, id, target)
            .unwrap();
        let total = io.calls.get();
        for fail in 1..=total {
            for before in [true, false] {
                let io = Fake::new();
                let original = io.bytes.borrow().clone();
                io.fail.set(fail);
                io.before.set(before);
                let mut txn = DeviceTransaction::default();
                assert!(txn.switch(&io, id, target).is_err());
                assert!(!txn.faulted());
                assert_eq!(*io.bytes.borrow(), original);
            }
        }
    }
}
#[test]
fn lost_toggle_before_transfer_never_retries_pulse() {
    let io = Fake::new();
    io.fail.set(4);
    io.before.set(true);
    assert!(DeviceTransaction::default()
        .switch(&io, DeviceSwitch::WinLock, true)
        .is_err());
    assert_eq!(io.pulses.get(), 0);
    assert_eq!(io.writes.get(), 0);
}
