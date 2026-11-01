/* SPDX-License-Identifier: GPL-3.0-or-later
 * Semantic OEM requests. Included by the kernel and hardware-free tests.
 * The caller holds ec_lock. No request contains a register address.
 */
enum control_field {
    CONTROL_AP, CONTROL_LIGHT_A, CONTROL_LIGHT_B, CONTROL_INDEX,
    CONTROL_TURBO, CONTROL_RAMP, CONTROL_ISOLATED,
    CONTROL_PL4, CONTROL_PL2, CONTROL_PL1, CONTROL_CLAMP, CONTROL_CUSTOM,
    CONTROL_FIELDS
};
static const u16 control_addresses[] = {
    ADDR_AP_OEM_BYTE, ADDR_AC_RECOVERY, ADDR_CAMERA_CUTOFF, ADDR_MODE_INDEX,
    ADDR_TURBO_TRIGGER_1115, ADDR_FAN_RAMP_RATE, ADDR_FAN_ISOLATED,
    ADDR_CUSTOM_PL4, ADDR_CUSTOM_PL2, ADDR_CUSTOM_PL1,
    ADDR_FAN_BOOST_CLAMP, ADDR_CUSTOM_MODE,
};
struct control_saved {
    u8 values[CONTROL_FIELDS];
    u8 trigger;
};
static struct control_saved control_original;
static bool control_pending;
static bool control_fault;
static bool boost_release_pending;

static const char *control_action_name(u8 action)
{
    switch (action) {
    case OR_CONTROL_MODE: return "mode";
    case OR_CONTROL_POWER: return "cpu_power";
    case OR_CONTROL_BOOST: return "fan_boost";
    case OR_CONTROL_RECOVER: return "recovery";
    default: return "unknown";
    }
}

static u8 control_clamp(u8 mode)
{
    switch (mode) {
    case 1: return OR_FAN_OFFICE_CLAMP;
    case 2: return OR_FAN_BALANCED_CLAMP;
    default: return OR_FAN_TURBO_CLAMP;
    }
}

static u8 control_mode(u8 clamp)
{
    u8 mode;
    for (mode = 1; mode <= 3; mode++)
        if ((clamp & ~FAN_BOOST_CLAMP_BIT) == control_clamp(mode))
            return mode;
    return 0;
}

static u8 control_trigger(u8 previous, u8 mode)
{
    return (previous & TRIGGER_USB_CHARGE) | TRIGGER_RGB_KB |
           (mode == 1 ? TRIGGER_SILENT_MODE : 0);
}

static int control_capture(struct control_saved *saved)
{
    size_t i;
    int result;
    for (i = 0; i < CONTROL_FIELDS; i++) {
        result = transfer(control_addresses[i], &saved->values[i], false);
        if (result) {
            ec_report_failure("capture", "read", control_addresses[i], result, -1, -1);
            return result;
        }
    }
    result = transfer(ADDR_TRIGGER_BYTE, &saved->trigger, false);
    if (result)
        ec_report_failure("capture", "read", ADDR_TRIGGER_BYTE, result, -1, -1);
    return result;
}

/* GM5HG7A firmware may consume the silent-mode request after the commit.
 * Only that confirmed REQUEST -> IDLE transition is valid here. Immediate
 * write/readback remains exact, as do all other fields and read failures.
 */
static int control_verify_after_trigger(const struct control_saved *saved, const char *phase)
{
    size_t i;
    u8 actual;
    int result;
    for (i = 0; i < CONTROL_FIELDS; i++) {
        result = transfer(control_addresses[i], &actual, false);
        if (result) {
            ec_report_failure(phase, "read", control_addresses[i], result, saved->values[i], -1);
            return result;
        }
        if (actual != saved->values[i] &&
            !(i == CONTROL_TURBO &&
              saved->values[i] == OR_CONTROL_MODE_STATUS_REQUEST &&
              actual == OR_CONTROL_MODE_STATUS_IDLE)) {
            ec_report_failure(phase, "mismatch", control_addresses[i], -EIO, saved->values[i], actual);
            return -EIO;
        }
    }
    return 0;
}

