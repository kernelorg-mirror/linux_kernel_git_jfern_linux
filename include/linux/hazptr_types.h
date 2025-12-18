/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_HAZPTR_TYPES_H
#define _LINUX_HAZPTR_TYPES_H

/*
 * Type definitions for per-task hazard pointers.
 * Separated from hazptr.h to avoid circular includes with sched.h.
 */

#include <linux/types.h>

#define NR_HAZPTR_SLOTS          16  /* Fixed slots per task */
#define NR_HAZPTR_OVERFLOW_SLOTS  8  /* Slots per overflow chunk */

/*
 * Hazard pointer slot.
 */
struct hazptr_slot {
	void *addr;
};

/*
 * Overflow chunk - linked list of additional slots.
 * Allocated via GFP_ATOMIC when fixed slots exhausted.
 */
struct hazptr_overflow {
	struct hazptr_slot slots[NR_HAZPTR_OVERFLOW_SLOTS];
	struct hazptr_overflow *next;
	struct rcu_head rcu;  /* For kfree_rcu in exit */
};

/*
 * Per-task hazard pointer storage (embedded in task_struct).
 */
struct hazptr_task_ctx {
	unsigned int in_use_count;  /* Slots in use (single-writer) */
	struct hazptr_slot slots[NR_HAZPTR_SLOTS];
	struct hazptr_overflow *overflow;
};

/*
 * Per-acquire context (caller provides on stack).
 */
struct hazptr_ctx {
	struct hazptr_slot *slot;  /* Slot used for this protection */
};

#endif /* _LINUX_HAZPTR_TYPES_H */
