/*
 * Linux Security Module for Chromium OS
 *
 * Copyright 2011 Google Inc. All Rights Reserved
 *
 * Authors:
 *      Stephan Uphoff  <ups@google.com>
 *      Kees Cook       <keescook@chromium.org>
 *
 * This software is licensed under the terms of the GNU General Public
 * License version 2, as published by the Free Software Foundation, and
 * may be copied, distributed, and modified under those terms.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 */

#define pr_fmt(fmt) "Chromium OS LSM: " fmt

#include <asm/syscall.h>
#include <linux/audit.h>
#include <linux/binfmts.h>
#include <linux/cred.h>
#include <linux/fs.h>
#include <linux/fs_parser.h>
#include <linux/fs_struct.h>
#include <linux/lsm_hooks.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/mount.h>
#include <linux/namei.h>	/* for nameidata_get_total_link_count */
#include <linux/path.h>
#include <linux/ptrace.h>
#include <linux/sched/task_stack.h>
#include <linux/sched.h>	/* current and other task related stuff */
#include <linux/security.h>
#include <linux/shmem_fs.h>
#include <uapi/linux/mount.h>

#include "inode_mark.h"
#include "utils.h"

static int chromiumos_security_sb_mount(const char *dev_name,
					const struct path *path,
					const char *type, unsigned long flags,
					void *data)
{
	return 0;
}

/*
 * NOTE: The WARN() calls will emit a warning in cases of blocked symlink
 * traversal attempts. These will show up in kernel warning reports
 * collected by the crash reporter, so we have some insight on spurious
 * failures that need addressing.
 */
static int chromiumos_security_inode_follow_link(struct dentry *dentry,
						 struct inode *inode, bool rcu)
{
	return 0;
}

static int chromiumos_security_file_open(struct file *file)
{
	return 0;
}

static int chromiumos_sb_eat_lsm_opts(char *options, void **mnt_opts)
{
	char *from = options, *to = options;
	bool found = false;
	bool first = true;

	while (1) {
		char *next = strchr(from, ',');
		int len;

		if (next)
			len = next - from;
		else
			len = strlen(from);

		/*
		 * Remove the option so that filesystems won't see it.
		 * do_mount() has already forced the MS_NOSYMFOLLOW flag on
		 * if it found this option, so no other action is needed.
		 */
		if (len == strlen("nosymfollow") && !strncmp(from, "nosymfollow", len)) {
			found = true;
		} else {
			if (!first) {   /* copy with preceding comma */
				from--;
				len++;
			}
			if (to != from)
				memmove(to, from, len);
			to += len;
			first = false;
		}
		if (!next)
			break;
		from += len + 1;
	}
	*to = '\0';

	if (found)
		pr_notice("nosymfollow option should be changed to MS_NOSYMFOLLOW flag.");

	return 0;
}

static int chromiumos_bprm_creds_for_exec(struct linux_binprm *bprm)
{
	struct file *file = bprm->file;

	if (shmem_file(file)) {
		char *cmdline = printable_cmdline(current);

		audit_log(
			audit_context(),
			GFP_ATOMIC,
			AUDIT_AVC,
			"ChromeOS LSM: memfd execution attempt, cmd=%s, pid=%d",
			cmdline ? cmdline : "(null)",
			task_pid_nr(current));
		kfree(cmdline);

		pr_notice_ratelimited("memfd execution blocked\n");
		return -EACCES;
	}
	return 0;
}

static int chromiumos_locked_down(enum lockdown_reason what)
{
	if (what == LOCKDOWN_BPF_WRITE_USER) {
		pr_notice_ratelimited("BPF_WRITE_USER blocked\n");
		return -EACCES;
	}

	return 0;
}

#ifdef CONFIG_BPF_SYSCALL

static const char secagentd[] = "/usr/sbin/secagentd";

static int chromiumos_bpf(int cmd, union bpf_attr *attr, unsigned int size)
{
	char buf[128];
	int res;
	int len;

	len = strlen(secagentd);
	res = get_cmdline(current, buf, sizeof(buf));
	if (res > 0 && buf[res - 1] == '\0') {
		// null terminated.
		res = res - 1;
	}

	if (res < len || strncmp(buf, secagentd, len)) {
		pr_notice_ratelimited("bpf syscall blocked\n");
		return -EACCES;
	}

	return 0;
}
#endif

static struct security_hook_list chromiumos_security_hooks[] = {
	LSM_HOOK_INIT(sb_mount, chromiumos_security_sb_mount),
	LSM_HOOK_INIT(inode_follow_link, chromiumos_security_inode_follow_link),
	LSM_HOOK_INIT(file_open, chromiumos_security_file_open),
	LSM_HOOK_INIT(sb_eat_lsm_opts, chromiumos_sb_eat_lsm_opts),
	LSM_HOOK_INIT(bprm_creds_for_exec, chromiumos_bprm_creds_for_exec),
	LSM_HOOK_INIT(locked_down, chromiumos_locked_down),
#ifdef CONFIG_BPF_SYSCALL
	LSM_HOOK_INIT(bpf, chromiumos_bpf),
#endif
};

static int __init chromiumos_security_init(void)
{
	security_add_hooks(chromiumos_security_hooks,
			   ARRAY_SIZE(chromiumos_security_hooks), "chromiumos");

	pr_info("enabled");

	return 0;
}
DEFINE_LSM(chromiumos) = {
	.name = "chromiumos",
	.init = chromiumos_security_init
};