static int control_power_available(void)
{
    u8 power;
    int result;
    if (!system_power())
        return -EHOSTDOWN;
    result = transfer(ADDR_POWER_STATUS, &power, false);
    return result ? result : ((power & POWER_STATUS_DC_JACK) ? 0 : -EHOSTDOWN);
}

static int control_factory(u8 ceiling[3])
{
    static const u16 addresses[][3] = {
        { ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4 },
        { ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4 },
        { ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4 },
    };
    size_t mode, i;
    int result;
    for (mode = 0; mode < 3; mode++) {
        for (i = 0; i < 3; i++) {
            result = transfer(addresses[mode][i], &ceiling[i], false);
            if (result)
                return result;
        }
        if (ceiling[0] < OR_CONTROL_MIN_POWER || ceiling[1] < ceiling[0] ||
            ceiling[2] < ceiling[1] || ceiling[2] == OR_CONTROL_POWER_INVALID)
            return -EOPNOTSUPP;
    }
    return 0;
}

static void control_fail_safe(void)
{
    u8 value;
    control_fault = true;
    if (!transfer(ADDR_FAN_BOOST_CLAMP, &value, false)) {
        value |= FAN_BOOST_CLAMP_BIT;
        transfer(ADDR_FAN_BOOST_CLAMP, &value, true);
    }
}

/* A failed write may have reached the EC. Restore the complete snapshot,
 * keeping custom power disabled until its complete tuple is verified.
 * Never clear the backup or cooling protection after a failed restoration.
 */
static int control_restore(void)
{
    size_t i;
    u8 trigger;
    int result;
    bool failed = false;
    if (!control_pending)
        return 0;
    result = fan_put_at_stage(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF, "rollback");
    if (result) {
        control_fail_safe();
        return result;
    }
    for (i = 0; i < CONTROL_FIELDS; i++) {
        if (i == CONTROL_CUSTOM || i == CONTROL_CLAMP)
            continue;
        if (fan_put_at_stage(control_addresses[i], control_original.values[i], "rollback"))
            failed = true;
    }
    if (!failed && fan_put_at_stage(ADDR_CUSTOM_MODE, control_original.values[CONTROL_CUSTOM], "rollback"))
        failed = true;
    if (!failed && fan_put_at_stage(ADDR_FAN_BOOST_CLAMP, control_original.values[CONTROL_CLAMP], "rollback"))
        failed = true;
    if (!failed) {
        trigger = control_trigger(control_original.trigger,
            control_original.values[CONTROL_CUSTOM] == OR_FAN_CUSTOM_ON
                ? 2 : control_mode(control_original.values[CONTROL_CLAMP]));
        result = transfer(ADDR_TRIGGER_BYTE, &trigger, true);
        if (result) {
            ec_report_failure("rollback", "commit", ADDR_TRIGGER_BYTE, result, trigger, -1);
            failed = true;
        }
    }
    if (!failed && control_verify_after_trigger(&control_original, "rollback_after_trigger"))
        failed = true;
    if (failed) {
        control_fail_safe();
        return -EIO;
    }
    control_pending = control_fault = false;
    return 0;
}

static int control_validate(const u8 *frame)
{
    size_t i;
    const u8 *args = frame + OR_CONTROL_ARGUMENT_OFFSET;
    if (frame[OR_CONTROL_VERSION_OFFSET] != OR_CONTROL_VERSION)
        return -EINVAL;
    for (i = OR_CONTROL_RESERVED_OFFSET; i < OR_CONTROL_FRAME_SIZE; i++)
        if (frame[i])
            return -EINVAL;
    switch (frame[OR_CONTROL_ACTION_OFFSET]) {
    case OR_CONTROL_MODE:
        return args[0] >= 1 && args[0] <= 3 && !args[1] && !args[2] ? 0 : -EINVAL;
    case OR_CONTROL_POWER:
        return args[0] >= OR_CONTROL_MIN_POWER && args[0] <= args[1] &&
               (unsigned int)args[2] >= (unsigned int)args[1] + OR_CONTROL_PL4_MARGIN
            ? 0 : -EINVAL;
    case OR_CONTROL_BOOST:
        return args[0] <= 1 && !args[1] && !args[2] ? 0 : -EINVAL;
    case OR_CONTROL_RECOVER:
        return !args[0] && !args[1] && !args[2] ? 0 : -EINVAL;
    default:
        return -EINVAL;
    }
}

