/* SPDX-License-Identifier: MIT */
#ifndef __NOUVEAU_DEBUGFS_H__
#define __NOUVEAU_DEBUGFS_H__

#include <drm/drm_debugfs.h>

#if defined(CONFIG_DEBUG_FS)

#include "novac_drv.h"

struct nouveau_debugfs {
};

static inline struct nouveau_debugfs *
nouveau_debugfs(struct drm_device *dev)
{
	return nouveau_drm(dev)->debugfs;
}

extern void  nouveau_drm_debugfs_init(struct drm_minor *);
extern int  nouveau_debugfs_init(struct nouveau_drm *);
extern void nouveau_debugfs_fini(struct nouveau_drm *);
#else
static inline void
nouveau_drm_debugfs_init(struct drm_minor *minor)
{}

static inline int
nouveau_debugfs_init(struct nouveau_drm *drm)
{
	return 0;
}

static inline void
nouveau_debugfs_fini(struct nouveau_drm *drm)
{
}

#endif

#endif
