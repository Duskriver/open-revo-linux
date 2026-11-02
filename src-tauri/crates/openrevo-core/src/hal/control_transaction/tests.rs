use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

struct Fake {
    bytes: RefCell<BTreeMap<u16, u8>>,
    calls: Cell<usize>,
    writes: Cell<usize>,
    fail: Cell<usize>,
    fail_before: Cell<bool>,
    fail_from: Cell<usize>,
    consume: Cell<bool>,
    corrupt_commit: Cell<bool>,
    corrupt_request: Cell<bool>,
    unsafe_tuple: Cell<bool>,
}
impl Fake {
    fn new(custom: bool) -> Self {
        let mut bytes = BTreeMap::new();
        for address in FIELDS {
            bytes.insert(address, 0);
        }
        for (addresses, watts) in [
            (
                [ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4],
                [35, 35, 70],
            ),
            (
                [ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4],
                [65, 65, 100],
            ),
            (
                [ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4],
                [90, 90, 100],
            ),
        ] {
            for (address, value) in addresses.into_iter().zip(watts) {
                bytes.insert(address, value);
            }
        }
        bytes.extend([
            (ADDR_FAN_BOOST_CLAMP, values::OFFICE_CLAMP),
            (
                ADDR_CUSTOM_MODE,
                if custom {
                    values::CUSTOM_ON
                } else {
                    values::CUSTOM_OFF
                },
            ),
            (ADDR_CUSTOM_PL1, 35),
            (ADDR_CUSTOM_PL2, 45),
            (ADDR_CUSTOM_PL4, 70),
            (ADDR_TRIGGER_BYTE, TRIGGER_USB_CHARGE),
            (ADDR_MODE_INDEX, values::MODE_PRESERVE),
            (ADDR_AC_RECOVERY, AC_RECOVERY_BITMASK),
            (ADDR_CAMERA_CUTOFF, CAMERA_CUTOFF_BITMASK),
            (ADDR_POWER_STATUS, POWER_STATUS_DC_JACK),
        ]);
        Self {
            bytes: RefCell::new(bytes),
            calls: Cell::new(0),
            writes: Cell::new(0),
            fail: Cell::new(0),
            fail_before: Cell::new(false),
            fail_from: Cell::new(0),
            consume: Cell::new(false),
            corrupt_commit: Cell::new(false),
            corrupt_request: Cell::new(false),
            unsafe_tuple: Cell::new(false),
        }
    }
    fn begin(&self) -> Result<bool, String> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        let fail =
            call == self.fail.get() || (self.fail_from.get() > 0 && call >= self.fail_from.get());
        if fail && self.fail_before.get() {
            Err("injected before transfer".into())
        } else {
            Ok(fail)
        }
    }
    fn same(&self, before: &Saved) {
        for (index, address) in FIELDS.iter().enumerate() {
            assert_eq!(
                self.bytes.borrow()[address],
                before.values[index],
                "field {index}, call {}",
                self.fail.get()
            );
        }
        assert_eq!(
            self.bytes.borrow()[&ADDR_TRIGGER_BYTE] & TRIGGER_USB_CHARGE,
            before.trigger & TRIGGER_USB_CHARGE
        );
        assert!(!self.unsafe_tuple.get());
    }
}
impl EcIo for Fake {
    fn read(&self, address: u16) -> Result<u8, String> {
        if self.begin()? {
            return Err("injected read failure".into());
        }
        Ok(*self.bytes.borrow().get(&address).unwrap_or(&0))
    }
    fn write(&self, address: u16, value: u8) -> Result<(), String> {
        let fail = self.begin()?;
        assert!(crate::hal::vault::linux_access::WRITABLE.contains(&address));
        self.writes.set(self.writes.get() + 1);
        let mut bytes = self.bytes.borrow_mut();
        bytes.insert(address, value);
        if address == ADDR_CUSTOM_MODE
            && value == values::CUSTOM_ON
            && (bytes[&ADDR_CUSTOM_PL1] < policy::MIN_POWER
                || bytes[&ADDR_CUSTOM_PL1] > bytes[&ADDR_CUSTOM_PL2]
                || bytes[&ADDR_CUSTOM_PL2] > bytes[&ADDR_CUSTOM_PL4])
        {
            self.unsafe_tuple.set(true);
        }
        if address == ADDR_TRIGGER_BYTE {
            bytes.insert(address, value & TRIGGER_USB_CHARGE);
            if self.consume.get() {
                bytes.insert(ADDR_MODE_REQUEST, policy::MODE_STATUS_IDLE);
            }
            if self.corrupt_request.replace(false) {
                bytes.insert(ADDR_MODE_REQUEST, policy::MODE_STATUS_REQUEST + 1);
            }
            if self.corrupt_commit.replace(false) {
                *bytes.get_mut(&ADDR_CUSTOM_PL1).unwrap() += 1;
            }
        }
        if fail {
            Err("injected lost reply".into())
        } else {
            Ok(())
        }
    }
}
fn request() -> Setting {
    Setting::Power(CpuLimits {
        pl1: 45,
        pl2: 55,
        pl4: 80,
    })
}

