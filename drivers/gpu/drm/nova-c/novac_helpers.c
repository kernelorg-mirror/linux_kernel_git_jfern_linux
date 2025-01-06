
#include <linux/bits.h>
#include "novac_helpers.h"

u64
novac_fifo_runlist(struct nova_core_info *info, u8 eng_type)
{
        u64 runm = 0;

        for (int i = 0; i < info->runl_nr; i++) {
                for (int j = 0; j < info->runl[i].engn_nr; j++) {
                        if (info->engine[info->runl[i].engn[j].engine].eng_type == eng_type) {
				runm |= BIT_ULL(i);
                                continue;
                        }
                }
        }
	
        return runm;
}
