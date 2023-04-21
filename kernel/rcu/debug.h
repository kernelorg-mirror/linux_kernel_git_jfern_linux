#include <linux/jiffies.h>
#include <linux/percpu-defs.h>
#include <linux/preempt.h>
#include <linux/types.h>
#include <linux/string.h>
#include <linux/spinlock.h>
#include <trace/events/sched.h>
#include <linux/types.h>

// Required for rcu_data definition.
#include "rcu.h"
#include "tree.h"

#include <linux/debugfs.h>

#ifdef CONFIG_RCU_DEBUGFS
// rdp lock must be held.
static void __find_func(struct rcu_data *rdp, unsigned long ip, int *B, int *E,
			int *N, bool *valid)
{
	rcu_debug_entry *p;
	int b, e, n;

	b = n = 0;
	e = rdp->rcu_debug_ptrs_nr - 1;

	while (b <= e) {
		n = (b + e) / 2;
		p = &rdp->rcu_debug_ptrs[n];
		if (ip > p->ip) {
			b = n + 1;
		} else if (ip < p->ip) {
			e = n - 1;
		} else
			break;
	}

	if (B)
		*B = b;
	if (E)
		*E = e;
	if (N)
		*N = n;

	// Once a ptr is found, check the valid flag.
	if (valid)
		*valid = (b <= e) && rdp->rcu_debug_ptrs[n].valid;
	return;
}

// Functions are only ever added and not deleted till reboot.
int rcu_debug_cb_done(struct rcu_data *rdp, void* ip_ptr, struct rcu_head_debug_data *data)
{
	bool valid;
	int b, e, n;
	unsigned long ip = (unsigned long)ip_ptr;
	u64 wait_ns;

	if (rdp->rcu_debug_ptrs_nr >= RCU_DEBUGFS_PTRS_SIZE) {
		WARN_ON_ONCE(1);
		return -1;
	}

	__find_func(rdp, ip, &b, &e, &n, &valid);

	if (b > e) {
		if (n != rdp->rcu_debug_ptrs_nr)
			memmove(&rdp->rcu_debug_ptrs[n+1], &rdp->rcu_debug_ptrs[n],
				(sizeof(rcu_debug_entry) * (rdp->rcu_debug_ptrs_nr - n)));

		rdp->rcu_debug_ptrs[n].ip = ip;
		rdp->rcu_debug_ptrs[n].valid = true;
		rdp->rcu_debug_ptrs_nr++;
	}

	WARN_ON_ONCE(valid && b > e);

	wait_ns = ktime_get_mono_fast_ns() - data->enqueue_nsecs;

	if (wait_ns > rdp->rcu_debug_ptrs[n].max_wait_ns)
		rdp->rcu_debug_ptrs[n].max_wait_ns = wait_ns;
	rdp->rcu_debug_ptrs[n].total_execs++;

	return 0;
}

DECLARE_PER_CPU_SHARED_ALIGNED(struct rcu_data, rcu_data);

static int rcu_debug_show(struct seq_file *m, void *v)
{
	struct rcu_data *rdp;
	int i, cpu;
	unsigned long flags;

	for_each_possible_cpu(cpu) {
		rdp = per_cpu_ptr(&rcu_data, cpu);
		rcu_nocb_lock_irqsave(rdp, flags);
		seq_printf(m, "cpu: %d, rcu_debug_ptrs_nr: %d\n", cpu, rdp->rcu_debug_ptrs_nr);

		for (i = 0; i < rdp->rcu_debug_ptrs_nr; i++) {
			if (!rdp->rcu_debug_ptrs[i].valid)
				continue;
			seq_printf(m, "ip: %ps, max_wait in ms: %llu, total_execs: %d\n",
					(void *)rdp->rcu_debug_ptrs[i].ip,
					rdp->rcu_debug_ptrs[i].max_wait_ns / 1000000,
					rdp->rcu_debug_ptrs[i].total_execs);
		}
		rcu_nocb_unlock_irqrestore(rdp, flags);
	}
	return 0;
}

static int rcu_debug_open(struct inode *inode, struct file *file)
{
	return single_open(file, rcu_debug_show, inode->i_private);
}

static const struct file_operations rcu_debug_fops = {
	.owner = THIS_MODULE,
	.open = rcu_debug_open,
	.read = seq_read,
	.llseek = seq_lseek,
	.release = single_release,
};

void __init rcu_debug_init(void)
{
	struct dentry *rcu_dir, *rcu_file;

	if (WARN_ON_ONCE(sizeof(rcu_head_debug_data) > CONFIG_DEBUG_OBJECTS_RCU_HEAD_SIZE))
		return;

	rcu_dir = debugfs_create_dir("rcu", NULL);
	if (!rcu_dir || IS_ERR(rcu_dir))
		return;

	rcu_file = debugfs_create_file("callbacks", 0444, rcu_dir, NULL,
			&rcu_debug_fops);
	if (!rcu_file)
		debugfs_remove(rcu_dir);
}
#else
int rcu_debug_cb_done(struct rcu_data *rdp, void* ip_ptr, struct rcu_head_debug_data *data)
{
	return -1;
}

void rcu_debug_init(void)
{
}
#endif /* CONFIG_RCU_DEBUGFS */
