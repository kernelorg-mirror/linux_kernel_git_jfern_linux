#ifndef __NOVAC_DRV_H__
#define __NOVAC_DRV_H__

#define DRIVER_AUTHOR		"NovaC Project"
#define DRIVER_EMAIL		"novac@lists.freedesktop.org"

#define DRIVER_NAME		"nouveau"
#define DRIVER_DESC		"HACKS"
#define DRIVER_DATE		"20120801"

#define DRIVER_MAJOR		1
#define DRIVER_MINOR		4
#define DRIVER_PATCHLEVEL	0

#include <drm/drm_connector.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>

#include <drm/ttm/ttm_bo.h>

#include "uapi/drm/nouveau_drm.h"

struct nouveau_channel;
struct nouveau_drm;

#include <drm/nova_core_if.h>
#include "novac_helpers.h"
#include "nouveau_sched.h"
#include "nouveau_vmm.h"
#include "nouveau_uvmm.h"

struct nouveau_fence;
struct nouveau_cli {
	struct nouveau_drm *drm;
	struct mutex mutex;

	struct nova_core_gsp_client gsp;

	struct nova_core_mmu mmu;
	struct nouveau_vmm vmm;
	struct {
		struct nouveau_uvmm *ptr;
		bool disabled;
	} uvmm;
	struct nouveau_sched *sched;

	struct list_head head;
	void *abi16;
	struct list_head objects;	
	char name[32];

	struct work_struct work;
	struct list_head worker;
	struct mutex lock;
};

struct nouveau_cli_work {
	void (*func)(struct nouveau_cli_work *);
	struct nouveau_cli *cli;
	struct list_head head;

	struct dma_fence *fence;
	struct dma_fence_cb cb;
};

static inline struct nouveau_uvmm *
nouveau_cli_uvmm(struct nouveau_cli *cli)
{
	return cli ? cli->uvmm.ptr : NULL;
}

static inline struct nouveau_uvmm *
nouveau_cli_uvmm_locked(struct nouveau_cli *cli)
{
	struct nouveau_uvmm *uvmm;

	mutex_lock(&cli->mutex);
	uvmm = nouveau_cli_uvmm(cli);
	mutex_unlock(&cli->mutex);

	return uvmm;
}

static inline struct nouveau_vmm *
nouveau_cli_vmm(struct nouveau_cli *cli)
{
	struct nouveau_uvmm *uvmm;

	uvmm = nouveau_cli_uvmm(cli);
	if (uvmm)
		return &uvmm->vmm;

//	if (cli->svm.cli)
//		return &cli->svm;

	return &cli->vmm;
}

static inline void
__nouveau_cli_disable_uvmm_noinit(struct nouveau_cli *cli)
{
	struct nouveau_uvmm *uvmm = nouveau_cli_uvmm(cli);

	if (!uvmm)
		cli->uvmm.disabled = true;
}

static inline void
nouveau_cli_disable_uvmm_noinit(struct nouveau_cli *cli)
{
	mutex_lock(&cli->mutex);
	__nouveau_cli_disable_uvmm_noinit(cli);
	mutex_unlock(&cli->mutex);
}

void nouveau_cli_work_queue(struct nouveau_cli *, struct dma_fence *,
			    struct nouveau_cli_work *);

static inline struct nouveau_cli *
nouveau_cli(struct drm_file *fpriv)
{
	return fpriv ? fpriv->driver_priv : NULL;
}

static inline void
u_free(void *addr)
{
	kvfree(addr);
}

static inline void *
u_memcpya(uint64_t user, unsigned int nmemb, unsigned int size)
{
	void __user *userptr = u64_to_user_ptr(user);
	size_t bytes;

	if (unlikely(check_mul_overflow(nmemb, size, &bytes)))
		return ERR_PTR(-EOVERFLOW);
	return vmemdup_user(userptr, bytes);
}

struct nouveau_drm {
	struct drm_device *dev;

	struct auxiliary_device *auxdev;
	struct nouveau_cli cli;

	struct list_head clients;
	/**
	 * @clients_lock: Protects access to the @clients list of &struct nouveau_cli.
	 */
	struct mutex clients_lock;

	struct nova_core_info info;
	
	/* TTM interface support */
	struct {
		struct ttm_device bdev;
		int (*move)(struct nouveau_channel *,
			    struct ttm_buffer_object *,
			    struct ttm_resource *, struct ttm_resource *);
		struct nouveau_channel *chan;
		struct nova_core_chan_obj copy;
		int mtrr;
		int type_vram;
		int type_host[2];
		int type_ncoh[2];
		struct mutex io_reserve_mutex;
		struct list_head io_reserve_lru;		
	} ttm;

	/* GEM interface support */
	struct {
		u64 vram_available;
		u64 gart_available;
	} gem;

	/* synchronisation */
	void *fence;

	int chan_total; /* Number of channels across all runlists. */
	int runl_nr;
	struct {
		int chan_nr;
		int chan_id_base;
		u64 context_base;		
	} *runl;

	/* Workqueue used for channel schedulers. */
	struct workqueue_struct *sched_wq;

	struct nouveau_channel *cechan;
	struct nouveau_channel *channel;

	struct nova_core_user_info user;

	struct nouveau_debugfs *debugfs;
};

static inline struct nouveau_drm *
nouveau_drm(struct drm_device *dev)
{
	return dev->dev_private;
}

static inline bool
nouveau_drm_use_coherent_gpu_mapping(struct nouveau_drm *drm)
{
	struct nova_core_mmu *mmu = &drm->cli.mmu;
	return !(mmu->info.mmu_type[drm->ttm.type_host[0]].mmu_type & NVIF_MEM_UNCACHED);	
}

#define NV_PRINTK(l,c,f,a...) do {                                             \
	struct nouveau_cli *_cli = (c);                                        \
	dev_##l(_cli->drm->dev->dev, "%s: "f, _cli->name, ##a);                 \
} while(0)

#define NV_PRINTK_(l,drm,f,a...) do {          \
	dev_##l(drm->dev->dev, "drm: "f, ##a);  \
} while(0)
#define NV_FATAL(drm,f,a...) NV_PRINTK_(crit, (drm), f, ##a)
#define NV_ERROR(drm,f,a...) NV_PRINTK_(err, (drm), f, ##a)
#define NV_WARN(drm,f,a...) NV_PRINTK_(warn, (drm), f, ##a)
#define NV_INFO(drm,f,a...) NV_PRINTK_(info, (drm), f, ##a)

#define NV_DEBUG(drm,f,a...) do {                                              \
	if (drm_debug_enabled(DRM_UT_DRIVER))                                  \
		NV_PRINTK_(info, (drm), f, ##a);                               \
} while(0)
#define NV_ATOMIC(drm,f,a...) do {                                             \
	if (drm_debug_enabled(DRM_UT_ATOMIC))                                  \
		NV_PRINTK_(info, (drm), f, ##a);                               \
} while(0)

#define NV_PRINTK_ONCE(l,c,f,a...) NV_PRINTK(l##_once,c,f, ##a)

#define NV_ERROR_ONCE(drm,f,a...) NV_PRINTK_ONCE(err, &(drm)->cli, f, ##a)
#define NV_WARN_ONCE(drm,f,a...) NV_PRINTK_ONCE(warn, &(drm)->cli, f, ##a)
#define NV_INFO_ONCE(drm,f,a...) NV_PRINTK_ONCE(info, &(drm)->cli, f, ##a)

#endif
