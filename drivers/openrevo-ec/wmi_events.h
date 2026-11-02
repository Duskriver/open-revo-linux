/* SPDX-License-Identifier: GPL-3.0-or-later
 * Raw Uniwill events, independent of EC access. Unknown events have no effects.
 * All queue and lifetime state is protected against concurrent WMI callbacks.
 */
#include <linux/kfifo.h>
#include <linux/poll.h>

struct event_frame { u8 bytes[OR_EVENT_FRAME_SIZE]; };
static DEFINE_MUTEX(event_lock);
static DECLARE_WAIT_QUEUE_HEAD(event_wait);
static DEFINE_KFIFO(event_fifo, struct event_frame, OR_EVENT_QUEUE_SIZE);
static bool event_present, event_opened;
static u64 event_sequence;

static void event_notify(struct wmi_device *wdev, union acpi_object *obj)
{
    struct event_frame frame = {0};
    if (!obj || obj->type != ACPI_TYPE_INTEGER || obj->integer.value > U32_MAX)
        return;
    mutex_lock(&event_lock);
    if (event_present) {
        put_unaligned_le64(++event_sequence, frame.bytes + OR_EVENT_SEQUENCE_OFFSET);
        put_unaligned_le32(obj->integer.value, frame.bytes + OR_EVENT_CODE_OFFSET);
        put_unaligned_le32(OR_EVENT_VERSION, frame.bytes + OR_EVENT_VERSION_OFFSET);
        if (kfifo_is_full(&event_fifo))
            kfifo_skip(&event_fifo);
        kfifo_in(&event_fifo, &frame, 1);
    }
    mutex_unlock(&event_lock);
    wake_up_interruptible(&event_wait);
}

static int event_open(struct inode *inode, struct file *file)
{
    int result = 0;
    mutex_lock(&event_lock);
    if (!event_present)
        result = -ENODEV;
    else if (event_opened)
        result = -EBUSY;
    else {
        event_opened = true;
        /* Never replay keys pressed before this runtime acquired ownership. */
        kfifo_reset(&event_fifo);
    }
    mutex_unlock(&event_lock);
    return result ? result : nonseekable_open(inode, file);
}

static int event_release(struct inode *inode, struct file *file)
{
    mutex_lock(&event_lock);
    event_opened = false;
    mutex_unlock(&event_lock);
    return 0;
}

static bool event_ready(void)
{
    bool ready;
    mutex_lock(&event_lock);
    ready = !event_present || !kfifo_is_empty(&event_fifo);
    mutex_unlock(&event_lock);
    return ready;
}

static ssize_t event_read(struct file *file, char __user *buffer, size_t count, loff_t *pos)
{
    struct event_frame frame;
    int result;
    if (count < sizeof(frame))
        return -EINVAL;
    for (;;) {
        mutex_lock(&event_lock);
        if (!event_present) {
            mutex_unlock(&event_lock);
            return -ENODEV;
        }
        if (kfifo_out_peek(&event_fifo, &frame, 1)) {
            result = copy_to_user(buffer, &frame, sizeof(frame)) ? -EFAULT : sizeof(frame);
            if (result > 0)
                kfifo_skip(&event_fifo);
            mutex_unlock(&event_lock);
            return result;
        }
        mutex_unlock(&event_lock);
        if (file->f_flags & O_NONBLOCK)
            return -EAGAIN;
        result = wait_event_interruptible(event_wait, event_ready());
        if (result)
            return result;
    }
}

static __poll_t event_poll(struct file *file, poll_table *wait)
{
    __poll_t mask = 0;
    poll_wait(file, &event_wait, wait);
    mutex_lock(&event_lock);
    if (!event_present)
        mask = EPOLLHUP | EPOLLERR;
    else if (!kfifo_is_empty(&event_fifo))
        mask = EPOLLIN | EPOLLRDNORM;
    mutex_unlock(&event_lock);
    return mask;
}

static const struct file_operations event_fops = {
    .owner = THIS_MODULE, .open = event_open, .release = event_release,
    .read = event_read, .poll = event_poll,
};
static struct miscdevice event_misc = {
    .minor = MISC_DYNAMIC_MINOR, .name = "openrevo_events",
    .fops = &event_fops, .mode = 0400,
};
static int event_probe(struct wmi_device *wdev, const void *context)
{
    int result;
    if (!force && !dmi_match(DMI_SYS_VENDOR, "MECHREVO"))
        return -ENODEV;
    mutex_lock(&event_lock);
    if (event_present || event_opened) {
        mutex_unlock(&event_lock);
        return -EBUSY;
    }
    kfifo_reset(&event_fifo);
    result = misc_register(&event_misc);
    event_present = !result;
    mutex_unlock(&event_lock);
    return result;
}
static void event_remove(struct wmi_device *wdev)
{
    misc_deregister(&event_misc);
    mutex_lock(&event_lock);
    event_present = false;
    kfifo_reset(&event_fifo);
    mutex_unlock(&event_lock);
    wake_up_interruptible(&event_wait);
}
static const struct wmi_device_id event_ids[] = {
    { OR_WMI_EVENT_GUID, NULL }, { }
};
MODULE_DEVICE_TABLE(wmi, event_ids);
static struct wmi_driver event_driver = {
    .driver = { .name = "openrevo_events" }, .id_table = event_ids,
    .probe = event_probe, .remove = event_remove, .notify = event_notify,
};
