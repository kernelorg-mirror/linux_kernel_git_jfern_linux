#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/delay.h>

int init_module(void) 
{ 
    for (int i = 0; i < 5; i++) {
	trace_printk("Start\n");
	usleep_range(100, 110);
	trace_printk("End\n");
    }
 
    /* Fail to load */
    return 1; 
} 
 
void cleanup_module(void) 
{ 
} 
 
MODULE_LICENSE("GPL");
