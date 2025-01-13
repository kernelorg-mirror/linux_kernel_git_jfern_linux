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
 * Authors: Ben Skeggs
 */

#include "novac_drv.h"
#include "nouveau_dma.h"
#include "nouveau_bo.h"
#include "nouveau_chan.h"
#include "nouveau_fence.h"
#include "nouveau_abi16.h"
#include "nouveau_vmm.h"
//#include "nouveau_svm.h"

MODULE_PARM_DESC(vram_pushbuf, "Create DMA push buffers in VRAM");
int nouveau_vram_pushbuf;
module_param_named(vram_pushbuf, nouveau_vram_pushbuf, int, 0400);

void
nouveau_channel_kill(struct nouveau_channel *chan)
{
	atomic_set(&chan->killed, 1);
	if (chan->fence)
		nouveau_fence_context_kill(chan->fence, -ENODEV);
}
#if 0
static enum nvif_event_stat
nouveau_channel_killed(struct nvif_event *event, void *repv, u32 repc)
{
	struct nouveau_channel *chan = container_of(event, typeof(*chan), kill);
	struct nouveau_cli *cli = chan->cli;

	NV_PRINTK(warn, cli, "channel %d killed!\n", chan->chid);

	if (unlikely(!atomic_read(&chan->killed)))
		nouveau_channel_kill(chan);

	return NVIF_EVENT_DROP;
}
#endif

int
nouveau_channel_idle(struct nouveau_channel *chan)
{
	if (likely(chan && chan->fence && !atomic_read(&chan->killed))) {
		struct nouveau_cli *cli = chan->cli;
		struct nouveau_fence *fence = NULL;
		int ret;

		ret = nouveau_fence_new(&fence, chan);
		if (!ret) {
			ret = nouveau_fence_wait(fence, false, false);
			nouveau_fence_unref(&fence);
		}

		if (ret) {
			NV_PRINTK(err, cli, "failed to idle channel %d [%s]\n",
				  chan->chid, cli->name);
			return ret;
		}
	}
	return 0;
}

void
nouveau_channel_del(struct nouveau_channel **pchan)
{
	struct nouveau_channel *chan = *pchan;
	if (chan) {
		if (chan->fence)
			nouveau_fence(chan->cli->drm)->context_del(chan);

		nova_core_free_chan(&chan->chan.nova);
		nova_core_free_mem(&chan->userd.mem);
//		if (chan->chan.impl)
//			nouveau_svmm_part(chan->vmm->svmm, chan->inst);


//		nvif_ctxdma_dtor(&chan->vram);
//		nvif_event_dtor(&chan->kill);
//		nvif_object_unmap_cpu(&chan->userd.map);
//		nvif_chan_dtor(&chan->chan);
//		nvif_ctxdma_dtor(&chan->push.ctxdma);
		nouveau_vma_del(&chan->push.vma);
		nouveau_bo_unmap(chan->push.buffer);
		if (chan->push.buffer && chan->push.buffer->bo.pin_count)
			nouveau_bo_unpin(chan->push.buffer);
		nouveau_bo_fini(chan->push.buffer);
		kfree(chan);
	}
	*pchan = NULL;
}

static int
nouveau_channel_kick(struct nvif_push *push)
{
	struct nouveau_channel *chan = container_of(push, typeof(*chan), chan.push);
	chan->dma.cur = chan->dma.cur + (chan->chan.push.cur - chan->chan.push.bgn);
	FIRE_RING(chan);
	chan->chan.push.bgn = chan->chan.push.cur;
	return 0;
}

static int
nouveau_channel_wait(struct nvif_push *push, u32 size)
{
	struct nouveau_channel *chan = container_of(push, typeof(*chan), chan.push);
	int ret;
	chan->dma.cur = chan->dma.cur + (chan->chan.push.cur - chan->chan.push.bgn);
	ret = RING_SPACE(chan, size);
	if (ret == 0) {
		chan->chan.push.bgn = chan->chan.push.ptr;
		chan->chan.push.bgn = chan->chan.push.bgn + chan->dma.cur;
		chan->chan.push.cur = chan->chan.push.bgn;
		chan->chan.push.end = chan->chan.push.bgn + size;
	}
	return ret;
}

