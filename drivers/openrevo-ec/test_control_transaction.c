/* Hardware-free tests of the semantic control implementation used by Linux. */
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
typedef uint8_t u8;
typedef uint16_t u16;
#include "vault_generated.h"
static u8 fake_ec[UINT16_MAX + 1];
static size_t calls, writes, fail_at, fail_before_at, fail_from, unplug_at;
static u16 corrupt_address;
static bool corrupt_once, supplied, unsafe_custom, corrupt_trigger_once;
static bool mode_trigger_clears_immediately, mode_trigger_clears_on_commit;
static bool mode_trigger_corrupts_on_commit_once, mode_trigger_read_fails_after_commit_once;
static bool mode_trigger_read_failure_armed;
static bool boost_owned;
static u8 boost_original, boost_last;
static bool ec_stopping;
static size_t queued_watchdogs;
static void queue_watchdog(void) { queued_watchdogs++; }
struct diagnostic {
    const char *phase, *step;
    u16 address;
    int error, expected, actual;
};
static struct diagnostic diagnostics[1024];
static size_t diagnostic_count;
static void ec_report_failure(const char *phase, const char *step, u16 address,
                              int error, int expected, int actual)
{
    assert(diagnostic_count < sizeof(diagnostics) / sizeof(diagnostics[0]));
    diagnostics[diagnostic_count++] = (struct diagnostic) {
        phase, step, address, error, expected, actual
    };
}
static bool system_power(void) { return supplied; }
static int transfer(u16 address, u8 *value, bool write)
{
    calls++;
    if (calls == unplug_at)
        fake_ec[ADDR_POWER_STATUS] = 0;
    if (calls == fail_before_at || (fail_from && calls >= fail_from))
        return -EIO;
    if (write) {
        fake_ec[address] = *value;
        writes++;
        if (address == ADDR_TURBO_TRIGGER_1115 && mode_trigger_clears_immediately)
            fake_ec[address] = OR_CONTROL_MODE_STATUS_IDLE;
        if (address == ADDR_CUSTOM_MODE && *value == OR_FAN_CUSTOM_ON &&
            (fake_ec[ADDR_CUSTOM_PL1] < OR_CONTROL_MIN_POWER ||
             fake_ec[ADDR_CUSTOM_PL2] < fake_ec[ADDR_CUSTOM_PL1] ||
             fake_ec[ADDR_CUSTOM_PL4] < fake_ec[ADDR_CUSTOM_PL2]))
            unsafe_custom = true;
        if (address == corrupt_address && corrupt_once) {
            fake_ec[address]++;
            corrupt_once = false;
        }
        /* Firmware consumes the command, but retains USB charging policy. */
        if (address == ADDR_TRIGGER_BYTE) {
            fake_ec[address] &= TRIGGER_USB_CHARGE;
            if (mode_trigger_clears_on_commit)
                fake_ec[ADDR_TURBO_TRIGGER_1115] = OR_CONTROL_MODE_STATUS_IDLE;
            if (mode_trigger_corrupts_on_commit_once) {
                fake_ec[ADDR_TURBO_TRIGGER_1115] = OR_CONTROL_MODE_STATUS_REQUEST + 1;
                mode_trigger_corrupts_on_commit_once = false;
            }
            if (mode_trigger_read_fails_after_commit_once) {
                mode_trigger_read_failure_armed = true;
                mode_trigger_read_fails_after_commit_once = false;
            }
            if (corrupt_trigger_once) {
                fake_ec[ADDR_CUSTOM_PL1]++;
                corrupt_trigger_once = false;
            }
        }
    } else {
        if (address == ADDR_TURBO_TRIGGER_1115 && mode_trigger_read_failure_armed) {
            mode_trigger_read_failure_armed = false;
            return -EIO;
        }
        *value = fake_ec[address];
    }
    return calls == fail_at ? -EIO : 0;
}
#include "fan_transaction.h"
#include "control_transaction.h"

