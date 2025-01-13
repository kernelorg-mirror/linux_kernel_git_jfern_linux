#ifndef NOVA_CORE_HELPERS
#define NOVA_CORE_HELPERS

#include <linux/types.h>
#include <linux/errno.h>
#include <asm/io.h>
#include <linux/io.h>

#include <drm/nova_core_if.h>

#include "nvif/push.h"

struct novac_chan {
	struct nova_core_chan nova;
	u8 runl;
	u8 runq;
	
	struct nvif_push push;
};

u64 novac_fifo_runlist(struct nova_core_info *info, u8 eng_type);
int novac_get_engine_inst(struct nova_core_info *info, u8 runl_index, u8 eng_type, u8 *eng_inst);

/* CE-supporting runlists (excluding GRCE, if others exist). */
static inline u64
novac_fifo_runlist_ce(struct nova_core_info *info)
{
        u64 runmgr = novac_fifo_runlist(info, NOVA_CORE_ENGINE_GR);
        u64 runmce = novac_fifo_runlist(info, NOVA_CORE_ENGINE_CE);
        if (runmce && !(runmce &= ~runmgr))
                runmce = runmgr;
        return runmce;
}

static inline bool
nvif_mmu_kind_valid(struct nova_core_mmu *mmu, u8 kind)
{
        if (kind) {
		if (kind >= mmu->info.kind_nr || mmu->info.kind[kind] == mmu->info.kind_inv)
                        return false;
        }
        return true;
}

static inline int
nvif_mmu_type(struct nova_core_mmu *mmu, u8 mask)
{
        int i;
        for (i = 0; i < mmu->info.type_nr; i++) {
                if ((mmu->info.mmu_type[i].mmu_type & mask) == mask)
                        return i;
        }
        return -EINVAL;
}

#endif
