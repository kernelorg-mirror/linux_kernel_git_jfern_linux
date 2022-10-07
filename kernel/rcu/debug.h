#include <linux/jiffies.h>
#include <linux/percpu-defs.h>
#include <linux/preempt.h>
#include <linux/types.h>
#include <linux/string.h>
#include <linux/spinlock.h>
#include <trace/events/sched.h>
#include <linux/types.h>

// Required for rcu_data definition.
#include "tree.h"

#ifdef CONFIG_RCU_DEBUGFS
// Below 2 definitions are for detecting scheduler-wakeup
// during lazy CB invocation, which is forbidden (as such users)
// should use call_rcu_hurry() instead.
static DEFINE_PER_CPU(bool, rcu_lazy_cb_exec) = false;
static DEFINE_PER_CPU(void *, rcu_lazy_ip) = NULL;

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

static bool rcu_debug_ptr_exists(struct rcu_data *rdp, void *ip)
{
	bool valid;

	__find_func(rdp, (unsigned long)ip, NULL, NULL, NULL, &valid);
	return valid;
}

// Functions are only ever added and not deleted till reboot.
int rcu_debug_ptr_queue(struct rcu_data *rdp, void* ip_ptr, bool lazy)
{
	bool valid;
	int b, e, n;
	unsigned long ip = (unsigned long)ip_ptr;

	trace_printk("Queuing %ps (%p) at %llu\n", ((struct rcu_head *)ip_ptr)->func, ip_ptr, get_jiffies_64());

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
		rdp->rcu_debug_ptrs[n].in_flight = false;
		rdp->rcu_debug_ptrs_nr++;
	}

	WARN_ON_ONCE(valid && b > e);
	WARN_ON_ONCE(valid && rdp->rcu_debug_ptrs[n].in_flight);

	rdp->rcu_debug_ptrs[n].lazy = lazy;
	rdp->rcu_debug_ptrs[n].in_flight = true;
	rdp->rcu_debug_ptrs[n].queue_jiffies = get_jiffies_64();
	return 0;
}

// Called when callback functions are invoked.
rcu_debug_entry rcu_debug_ptr_unqueue(struct rcu_data *rdp, void* ip_ptr)
{
	int n;
	rcu_debug_entry ret_none = {};
	bool valid;
	unsigned long ip = (unsigned long)ip_ptr;

	trace_printk("DeQueuing %ps (%p) at %llu\n", ((struct rcu_head *)ip_ptr)->func, ip_ptr, get_jiffies_64());

	__find_func(rdp, ip, NULL, NULL, &n, &valid);

	// Only valid and inflight entries will get an invocation
	// update. For instance, entries migrated before being
	// invoked will be ignored, as they will have a new house.
	if (!valid ||!rdp->rcu_debug_ptrs[n].in_flight)
		return ret_none;
	WARN_ON_ONCE(!rdp->rcu_debug_ptrs[n].valid);

	rdp->rcu_debug_ptrs[n].in_flight = false;
	return rdp->rcu_debug_ptrs[n];
}

void rcu_debug_set_context(struct rcu_data *rdp, void *ip)
{
	bool *flag = this_cpu_ptr(&rcu_lazy_cb_exec);
	*flag = rcu_debug_ptr_exists(rdp, ip);

	*this_cpu_ptr(&rcu_lazy_ip) = *flag ? ip : NULL;
}

void rcu_debug_reset_context(void)
{
	bool *flag = this_cpu_ptr(&rcu_lazy_cb_exec);
	*flag = false;
}

bool rcu_is_lazy_context(void)
{
	return *(this_cpu_ptr(&rcu_lazy_cb_exec));
}

static void
probe_waking(void *ignore, struct task_struct *p)
{
	// kworker wake ups don't appear to cause performance issues.
	// Ignore for now.
	if (!strncmp(p->comm, "kworker", 7))
		return;

	if (WARN_ON(!in_nmi() && !in_hardirq() && rcu_is_lazy_context())) {
		pr_err("*****************************************************\n");
		pr_err("RCU: A wake up has been detected from a lazy callback!\n");
		pr_err("The callback name is: %ps\n", *this_cpu_ptr(&rcu_lazy_ip));
		pr_err("The task it woke up is: %s (%d)\n", p->comm, p->pid);
		pr_err("This could cause performance issues! Check the stack.\n");
		pr_err("*****************************************************\n");
	}
}

void rcu_debug_init(void)
{
	int ret;
	pr_info("RCU Lazy CB debugging is turned on, system may be slow.\n");

	ret = register_trace_sched_waking(probe_waking, NULL);
	if (ret)
		pr_info("RCU: Lazy debug ched_waking probe could not be registered.");
}
#else
int rcu_debug_ptr_queue(struct rcu_data *rdp, void* ip_ptr)
{
	return -1;
}

void rcu_debug_set_context(struct rcu_data *rdp, void *ip_ptr)
{
}

void rcu_debug_reset_context(void)
{
}

void rcu_debug_init(void)
{
}
#endif /* CONFIG_RCU_DEBUGFS */