static int
nouveau_channel_prep(struct nouveau_cli *cli,
		     u32 size, struct nouveau_channel **pchan)
{
	struct nouveau_drm *drm = cli->drm;
	struct nouveau_channel *chan;
	u32 target;
	int ret;

	chan = *pchan = kzalloc(sizeof(*chan), GFP_KERNEL);
	if (!chan)
		return -ENOMEM;

	chan->cli = cli;
	chan->vmm = nouveau_cli_vmm(cli);
	atomic_set(&chan->killed, 0);

	/* allocate memory for dma push buffer */
	target = NOUVEAU_GEM_DOMAIN_GART | NOUVEAU_GEM_DOMAIN_COHERENT;
	if (nouveau_vram_pushbuf)
		target = NOUVEAU_GEM_DOMAIN_VRAM;

	ret = nouveau_bo_new(cli, size, 0, target, 0, 0, NULL, NULL,
			    &chan->push.buffer);
	if (ret == 0) {
		ret = nouveau_bo_pin(chan->push.buffer, target, false);
		if (ret == 0)
			ret = nouveau_bo_map(chan->push.buffer);
	}

	if (ret) {
		nouveau_channel_del(pchan);
		return ret;
	}
#if 0
	chan->chan.push.mem.object.parent = cli->base.object.parent;
	chan->chan.push.mem.object.client = &cli->base;
	chan->chan.push.mem.object.name = "chanPush";

#endif
	chan->chan.push.ptr = chan->push.buffer->kmap.virtual;
	chan->chan.push.wait = nouveau_channel_wait;
	chan->chan.push.kick = nouveau_channel_kick;

	/* create dma object covering the *entire* memory space that the
	 * pushbuf lives in, this is because the GEM code requires that
	 * we be able to call out to other (indirect) push buffers
	 */
	chan->push.addr = chan->push.buffer->offset;

  	ret = nouveau_vma_new(chan->push.buffer, chan->vmm,
			      &chan->push.vma);
	if (ret) {
		nouveau_channel_del(pchan);
		return ret;
	}

	chan->push.addr = chan->push.vma->addr;
	return 0;
}

static int
nouveau_channel_ctor(struct nouveau_cli *cli, bool priv, u64 runm,
		     struct nouveau_channel **pchan)
{
	const u32 oclass = cli->drm->info.fifo_class;
	struct nouveau_channel *chan;
	const u64 plength = 0x10000;
	const u64 ioffset = plength;
	const u64 ilength = 0x02000;
	char name[TASK_COMM_LEN];
	int ret;
	u64 size;
	struct nova_core_vmm *vmm = NULL;
	struct nova_core_memory_obj *userd = NULL;
	u64 offset, length = 0;
	u16 userd_offset = 0;

	switch (oclass) {
	case  AMPERE_CHANNEL_GPFIFO_B:
	case  AMPERE_CHANNEL_GPFIFO_A:
	case  TURING_CHANNEL_GPFIFO_A:
		break;
	default:
		NV_PRINTK(err, cli, "No supported host channel class (0x%04x)", oclass);
		return -ENODEV;
	}

	size = ioffset + ilength;

	/* allocate dma push buffer */
	ret = nouveau_channel_prep(cli, size, &chan);
	*pchan = chan;
	if (ret)
		return ret;

	chan->runlist = __ffs64(runm);

	/* create channel object */
	vmm = &chan->vmm->vmm;

	offset = ioffset + chan->push.addr;
	length = ilength;

	/* allocate userd */
	int typev = nvif_mmu_type(&cli->mmu,
				  NVIF_MEM_VRAM | NVIF_MEM_COHERENT | NVIF_MEM_MAPPABLE);
	if (!typev)
		return -EINVAL;

	ret = nova_core_alloc_mem(cli->drm->auxdev, &cli->mmu,
				  "abi16ChanUSERD",
				  typev, false, NULL, 0,
				  PAGE_SIZE, &chan->userd.mem);
	if (ret)
		return ret;

	userd = &chan->userd.mem;
	get_task_comm(name, current);
	snprintf(chan->name, sizeof(chan->name), "%s[%d]", name, task_pid_nr(current));

	printk(KERN_ERR "nova userd priv %p\n", chan->userd.mem.obj);

