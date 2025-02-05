/*
 * Copyright (C) 2009 Red Hat <bskeggs@redhat.com>
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 *
 */

/*
 * Authors:
 *  Ben Skeggs <bskeggs@redhat.com>
 */

#include <linux/debugfs.h>
#include "nouveau_debugfs.h"

static void
nouveau_debugfs_gpuva_regions(struct seq_file *m, struct nouveau_uvmm *uvmm)
{
	MA_STATE(mas, &uvmm->region_mt, 0, 0);
	struct nouveau_uvma_region *reg;

	seq_puts  (m, " VA regions  | start              | range              | end                \n");
	seq_puts  (m, "----------------------------------------------------------------------------\n");
	mas_for_each(&mas, reg, ULONG_MAX)
		seq_printf(m, "             | 0x%016llx | 0x%016llx | 0x%016llx\n",
			   reg->va.addr, reg->va.range, reg->va.addr + reg->va.range);
}

static int
nouveau_debugfs_gpuva(struct seq_file *m, void *data)
{
	struct drm_info_node *node = (struct drm_info_node *) m->private;
	struct nouveau_drm *drm = nouveau_drm(node->minor->dev);
	struct nouveau_cli *cli;

	mutex_lock(&drm->clients_lock);
	list_for_each_entry(cli, &drm->clients, head) {
		struct nouveau_uvmm *uvmm = nouveau_cli_uvmm(cli);

		if (!uvmm)
			continue;

		nouveau_uvmm_lock(uvmm);
		drm_debugfs_gpuva_info(m, &uvmm->base);
		seq_puts(m, "\n");
		nouveau_debugfs_gpuva_regions(m, uvmm);
		nouveau_uvmm_unlock(uvmm);
	}
	mutex_unlock(&drm->clients_lock);

	return 0;
}

static struct drm_info_list nouveau_debugfs_list[] = {
	DRM_DEBUGFS_GPUVA_INFO(nouveau_debugfs_gpuva, NULL),
};
#define NOUVEAU_DEBUGFS_ENTRIES ARRAY_SIZE(nouveau_debugfs_list)

void
nouveau_drm_debugfs_init(struct drm_minor *minor)
{
	drm_debugfs_create_files(nouveau_debugfs_list,
				 NOUVEAU_DEBUGFS_ENTRIES,
				 minor->debugfs_root, minor);
}

int
nouveau_debugfs_init(struct nouveau_drm *drm)
{
	drm->debugfs = kzalloc(sizeof(*drm->debugfs), GFP_KERNEL);
	if (!drm->debugfs)
		return -ENOMEM;

	return 0;
}

void
nouveau_debugfs_fini(struct nouveau_drm *drm)
{
	kfree(drm->debugfs);
	drm->debugfs = NULL;
}