static void setup(u8 *frame, u8 action, bool custom)
{
    static const u16 presets[][3] = {
        { ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4 },
        { ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4 },
        { ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4 },
    };
    static const u8 watts[][3] = { {35, 35, 70}, {65, 65, 100}, {90, 90, 100} };
    size_t mode, i;
    memset(fake_ec, 0, sizeof(fake_ec));
    memset(frame, 0, OR_CONTROL_FRAME_SIZE);
    calls = writes = fail_at = fail_before_at = fail_from = unplug_at = 0;
    corrupt_once = unsafe_custom = corrupt_trigger_once = false;
    mode_trigger_clears_immediately = mode_trigger_clears_on_commit = false;
    mode_trigger_corrupts_on_commit_once = mode_trigger_read_fails_after_commit_once = false;
    mode_trigger_read_failure_armed = false;
    diagnostic_count = 0;
    supplied = true;
    fan_curve_active = fan_curve_fault = false;
    control_pending = control_fault = boost_owned = boost_release_pending = false;
    ec_stopping = false;
    queued_watchdogs = 0;
    fake_ec[ADDR_FAN_BOOST_CLAMP] = OR_FAN_OFFICE_CLAMP;
    fake_ec[ADDR_POWER_STATUS] = POWER_STATUS_DC_JACK;
    fake_ec[ADDR_CUSTOM_MODE] = custom ? OR_FAN_CUSTOM_ON : OR_FAN_CUSTOM_OFF;
    fake_ec[ADDR_CUSTOM_PL1] = 35;
    fake_ec[ADDR_CUSTOM_PL2] = 45;
    fake_ec[ADDR_CUSTOM_PL4] = 70;
    fake_ec[ADDR_TRIGGER_BYTE] = TRIGGER_USB_CHARGE;
    fake_ec[ADDR_MODE_INDEX] = OR_CONTROL_MODE_PRESERVE;
    fake_ec[ADDR_AC_RECOVERY] = AC_RECOVERY_BITMASK;
    fake_ec[ADDR_CAMERA_CUTOFF] = CAMERA_CUTOFF_BITMASK;
    for (mode = 0; mode < 3; mode++)
        for (i = 0; i < 3; i++)
            fake_ec[presets[mode][i]] = watts[mode][i];
    frame[OR_CONTROL_VERSION_OFFSET] = OR_CONTROL_VERSION;
    frame[OR_CONTROL_ACTION_OFFSET] = action;
    if (action == OR_CONTROL_MODE)
        frame[OR_CONTROL_ARGUMENT_OFFSET] = 3;
    else if (action == OR_CONTROL_POWER) {
        frame[OR_CONTROL_ARGUMENT_OFFSET] = 45;
        frame[OR_CONTROL_ARGUMENT_OFFSET + 1] = 55;
        frame[OR_CONTROL_ARGUMENT_OFFSET + 2] = 80;
    } else if (action == OR_CONTROL_BOOST)
        frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
}

static void same(const struct control_saved *saved)
{
    size_t i;
    for (i = 0; i < CONTROL_FIELDS; i++)
        assert(fake_ec[control_addresses[i]] == saved->values[i]);
    assert((fake_ec[ADDR_TRIGGER_BYTE] & TRIGGER_USB_CHARGE) ==
           (saved->trigger & TRIGGER_USB_CHARGE));
    assert(!control_pending && !control_fault && !unsafe_custom);
}

static size_t test_every_failure(u8 action, bool custom)
{
    struct control_saved before;
    u8 frame[OR_CONTROL_FRAME_SIZE];
    size_t total, i;
    setup(frame, action, custom);
    assert(!control_capture(&before));
    calls = 0;
    assert(!control_apply(frame));
    total = calls;
    assert(!control_pending && !control_fault && !unsafe_custom);
    assert(fake_ec[ADDR_MODE_INDEX] & OR_CONTROL_MODE_PRESERVE);
    assert(fake_ec[ADDR_TRIGGER_BYTE] & TRIGGER_USB_CHARGE);
    if (action == OR_CONTROL_MODE) {
        assert(control_mode(fake_ec[ADDR_FAN_BOOST_CLAMP]) == 3);
        assert(fake_ec[ADDR_CUSTOM_MODE] == OR_FAN_CUSTOM_OFF);
    } else {
        assert(fake_ec[ADDR_CUSTOM_MODE] == OR_FAN_CUSTOM_ON);
        assert(fake_ec[ADDR_CUSTOM_PL1] == frame[OR_CONTROL_ARGUMENT_OFFSET]);
        assert(fake_ec[ADDR_CUSTOM_PL2] == frame[OR_CONTROL_ARGUMENT_OFFSET + 1]);
        assert(fake_ec[ADDR_CUSTOM_PL4] == frame[OR_CONTROL_ARGUMENT_OFFSET + 2]);
    }
    for (i = 1; i <= total; i++) {
        setup(frame, action, custom);
        fail_at = i;
        assert(control_apply(frame));
        same(&before);
        setup(frame, action, custom);
        fail_before_at = i;
        assert(control_apply(frame));
        same(&before);
    }
    return total * 2;
}

