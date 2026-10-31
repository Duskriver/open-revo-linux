/* SPDX-License-Identifier: GPL-3.0-or-later
 * Called under ec_lock. Also compiled by the hardware-free fault harness.
 * Only transfer() touches the transport; addresses come from the vault.
 */
struct fan_saved {
    u8 table[OR_FAN_TABLE_BYTES];
    u8 ap, ramp, isolated, custom;
    u8 power[3];
};
static struct fan_saved fan_original;
static bool fan_curve_active;
static bool fan_curve_fault;
static u8 fan_fault_boost;
static bool fan_fault_boost_valid;

static const u16 fan_bases[] = {
    ADDR_RAMFAN_CPU_UPT_BASE, ADDR_RAMFAN_CPU_DOWNT_BASE,
    ADDR_RAMFAN_CPU_DUTY_BASE, ADDR_RAMFAN_GPU_UPT_BASE,
    ADDR_RAMFAN_GPU_DOWNT_BASE, ADDR_RAMFAN_GPU_DUTY_BASE,
};
static const u16 fan_power_addresses[] = { ADDR_CUSTOM_PL1, ADDR_CUSTOM_PL2, ADDR_CUSTOM_PL4 };

/* Raw user writes must not bypass the transaction's cooling protection. */
static int fan_check_write(u16 address, u8 value)
{
    if (fan_curve_fault && address == ADDR_FAN_BOOST_CLAMP &&
        !(value & FAN_BOOST_CLAMP_BIT))
        return -EBUSY;
    if (fan_curve_active && (address == ADDR_CUSTOM_MODE || address == ADDR_CUSTOM_PL1 ||
        address == ADDR_CUSTOM_PL2 || address == ADDR_CUSTOM_PL4))
        return -EBUSY;
    return 0;
}

static int fan_put(u16 address, u8 value)
{
    u8 actual;
    int result = transfer(address, &value, true);
    if (result)
        return result;
    result = transfer(address, &actual, false);
    return result ? result : (actual == value ? 0 : -EIO);
}

static int fan_pulse(void)
{
    u8 value = FAN_TABLE_LATCH_BIT;
    int result = transfer(ADDR_FAN_TABLE_LATCH, &value, true);
    if (result)
        return result;
    value = 0;
    return transfer(ADDR_FAN_TABLE_LATCH, &value, true);
}

static int fan_capture(struct fan_saved *saved)
{
    size_t i;
    int result;
    for (i = 0; i < OR_FAN_TABLE_BYTES; i++) {
        result = transfer(fan_bases[i / OR_FAN_TABLE_STEPS] + i % OR_FAN_TABLE_STEPS,
                          &saved->table[i], false);
        if (result)
            return result;
    }
    /* Never capture a table which would require writing a zero control byte. */
    if (!saved->table[OR_FAN_TABLE_BYTES - 1] ||
        saved->table[OR_FAN_TABLE_BYTES - 1] > OR_FAN_DUTY_MAX)
        return -EOPNOTSUPP;
    if ((result = transfer(ADDR_AP_OEM_BYTE, &saved->ap, false)) ||
        (result = transfer(ADDR_FAN_RAMP_RATE, &saved->ramp, false)) ||
        (result = transfer(ADDR_FAN_ISOLATED, &saved->isolated, false)) ||
        (result = transfer(ADDR_CUSTOM_MODE, &saved->custom, false)))
        return result;
    for (i = 0; i < 3; i++) {
        result = transfer(fan_power_addresses[i], &saved->power[i], false);
        if (result)
            return result;
    }
    return 0;
}

