
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

int
novac_get_engine_inst(struct nova_core_info *info, u8 runl_index, u8 eng_type, u8 *eng_inst)
{
	unsigned engi;
	u8 engine;
	int ret = -ENODEV;
	for (engi = 0; engi < info->runl[runl_index].engn_nr; engi++) {
		engine = info->runl[runl_index].engn[engi].engine;

		if (info->engine[engine].eng_type == eng_type) {
			ret = 0;
			break;
		}
	}

	if (ret)
		return ret;

	*eng_inst = info->runl[runl_index].engn[engi].inst;

	printk(KERN_ERR "%s: %d %d\n", __func__, ret, *eng_inst);
	return engi;
}
