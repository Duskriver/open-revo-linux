// SPDX-License-Identifier: GPL-3.0-or-later
/* Uniwill firmware-serialized EC mailbox.
 * Vault-listed byte transport, cooling watchdog and WMI events.
 * No raw ports, ACPI debug interface, arbitrary method or address access.
 */
#include <linux/acpi.h>
#include <linux/dmi.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/power_supply.h>
#include <linux/mutex.h>
#include <linux/uaccess.h>
#include <linux/wmi.h>
#include <linux/workqueue.h>
#include <linux/jiffies.h>
#include <linux/version.h>
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 12, 0)
#include <linux/unaligned.h>
#else
#include <asm/unaligned.h>
#endif
#include "vault_generated.h"

static bool allow_writes;
module_param(allow_writes, bool, 0644);
MODULE_PARM_DESC(allow_writes, "Enable vault-listed EC writes (default read only)");
static bool force;
module_param(force, bool, 0444);
MODULE_PARM_DESC(force, "Allow non-MECHREVO vendor; Uniwill WMI GUID is still required");
static DEFINE_MUTEX(ec_lock);
static struct wmi_device *ec_device;
static bool ec_stopping;
static bool opened;
static bool boost_owned;
static u8 boost_original;
static u8 boost_last;

static bool listed(loff_t address, const u16 *list, size_t length)
{
    size_t i;
    for (i = 0; i < length; i++)
        if (address == list[i])
            return true;
    return false;
}

/* Caller holds ec_lock, including during WMI device removal. */
static int transfer(u16 address, u8 *value, bool write)
{
    u8 frame[OR_WMI_FRAME_SIZE] = {0};
    struct acpi_buffer input = {sizeof(frame), frame};
    struct acpi_buffer output = {ACPI_ALLOCATE_BUFFER, NULL};
    union acpi_object *obj;
    acpi_status status;
    int result = -EIO;

    if (!ec_device)
        return -ENODEV;
    put_unaligned_le16(address, frame + OR_WMI_ADDRESS_OFFSET);
    frame[OR_WMI_DATA_OFFSET] = write ? *value : 0;
    frame[OR_WMI_FUNCTION_OFFSET] = write ? OR_WMI_WRITE : OR_WMI_READ;
    status = wmidev_evaluate_method(ec_device, OR_WMI_INSTANCE, OR_WMI_METHOD,
                                   &input, &output);
    if (ACPI_FAILURE(status))
        goto out;
    obj = output.pointer;
    if (!obj || obj->type != ACPI_TYPE_BUFFER ||
        obj->buffer.length < OR_WMI_MIN_REPLY_SIZE || !obj->buffer.pointer)
        goto out;
    if (get_unaligned_le32(obj->buffer.pointer) == OR_WMI_ERROR_SENTINEL)
        goto out;
    if (!write)
        *value = obj->buffer.pointer[0];
    result = 0;
out:
    kfree(output.pointer);
    return result;
}

static bool system_power(void)
{
    return power_supply_is_system_supplied() > 0;
}

static const char *ec_field_name(u16 address)
{
    switch (address) {
    case ADDR_AP_OEM_BYTE: return "oem_presence";
    case ADDR_AC_RECOVERY: return "mode_indicator_a";
    case ADDR_CAMERA_CUTOFF: return "mode_indicator_b";
    case ADDR_MODE_INDEX: return "mode_index";
    case ADDR_MODE_REQUEST: return "mode_trigger";
    case ADDR_FAN_RAMP_RATE: return "fan_ramp";
    case ADDR_FAN_ISOLATED: return "fan_isolation";
    case ADDR_CUSTOM_PL4: return "cpu_peak_limit";
    case ADDR_CUSTOM_PL2: return "cpu_short_limit";
    case ADDR_CUSTOM_PL1: return "cpu_sustained_limit";
    case ADDR_FAN_BOOST_CLAMP: return "mode_and_fan_boost";
    case ADDR_CUSTOM_MODE: return "custom_power";
    case ADDR_TRIGGER_BYTE: return "commit_trigger";
    case ADDR_FAN_TABLE_LATCH: return "fan_table_latch";
    default: return "fan_table";
    }
}

/* Diagnostics reuse already observed values and never issue extra EC calls.
 * A value of -1 means no value was available due to a transport failure.
 */