static int fan_load(const struct fan_saved *saved)
{
    u8 ap, isolated, trigger, clamp, commit = FAN_TABLE_LATCH_BIT;
    size_t i;
    int result = fan_pulse();
    if (result)
        return result;
    if ((result = transfer(ADDR_AP_OEM_BYTE, &ap, false)) ||
        (result = fan_put(ADDR_AP_OEM_BYTE, ap | OR_FAN_AP_ACTIVE_MASK)) ||
        (result = fan_put(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF)))
        return result;
    /* Stage the power tuple while inactive; CUSTOM_ON also gates fan isolation. */
    for (i = 3; i > 0; i--) {
        result = fan_put(fan_power_addresses[i - 1], saved->power[i - 1]);
        if (result)
            return result;
    }
    for (i = 0; i < OR_FAN_TABLE_BYTES; i++) {
        result = fan_put(fan_bases[i / OR_FAN_TABLE_STEPS] + i % OR_FAN_TABLE_STEPS,
                         saved->table[i]);
        if (result)
            return result;
    }
    if ((result = fan_put(ADDR_FAN_RAMP_RATE, saved->ramp)) ||
        (result = transfer(ADDR_FAN_ISOLATED, &isolated, false)) ||
        (result = fan_put(ADDR_FAN_ISOLATED,
            (isolated & ~OR_FAN_ISOLATED_BIT) | (saved->isolated & OR_FAN_ISOLATED_BIT))) ||
        (result = fan_put(ADDR_CUSTOM_MODE, saved->custom)) ||
        (result = transfer(ADDR_FAN_TABLE_LATCH, &commit, true)) ||
        (result = transfer(ADDR_TRIGGER_BYTE, &trigger, false)))
        return result;
    trigger = (trigger & TRIGGER_USB_CHARGE) | TRIGGER_RGB_KB;
    if (saved->custom != OR_FAN_CUSTOM_ON) {
        if ((result = transfer(ADDR_FAN_BOOST_CLAMP, &clamp, false)))
            return result;
        if ((clamp & ~FAN_BOOST_CLAMP_BIT) == OR_FAN_OFFICE_CLAMP)
            trigger |= TRIGGER_SILENT_MODE;
    }
    if ((result = transfer(ADDR_TRIGGER_BYTE, &trigger, true)) ||
        (result = transfer(ADDR_AP_OEM_BYTE, &ap, false)))
        return result;
    return fan_put(ADDR_AP_OEM_BYTE,
                   (ap & ~OR_FAN_AP_ACTIVE_MASK) | (saved->ap & OR_FAN_AP_ACTIVE_MASK));
}

static int fan_boost_guard(u8 *previous)
{
    int result = transfer(ADDR_FAN_BOOST_CLAMP, previous, false);
    return result ? result : fan_put(ADDR_FAN_BOOST_CLAMP, *previous | FAN_BOOST_CLAMP_BIT);
}

static int fan_restore_boost(u8 previous)
{
    u8 value;
    int result = transfer(ADDR_FAN_BOOST_CLAMP, &value, false);
    return result ? result : fan_put(ADDR_FAN_BOOST_CLAMP,
        (value & ~FAN_BOOST_CLAMP_BIT) | (previous & FAN_BOOST_CLAMP_BIT));
}

/* If restoration fails, retain the original backup and force maximum cooling.
 * Never leave an incomplete table active with a low fan command. */
static void fan_fail_safe(u8 desired_boost, bool boost_known)
{
    u8 value;
    if (!fan_curve_fault)
        fan_fault_boost_valid = false;
    if (!fan_fault_boost_valid && boost_known) {
        fan_fault_boost = desired_boost;
        fan_fault_boost_valid = true;
    }
    fan_curve_fault = true;
    if (!transfer(ADDR_FAN_BOOST_CLAMP, &value, false)) {
        /* Capture only a successful read, before forcing the protection bit. */
        if (!fan_fault_boost_valid) {
            fan_fault_boost = value;
            fan_fault_boost_valid = true;
        }
        value |= FAN_BOOST_CLAMP_BIT;
        transfer(ADDR_FAN_BOOST_CLAMP, &value, true);
    }
}

static int fan_restore(void)
{
    u8 boost;
    int result;
    if (!fan_curve_active)
        return 0;
    result = transfer(ADDR_FAN_BOOST_CLAMP, &boost, false);
    if (result) {
        fan_fail_safe(0, false);
        return result;
    }
    result = fan_put(ADDR_FAN_BOOST_CLAMP, boost | FAN_BOOST_CLAMP_BIT);
    if (fan_curve_fault && fan_fault_boost_valid)
        boost = fan_fault_boost;
    if (!result)
        result = fan_load(&fan_original);
    if (!result)
        result = fan_restore_boost(boost);
    if (result) {
        fan_fail_safe(boost, true);
        return result;
    }
    fan_curve_active = false;
    fan_curve_fault = false;
    fan_fault_boost_valid = false;
    return 0;
}

