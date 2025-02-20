/*
 * Copyright 2012 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 */

#include "novac_drv.h"
#include "nouveau_dma.h"
#include "nouveau_exec.h"
#include "nouveau_gem.h"
#include "nouveau_chan.h"
#include "nouveau_abi16.h"
#include "nouveau_vmm.h"
#include "nouveau_sched.h"
#include "novac_helpers.h"
#include <drm/nova_core_if.h>

struct nvif_ioctl_v0 {
        __u8  version;
#define NVIF_IOCTL_V0_SCLASS                                               0x01
#define NVIF_IOCTL_V0_NEW                                                  0x02
#define NVIF_IOCTL_V0_DEL                                                  0x03
#define NVIF_IOCTL_V0_MTHD                                                 0x04
        __u8  type;
        __u8  pad02[4];
#define NVIF_IOCTL_V0_OWNER_NVIF                                           0x00
#define NVIF_IOCTL_V0_OWNER_ANY                                            0xff
        __u8  owner;
#define NVIF_IOCTL_V0_ROUTE_NVIF                                           0x00
#define NVIF_IOCTL_V0_ROUTE_HIDDEN                                         0xff
        __u8  route;
        __u64 token;
        __u64 object;
        __u8  data[];           /* ioctl data (below) */
};

struct nvif_ioctl_sclass_v0 {
	/* nvif_ioctl ... */
	__u8  version;
	__u8  count;
	__u8  pad02[6];
	struct nvif_ioctl_sclass_oclass_v0 {
		__s32 oclass;
		__s16 minver;
		__s16 maxver;
	} oclass[];
};

struct nvif_ioctl_new_v0 {
	/* nvif_ioctl ... */
	__u8  version;
	__u8  pad01[6];
	__u8  route;
	__u64 token;
	__u64 object;
	__u32 handle;
	__s32 oclass;
	__u8  data[];		/* class data (class.h) */
};


struct nvif_ioctl_mthd_v0 {
	/* nvif_ioctl ... */
	__u8  version;
	__u8  method;
	__u8  pad02[6];
	__u8  data[];		/* method data (class.h) */
};

#define NV_DEVICE                                     /* cl0080.h */ 0x00000080
#define NV_DEVICE_V0_INFO                                                  0x00

struct nv_device_info_v0 {
        __u8  version;
#define NV_DEVICE_INFO_V0_IGP                                              0x00
#define NV_DEVICE_INFO_V0_PCI                                              0x01
#define NV_DEVICE_INFO_V0_AGP                                              0x02
#define NV_DEVICE_INFO_V0_PCIE                                             0x03
#define NV_DEVICE_INFO_V0_SOC                                              0x04
        __u8  platform;
        __u16 chipset;  /* from NV_PMC_BOOT_0 */
        __u8  revision; /* from NV_PMC_BOOT_0 */
#define NV_DEVICE_INFO_V0_TURING                                           0x0c
#define NV_DEVICE_INFO_V0_AMPERE                                           0x0d
#define NV_DEVICE_INFO_V0_ADA                                              0x0e
        __u8  family;
        __u8  pad06[2];
        __u64 ram_size;
        __u64 ram_user;
        char  chip[16];
        char  name[64];
};

static struct nouveau_abi16 *
nouveau_abi16(struct drm_file *file_priv)
{
	struct nouveau_cli *cli = nouveau_cli(file_priv);
	if (!cli->abi16) {
		struct nouveau_abi16 *abi16;
		cli->abi16 = abi16 = kzalloc(sizeof(*abi16), GFP_KERNEL);
		if (cli->abi16) {
			abi16->cli = cli;
			INIT_LIST_HEAD(&abi16->channels);
			INIT_LIST_HEAD(&abi16->objects);
		}
	}
	return cli->abi16;
}

struct nouveau_abi16 *
nouveau_abi16_get(struct drm_file *file_priv)
{
	struct nouveau_cli *cli = nouveau_cli(file_priv);
	mutex_lock(&cli->mutex);
	if (nouveau_abi16(file_priv))
		return cli->abi16;
	mutex_unlock(&cli->mutex);
	return NULL;
}

int
nouveau_abi16_put(struct nouveau_abi16 *abi16, int ret)
{
	struct nouveau_cli *cli = abi16->cli;
	mutex_unlock(&cli->mutex);
	return ret;
}

