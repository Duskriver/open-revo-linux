// SPDX-License-Identifier: GPL-3.0-or-later
/* Uniwill's firmware-serialized EC mailbox, verified on GM5HG7A.
 * pread/pwrite exactly one byte at a vault-listed register offset.
 * No raw ports, ACPI debug interface, arbitrary method or address access.
 */
#include <linux/acpi.h>
#include <linux/dmi.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/uaccess.h>
#include <linux/wmi.h>
#include <linux/workqueue.h>
#include <linux/jiffies.h>
#include <linux/unaligned.h>
#include "vault_generated.h"

static bool allow_writes;
module_param(allow_writes, bool, 0644);
MODULE_PARM_DESC(allow_writes, "Enable vault-listed EC writes (default read only)");
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

#include "fan_transaction.h"
static unsigned int fan_curve_abi = OR_FAN_VERSION;
module_param(fan_curve_abi, uint, 0444);
module_param(fan_curve_active, bool, 0444);
module_param(fan_curve_fault, bool, 0444);
static unsigned long fan_deadline;
static void fan_watchdog(struct work_struct *work);
static DECLARE_DELAYED_WORK(fan_work, fan_watchdog);

static void fan_watchdog(struct work_struct *work)
{
    u8 power;
    mutex_lock(&ec_lock);
    if (fan_curve_active) {
        if (time_after_eq(jiffies, fan_deadline) ||
            transfer(ADDR_POWER_STATUS, &power, false) || !(power & POWER_STATUS_DC_JACK)) {
            if (fan_restore())
                pr_warn_ratelimited("openrevo_ec: fan table restore failed; maximum cooling requested\n");
        }
        if (fan_curve_active)
            schedule_delayed_work(&fan_work, msecs_to_jiffies(2000));
    }
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
        result = allow_writes ? fan_apply(frame) : -EROFS;
        break;
    case OR_FAN_RESTORE:
        result = fan_restore();
        break;
    case OR_FAN_HEARTBEAT:
        result = fan_curve_fault ? -EIO : 0;
        break;
    default:
        result = -EINVAL;
    }
    if (fan_curve_active) {
        if (!result)
            fan_deadline = jiffies + msecs_to_jiffies(OR_FAN_LEASE_MS);
        mod_delayed_work(system_wq, &fan_work, msecs_to_jiffies(2000));
    }
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
    else if (opened)
        result = -EBUSY;
    else
        opened = true;
    mutex_unlock(&ec_lock);
    return result;
}

static int ec_release(struct inode *inode, struct file *file)
{
    u8 ec_value;
    mutex_lock(&ec_lock);
    if (fan_restore())
        pr_warn("openrevo_ec: fan table restore failed on release\n");
    if (!fan_curve_fault && boost_owned && !transfer(ADDR_FAN_BOOST_CLAMP, &ec_value, false) &&
        (ec_value & FAN_BOOST_CLAMP_BIT) == boost_last) {
        ec_value = (ec_value & ~FAN_BOOST_CLAMP_BIT) | boost_original;
        if (transfer(ADDR_FAN_BOOST_CLAMP, &ec_value, true))
            pr_warn("openrevo_ec: could not restore fan boost on release\n");
    }
    boost_owned = false;
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
    u8 value, ec_value = 0;
    int result;
    if (*pos == OR_FAN_REQUEST_OFFSET && count == OR_FAN_FRAME_SIZE)
        return fan_request(buffer, count);
    if (!allow_writes)
        return -EROFS;
    if (count != 1 || !listed(*pos, linux_ec_write, ARRAY_SIZE(linux_ec_write)))
        return -EINVAL;
    if (copy_from_user(&value, buffer, 1))
        return -EFAULT;
    mutex_lock(&ec_lock);
    if (ec_stopping) {
        result = -ENODEV;
        goto out;
    }
    result = fan_check_write(*pos, value);
    if (result)
        goto out;
    if (*pos == ADDR_FAN_BOOST_CLAMP) {
        result = transfer(*pos, &ec_value, false);
        if (result)
            goto out;
        if (!boost_owned) {
            boost_original = ec_value & FAN_BOOST_CLAMP_BIT;
            boost_owned = true;
        }
        boost_last = value & FAN_BOOST_CLAMP_BIT;
    }
    result = transfer(*pos, &value, true);
out:
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
    /* Never assume that a shared WMI GUID implies compatible EC registers. */
    if (!dmi_match(DMI_SYS_VENDOR, "MECHREVO") ||
        !dmi_match(DMI_BOARD_NAME, "GM5HG7A") ||
        !dmi_match(DMI_BIOS_VERSION, "N.1.09MRO19"))
        return -ENODEV;
    mutex_lock(&ec_lock);
    if (ec_device || opened) {
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
    if (fan_restore())
        pr_warn("openrevo_ec: fan table restore failed on removal\n");
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
    result = fan_restore();
    if (result) {
        ec_stopping = false;
        schedule_delayed_work(&fan_work, msecs_to_jiffies(2000));
    }
    mutex_unlock(&ec_lock);
    return result;
}
static int ec_resume(struct device *device)
{
    mutex_lock(&ec_lock);
    ec_stopping = false;
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
module_wmi_driver(ec_driver);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("OpenRevo GM5HG7A Uniwill WMI EC transport");