static int fan_validate(const u8 *frame)
{
    const u8 *table = frame + OR_FAN_TABLE_OFFSET;
    size_t channel, i;
    if (frame[OR_FAN_VERSION_OFFSET] != OR_FAN_VERSION ||
        frame[OR_FAN_ACTION_OFFSET] != OR_FAN_APPLY ||
        frame[OR_FAN_ISOLATED_OFFSET] != OR_FAN_ISOLATED_BIT ||
        (frame[OR_FAN_RAMP_OFFSET] != (OR_FAN_RAMP_ENABLE | 1) &&
         frame[OR_FAN_RAMP_OFFSET] != (OR_FAN_RAMP_ENABLE | 3) &&
         frame[OR_FAN_RAMP_OFFSET] != (OR_FAN_RAMP_ENABLE | 5) &&
         frame[OR_FAN_RAMP_OFFSET] != (OR_FAN_RAMP_ENABLE | 10)))
        return -EINVAL;
    for (channel = 0; channel < 2; channel++) {
        const u8 *up = table + channel * 3 * OR_FAN_TABLE_STEPS;
        const u8 *down = up + OR_FAN_TABLE_STEPS;
        const u8 *duty = down + OR_FAN_TABLE_STEPS;
        if (up[0] || down[0] || duty[0])
            return -EINVAL;
        for (i = 1; i < OR_FAN_TABLE_STEPS; i++) {
            unsigned int temp = 30 + (i - 1) * 5;
            unsigned int hysteresis = temp <= 55 ? 8 : 5;
            if (up[i] != temp || down[i] != temp - hysteresis ||
                duty[i] < OR_FAN_DUTY_MIN_ACTIVE || duty[i] > OR_FAN_DUTY_MAX || duty[i] < duty[i - 1])
                return -EINVAL;
        }
        if (duty[OR_FAN_TABLE_STEPS - 2] != OR_FAN_DUTY_MAX ||
            duty[OR_FAN_TABLE_STEPS - 1] != OR_FAN_DUTY_MAX)
            return -EINVAL;
    }
    return 0;
}

static int fan_apply(const u8 *frame)
{
    struct fan_saved previous, next;
    bool was_active = fan_curve_active;
    u8 power, boost;
    int result = fan_validate(frame);
    if (result)
        return result;
    if (fan_curve_fault)
        return -EIO;
    if ((result = transfer(ADDR_POWER_STATUS, &power, false)))
        return result;
    if (!(power & POWER_STATUS_DC_JACK))
        return -EHOSTDOWN;
    if ((result = transfer(ADDR_FAN_BOOST_CLAMP, &boost, false)))
        return result;
    if (boost & FAN_BOOST_CLAMP_BIT)
        return -EBUSY;
    if ((result = fan_capture(&previous)))
        return result;
    if (!fan_curve_active)
        fan_original = previous;
    next = previous;
    if (previous.custom != OR_FAN_CUSTOM_ON) {
        const u16 *preset;
        const u16 office[] = { ADDR_OFFICE_PL1, ADDR_OFFICE_PL2, ADDR_OFFICE_PL4 };
        const u16 balanced[] = { ADDR_BALANCE_PL1, ADDR_BALANCE_PL2, ADDR_BALANCE_PL4 };
        const u16 turbo[] = { ADDR_TURBO_PL1, ADDR_TURBO_PL2, ADDR_TURBO_PL4 };
        size_t i;
        switch (boost & ~FAN_BOOST_CLAMP_BIT) {
        case OR_FAN_OFFICE_CLAMP: preset = office; break;
        case OR_FAN_BALANCED_CLAMP: preset = balanced; break;
        case OR_FAN_TURBO_CLAMP: preset = turbo; break;
        default: return -EOPNOTSUPP;
        }
        for (i = 0; i < 3; i++) {
            result = transfer(preset[i], &next.power[i], false);
            if (result)
                return result;
        }
    }
    if (next.power[0] < 15 || next.power[1] < next.power[0] ||
        next.power[2] < next.power[1] || next.power[2] == 255)
        return -EOPNOTSUPP;
    next.custom = OR_FAN_CUSTOM_ON;
    memcpy(next.table, frame + OR_FAN_TABLE_OFFSET, OR_FAN_TABLE_BYTES);
    /* GPU's last three bytes are thresholds and control, not duty steps. */
    next.table[OR_FAN_TABLE_BYTES - 3] = frame[OR_FAN_TABLE_OFFSET + OR_FAN_TABLE_BYTES - 1];
    next.table[OR_FAN_TABLE_BYTES - 2] = next.table[OR_FAN_TABLE_BYTES - 3];
    next.table[OR_FAN_TABLE_BYTES - 1] = next.table[OR_FAN_TABLE_BYTES - 3] < OR_FAN_CONTROL_MIN
        ? OR_FAN_CONTROL_MIN : next.table[OR_FAN_TABLE_BYTES - 3];
    next.ramp = frame[OR_FAN_RAMP_OFFSET];
    next.isolated = frame[OR_FAN_ISOLATED_OFFSET];
    next.ap |= OR_FAN_AP_ACTIVE_MASK;
    /* Mark owned before the first write, including a write that reports error. */
    fan_curve_active = true;
    result = fan_boost_guard(&boost);
    if (!result)
        result = fan_load(&next);
    if (!result) {
        result = transfer(ADDR_POWER_STATUS, &power, false);
        if (!result && !(power & POWER_STATUS_DC_JACK))
            result = -EHOSTDOWN;
    }
    if (!result)
        result = fan_restore_boost(boost);
    if (result) {
        if (fan_load(&previous) || fan_restore_boost(boost))
            fan_fail_safe(boost, true);
        else
            fan_curve_active = was_active;
        return result;
    }
    return 0;
}