/* Tracks objects created via the DRM_NOUVEAU_NVIF ioctl.
 *
 * The only two types of object that userspace ever allocated via this
 * interface are 'device', in order to retrieve basic device info, and
 * 'engine objects', which instantiate HW classes on a channel.
 *
 * The remainder of what used to be available via DRM_NOUVEAU_NVIF has
 * been removed, but these object types need to be tracked to maintain
 * compatibility with userspace.
 */
struct nouveau_abi16_obj {
	enum nouveau_abi16_obj_type {
		DEVICE,
		ENGOBJ,
	} type;
	u64 object;

	union {
		struct nova_core_chan_obj obj;
	};

	struct list_head head; /* protected by nouveau_abi16.cli.mutex */
};

static struct nouveau_abi16_obj *
nouveau_abi16_obj_find(struct nouveau_abi16 *abi16, u64 object)
{
	struct nouveau_abi16_obj *obj;

	list_for_each_entry(obj, &abi16->objects, head) {
		if (obj->object == object)
			return obj;
	}

	return NULL;
}

static void
nouveau_abi16_obj_del(struct nouveau_abi16_obj *obj)
{
	if (obj->type == ENGOBJ)
		nova_core_chan_free_object(&obj->obj);
	
	list_del(&obj->head);
	kfree(obj);
}

static struct nouveau_abi16_obj *
nouveau_abi16_obj_new(struct nouveau_abi16 *abi16, enum nouveau_abi16_obj_type type, u64 object)
{
	struct nouveau_abi16_obj *obj;

	obj = nouveau_abi16_obj_find(abi16, object);
	if (obj)
		return ERR_PTR(-EEXIST);

	obj = kzalloc(sizeof(*obj), GFP_KERNEL);
	if (!obj)
		return ERR_PTR(-ENOMEM);

	obj->type = type;
	obj->object = object;
	list_add_tail(&obj->head, &abi16->objects);
	return obj;
}

s32
nouveau_abi16_swclass(struct nouveau_drm *drm)
{
//	return nvif_fifo_engine_oclass(&drm->device, NVIF_ENGINE_SW);
	return 0;
}

static void
nouveau_abi16_chan_fini(struct nouveau_abi16 *abi16,
			struct nouveau_abi16_chan *chan)
{
	/* Cancel all jobs from the entity's queue. */
	if (chan->sched)
		drm_sched_entity_fini(&chan->sched->entity);

	if (chan->chan)
		nouveau_channel_idle(chan->chan);

	if (chan->sched)
		nouveau_sched_destroy(&chan->sched);

	/* destroy channel object, all children will be killed too */
	if (chan->chan) {
//		nvif_engobj_dtor(&chan->ce);
		nouveau_channel_del(&chan->chan);
	}

	list_del(&chan->head);
	kfree(chan);
}

void
nouveau_abi16_fini(struct nouveau_abi16 *abi16)
{
	struct nouveau_cli *cli = abi16->cli;
	struct nouveau_abi16_chan *chan, *temp;
	struct nouveau_abi16_obj *obj, *tmp;

	/* cleanup objects */
	list_for_each_entry_safe(obj, tmp, &abi16->objects, head) {
		nouveau_abi16_obj_del(obj);
	}

	/* cleanup channels */
	list_for_each_entry_safe(chan, temp, &abi16->channels, head) {
		nouveau_abi16_chan_fini(abi16, chan);
	}

	kfree(cli->abi16);
	cli->abi16 = NULL;
}