static int control_boost(bool enabled)
{
    u8 before, after;
    int result;
    if (!enabled && (fan_curve_fault || control_fault))
        return -EBUSY;
    result = transfer(ADDR_FAN_BOOST_CLAMP, &before, false);
    if (result)
        return result;
    after = enabled ? before | FAN_BOOST_CLAMP_BIT : before & ~FAN_BOOST_CLAMP_BIT;
    result = fan_check_write(ADDR_FAN_BOOST_CLAMP, after);
    if (result)
        return result;
    /* Record ownership before writing, including a write that reports error. */
    if (!boost_owned) {
        boost_original = before & FAN_BOOST_CLAMP_BIT;
        boost_owned = true;
    }
    boost_last = after & FAN_BOOST_CLAMP_BIT;
    result = fan_put_at_stage(ADDR_FAN_BOOST_CLAMP, after, "boost_apply");
    if (result) {
        if (!fan_put_at_stage(ADDR_FAN_BOOST_CLAMP, before, "boost_rollback"))
            boost_last = before & FAN_BOOST_CLAMP_BIT;
    }
    return result;
}

static int control_apply(const u8 *frame)
{
    const u8 *args = frame + OR_CONTROL_ARGUMENT_OFFSET;
    u8 action = frame[OR_CONTROL_ACTION_OFFSET], ceiling[3], trigger;
    const char *phase = control_action_name(action);
    struct control_saved next;
    bool needs_power;
    size_t i;
    int result = control_validate(frame);
    if (result)
        return result;
    if (action == OR_CONTROL_RECOVER)
        return control_restore();
    if (action == OR_CONTROL_BOOST)
        return control_boost(args[0] != 0);
    if (control_fault)
        return -EIO;
    if (fan_curve_active || fan_curve_fault)
        return -EBUSY;
    needs_power = action == OR_CONTROL_POWER || args[0] != 1;
    if (needs_power && (result = control_power_available()))
        return result;
    if ((result = control_factory(ceiling)))
        return result;
    if (action == OR_CONTROL_POWER &&
        (args[0] > ceiling[0] || args[1] > ceiling[1] || args[2] > ceiling[2]))
        return -ERANGE;
    if ((result = control_capture(&control_original)))
        return result;
    if (!control_mode(control_original.values[CONTROL_CLAMP]))
        return -EOPNOTSUPP;
    next = control_original;
    next.values[CONTROL_AP] |= OR_CONTROL_AP_PRESENT;
    next.values[CONTROL_INDEX] &= OR_CONTROL_MODE_PRESERVE;
    if (action == OR_CONTROL_MODE) {
        next.values[CONTROL_LIGHT_A] &= ~MASK_QKEY_WHITE_1830;
        next.values[CONTROL_LIGHT_B] &= ~MASK_QKEY_WHITE_1831;
        next.values[CONTROL_CLAMP] = control_clamp(args[0]) |
            (next.values[CONTROL_CLAMP] & FAN_BOOST_CLAMP_BIT);
        if (args[0] == 3)
            next.values[CONTROL_INDEX] |= OR_CONTROL_MODE_TURBO;
        next.values[CONTROL_TURBO] = args[0] == 1
            ? OR_CONTROL_MODE_STATUS_REQUEST : OR_CONTROL_MODE_STATUS_IDLE;
        next.values[CONTROL_CUSTOM] = OR_FAN_CUSTOM_OFF;
        next.values[CONTROL_RAMP] = args[0] == 3 ? 0 : OR_CONTROL_RAMP_SMOOTH;
        next.values[CONTROL_ISOLATED] &= ~OR_FAN_ISOLATED_BIT;
        trigger = control_trigger(next.trigger, args[0]);
    } else {
        next.values[CONTROL_INDEX] |= OR_CONTROL_MODE_TURBO;
        next.values[CONTROL_TURBO] = OR_CONTROL_MODE_STATUS_IDLE;
        next.values[CONTROL_LIGHT_A] |= MASK_QKEY_WHITE_1830;
        next.values[CONTROL_LIGHT_B] |= MASK_QKEY_WHITE_1831;
        next.values[CONTROL_PL1] = args[0];
        next.values[CONTROL_PL2] = args[1];
        next.values[CONTROL_PL4] = args[2];
        next.values[CONTROL_CUSTOM] = OR_FAN_CUSTOM_ON;
        trigger = control_trigger(next.trigger, 2);
    }
    control_pending = true;
    result = fan_put_at_stage(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF, phase);
    for (i = 0; !result && i < CONTROL_FIELDS; i++) {
        if (i == CONTROL_CUSTOM || next.values[i] == control_original.values[i])
            continue;
        result = fan_put_at_stage(control_addresses[i], next.values[i], phase);
    }
    if (!result && needs_power)
        result = control_power_available();
    if (!result)
        result = fan_put_at_stage(ADDR_CUSTOM_MODE, next.values[CONTROL_CUSTOM], phase);
    if (!result) {
        result = transfer(ADDR_TRIGGER_BYTE, &trigger, true);
        if (result)
            ec_report_failure(phase, "commit", ADDR_TRIGGER_BYTE, result, trigger, -1);
    }
    /* Acknowledging the trigger is not proof that firmware kept the values. */
    if (!result)
        result = control_verify_after_trigger(&next, "apply_after_trigger");
    if (!result && needs_power)
        result = control_power_available();
    if (result) {
        control_restore();
        return result;
    }
    control_pending = false;
    return 0;
}

