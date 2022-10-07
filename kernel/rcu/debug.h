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

void rcu_debug_init(void)
{
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