int
nouveau_abi16_ioctl_getparam(ABI16_IOCTL_ARGS)
{
	struct nouveau_cli *cli = nouveau_cli(file_priv);
	struct nouveau_drm *drm = nouveau_drm(dev);
	struct drm_nouveau_getparam *getparam = data;

	switch (getparam->param) {
	case NOUVEAU_GETPARAM_CHIPSET_ID:
		getparam->value = drm->info.chipset;
		break;
	case NOUVEAU_GETPARAM_PCI_VENDOR:
		getparam->value = drm->info.pci_vendor_id;
		break;
	case NOUVEAU_GETPARAM_PCI_DEVICE:
		getparam->value = drm->info.pci_device_id;
		break;
	case NOUVEAU_GETPARAM_BUS_TYPE:
		getparam->value = 2;//PCIE
		break;
	case NOUVEAU_GETPARAM_FB_SIZE:
		getparam->value = drm->gem.vram_available;
		break;
	case NOUVEAU_GETPARAM_AGP_SIZE:
		getparam->value = drm->gem.gart_available;
		break;
	case NOUVEAU_GETPARAM_VM_VRAM_BASE:
		getparam->value = 0; /* deprecated */
		break;
	case NOUVEAU_GETPARAM_PTIMER_TIME:
		getparam->value = nova_core_timer_time(drm->auxdev);
		break;
	case NOUVEAU_GETPARAM_HAS_BO_USAGE:
		getparam->value = 1;
		break;
	case NOUVEAU_GETPARAM_HAS_PAGEFLIP:
		getparam->value = 1;
		break;
	case NOUVEAU_GETPARAM_GRAPH_UNITS:
		getparam->value = drm->info.gr_units;
		break;
	case NOUVEAU_GETPARAM_EXEC_PUSH_MAX: {
		int ib_max = 0;

		ib_max = NV50_DMA_IB_MAX;

		getparam->value = nouveau_exec_push_max_from_ib_max(ib_max);
		break;
	}
	case NOUVEAU_GETPARAM_VRAM_BAR_SIZE:
		getparam->value = drm->info.resource_size[1];
		break;
	case NOUVEAU_GETPARAM_VRAM_USED: {
		struct ttm_resource_manager *vram_mgr = ttm_manager_type(&drm->ttm.bdev, TTM_PL_VRAM);
		getparam->value = (u64)ttm_resource_manager_usage(vram_mgr);
		break;
	}
	case NOUVEAU_GETPARAM_HAS_VMA_TILEMODE:
		getparam->value = 1;
		break;
	default:
		NV_PRINTK(dbg, cli, "unknown parameter %lld\n", getparam->param);
		return -EINVAL;
	}

	return 0;
}

int
nouveau_abi16_ioctl_channel_alloc(ABI16_IOCTL_ARGS)
{
	struct drm_nouveau_channel_alloc *init = data;
	struct nouveau_cli *cli = nouveau_cli(file_priv);
	struct nouveau_drm *drm = nouveau_drm(dev);
	struct nouveau_abi16 *abi16 = nouveau_abi16_get(file_priv);
	struct nouveau_abi16_chan *chan;
//	struct nvif_device *device = &cli->device;
	u64 engine, runm;
	int ret;

	if (unlikely(!abi16))
		return -ENOMEM;

	if (!drm->channel)
		return nouveau_abi16_put(abi16, -ENODEV);

	/* If uvmm wasn't initialized until now disable it completely to prevent
	 * userspace from mixing up UAPIs.
	 *
	 * The client lock is already acquired by nouveau_abi16_get().
	 */
	__nouveau_cli_disable_uvmm_noinit(cli);
	engine = NOVA_CORE_ENGINE_GR;

	/* hack to allow channel engine type specification on kepler */
	if (init->fb_ctxdma_handle == ~0) {
		switch (init->tt_ctxdma_handle) {
		case NOUVEAU_FIFO_ENGINE_GR:
			engine = NOVA_CORE_ENGINE_GR;
			break;
		case NOUVEAU_FIFO_ENGINE_CE:
			engine = NOVA_CORE_ENGINE_CE;
			break;
		default:
			return nouveau_abi16_put(abi16, -ENOSYS);
		}

		init->fb_ctxdma_handle = 0;
		init->tt_ctxdma_handle = 0;
	}

	if (engine != NOVA_CORE_ENGINE_CE)
		runm = novac_fifo_runlist(&drm->info, engine);
	else
		runm = novac_fifo_runlist_ce(&drm->info);

	if (!runm || init->fb_ctxdma_handle == ~0 || init->tt_ctxdma_handle == ~0)
		return nouveau_abi16_put(abi16, -EINVAL);

	/* allocate "abi16 channel" data and make up a handle for it */
	chan = kzalloc(sizeof(*chan), GFP_KERNEL);
	if (!chan)
		return nouveau_abi16_put(abi16, -ENOMEM);

	list_add(&chan->head, &abi16->channels);

	/* create channel object and initialise dma and fence management */
	ret = nouveau_channel_new(cli, false, runm, &chan->chan);
	if (ret)
		goto done;

	/* If we're not using the VM_BIND uAPI, we don't need a scheduler.
	 *
	 * The client lock is already acquired by nouveau_abi16_get().
	 */
	if (nouveau_cli_uvmm(cli)) {
		ret = nouveau_sched_create(&chan->sched, drm, drm->sched_wq,
					   chan->chan->dma.ib_max);
		if (ret)
			goto done;
	}

	init->channel = chan->chan->chid;

	init->pushbuf_domains = NOUVEAU_GEM_DOMAIN_VRAM |
		NOUVEAU_GEM_DOMAIN_GART;

done:
	if (ret)
		nouveau_abi16_chan_fini(abi16, chan);
	return nouveau_abi16_put(abi16, ret);
}