static void test_guards(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    size_t i;
    for (i = OR_CONTROL_RESERVED_OFFSET; i < OR_CONTROL_FRAME_SIZE; i++) {
        setup(frame, OR_CONTROL_MODE, false);
        frame[i] = 1;
        assert(control_apply(frame) == -EINVAL && writes == 0);
    }
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_VERSION_OFFSET]++;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 4;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET + 1] = 1;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 5;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET + 1] = 80;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET + 2] = 120;
    assert(control_apply(frame) == -ERANGE && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET + 1] = OR_CONTROL_POWER_INVALID;
    frame[OR_CONTROL_ARGUMENT_OFFSET + 2] = OR_CONTROL_POWER_INVALID;
    assert(control_apply(frame) == -EINVAL && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    supplied = false;
    assert(control_apply(frame) == -EHOSTDOWN && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    fake_ec[ADDR_POWER_STATUS] = POWER_STATUS_TYPE_C_MASK;
    assert(control_apply(frame) == -EHOSTDOWN && writes == 0);
    setup(frame, OR_CONTROL_MODE, false);
    supplied = false;
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    assert(!control_apply(frame));
    setup(frame, OR_CONTROL_MODE, false);
    fake_ec[ADDR_TURBO_PL1] = 0;
    assert(control_apply(frame) == -EOPNOTSUPP && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    fan_curve_active = true;
    assert(control_apply(frame) == -EBUSY && writes == 0);
    setup(frame, OR_CONTROL_POWER, false);
    assert(control_dispatch(frame, false) == -EROFS && writes == 0);
    setup(frame, OR_CONTROL_RECOVER, false);
    assert(!control_dispatch(frame, false) && writes == 0);
}

static void test_restore_and_power_loss(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    struct control_saved before;
    size_t writes_started;
    setup(frame, OR_CONTROL_POWER, true);
    assert(!control_capture(&before));
    calls = 0;
    /* Factory + supply + snapshot finish before the first write. */
    writes_started = 1 + 9 + CONTROL_FIELDS + 1;
    fail_from = writes_started + 3;
    assert(control_apply(frame));
    assert(control_pending && control_fault);
    fail_from = 0;
    assert(control_apply(frame) == -EIO);
    frame[OR_CONTROL_ACTION_OFFSET] = OR_CONTROL_BOOST;
    memset(frame + OR_CONTROL_ARGUMENT_OFFSET, 0, 3);
    assert(control_apply(frame) == -EBUSY);
    frame[OR_CONTROL_ACTION_OFFSET] = OR_CONTROL_RECOVER;
    assert(!control_apply(frame));
    same(&before);

    setup(frame, OR_CONTROL_POWER, true);
    corrupt_address = ADDR_CUSTOM_PL1;
    corrupt_once = true;
    assert(control_apply(frame) == -EIO);
    same(&before);
    setup(frame, OR_CONTROL_POWER, true);
    unplug_at = writes_started + 3;
    assert(control_apply(frame) == -EHOSTDOWN);
    same(&before);
}

static void test_boost(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    setup(frame, OR_CONTROL_BOOST, false);
    assert(!control_apply(frame));
    assert(boost_owned && boost_original == 0 && boost_last == FAN_BOOST_CLAMP_BIT);
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == (OR_FAN_OFFICE_CLAMP | FAN_BOOST_CLAMP_BIT));
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 0;
    assert(!control_apply(frame));
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == OR_FAN_OFFICE_CLAMP);
    setup(frame, OR_CONTROL_BOOST, false);
    fail_at = 2;
    assert(control_apply(frame) == -EIO);
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == OR_FAN_OFFICE_CLAMP);
    setup(frame, OR_CONTROL_BOOST, false);
    fan_curve_fault = true;
    assert(!control_apply(frame));
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 0;
    assert(control_apply(frame) == -EBUSY);
}