#[test]
fn every_transport_failure_before_or_after_transfer_restores_original() {
    let mut injections = 0;
    for custom in [false, true] {
        for setting in [
            Setting::Mode(1),
            Setting::Mode(2),
            Setting::Mode(3),
            request(),
        ] {
            let io = Fake::new(custom);
            let mut txn = ControlTransaction::default();
            txn.apply(&io, setting, || true).unwrap();
            let total = io.calls.get();
            for fail in 1..=total {
                for before_transfer in [false, true] {
                    let io = Fake::new(custom);
                    let before = Saved::capture(&io).unwrap();
                    io.calls.set(0);
                    io.fail.set(fail);
                    io.fail_before.set(before_transfer);
                    let mut txn = ControlTransaction::default();
                    assert!(txn.apply(&io, setting, || true).is_err(), "{fail}");
                    assert!(!txn.faulted(), "{fail}");
                    io.same(&before);
                    injections += 1;
                }
            }
        }
    }
    assert!(injections > 500);
}

#[test]
fn persistent_failure_blocks_actions_and_recovery_can_retry() {
    let io = Fake::new(true);
    let before = Saved::capture(&io).unwrap();
    io.calls.set(0);
    io.fail_from.set(25);
    io.fail_before.set(true);
    let mut txn = ControlTransaction::default();
    assert!(txn.apply(&io, request(), || true).is_err());
    assert!(txn.faulted());
    let writes = io.writes.get();
    assert!(txn.apply(&io, Setting::Mode(1), || true).is_err());
    assert_eq!(writes, io.writes.get());
    io.fail_from.set(0);
    txn.recover(&io).unwrap();
    assert!(!txn.faulted());
    io.same(&before);
}

#[test]
fn invalid_requests_and_power_source_do_not_write() {
    for setting in [
        Setting::Mode(0),
        Setting::Mode(4),
        Setting::Power(CpuLimits {
            pl1: 0,
            pl2: 55,
            pl4: 80,
        }),
        Setting::Power(CpuLimits {
            pl1: 90,
            pl2: 55,
            pl4: 80,
        }),
        Setting::Power(CpuLimits {
            pl1: 45,
            pl2: 55,
            pl4: 255,
        }),
    ] {
        let io = Fake::new(false);
        assert!(ControlTransaction::default()
            .apply(&io, setting, || true)
            .is_err());
        assert_eq!(io.writes.get(), 0);
    }
    for dc in [true, false] {
        let io = Fake::new(false);
        if !dc {
            io.bytes.borrow_mut().insert(ADDR_POWER_STATUS, 0);
        }
        assert!(ControlTransaction::default()
            .apply(&io, request(), || !dc)
            .is_err());
        assert_eq!(io.writes.get(), 0);
    }
    let io = Fake::new(false);
    io.bytes.borrow_mut().insert(ADDR_TURBO_PL4, 0);
    assert!(ControlTransaction::default()
        .apply(&io, request(), || true)
        .is_err());
    assert_eq!(io.writes.get(), 0);
}

