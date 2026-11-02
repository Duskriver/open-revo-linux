#include "test_ec_fixture.h"
static void same(const struct fan_saved *expected)
{
    struct fan_saved actual;
    assert(!fan_capture(&actual));
    assert(!memcmp(actual.table, expected->table, sizeof(actual.table)));
    assert(actual.ap == expected->ap && actual.ramp == expected->ramp && actual.isolated == expected->isolated);
    assert(actual.custom == expected->custom);
    assert(!memcmp(actual.power, expected->power, sizeof(actual.power)));
    assert(!(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT));
    assert(fake_ec[ADDR_TRIGGER_BYTE] & TRIGGER_USB_CHARGE);
}

static void test_restore_read_failures(void)
{
    u8 frame[OR_FAN_FRAME_SIZE];
    unsigned int enabled, outage;
    for (enabled = 0; enabled < 2; enabled++) {
        for (outage = 0; outage < 2; outage++) {
            const u8 expected = enabled ? FAN_BOOST_CLAMP_BIT : 0;
            setup(frame);
            assert(!fan_apply(frame));
            fake_ec[ADDR_FAN_BOOST_CLAMP] = expected;
            /* A stale backup must not substitute for a failed first read. */
            fan_fault_boost = expected ^ FAN_BOOST_CLAMP_BIT;
            calls = 0;
            if (outage) fail_from = 1;
            else fail_before_at = 1;
            assert(fan_restore() == -EIO);
            assert(fan_curve_active && fan_curve_fault);
            assert(fan_fault_boost_valid == !outage);
            if (!outage) {
                assert((fan_fault_boost & FAN_BOOST_CLAMP_BIT) == expected);
                assert(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT);
                /* Further read failures must retain the already known target. */
                fail_from = calls + 1;
                assert(fan_restore() == -EIO);
                assert(fan_fault_boost_valid);
                assert((fan_fault_boost & FAN_BOOST_CLAMP_BIT) == expected);
            }
            assert(fan_check_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_OFFICE_CLAMP) == -EBUSY);
            assert(fan_check_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_TURBO_CLAMP) == -EBUSY);
            assert(!fan_check_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_OFFICE_CLAMP | FAN_BOOST_CLAMP_BIT));
            assert(fan_check_write(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF) == -EBUSY);
            fail_before_at = fail_from = 0;
            assert(!fan_restore());
            assert(!fan_curve_active && !fan_curve_fault && !fan_fault_boost_valid);
            assert((fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT) == expected);
            assert(!fan_check_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_OFFICE_CLAMP));
            assert(!fan_check_write(ADDR_CUSTOM_MODE, OR_FAN_CUSTOM_OFF));
        }
    }
}

int main(void)
{
    u8 frame[OR_FAN_FRAME_SIZE];
    struct fan_saved initial, applied;
    size_t total, i;
    setup(frame);
    assert(!fan_capture(&initial));
    calls = 0;
    assert(!fan_apply(frame));
    total = calls;
    assert(fan_curve_active && !fan_curve_fault);
    assert(fake_ec[ADDR_RAMFAN_TABLE_CTRL] == OR_FAN_DUTY_MAX);
    assert(fake_ec[ADDR_RAMFAN_CPU_DUTY_BASE] == 0);
    assert(fake_ec[ADDR_RAMFAN_GPU_DUTY_BASE] == 0);
    assert(fake_ec[ADDR_CUSTOM_MODE] == OR_FAN_CUSTOM_ON);
    assert(fake_ec[ADDR_CUSTOM_PL1] == fake_ec[ADDR_BALANCE_PL1]);
    assert(fake_ec[ADDR_CUSTOM_PL2] == fake_ec[ADDR_BALANCE_PL2]);
    assert(fake_ec[ADDR_CUSTOM_PL4] == fake_ec[ADDR_BALANCE_PL4]);
    assert(!fan_restore());
    same(&initial);
    /* Failure at every read/write, including errors after a write reached EC. */
    for (i = 1; i <= total; i++) {
        setup(frame); fail_at = i;
        assert(fan_apply(frame));
        fail_at = 0;
        same(&initial);
        assert(!fan_curve_active && !fan_curve_fault);
    }
    /* An unsuccessful edit restores the previous custom table, not factory. */
    setup(frame); assert(!fan_apply(frame)); assert(!fan_capture(&applied));
    frame[OR_FAN_RAMP_OFFSET] = OR_FAN_RAMP_ENABLE | 5;
    calls = 0; fail_at = total - 10;
    assert(fan_apply(frame)); fail_at = 0;
    assert(fan_curve_active); same(&applied);
    assert(!fan_restore()); same(&initial);
    /* Persistent transport failure retains backup and reports a fault. */
    setup(frame); assert(!fan_apply(frame)); calls = 0; fail_from = 8;
    assert(fan_restore()); assert(fan_curve_active && fan_curve_fault);
    fail_from = 0; assert(!fan_restore()); same(&initial);
    /* No hardware write on malformed input, battery, or existing boost. */
    setup(frame); frame[OR_FAN_TABLE_OFFSET + 2 * OR_FAN_TABLE_STEPS] = 1;
    assert(fan_apply(frame) == -EINVAL && writes == 0);
    setup(frame); fake_ec[ADDR_POWER_STATUS] = 0;
    assert(fan_apply(frame) == -EHOSTDOWN && writes == 0);
    setup(frame); supplied = false;
    assert(fan_apply(frame) == -EHOSTDOWN && writes == 0);
    setup(frame); drop_supply_at = total - 10;
    assert(fan_apply(frame) == -EHOSTDOWN && writes > 0);
    same(&initial);
    setup(frame); fake_ec[ADDR_FAN_BOOST_CLAMP] = FAN_BOOST_CLAMP_BIT;
    assert(fan_apply(frame) == -EBUSY && writes == 0);
    setup(frame); fake_ec[ADDR_RAMFAN_TABLE_CTRL] = 0;
    assert(fan_apply(frame) == -EOPNOTSUPP && writes == 0);
    setup(frame); corrupt_once = true;
    assert(fan_apply(frame) == -EIO); same(&initial);
    test_restore_read_failures();
    printf("PASS: original restore, update rollback, %zu injected transfer failures, read-failure recovery, protected boost writes and input guards\n", total);
    return 0;
}