static struct nouveau_abi16_chan *
nouveau_abi16_chan(struct nouveau_abi16 *abi16, int channel)
{
	struct nouveau_abi16_chan *chan;

	list_for_each_entry(chan, &abi16->channels, head) {
		if (chan->chan->chid == channel)
			return chan;
	}

	return NULL;
}

int
nouveau_abi16_ioctl_channel_free(ABI16_IOCTL_ARGS)
{
	struct drm_nouveau_channel_free *req = data;
	struct nouveau_abi16 *abi16 = nouveau_abi16_get(file_priv);
	struct nouveau_abi16_chan *chan;

	if (unlikely(!abi16))
		return -ENOMEM;

	chan = nouveau_abi16_chan(abi16, req->channel);
	if (!chan)
		return nouveau_abi16_put(abi16, -ENOENT);
	nouveau_abi16_chan_fini(abi16, chan);
	return nouveau_abi16_put(abi16, 0);
}

int
nouveau_abi16_ioctl_grobj_alloc(ABI16_IOCTL_ARGS)
{
	return -ENOSYS;
}

int
nouveau_abi16_ioctl_notifierobj_alloc(ABI16_IOCTL_ARGS)
{
	return -ENOSYS;
}

int
nouveau_abi16_ioctl_gpuobj_free(ABI16_IOCTL_ARGS)
{
	return -ENOSYS;
}

static int
nouveau_abi16_ioctl_mthd(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	struct nouveau_cli *cli = abi16->cli;
	struct nvif_ioctl_mthd_v0 *args;
	struct nouveau_abi16_obj *obj;
	struct nv_device_info_v0 *info;

	if (ioctl->route || argc < sizeof(*args))
		return -EINVAL;
	args = (void *)ioctl->data;
	argc -= sizeof(*args);

	obj = nouveau_abi16_obj_find(abi16, ioctl->object);
	if (!obj || obj->type != DEVICE)
		return -EINVAL;

	if (args->method != NV_DEVICE_V0_INFO ||
	    argc != sizeof(*info))
		return -EINVAL;

	info = (void *)args->data;
	if (info->version != 0x00)
		return -EINVAL;

	info->platform = 3;
	info->chipset = cli->drm->info.chipset;
	info->revision = 0;//cli->drm->info.revision;
	info->ram_size = cli->drm->info.ram_user;
	info->ram_user = cli->drm->info.ram_user;	
#if 0
	info->platform = device->impl->platform;
	info->chipset = device->impl->chipset;
	info->revision = device->impl->revision;
	info->family = device->impl->family;
	info->ram_size = device->impl->ram_size;
	info->ram_user = device->impl->ram_user;
	strscpy(info->chip, device->impl->chip, sizeof(info->chip));
	strscpy(info->name, device->impl->name, sizeof(info->name));
#endif
	return 0;
}

static int
nouveau_abi16_ioctl_del(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	struct nouveau_abi16_obj *obj;

	if (ioctl->route || argc)
		return -EINVAL;

	obj = nouveau_abi16_obj_find(abi16, ioctl->object);
	if (obj) {
		nouveau_abi16_obj_del(obj);
	}

	return 0;
}