	u32 runl_id = cli->drm->info.runl[chan->runlist].id;
	ret = nova_core_alloc_chan(cli->drm->auxdev, &cli->gsp,
				   runl_id, priv,
				   offset, length, vmm, userd,
				   chan->name,
				   &chan->chan.nova);
	if (ret) {
		nouveau_channel_del(pchan);
		return ret;
	}

	chan->chid = chan->chan.nova.info.id;
	chan->token = chan->chan.nova.info.doorbell_token;
#if 0
	nvif_chan_ctor(device, NULL, chan->name, chan->runlist, 0, &chan->chan);

	chan->inst = chan->chan.impl->inst.addr;

#endif
	return 0;
}

static int
nouveau_channel_init(struct nouveau_channel *chan)
{
	struct nouveau_cli *cli = chan->cli;
	struct nouveau_drm *drm = cli->drm;
	int ret, i;

	ret = nova_core_mem_bar1_map(drm->auxdev,
				     &chan->userd.mem, 0);
	if (ret) {
		printk(KERN_ERR "FAILING BAR1 MAP for USERD\n");
		return ret;
	}

	chan->userd.ptr = chan->userd.mem.bar1_map_handle;
#if 0
	struct nvif_device *device = &cli->device;

	ret = nvif_mem_map(&chan->userd.mem, NULL, 0, &chan->userd.map);
	if (ret)
		return ret;

	ret = nvif_chan_event_ctor(&chan->chan, "abi16ChanKilled",
				   chan->chan.impl->event.killed,
				   nouveau_channel_killed, &chan->kill);
	if (ret == 0)
		ret = nvif_event_allow(&chan->kill);
	if (ret) {
		NV_ERROR(drm, "Failed to request channel kill "
			 "notification: %d\n", ret);
		return ret;
	}
#endif
	/* initialise dma tracking parameters */
	chan->user_put = 0x40;
	chan->user_get = 0x44;
	chan->user_get_hi = 0x60;
	chan->dma.ib_base =  0x10000 / 4;
	chan->dma.ib_max  = NV50_DMA_IB_MAX;
	chan->dma.ib_put  = 0;
	chan->dma.ib_free = chan->dma.ib_max - chan->dma.ib_put;
	chan->dma.max = chan->dma.ib_base;

	chan->dma.put = 0;
	chan->dma.cur = chan->dma.put;
	chan->dma.free = chan->dma.max - chan->dma.cur;

	ret = PUSH_WAIT(&chan->chan.push, NOUVEAU_DMA_SKIPS);
	if (ret)
		return ret;

	for (i = 0; i < NOUVEAU_DMA_SKIPS; i++)
		PUSH_DATA(&chan->chan.push, 0x00000000);

	/* initialise synchronisation */
	return nouveau_fence(drm)->context_new(chan);
}

int
nouveau_channel_new(struct nouveau_cli *cli,
		    bool priv, u64 runm, struct nouveau_channel **pchan)
{
	int ret;

	ret = nouveau_channel_ctor(cli, priv, runm, pchan);
	if (ret) {
		NV_PRINTK(dbg, cli, "channel create, %d\n", ret);
		return ret;
	}

	ret = nouveau_channel_init(*pchan);
	if (ret) {
		NV_PRINTK(err, cli, "channel failed to initialise, %d\n", ret);
		nouveau_channel_del(pchan);
		return ret;
	}
#if 0
	ret = nouveau_svmm_join((*pchan)->vmm->svmm, (*pchan)->inst);
	if (ret)
		nouveau_channel_del(pchan);
#endif
	return ret;
}

void
nouveau_channels_fini(struct nouveau_drm *drm)
{
	kfree(drm->runl);
}

int
nouveau_channels_init(struct nouveau_drm *drm)
{
	int i;

	drm->chan_total = 0;
	drm->runl_nr = drm->info.runl_nr;
	drm->runl = kcalloc(drm->runl_nr, sizeof(*drm->runl), GFP_KERNEL);
	if (!drm->runl)
		return -ENOMEM;

	for (i = 0; i < drm->runl_nr; i++) {
		drm->runl[i].chan_nr = drm->info.runl[i].chan_nr;
		drm->runl[i].chan_id_base = drm->chan_total;
		drm->runl[i].context_base = dma_fence_context_alloc(drm->runl[i].chan_nr);

		drm->chan_total += drm->runl[i].chan_nr;
	}

	return 0;
}
