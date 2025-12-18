// SPDX-License-Identifier: GPL-2.0
/*
 * hazptr: Per-Task Hazard Pointers
 *
 * This API provides existence guarantees of objects through hazard
 * pointers stored per-task (not per-CPU).
 *
 * Benefits over RCU:
 * - Fast reclaim without waiting for grace period
 * - No synchronize_rcu() latency
 *
 * Benefits over per-CPU hazptr:
 * - No preemption concerns (slots stay with task)
 * - No context switch hooks needed
 * - 2-level scan optimization (skip tasks with in_use_count==0)
 *
 * References:
 * [1]: M. M. Michael, "Hazard pointers: safe memory reclamation for
 *      lock-free objects," in IEEE Transactions on Parallel and
 *      Distributed Systems, vol. 15, no. 6, pp. 491-504, June 2004
 */

#ifndef _LINUX_HAZPTR_H
#define _LINUX_HAZPTR_H

#include <linux/hazptr_types.h>
#include <linux/rcupdate.h>

/*
 * hazptr_acquire - Load pointer and protect with hazard pointer.
 * @ctx: Hazard pointer context (caller-allocated on stack)
 * @addr_p: Pointer to the shared pointer to protect
 *
 * Loads @addr_p and protects the loaded pointer with a hazard pointer.
 * Internally retries if pointer changes during protection setup.
 *
 * Returns: Protected address, or NULL if *addr_p is NULL or OOM.
 * On success, @ctx->slot points to the protecting slot.
 */
void *hazptr_acquire(struct hazptr_ctx *ctx, void * const *addr_p);

/*
 * hazptr_release - Release hazard pointer protection.
 * @ctx: Context from hazptr_acquire
 * @addr: The protected address (for validation)
 *
 * Releases the hazard pointer slot. Must be called with the same
 * address that was returned by hazptr_acquire.
 */
void hazptr_release(struct hazptr_ctx *ctx, void *addr);

/*
 * hazptr_synchronize - Wait for all HP references to addr to clear.
 * @addr: Pointer to wait for
 *
 * Scans all tasks and waits until no hazard pointer slot contains @addr.
 * Uses 2-level scan: first checks in_use_count, then scans slots only
 * for tasks with active hazard pointers.
 *
 * Must be called from preemptible context.
 */
void hazptr_synchronize(void *addr);

/*
 * hazptr_init - Initialize hazard pointer subsystem.
 *
 * Called at boot time.
 */
void hazptr_init(void);

/*
 * hazptr_fork_init - Initialize hazptr context for new task.
 * @p: The new task
 *
 * Called from copy_process during fork.
 */
void hazptr_fork_init(struct task_struct *p);

/*
 * hazptr_exit - Cleanup hazptr context on task exit.
 *
 * Called from do_exit. Frees any overflow chunks via kfree_rcu.
 */
void hazptr_exit(void);

#endif /* _LINUX_HAZPTR_H */
