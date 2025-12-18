// SPDX-License-Identifier: GPL-2.0
/*
 * hazptr: Per-Task Hazard Pointers
 *
 * Implementation of hazard pointers with per-task storage.
 * Each task has 16 embedded slots + overflow via GFP_ATOMIC allocation.
 * Uses 2-level scan optimization: check in_use_count before scanning slots.
 */

#include <linux/hazptr.h>
#include <linux/sched.h>
#include <linux/sched/task.h>
#include <linux/sched/signal.h>
#include <linux/slab.h>
#include <linux/export.h>
#include <linux/rcupdate.h>

/*
 * hazptr_get_free_slot - Find or allocate a free hazard pointer slot.
 * @tctx: Per-task hazard pointer context
 *
 * Searches fixed slots first, then overflow chunks.
 * If all are in use, allocates a new overflow chunk.
 *
 * Returns: Pointer to free slot, or NULL on allocation failure.
 */
static struct hazptr_slot *hazptr_get_free_slot(struct hazptr_task_ctx *tctx)
{
	struct hazptr_overflow *chunk;
	int i;

	/* Try fixed slots first. */
	for (i = 0; i < NR_HAZPTR_SLOTS; i++) {
		if (!tctx->slots[i].addr)
			return &tctx->slots[i];
	}

	/* Try existing overflow chunks. */
	for (chunk = tctx->overflow; chunk; chunk = chunk->next) {
		for (i = 0; i < NR_HAZPTR_OVERFLOW_SLOTS; i++) {
			if (!chunk->slots[i].addr)
				return &chunk->slots[i];
		}
	}

	/* Allocate new overflow chunk. */
	chunk = kzalloc(sizeof(*chunk), GFP_ATOMIC);
	if (!chunk) {
		WARN_ONCE(1, "hazptr: overflow alloc failed\n");
		return NULL;
	}

	/* Link at head. */
	chunk->next = tctx->overflow;
	tctx->overflow = chunk;
	return &chunk->slots[0];
}

/*
 * hazptr_acquire - Load pointer and protect with hazard pointer.
 */
void *hazptr_acquire(struct hazptr_ctx *ctx, void * const *addr_p)
{
	struct hazptr_task_ctx *tctx = &current->hazptr_ctx;
	struct hazptr_slot *slot;
	void *addr, *addr2;

	ctx->slot = NULL;

	/* Load pointer to know what to protect. */
	addr = READ_ONCE(*addr_p);
	for (;;) {
		if (!addr)
			return NULL;

		slot = hazptr_get_free_slot(tctx);
		if (!slot)
			return NULL;  /* OOM */

		/* Increment count BEFORE storing addr. */
		WRITE_ONCE(tctx->in_use_count, tctx->in_use_count + 1);
		smp_wmb();
		WRITE_ONCE(slot->addr, addr);

		/* Memory ordering: Store before re-load. */
		smp_mb();

		/* Re-load to verify pointer didn't change. */
		addr2 = READ_ONCE(*addr_p);
		if (likely(addr2 == addr)) {
			ctx->slot = slot;
			return addr2;  /* Success */
		}

		/* Pointer changed - release and retry. */
		smp_store_release(&slot->addr, NULL);
		smp_wmb();
		WRITE_ONCE(tctx->in_use_count, tctx->in_use_count - 1);

		if (!addr2)
			return NULL;  /* Became NULL */

		addr = addr2;  /* Retry with new value */
	}
}
EXPORT_SYMBOL_GPL(hazptr_acquire);

/*
 * hazptr_release - Release hazard pointer protection.
 */
void hazptr_release(struct hazptr_ctx *ctx, void *addr)
{
	struct hazptr_task_ctx *tctx = &current->hazptr_ctx;
	struct hazptr_slot *slot = ctx->slot;

	if (!addr || !slot)
		return;

	WARN_ON_ONCE(slot->addr != addr);

	/* Clear addr BEFORE decrementing count. */
	smp_store_release(&slot->addr, NULL);
	smp_wmb();
	WRITE_ONCE(tctx->in_use_count, tctx->in_use_count - 1);
}
EXPORT_SYMBOL_GPL(hazptr_release);

/*
 * hazptr_synchronize - Wait for all HP references to addr to clear.
 *
 * 2-level scan:
 * - Level 1: Check in_use_count, skip tasks with count==0
 * - Level 2: Scan fixed slots + overflow chunks
 */
void hazptr_synchronize(void *addr)
{
	struct task_struct *g, *t;
	struct hazptr_overflow *chunk;
	int i;

	if (!addr)
		return;

	/* Busy-wait should only be done from preemptible context. */
	lockdep_assert_preemption_enabled();

	/* Memory ordering: Ensure addr unpublish visible before scan. */
	smp_mb();

	rcu_read_lock();
	for_each_process_thread(g, t) {
		struct hazptr_task_ctx *tctx = &t->hazptr_ctx;

		/* Level 1: Skip task if no slots in use. */
		if (READ_ONCE(tctx->in_use_count) == 0)
			continue;

		/*
		 * Pair with smp_wmb() in hazptr_acquire between
		 * incrementing in_use_count and storing to slot.
		 * Ensures if we see count > 0, we see slot stores.
		 */
		smp_rmb();

		/* Level 2a: Scan fixed slots. */
		for (i = 0; i < NR_HAZPTR_SLOTS; i++) {
			while (smp_load_acquire(&tctx->slots[i].addr) == addr)
				cpu_relax();
		}

		/* Level 2b: Scan overflow chunks. */
		for (chunk = READ_ONCE(tctx->overflow); chunk;
		     chunk = READ_ONCE(chunk->next)) {
			for (i = 0; i < NR_HAZPTR_OVERFLOW_SLOTS; i++) {
				while (smp_load_acquire(&chunk->slots[i].addr) == addr)
					cpu_relax();
			}
		}
	}
	rcu_read_unlock();

	/* Memory ordering: Ensure all slot clears visible. */
	smp_mb();
}
EXPORT_SYMBOL_GPL(hazptr_synchronize);

/*
 * hazptr_fork_init - Initialize hazptr context for new task.
 */
void hazptr_fork_init(struct task_struct *p)
{
	int i;

	p->hazptr_ctx.in_use_count = 0;
	p->hazptr_ctx.overflow = NULL;
	for (i = 0; i < NR_HAZPTR_SLOTS; i++)
		p->hazptr_ctx.slots[i].addr = NULL;
}

/*
 * hazptr_exit - Cleanup hazptr context on task exit.
 */
void hazptr_exit(void)
{
	struct hazptr_task_ctx *tctx = &current->hazptr_ctx;
	struct hazptr_overflow *chunk, *next;
	int i;

	/* Warn if any slots still in use - indicates bug. */
	if (WARN_ON(tctx->in_use_count != 0)) {
		for (i = 0; i < NR_HAZPTR_SLOTS; i++)
			tctx->slots[i].addr = NULL;
		tctx->in_use_count = 0;
	}

	/* Free overflow chunks via RCU (synchronize may be reading them). */
	chunk = tctx->overflow;
	tctx->overflow = NULL;
	while (chunk) {
		next = chunk->next;
		kfree_rcu(chunk, rcu);
		chunk = next;
	}
}

/*
 * hazptr_init - Initialize hazard pointer subsystem.
 *
 * Nothing to do for per-task implementation (init_task handled separately).
 */
void __init hazptr_init(void)
{
	/* Per-task storage is initialized via hazptr_fork_init. */
}