static void test_release_survives_failed_transaction_restore(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    size_t first_write = 1 + 9 + CONTROL_FIELDS + 1;
    setup(frame, OR_CONTROL_BOOST, false);
    assert(!control_apply(frame));
    assert(boost_original == 0 && boost_last == FAN_BOOST_CLAMP_BIT);
    memset(frame + OR_CONTROL_ARGUMENT_OFFSET, 0, 3);
    frame[OR_CONTROL_ACTION_OFFSET] = OR_CONTROL_MODE;
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 3;
    calls = 0;
    fail_from = first_write + 3;
    assert(control_apply(frame));
    assert(control_pending && control_fault);
    assert(control_original.values[CONTROL_CLAMP] & FAN_BOOST_CLAMP_BIT);
    assert(control_release_owned());
    assert(boost_owned && boost_release_pending && boost_original == 0);
    fail_from = 0;
    /* A firmware rewrite after trigger ACK must retain the backup too. */
    corrupt_trigger_once = true;
    assert(control_restore() == -EIO);
    assert(control_pending && control_fault);
    /* The watchdog first restores the transaction, then the released boost. */
    assert(!control_restore());
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT);
    fail_before_at = calls + 1;
    assert(control_release_boost());
    assert(boost_owned && boost_release_pending);
    fail_before_at = 0;
    assert(!control_release_boost());
    assert(!(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT));
    assert(!boost_owned && !boost_release_pending);
}

static void test_pending_release_across_suspend_resume(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    setup(frame, OR_CONTROL_BOOST, false);
    assert(!control_apply(frame));
    fail_from = calls + 1;
    assert(control_release_owned() == -EIO);
    assert(boost_release_pending && boost_owned);
    /* The PM wrapper stops writes and cancels work before this shared body. */
    ec_stopping = true;
    assert(control_suspend_quiesce() == -EIO);
    assert(!ec_stopping && queued_watchdogs == 1 && boost_release_pending);
    /* Once the bus recovers, suspend completes only after boost restoration. */
    fail_from = 0;
    ec_stopping = true;
    queued_watchdogs = 0;
    assert(!control_suspend_quiesce());
    assert(ec_stopping && !boost_release_pending && !boost_owned);
    assert(!(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT));
    assert(queued_watchdogs == 0);
    control_resume_work();
    assert(!ec_stopping && queued_watchdogs == 0);

    /* An fd can be released after quiesce cancelled the old work item. */
    setup(frame, OR_CONTROL_BOOST, false);
    assert(!control_apply(frame));
    ec_stopping = true;
    assert(!control_suspend_quiesce());
    fail_from = calls + 1;
    assert(control_release_owned() == -EIO);
    assert(ec_stopping && boost_release_pending && queued_watchdogs == 0);
    control_resume_work();
    assert(!ec_stopping && queued_watchdogs == 1);
    fail_from = 0;
    assert(!control_release_boost());
    assert(!control_has_work() && !boost_owned && !boost_release_pending);
    assert(!(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT));
}

static void test_failure_diagnostics_preserve_strict_checks(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    struct control_saved before;
    struct diagnostic report;
    /* Immediate readback remains exact even for the consumable mode status. */
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    assert(!control_capture(&before));
    mode_trigger_clears_immediately = true;
    assert(control_apply(frame) == -EIO);
    assert(diagnostic_count == 1);
    report = diagnostics[0];
    assert(!strcmp(report.phase, "mode"));
    assert(!strcmp(report.step, "readback_mismatch"));
    assert(report.address == ADDR_TURBO_TRIGGER_1115);
    assert(report.error == -EIO && report.expected == 1 && report.actual == 0);
    same(&before);

    setup(frame, OR_CONTROL_MODE, false);
    assert(!fan_put_at_stage(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF, "diagnostic_probe"));
    assert(diagnostic_count == 0 && calls == 2);
    calls = 0;
    fail_before_at = 1;
    assert(fan_put_at_stage(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF, "diagnostic_probe") == -EIO);
    assert(calls == 1 && diagnostic_count == 1);
    assert(!strcmp(diagnostics[0].step, "write") && diagnostics[0].actual == -1);
    calls = diagnostic_count = 0;
    fail_before_at = 2;
    assert(fan_put_at_stage(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF, "diagnostic_probe") == -EIO);
    assert(calls == 2 && diagnostic_count == 1);
    assert(!strcmp(diagnostics[0].step, "readback") && diagnostics[0].actual == -1);
}

