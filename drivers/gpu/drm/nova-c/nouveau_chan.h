/* SPDX-License-Identifier: MIT */
#ifndef __NOUVEAU_CHAN_H__
#define __NOUVEAU_CHAN_H__

#include "novac_helpers.h"
#include <nvif/push.h>

struct nouveau_channel {
	char name[TASK_COMM_LEN+16];
	struct novac_chan chan;
	
	struct nouveau_cli *cli;
	struct nouveau_vmm *vmm;

	struct {
		struct nova_core_memory_obj mem;
  //		struct nvif_map map;
		void *ptr;
	} userd;

	int runlist;
	int chid;
	u64 inst;
	u32 token;

	struct {
		struct nouveau_bo *buffer;
		struct nouveau_vma *vma;
	  //		struct nvif_ctxdma ctxdma;
		u64 addr;
	} push;

	/* TODO: this will be reworked in the near future */
	bool accel_done;
	void *fence;
	struct {
		int max;
		int free;
		int cur;
		int put;
		int ib_base;
		int ib_max;
		int ib_free;
		int ib_put;
	} dma;
	u32 user_get_hi;
	u32 user_get;
	u32 user_put;

  //	struct nvif_event kill;
	atomic_t killed;
};

int nouveau_channels_init(struct nouveau_drm *);
void nouveau_channels_fini(struct nouveau_drm *);

int  nouveau_channel_new(struct nouveau_cli *, bool priv, u64 runm,
			 struct nouveau_channel **);
void nouveau_channel_del(struct nouveau_channel **);
int  nouveau_channel_idle(struct nouveau_channel *);
void nouveau_channel_kill(struct nouveau_channel *);

extern int nouveau_vram_pushbuf;

static inline u32
nvif_userd_rd32(struct nouveau_channel *chan, u32 offset)
{
	return ioread32((u8 __iomem *)chan->userd.ptr + offset);
}

static inline void
nvif_userd_wr32(struct nouveau_channel *chan, u32 offset, u32 val)
{
	iowrite32(val, (u8 __iomem *)chan->userd.ptr + offset);
}

#endif