static void ec_report_failure(const char *phase, const char *step, u16 address,
                              int error, int expected, int actual)
{
    pr_warn_ratelimited("openrevo_ec: phase=%s step=%s field=%s error=%d expected=%d actual=%d\n",
                        phase, step, ec_field_name(address), error, expected, actual);
}
#include "fan_transaction.h"
#include "fan_guard.h"
static unsigned int fan_curve_abi = OR_FAN_VERSION;
static unsigned int device_controls_abi = OR_DEVICE_VERSION;
module_param(device_controls_abi, uint, 0444);
static unsigned int transport_abi = OR_TRANSPORT_VERSION;
module_param(fan_curve_abi, uint, 0444);
module_param(transport_abi, uint, 0444);
module_param(boost_owned, bool, 0444);
module_param(boost_release_pending, bool, 0444);
module_param(fan_curve_active, bool, 0444);
module_param(fan_curve_fault, bool, 0444);
static unsigned long fan_deadline;
static void fan_watchdog(struct work_struct *work);
static DECLARE_DELAYED_WORK(fan_work, fan_watchdog);

static void queue_watchdog(void)
{
    /* Heartbeats must not push back a pending power-loss/recovery check. */
    if (!ec_stopping && cooling_pending())
        schedule_delayed_work(&fan_work, msecs_to_jiffies(2000));
}

static void fan_watchdog(struct work_struct *work)
{
    u8 power;
    mutex_lock(&ec_lock);
    if (time_after_eq(jiffies, fan_deadline)) {
        if (cooling_restore())
            pr_warn_ratelimited("openrevo_ec: cooling restoration pending after lease expiry\n");
    } else if (fan_curve_active &&
        (!system_power() || transfer(ADDR_POWER_STATUS, &power, false) || !(power & POWER_STATUS_DC_JACK))) {
        if (fan_restore())
            pr_warn_ratelimited("openrevo_ec: fan restoration pending after power loss\n");
    }
    if (boost_release_pending && boost_restore())
        pr_warn_ratelimited("openrevo_ec: fan boost restoration pending\n");
    queue_watchdog();
    mutex_unlock(&ec_lock);
}

static ssize_t fan_request(const char __user *buffer, size_t count)
{
    u8 frame[OR_FAN_FRAME_SIZE];
    int result;
    if (count != sizeof(frame) || copy_from_user(frame, buffer, sizeof(frame)))
        return -EFAULT;
    if (frame[OR_FAN_VERSION_OFFSET] != OR_FAN_VERSION)
        return -EINVAL;
    mutex_lock(&ec_lock);
    if (!ec_device || ec_stopping) {
        result = -ENODEV;
        goto unlock;
    }
    switch (frame[OR_FAN_ACTION_OFFSET]) {
    case OR_FAN_APPLY:
        result = !allow_writes ? -EROFS : boost_release_pending ? -EBUSY : fan_apply(frame);
        break;
    case OR_FAN_RESTORE:
        result = fan_restore();
        break;
    case OR_FAN_HEARTBEAT:
        result = cooling_heartbeat(time_after_eq(jiffies, fan_deadline));
        break;
    default:
        result = -EINVAL;
    }
    if (!result && cooling_pending())
        fan_deadline = jiffies + msecs_to_jiffies(OR_FAN_LEASE_MS);
    queue_watchdog();
unlock:
    mutex_unlock(&ec_lock);
    return result ? result : sizeof(frame);
}

static int ec_open(struct inode *inode, struct file *file)
{
    int result = 0;
    mutex_lock(&ec_lock);
    if (!ec_device || ec_stopping)
        result = -ENODEV;
    else if (opened || cooling_pending())
        result = -EBUSY;
    else
        opened = true;
    mutex_unlock(&ec_lock);
    return result;
}

static int ec_release(struct inode *inode, struct file *file)
{
    mutex_lock(&ec_lock);
    if (cooling_restore())
        pr_warn("openrevo_ec: hardware restoration pending after release\n");
    queue_watchdog();
    opened = false;
    mutex_unlock(&ec_lock);
    return 0;
}

static ssize_t or_read(struct file *file, char __user *buffer, size_t count, loff_t *pos)
{
    u8 value;
    int result;
    if (count != 1 || !listed(*pos, linux_ec_read, ARRAY_SIZE(linux_ec_read)))
        return -EINVAL;
    mutex_lock(&ec_lock);
    result = ec_stopping ? -ENODEV : transfer(*pos, &value, false);
    mutex_unlock(&ec_lock);
    if (result)
        return result;
    return copy_to_user(buffer, &value, 1) ? -EFAULT : 1;
}

