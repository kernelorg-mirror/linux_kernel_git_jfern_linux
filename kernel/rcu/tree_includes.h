#ifndef __TREE_INCLUDES
#define __TREE_INCLUDES
#include <linux/types.h>
#include <linux/kernel.h>
#include <linux/init.h>
#include <linux/spinlock.h>
#include <linux/smp.h>
#include <linux/rcupdate_wait.h>
#include <linux/interrupt.h>
#include <linux/sched.h>
#include <linux/sched/debug.h>
#include <linux/nmi.h>
#include <linux/atomic.h>
#include <linux/bitops.h>
#include <linux/export.h>
#include <linux/completion.h>
#include <linux/moduleparam.h>
#include <linux/panic.h>
#include <linux/panic_notifier.h>
#include <linux/percpu.h>
#include <linux/notifier.h>
#include <linux/cpu.h>
#include <linux/mutex.h>
#include <linux/time.h>
#include <linux/kernel_stat.h>
#include <linux/wait.h>
#include <linux/kthread.h>
#include <uapi/linux/sched/types.h>
#include <linux/prefetch.h>
#include <linux/delay.h>
#include <linux/random.h>
#include <linux/trace_events.h>
#include <linux/suspend.h>
#include <linux/ftrace.h>
#include <linux/tick.h>
#include <linux/sysrq.h>
#include <linux/kprobes.h>
#include <linux/gfp.h>
#include <linux/oom.h>
#include <linux/smpboot.h>
#include <linux/jiffies.h>
#include <linux/slab.h>
#include <linux/sched/isolation.h>
#include <linux/sched/clock.h>
#include <linux/vmalloc.h>
#include <linux/mm.h>
#include <linux/kasan.h>
#include <linux/context_tracking.h>
#include "../time/tick-internal.h"

#include "tree.h"
#include "rcu.h"

static struct rcu_state rcu_state;
static int rcu_scheduler_fully_active;
static long qhimark;
static int kthread_prio;
static bool dump_tree;

extern void __maybe_unused rcu_advance_cbs_nowake(struct rcu_node *rnp,
						  struct rcu_data *rdp);
extern bool rcu_advance_cbs(struct rcu_node *rnp, struct rcu_data *rdp);

static void rcu_gp_kthread_wake(void);

extern void rcu_do_batch(struct rcu_data *rdp);

extern void invoke_rcu_core(void);

extern DEFINE_PER_CPU_SHARED_ALIGNED(struct rcu_data, rcu_data);

extern inline int rcu_lockdep_is_held_nocb(struct rcu_data *rdp);

extern inline bool rcu_current_is_nocb_kthread(struct rcu_data *rdp);


extern void trace_rcu_this_gp(struct rcu_node *rnp, struct rcu_data *rdp,
			      unsigned long gp_seq_req, const char *s);

#endif