#[test]
fn power_loss_before_and_after_commit_rolls_back() {
    for power_check in [2, 3] {
        let io = Fake::new(true);
        let before = Saved::capture(&io).unwrap();
        let mut checks = 0;
        let mut txn = ControlTransaction::default();
        assert!(txn
            .apply(&io, request(), || {
                checks += 1;
                checks != power_check
            })
            .is_err());
        assert!(!txn.faulted());
        io.same(&before);
    }
}

#[test]
fn only_expected_firmware_request_consumption_is_accepted() {
    let io = Fake::new(false);
    io.consume.set(true);
    ControlTransaction::default()
        .apply(&io, Setting::Mode(1), || false)
        .unwrap();
    assert_eq!(
        io.bytes.borrow()[&ADDR_MODE_REQUEST],
        policy::MODE_STATUS_IDLE
    );
    for request_corruption in [false, true] {
        let io = Fake::new(true);
        let before = Saved::capture(&io).unwrap();
        io.corrupt_commit.set(!request_corruption);
        io.corrupt_request.set(request_corruption);
        assert!(ControlTransaction::default()
            .apply(&io, Setting::Mode(1), || true)
            .is_err());
        io.same(&before);
    }
}

#[test]
fn power_and_mode_preserve_shared_bits_and_enable_safe_tuples() {
    for setting in [Setting::Mode(3), request()] {
        let io = Fake::new(true);
        ControlTransaction::default()
            .apply(&io, setting, || true)
            .unwrap();
        let bytes = io.bytes.borrow();
        assert_ne!(bytes[&ADDR_MODE_INDEX] & values::MODE_PRESERVE, 0);
        assert_ne!(bytes[&ADDR_TRIGGER_BYTE] & TRIGGER_USB_CHARGE, 0);
        assert_ne!(bytes[&ADDR_AC_RECOVERY] & AC_RECOVERY_BITMASK, 0);
        assert_ne!(bytes[&ADDR_CAMERA_CUTOFF] & CAMERA_CUTOFF_BITMASK, 0);
        assert!(!io.unsafe_tuple.get());
    }
}

#[test]
fn boost_restores_a_write_with_a_lost_reply() {
    let io = Fake::new(false);
    io.fail.set(2);
    assert!(boost(&io, true).is_err());
    assert_eq!(
        io.bytes.borrow()[&ADDR_FAN_BOOST_CLAMP],
        values::OFFICE_CLAMP
    );
}

#[test]
fn every_rollback_failure_keeps_backup_for_retry() {
    for custom in [false, true] {
        let prepare = || {
            let io = Fake::new(custom);
            let original = Saved::capture(&io).unwrap();
            io.bytes
                .borrow_mut()
                .insert(ADDR_CUSTOM_MODE, values::CUSTOM_OFF);
            io.bytes.borrow_mut().insert(ADDR_CUSTOM_PL4, 100);
            io.calls.set(0);
            (io, original)
        };
        let (io, original) = prepare();
        ControlTransaction {
            pending: Some(original),
        }
        .recover(&io)
        .unwrap();
        let total = io.calls.get();
        for fail in 1..=total {
            for before_transfer in [false, true] {
                let (io, original) = prepare();
                let mut txn = ControlTransaction {
                    pending: Some(original),
                };
                io.fail.set(fail);
                io.fail_before.set(before_transfer);
                assert!(txn.recover(&io).is_err(), "rollback call {fail}");
                assert!(txn.faulted());
                io.fail.set(0);
                txn.recover(&io).unwrap();
                assert!(!txn.faulted());
                io.same(&original);
            }
        }
    }
}
