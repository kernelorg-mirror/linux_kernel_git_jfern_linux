#include <linux/init.h>
#include <linux/module.h>
#include <linux/smp.h>
#include <linux/kthread.h>
#include <linux/delay.h>
#include <linux/slab.h>

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("A module to fire a test warning on load");

static int __init my_module_init(void) {
    trace_printk("About to fire a warning...\n");
    WARN_ON_ONCE(true);
    return 0;
}

static void __exit my_module_exit(void) {
    printk(KERN_INFO "Unloading module...\n");
}

module_init(my_module_init);
module_exit(my_module_exit);

