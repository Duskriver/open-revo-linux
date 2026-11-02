/* SPDX-License-Identifier: GPL-3.0-or-later
 * Kernel-only cooling ownership. Mode and CPU policy live in Rust.
 * Called under ec_lock; shared with the offline fault harness.
 */
static bool boost_release_pending;

static int boost_restore(void)
{
    u8 value;
    int result;
    if (!boost_release_pending)
        return 0;
    if (fan_curve_fault)
        return -EBUSY;
    result = transfer(ADDR_FAN_BOOST_CLAMP, &value, false);
    if (result)
        return result;
    if ((value & FAN_BOOST_CLAMP_BIT) == boost_last) {
        value = (value & ~FAN_BOOST_CLAMP_BIT) | boost_original;
        result = fan_put_at_stage(ADDR_FAN_BOOST_CLAMP, value, "boost_restore");
        if (result)
            return result;
    }
    boost_owned = boost_release_pending = false;
    return 0;
}

static int cooling_restore(void)
{
    int fan_result, boost_result;
    boost_release_pending = boost_owned;
    fan_result = fan_restore();
    boost_result = boost_restore();
    return fan_result ? fan_result : boost_result;
}

static bool cooling_pending(void)
{
    return fan_curve_active || boost_owned || boost_release_pending;
}

static int raw_write(u16 address, u8 value, bool writable)
{
    u8 before;
    int result;
    if (!listed(address, linux_ec_write, ARRAY_SIZE(linux_ec_write)))
        return -EINVAL;
    if (!writable)
        return -EROFS;
    if (boost_release_pending)
        return -EBUSY;
    result = fan_check_write(address, value);
    if (result)
        return result;
    /* Curves own the mode/CPU tuple and ramp. Their backup cannot be
     * invalidated by writes while the kernel may need to restore it.
     */
    if (fan_curve_active && address != ADDR_FAN_BOOST_CLAMP)
        return -EBUSY;
    if (address == ADDR_FAN_BOOST_CLAMP) {
        result = transfer(address, &before, false);
        if (result)
            return result;
        if (fan_curve_active && ((value ^ before) & ~FAN_BOOST_CLAMP_BIT))
            return -EBUSY;
        if ((value ^ before) & FAN_BOOST_CLAMP_BIT) {
            if (!boost_owned) {
                boost_original = before & FAN_BOOST_CLAMP_BIT;
                boost_owned = true;
            }
            boost_last = value & FAN_BOOST_CLAMP_BIT;
        }
    }
    /* Capture ownership before transfer: a failing write can reach firmware. */
    return transfer(address, &value, true);
}

static int cooling_heartbeat(bool expired)
{
    if (cooling_pending() && expired) {
        cooling_restore();
        return -ETIME;
    }
    return fan_curve_fault || boost_release_pending ? -EIO : 0;
}
