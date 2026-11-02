/* Hardware-free tests of the exact kernel transaction implementation. */
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
static size_t calls, fail_at, fail_before_at, fail_from, writes, drop_supply_at;
static bool corrupt_once;
static bool supplied;
static bool system_power(void) { return supplied; }
static void ec_report_failure(const char *phase, const char *step, u16 address,
                              int error, int expected, int actual)
{
    (void)phase; (void)step; (void)address;
    (void)error; (void)expected; (void)actual;
}
static int transfer(u16 address, u8 *value, bool write)
{
    calls++;
    if (calls == drop_supply_at)
        supplied = false;
    if (calls == fail_before_at || (fail_from && calls >= fail_from)) return -EIO;
    if (write) { fake_ec[address] = *value; writes++; }
    else *value = fake_ec[address];
    if (write && address == ADDR_RAMFAN_CPU_UPT_BASE && corrupt_once) {
        fake_ec[address]++;
        corrupt_once = false;
    }
    return calls == fail_at ? -EIO : 0;
}
#include "fan_transaction.h"
static void setup(u8 *frame)
{
    size_t i, channel;
    memset(fake_ec, 0, sizeof(fake_ec));
    memset(frame, 0, OR_FAN_FRAME_SIZE);
    calls = fail_at = fail_before_at = fail_from = writes = drop_supply_at = 0;
    corrupt_once = false;
    supplied = true;
    fan_curve_active = fan_curve_fault = false;
    fan_fault_boost_valid = false;
    for (i = 0; i < OR_FAN_TABLE_BYTES; i++)
        fake_ec[fan_bases[i / OR_FAN_TABLE_STEPS] + i % OR_FAN_TABLE_STEPS] = i;
    fake_ec[ADDR_RAMFAN_TABLE_CTRL] = OR_FAN_CONTROL_MIN;
    fake_ec[ADDR_POWER_STATUS] = POWER_STATUS_DC_JACK;
    fake_ec[ADDR_TRIGGER_BYTE] = TRIGGER_USB_CHARGE;
    fake_ec[ADDR_CUSTOM_MODE] = OR_FAN_CUSTOM_OFF;
    fake_ec[ADDR_BALANCE_PL1] = 65;
    fake_ec[ADDR_BALANCE_PL2] = 65;
    fake_ec[ADDR_BALANCE_PL4] = 100;
    frame[OR_FAN_VERSION_OFFSET] = OR_FAN_VERSION;
    frame[OR_FAN_ACTION_OFFSET] = OR_FAN_APPLY;
    frame[OR_FAN_RAMP_OFFSET] = OR_FAN_RAMP_ENABLE | 3;
    frame[OR_FAN_ISOLATED_OFFSET] = OR_FAN_ISOLATED_BIT;
    for (channel = 0; channel < 2; channel++) {
        u8 *up = frame + OR_FAN_TABLE_OFFSET + channel * 3 * OR_FAN_TABLE_STEPS;
        u8 *down = up + OR_FAN_TABLE_STEPS, *duty = down + OR_FAN_TABLE_STEPS;
        for (i = 1; i < OR_FAN_TABLE_STEPS; i++) {
            up[i] = 30 + (i - 1) * 5;
            down[i] = up[i] - (up[i] <= 55 ? 8 : 5);
            duty[i] = i < OR_FAN_TABLE_STEPS - 2 ? OR_FAN_DUTY_MIN_ACTIVE + i * 6 : OR_FAN_DUTY_MAX;
        }
    }
}
