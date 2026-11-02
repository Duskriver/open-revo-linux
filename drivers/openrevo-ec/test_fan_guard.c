/* Exercise the actual kernel whitelist and cooling lease implementation. */
#include "test_ec_fixture.h"
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
static bool boost_owned;
static u8 boost_original, boost_last;
static bool listed(u16 address, const u16 *list, size_t length)
{
    size_t i;
    for (i = 0; i < length; i++)
        if (address == list[i]) return true;
    return false;
}
#include "fan_guard.h"
static void reset(u8 *frame)
{
    setup(frame);
    boost_owned = boost_release_pending = false;
    fake_ec[ADDR_FAN_BOOST_CLAMP] = OR_FAN_BALANCED_CLAMP;
}
static void boost_on(void)
{
    assert(!raw_write(ADDR_FAN_BOOST_CLAMP,
                     fake_ec[ADDR_FAN_BOOST_CLAMP] | FAN_BOOST_CLAMP_BIT, true));
    assert(boost_owned && cooling_pending());
}
int main(void)
{
    u8 frame[OR_FAN_FRAME_SIZE];
    size_t i, total;
    reset(frame);
    assert(raw_write(ADDR_CUSTOM_PL1, 45, false) == -EROFS);
    assert(raw_write(ADDR_RAMFAN_CPU_UPT_BASE, 0, true) == -EINVAL);
    assert(raw_write(ADDR_FAN_TABLE_LATCH, 0, true) == -EINVAL);
    assert(raw_write(ADDR_BALANCE_PL1, 45, true) == -EINVAL);
    assert(writes == 0);
    for (i = 0; i < ARRAY_SIZE(linux_ec_write); i++)
        assert(!raw_write(linux_ec_write[i], fake_ec[linux_ec_write[i]], true));
    assert(writes == ARRAY_SIZE(linux_ec_write));
    /* Guard the entire active curve backup; allow only the boost bit to vary. */
    reset(frame); assert(!fan_apply(frame));
    for (i = 0; i < ARRAY_SIZE(linux_ec_write); i++) {
        if (linux_ec_write[i] != ADDR_FAN_BOOST_CLAMP)
            assert(raw_write(linux_ec_write[i], 0, true) == -EBUSY);
    }
    assert(raw_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_OFFICE_CLAMP, true) == -EBUSY);
    boost_on();assert(!cooling_heartbeat(false));
    assert(cooling_heartbeat(true) == -ETIME);
    assert(!cooling_pending() && !fan_curve_fault);
    assert(!(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT));
    /* A lost write acknowledgement still creates kernel recovery ownership. */
    reset(frame); fail_at = 2;
    assert(raw_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_BALANCED_CLAMP | FAN_BOOST_CLAMP_BIT, true) == -EIO);
    assert(boost_owned); fail_at = 0; assert(!cooling_restore());assert(!cooling_pending());
    assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == OR_FAN_BALANCED_CLAMP);
    /* Last value comparison protects an external change and unrelated bits. */
    reset(frame); boost_on(); fake_ec[ADDR_FAN_BOOST_CLAMP] = OR_FAN_TURBO_CLAMP;
    assert(!cooling_restore());assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == OR_FAN_TURBO_CLAMP);
    reset(frame);boost_on(); fake_ec[ADDR_FAN_BOOST_CLAMP] = OR_FAN_TURBO_CLAMP | FAN_BOOST_CLAMP_BIT;
    assert(!cooling_restore());assert(fake_ec[ADDR_FAN_BOOST_CLAMP] == OR_FAN_TURBO_CLAMP);
    /* Also restore a boost that was already enabled when the app opened. */
    reset(frame);fake_ec[ADDR_FAN_BOOST_CLAMP] |= FAN_BOOST_CLAMP_BIT;
    assert(!raw_write(ADDR_FAN_BOOST_CLAMP, OR_FAN_BALANCED_CLAMP, true));
    assert(!cooling_restore());assert(fake_ec[ADDR_FAN_BOOST_CLAMP] & FAN_BOOST_CLAMP_BIT);
    reset(frame);boost_on();calls=0;assert(!cooling_restore());total=calls;
    for(i=1;i<=total;i++) {
        reset(frame);boost_on();calls=0;fail_at=i;
        assert(cooling_restore());assert(cooling_pending());
        assert(raw_write(ADDR_CUSTOM_PL1,45,true)==-EBUSY);
        assert(cooling_heartbeat(false)==-EIO);
        fail_at=0;assert(!cooling_restore());assert(!cooling_pending());
        assert(fake_ec[ADDR_FAN_BOOST_CLAMP]==OR_FAN_BALANCED_CLAMP);
        reset(frame);boost_on();calls=0;fail_before_at=i;
        assert(cooling_restore());assert(cooling_pending());
        fail_before_at=0;assert(!cooling_restore());assert(!cooling_pending());
    }
    /* Persistent fan restore failure must not release protective boost. */
    reset(frame);assert(!fan_apply(frame));boost_on();calls=0;fail_from=8;
    assert(cooling_restore());assert(fan_curve_fault && boost_release_pending);
    assert(boost_restore()==-EBUSY);
    fail_from=0;assert(!cooling_restore());assert(!cooling_pending());
    printf("PASS: whitelist, curve isolation, late heartbeat, lost reply, shared bits, boost restore and %zu restore faults\n",total*2);
    return 0;
}