static ssize_t or_write(struct file *file, const char __user *buffer, size_t count, loff_t *pos)
{
    u8 value;
    int result;
    bool was_owned;
    if (*pos == OR_FAN_REQUEST_OFFSET && count == OR_FAN_FRAME_SIZE)
        return fan_request(buffer, count);
    if (count != 1 || !listed(*pos, linux_ec_write, ARRAY_SIZE(linux_ec_write)))
        return -EINVAL;
    if (copy_from_user(&value, buffer, 1))
        return -EFAULT;
    mutex_lock(&ec_lock);
    was_owned = boost_owned;
    result = (!ec_device || ec_stopping) ? -ENODEV : raw_write(*pos, value, allow_writes);
    /* A new boost gets its initial lease even if the write lost its reply.
     * Subsequent writes never refresh ownership; only HEARTBEAT does that.
     */
    if (!was_owned && boost_owned)
        fan_deadline = jiffies + msecs_to_jiffies(OR_FAN_LEASE_MS);
    queue_watchdog();
    mutex_unlock(&ec_lock);
    return result ? result : 1;
}

static const struct file_operations ec_fops = {
    .owner = THIS_MODULE,
    .open = ec_open,
    .release = ec_release,
    .read = or_read,
    .write = or_write,
    .llseek = noop_llseek,
};
static struct miscdevice ec_misc = {
    .minor = MISC_DYNAMIC_MINOR,
    .name = "openrevo_ec",
    .fops = &ec_fops,
    .mode = 0600,
};

static int ec_probe(struct wmi_device *wdev, const void *context)
{
    int result;
    if (!force && !dmi_match(DMI_SYS_VENDOR, "MECHREVO"))
        return -ENODEV;
    mutex_lock(&ec_lock);
    if (ec_device || opened || cooling_pending()) {
        mutex_unlock(&ec_lock);
        return -EBUSY;
    }
    ec_device = wdev;
    ec_stopping = false;
    result = misc_register(&ec_misc);
    if (result)
        ec_device = NULL;
    mutex_unlock(&ec_lock);
    return result;
}

static void ec_remove(struct wmi_device *wdev)
{
    misc_deregister(&ec_misc);
    mutex_lock(&ec_lock);
    ec_stopping = true;
    mutex_unlock(&ec_lock);
    cancel_delayed_work_sync(&fan_work);
    mutex_lock(&ec_lock);
    if (cooling_restore())
        pr_warn("openrevo_ec: cooling restoration failed on removal\n");
    ec_device = NULL;
    mutex_unlock(&ec_lock);
}

static const struct wmi_device_id ec_ids[] = {
    { OR_WMI_GUID, NULL },
    { }
};
MODULE_DEVICE_TABLE(wmi, ec_ids);
static int ec_suspend(struct device *device)
{
    int result;
    mutex_lock(&ec_lock);
    ec_stopping = true;
    mutex_unlock(&ec_lock);
    cancel_delayed_work_sync(&fan_work);
    mutex_lock(&ec_lock);
    result = cooling_restore();
    if (result) {
        ec_stopping = false;
        queue_watchdog();
    }
    mutex_unlock(&ec_lock);
    return result;
}
static int ec_resume(struct device *device)
{
    mutex_lock(&ec_lock);
    ec_stopping = false;
    queue_watchdog();
    mutex_unlock(&ec_lock);
    return 0;
}
static const struct dev_pm_ops ec_pm = {
    .suspend = ec_suspend,
    .freeze = ec_suspend,
    .poweroff = ec_suspend,
    .resume = ec_resume,
    .thaw = ec_resume,
    .restore = ec_resume,
};
static struct wmi_driver ec_driver = {
    .driver = { .name = "openrevo_ec", .pm = &ec_pm },
    .id_table = ec_ids,
    .probe = ec_probe,
    .remove = ec_remove,
};
#include "wmi_events.h"

static int __init openrevo_init(void)
{
    int result = wmi_driver_register(&ec_driver);
    if (result)
        return result;
    result = wmi_driver_register(&event_driver);
    if (result)
        wmi_driver_unregister(&ec_driver);
    return result;
}

static void __exit openrevo_exit(void)
{
    wmi_driver_unregister(&event_driver);
    wmi_driver_unregister(&ec_driver);
}
module_init(openrevo_init);
module_exit(openrevo_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("OpenRevo Uniwill WMI EC transport and events");