static int
nouveau_abi16_ioctl_new(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	struct nvif_ioctl_new_v0 *args;
	struct nouveau_abi16_chan *chan;
	struct nouveau_abi16_obj *obj;
	struct nouveau_drm *drm;
	int ret;
	u32 engine_type, eng_inst;

	if (argc < sizeof(*args))
		return -EINVAL;
	args = (void *)ioctl->data;
	argc -= sizeof(*args);

	if (args->version != 0)
		return -EINVAL;

	if (!ioctl->route) {
		if (ioctl->object || args->oclass != NV_DEVICE)
			return -EINVAL;

		obj = nouveau_abi16_obj_new(abi16, DEVICE, args->object);
		if (IS_ERR(obj))
			return PTR_ERR(obj);

		return 0;
	}

	chan = nouveau_abi16_chan(abi16, ioctl->token);
	if (!chan)
		return -EINVAL;
	obj = nouveau_abi16_obj_new(abi16, ENGOBJ, args->object);
	if (IS_ERR(obj))
		return PTR_ERR(obj);

	drm = chan->chan->cli->drm;

	ret = novac_find_engine_info(&drm->info, chan->chan->runlist, args->oclass, &engine_type, &eng_inst);
	if (ret) {
		printk(KERN_ERR "failed to find class on runlist %d %08x\n", chan->chan->runlist, args->oclass);
		return -EINVAL;
	}

	if (engine_type == NOVA_CORE_ENGINE_GR && !chan->chan->chan.nova.gr_ctx_arc) {
		ret = nova_core_chan_init_gr(drm->auxdev,
					     &chan->chan->cli->gsp,
					     &chan->chan->vmm->vmm,
					     &chan->chan->chan.nova);
		if (ret) {
			printk(KERN_ERR "Failed to allocate GR object\n");
			return ret;
		}
	}
	
	obj->obj.class = args->oclass;
	obj->obj.handle = args->handle;
	obj->obj.engine_type = engine_type;
	obj->obj.engine_inst = eng_inst;
	ret = nova_core_chan_alloc_object(&chan->chan->chan.nova,
					  &obj->obj);

	return ret;
}

static int
nouveau_abi16_ioctl_sclass(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	const struct runl *runl;
	struct nvif_ioctl_sclass_v0 *args;
	struct nouveau_abi16_chan *chan;
	struct nova_core_info *info;
	int cnt = 0;

	if (!ioctl->route || argc < sizeof(*args))
		return -EINVAL;
	args = (void *)ioctl->data;
	argc -= sizeof(*args);

	if (argc != args->count * sizeof(args->oclass[0]))
		return -EINVAL;

	chan = nouveau_abi16_chan(abi16, ioctl->token);
	if (!chan)
		return -EINVAL;

	info = &chan->chan->cli->drm->info;
	runl = &info->runl[chan->chan->chan.runl];

	for (int engi = 0; engi < runl->engn_nr; engi++) {
		const struct engine *engine =
			&info->engine[runl->engn[engi].engine];

		for (int clsi = 0; clsi < engine->oclass_nr; clsi++) {
			if (cnt < args->count) {
				args->oclass[cnt].oclass = engine->oclass[clsi];
				args->oclass[cnt].minver = -1;
				args->oclass[cnt].maxver = -1;
			}
			cnt++;
		}
	}

	args->count = cnt;
	return 0;
}

int
nouveau_abi16_ioctl(struct drm_file *filp, void __user *user, u32 size)
{
	struct nouveau_abi16 *abi16;
	u32 argc = size;
	int ret;
	struct nvif_ioctl_v0 *ioctl;

	if (argc < sizeof(*ioctl))
		return -EINVAL;
	argc -= sizeof(*ioctl);

	ioctl = kmalloc(size, GFP_KERNEL);
	if (!ioctl)
		return -ENOMEM;

	ret = -EFAULT;
	if (copy_from_user(ioctl, user, size))
		goto done_free;

	if (ioctl->version != 0x00 ||
	    (ioctl->route && ioctl->route != 0xff)) {
		ret = -EINVAL;
		goto done_free;
	}

	abi16 = nouveau_abi16_get(filp);
	if (unlikely(!abi16)) {
		ret = -ENOMEM;
		goto done_free;
	}


	printk(KERN_ERR "NVIF %d\n", ioctl->type);
	switch (ioctl->type) {
	case NVIF_IOCTL_V0_SCLASS: ret = nouveau_abi16_ioctl_sclass(abi16, ioctl, argc); break;
	case NVIF_IOCTL_V0_NEW   : ret = nouveau_abi16_ioctl_new   (abi16, ioctl, argc); break;
	case NVIF_IOCTL_V0_DEL   : ret = nouveau_abi16_ioctl_del   (abi16, ioctl, argc); break;
	case NVIF_IOCTL_V0_MTHD  : ret = nouveau_abi16_ioctl_mthd  (abi16, ioctl, argc); break;
	default:
		ret = -EINVAL;
		break;
	}

	nouveau_abi16_put(abi16, 0);

	if (ret == 0) {
		if (copy_to_user(user, ioctl, size))
			ret = -EFAULT;
	}

done_free:
	kfree(ioctl);
	return ret;
}