static int control_dispatch(const u8 *frame, bool writable)
{
    int result = control_validate(frame);
    if (result)
        return result;
    if (!writable && frame[OR_CONTROL_ACTION_OFFSET] != OR_CONTROL_RECOVER)
        return -EROFS;
    return control_apply(frame);
}

/* File release can happen while the bus is failing. Retain the original
 * boost bit until both transaction backups and this restoration succeed.
 */
static int control_release_boost(void)
{
    u8 value;
    int result;
    if (!boost_release_pending)
        return 0;
    if (fan_curve_fault || control_fault)
        return -EBUSY;
    result = transfer(ADDR_FAN_BOOST_CLAMP, &value, false);
    if (result)
        return result;
    if ((value & FAN_BOOST_CLAMP_BIT) == boost_last) {
        value = (value & ~FAN_BOOST_CLAMP_BIT) | boost_original;
        result = fan_put_at_stage(ADDR_FAN_BOOST_CLAMP, value, "release_boost");
        if (result)
            return result;
    }
    /* A later external change owns the bit if it no longer matches ours. */
    boost_owned = boost_release_pending = false;
    return 0;
}

static int control_release_owned(void)
{
    int control_result, fan_result, boost_result;
    boost_release_pending = boost_owned;
    control_result = control_restore();
    fan_result = fan_restore();
    boost_result = control_release_boost();
    return control_result ? control_result : fan_result ? fan_result : boost_result;
}

static bool control_has_work(void)
{
    return fan_curve_active || control_fault || boost_release_pending;
}

/* Called after ec_stopping is set and the worker has been synchronously
 * cancelled. A failed quiesce aborts suspend and restarts pending recovery.
 */
static int control_suspend_quiesce(void)
{
    int result = control_restore();
    if (!result)
        result = fan_restore();
    if (!result)
        result = control_release_boost();
    if (result) {
        ec_stopping = false;
        if (control_has_work())
            queue_watchdog();
    }
    return result;
}

static void control_resume_work(void)
{
    ec_stopping = false;
    /* Release may have created work after suspend cancelled the worker. */
    if (control_has_work())
        queue_watchdog();
}
