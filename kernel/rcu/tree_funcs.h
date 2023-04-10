/* Forward declarations for tree_plugin.h */
static void rcu_bootup_announce(void);
static void rcu_qs(void);
static int rcu_preempt_blocked_readers_cgp(struct rcu_node *rnp);
#ifdef CONFIG_HOTPLUG_CPU
static bool rcu_preempt_has_tasks(struct rcu_node *rnp);
#endif /* #ifdef CONFIG_HOTPLUG_CPU */
static int rcu_print_task_exp_stall(struct rcu_node *rnp);
static void rcu_preempt_check_blocked_tasks(struct rcu_node *rnp);
static void rcu_flavor_sched_clock_irq(int user);
static void dump_blkd_tasks(struct rcu_node *rnp, int ncheck);
static void rcu_initiate_boost(struct rcu_node *rnp, unsigned long flags);
static void rcu_preempt_boost_start_gp(struct rcu_node *rnp);
static bool rcu_is_callbacks_kthread(struct rcu_data *rdp);
static void rcu_cpu_kthread_setup(unsigned int cpu);
static void rcu_spawn_one_boost_kthread(struct rcu_node *rnp);
static bool rcu_preempt_has_tasks(struct rcu_node *rnp);
static bool rcu_preempt_need_deferred_qs(struct task_struct *t);
static void zero_cpu_stall_ticks(struct rcu_data *rdp);
static struct swait_queue_head *rcu_nocb_gp_get(struct rcu_node *rnp);
static void rcu_nocb_gp_cleanup(struct swait_queue_head *sq);
static void rcu_init_one_nocb(struct rcu_node *rnp);
static bool wake_nocb_gp(struct rcu_data *rdp, bool force);
static bool rcu_nocb_flush_bypass(struct rcu_data *rdp, struct rcu_head *rhp,
				  unsigned long j, bool lazy);
static bool rcu_nocb_try_bypass(struct rcu_data *rdp, struct rcu_head *rhp,
				bool *was_alldone, unsigned long flags,
				bool lazy);
static void __call_rcu_nocb_wake(struct rcu_data *rdp, bool was_empty,
				 unsigned long flags);
static int rcu_nocb_need_deferred_wakeup(struct rcu_data *rdp, int level);
static bool do_nocb_deferred_wakeup(struct rcu_data *rdp);
static void rcu_boot_init_nocb_percpu_data(struct rcu_data *rdp);
static void rcu_spawn_cpu_nocb_kthread(int cpu);
static void show_rcu_nocb_state(struct rcu_data *rdp);
static void rcu_nocb_lock(struct rcu_data *rdp);
static void rcu_nocb_unlock(struct rcu_data *rdp);
static void rcu_nocb_unlock_irqrestore(struct rcu_data *rdp,
				       unsigned long flags);
static void rcu_lockdep_assert_cblist_protected(struct rcu_data *rdp);

static void rcu_bind_gp_kthread(void);
static bool rcu_nohz_full_cpu(void);

/* Forward declarations for tree_stall.h */
static void record_gp_stall_check_time(void);
static void rcu_iw_handler(struct irq_work *iwp);
static void check_cpu_stall(struct rcu_data *rdp);
static void rcu_check_gp_start_stall(struct rcu_node *rnp, struct rcu_data *rdp,
				     const unsigned long gpssdelay);

/* Forward declarations for tree_exp.h. */
static void sync_rcu_do_polled_gp(struct work_struct *wp);