static void test_mode_status_consumption_after_trigger(void)
{
    u8 frame[OR_CONTROL_FRAME_SIZE];
    struct control_saved before, restored;
    struct diagnostic report;

    /* A same-office request can be consumed before the final read. All stable
     * fields remain unchanged, matching the observed hardware transaction.
     */
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    fake_ec[ADDR_AP_OEM_BYTE] = OR_CONTROL_AP_PRESENT;
    fake_ec[ADDR_FAN_RAMP_RATE] = OR_CONTROL_RAMP_SMOOTH;
    assert(!control_capture(&before));
    mode_trigger_clears_on_commit = true;
    assert(!control_apply(frame));
    assert(!diagnostic_count);
    same(&before);

    /* Recovery re-latches the original request too; consumption must not leave
     * a healthy restoration stuck in fault or retain its backup forever.
     */
    setup(frame, OR_CONTROL_MODE, false);
    fake_ec[ADDR_TURBO_TRIGGER_1115] = OR_CONTROL_MODE_STATUS_REQUEST;
    assert(!control_capture(&control_original));
    restored = control_original;
    restored.values[CONTROL_TURBO] = OR_CONTROL_MODE_STATUS_IDLE;
    control_pending = control_fault = true;
    fake_ec[ADDR_CUSTOM_PL1]++;
    fake_ec[ADDR_FAN_BOOST_CLAMP] = OR_FAN_TURBO_CLAMP;
    mode_trigger_clears_on_commit = true;
    assert(!control_restore());
    assert(!diagnostic_count);
    same(&restored);

    /* Any other status still rejects the apply and restores the full backup. */
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    assert(!control_capture(&before));
    mode_trigger_corrupts_on_commit_once = true;
    assert(control_apply(frame) == -EIO);
    assert(diagnostic_count == 1);
    report = diagnostics[0];
    assert(!strcmp(report.phase, "apply_after_trigger"));
    assert(!strcmp(report.step, "mismatch"));
    assert(report.address == ADDR_TURBO_TRIGGER_1115);
    assert(report.expected == OR_CONTROL_MODE_STATUS_REQUEST);
    assert(report.actual == OR_CONTROL_MODE_STATUS_REQUEST + 1);
    same(&before);

    /* An unexpected restoration status keeps fault/cooling/backup until retry. */
    setup(frame, OR_CONTROL_MODE, false);
    fake_ec[ADDR_TURBO_TRIGGER_1115] = OR_CONTROL_MODE_STATUS_REQUEST;
    assert(!control_capture(&control_original));
    before = control_original;
    control_pending = true;
    mode_trigger_corrupts_on_commit_once = true;
    assert(control_restore() == -EIO);
    assert(control_pending && control_fault);
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT);
    assert(!strcmp(diagnostics[0].phase, "rollback_after_trigger"));
    assert(!control_restore());
    same(&before);

    /* A consumed status does not excuse a transport failure or stable-field
     * corruption at the same commit; both must still roll back completely.
     */
    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    assert(!control_capture(&before));
    mode_trigger_clears_on_commit = true;
    mode_trigger_read_fails_after_commit_once = true;
    assert(control_apply(frame) == -EIO);
    assert(diagnostic_count == 1);
    assert(!strcmp(diagnostics[0].step, "read"));
    assert(diagnostics[0].address == ADDR_TURBO_TRIGGER_1115);
    same(&before);

    setup(frame, OR_CONTROL_MODE, false);
    frame[OR_CONTROL_ARGUMENT_OFFSET] = 1;
    mode_trigger_clears_on_commit = true;
    corrupt_trigger_once = true;
    assert(control_apply(frame) == -EIO);
    assert(diagnostic_count == 1);
    assert(diagnostics[0].address == ADDR_CUSTOM_PL1);
    assert(!strcmp(diagnostics[0].step, "mismatch"));
    same(&before);
}

int main(void)
{
    size_t failures = 0;
    failures += test_every_failure(OR_CONTROL_MODE, false);
    failures += test_every_failure(OR_CONTROL_MODE, true);
    failures += test_every_failure(OR_CONTROL_POWER, false);
    failures += test_every_failure(OR_CONTROL_POWER, true);
    test_guards();
    test_restore_and_power_loss();
    test_boost();
    test_release_survives_failed_transaction_restore();
    test_pending_release_across_suspend_resume();
    test_failure_diagnostics_preserve_strict_checks();
    test_mode_status_consumption_after_trigger();
    printf("PASS: semantic controls, %zu injected transfer failures, power loss, rollback retry and ABI guards\n", failures);
    return 0;
}
