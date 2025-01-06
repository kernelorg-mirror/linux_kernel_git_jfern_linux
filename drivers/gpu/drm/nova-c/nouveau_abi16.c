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
#include <drm/nova_core_if.h>


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
//		struct nvif_engobj engobj;
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
	struct nouveau_abi16_ntfy *ntfy, *temp;

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
#if 0
	engine = NVIF_ENGINE_GR;

	/* hack to allow channel engine type specification on kepler */
	if (init->fb_ctxdma_handle == ~0) {
		switch (init->tt_ctxdma_handle) {
		case NOUVEAU_FIFO_ENGINE_GR:
			engine = NVIF_ENGINE_GR;
			break;
		case NOUVEAU_FIFO_ENGINE_VP:
			engine = NVIF_ENGINE_MSPDEC;
			break;
		case NOUVEAU_FIFO_ENGINE_PPP:
			engine = NVIF_ENGINE_MSPPP;
				break;
		case NOUVEAU_FIFO_ENGINE_BSP:
			engine = NVIF_ENGINE_MSVLD;
				break;
		case NOUVEAU_FIFO_ENGINE_CE:
			engine = NVIF_ENGINE_CE;
			break;
		default:
			return nouveau_abi16_put(abi16, -ENOSYS);
		}

		init->fb_ctxdma_handle = 0;
		init->tt_ctxdma_handle = 0;
	}

	if (engine != NVIF_ENGINE_CE)
		runm = nvif_fifo_runlist(device, engine);
	else
		runm = nvif_fifo_runlist_ce(device);
#endif
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

#if 0
	/* Workaround "nvc0" gallium driver using classes it doesn't allocate on
	 * Kepler and above.  NVKM no longer always sets CE_CTX_VALID as part of
	 * channel init, now we know what that stuff actually is.
	 *
	 * Doesn't matter for Kepler/Pascal, CE context stored in NV_RAMIN.
	 *
	 * Userspace was fixed prior to adding Ampere support.
	 */
	switch (device->impl->family) {
	case NVIF_DEVICE_VOLTA:
		ret = nvif_engobj_ctor(&chan->chan->chan, "abi16CeWar", 0, VOLTA_DMA_COPY_A,
				       &chan->ce);
		if (ret)
			goto done;
		break;
	case NVIF_DEVICE_TURING:
		ret = nvif_engobj_ctor(&chan->chan->chan, "abi16CeWar", 0, TURING_DMA_COPY_A,
				       &chan->ce);
		if (ret)
			goto done;
		break;
	default:
		break;
	}
#endif
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
#if 0
static int
nouveau_abi16_ioctl_mthd(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	struct nouveau_cli *cli = abi16->cli;
//	struct nvif_device *device = &cli->drm->device;
//	struct nvif_ioctl_mthd_v0 *args;
	struct nouveau_abi16_obj *obj;
//	struct nv_device_info_v0 *info;

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
///		if (obj->type == ENGOBJ)
//			nvif_engobj_dtor(&obj->engobj);
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
	int ret;

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
#if 0

	ret = nvif_engobj_ctor(&chan->chan->chan, "abi16EngObj", args->handle, args->oclass,
			       &obj->engobj);
	if (ret)
		nouveau_abi16_obj_del(obj);
#endif
	return ret;
}

static int
nouveau_abi16_ioctl_sclass(struct nouveau_abi16 *abi16, struct nvif_ioctl_v0 *ioctl, u32 argc)
{
	const struct nvif_device_impl_fifo *fifo;
	const struct nvif_device_impl_runl *runl;
	struct nvif_ioctl_sclass_v0 *args;
	struct nouveau_abi16_chan *chan;
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

	fifo = &chan->chan->cli->drm->device.impl->fifo;
	runl = &fifo->runl[chan->chan->chan.runl];

	for (int engi = 0; engi < runl->engn_nr; engi++) {
		const struct nvif_device_impl_engine *engine =
			&fifo->engine[runl->engn[engi].engine];

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
#endif
int
nouveau_abi16_ioctl(struct drm_file *filp, void __user *user, u32 size)
{
	struct nouveau_abi16 *abi16;
	u32 argc = size;
	int ret;
	struct nvif_ioctl_v0 *ioctl;

	return -EINVAL;
#if 0
	       
		
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

	switch (ioctl->type) {
//	case NVIF_IOCTL_V0_SCLASS: ret = nouveau_abi16_ioctl_sclass(abi16, ioctl, argc); break;
///	case NVIF_IOCTL_V0_NEW   : ret = nouveau_abi16_ioctl_new   (abi16, ioctl, argc); break;
//	case NVIF_IOCTL_V0_DEL   : ret = nouveau_abi16_ioctl_del   (abi16, ioctl, argc); break;
//	case NVIF_IOCTL_V0_MTHD  : ret = nouveau_abi16_ioctl_mthd  (abi16, ioctl, argc); break;
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
#endif
}
