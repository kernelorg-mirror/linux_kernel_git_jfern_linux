#include <linux/export.h>

#ifdef CONFIG_NOVA_CORE_VGPU_SUPPORT
extern int nvkm_vgpu_mgr_get_vfio_ops;

EXPORT_SYMBOL_GPL(nvkm_vgpu_mgr_get_vfio_ops);
#endif

extern int nova_core_fill_info;

EXPORT_SYMBOL_GPL(nova_core_fill_info);

extern int nova_core_alloc_gsp_client;
EXPORT_SYMBOL_GPL(nova_core_alloc_gsp_client);

extern int nova_core_free_gsp_client;
EXPORT_SYMBOL_GPL(nova_core_free_gsp_client);

extern int nova_core_alloc_mmu;
EXPORT_SYMBOL_GPL(nova_core_alloc_mmu);

extern int nova_core_free_mmu;
EXPORT_SYMBOL_GPL(nova_core_free_mmu);

extern int nova_core_alloc_vmm;
EXPORT_SYMBOL_GPL(nova_core_alloc_vmm);

extern int nova_core_free_vmm;
EXPORT_SYMBOL_GPL(nova_core_free_vmm);

extern int nova_core_alloc_mem;
EXPORT_SYMBOL_GPL(nova_core_alloc_mem);

extern int nova_core_free_mem;
EXPORT_SYMBOL_GPL(nova_core_free_mem);

extern int nova_core_vmm_get;
EXPORT_SYMBOL_GPL(nova_core_vmm_get);

extern int nova_core_vmm_put;
EXPORT_SYMBOL_GPL(nova_core_vmm_put);

extern int nova_core_vmm_map;
EXPORT_SYMBOL_GPL(nova_core_vmm_map);

extern int nova_core_vmm_unmap;
EXPORT_SYMBOL_GPL(nova_core_vmm_unmap);

extern int nova_core_timer_time;
EXPORT_SYMBOL_GPL(nova_core_timer_time);

extern int nova_core_alloc_chan;
EXPORT_SYMBOL_GPL(nova_core_alloc_chan);

extern int nova_core_free_chan;
EXPORT_SYMBOL_GPL(nova_core_free_chan);

extern int nova_core_mem_bar1_map;
EXPORT_SYMBOL_GPL(nova_core_mem_bar1_map);

extern int nova_core_mem_bar1_unmap;
EXPORT_SYMBOL_GPL(nova_core_mem_bar1_unmap);

extern int nova_core_map_user;
EXPORT_SYMBOL_GPL(nova_core_map_user);

extern int nova_core_unmap_user;
EXPORT_SYMBOL_GPL(nova_core_unmap_user);

extern int nova_core_chan_alloc_object;
EXPORT_SYMBOL_GPL(nova_core_chan_alloc_object);

extern int nova_core_chan_free_object;
EXPORT_SYMBOL_GPL(nova_core_chan_free_object);

extern int nova_core_chan_register_nonstall;
EXPORT_SYMBOL_GPL(nova_core_chan_register_nonstall);
