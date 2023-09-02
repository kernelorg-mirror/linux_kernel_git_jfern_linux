#include <linux/init.h>
#include <linux/module.h>
#include <linux/smp.h>
#include <linux/kthread.h>
#include <linux/delay.h>
#include <linux/slab.h>

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("A module to continuously send IPIs");

static struct task_struct *ipi_thread;
static int target_cpu;

static void ipi_handler(void *info) {
     mdelay(2); // Introduce a small delay between IPIs
}

static int send_ipis(void *data) {
    unsigned long end_time = jiffies + msecs_to_jiffies(50000); // 50 seconds
    int i = 0;

    printk(KERN_ERR "IPI sender thread started on cpu %d\n", smp_processor_id());

    while (time_before(jiffies, end_time)) {
        struct __call_single_data *csd =
            (struct __call_single_data *) kmalloc(sizeof(struct __call_single_data), GFP_KERNEL);
        if (!csd) {
            printk(KERN_ERR "Failed to allocate memory for csd\n");
            continue;
        }
        memset(csd, 0, sizeof(struct __call_single_data));
        csd->func = ipi_handler;
        smp_call_function_single_async(target_cpu, csd);
        if (i++ % 1000 == 0) {
            msleep(5);
        }
    }

    return 0;
}

static int __init my_module_init(void) {
    // WARN_ON_ONCE(true);
    printk(KERN_INFO "Loading module to continuously send IPIs...\n");

    target_cpu = (smp_processor_id() + 1) % nr_cpu_ids; // Choose the next CPU as target

    printk(KERN_ERR "Target CPU: %d\n", target_cpu);

    // ipi_thread = kthread_run(send_ipis, NULL, "ipi_sender_thread");
    // Run the thread on the current cpu
    ipi_thread = kthread_create_on_cpu(send_ipis, NULL, smp_processor_id(), "ipi_sender_thread");
    if (IS_ERR(ipi_thread)) {
        printk(KERN_ERR "Failed to create IPI sender thread\n");
        return PTR_ERR(ipi_thread);
    }

    wake_up_process(ipi_thread);
    return 0;
}

static void __exit my_module_exit(void) {
    printk(KERN_INFO "Unloading module...\n");
    kthread_stop(ipi_thread);
}

module_init(my_module_init);
module_exit(my_module_exit);

