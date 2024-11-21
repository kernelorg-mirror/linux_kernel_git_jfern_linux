/* SPDX-License-Identifier: MIT */
/*
 * Copyright © 2024 NVIDIA Corporation
 */

#ifndef __NVKM_VGPU_MGR_VFIO_H__
#define __NVKM_VGPU_MGR_VFIO_H__

enum {
	NVIDIA_VGPU_EVENT_PCI_SRIOV_CONFIGURE = 0,
};

struct nvidia_vgpu_vfio_handle_data {
	void *priv;
};

/* A combo of handles of RmClient and RmDevice */
struct nvidia_vgpu_gsp_client {
	void *gsp_client;
	void *gsp_device;
};

struct nvidia_vgpu_mem {
	u64 addr;
	u64 size;
	void * __iomem bar1_vaddr;
};

struct bootload_vgpu {
	u32 dbdf;
	u32 gfid;
	u32 num_channels;
	u32 chid_offset[62];
	u64 fbmem_heap_addr;
	u64 fbmem_heap_size;
	u64 heap_mem_addr;
	u64 heap_mem_size;
	u64 init_task_log_buf_offset;
	u64 init_task_log_buf_size;
	u64 vgpu_task_log_buf_offset;
	u64 vgpu_task_log_buf_size;	
};
	
struct nvkm_vgpu_mgr_vfio_ops {
	bool (*vgpu_mgr_is_enabled)(void *handle);
	void (*get_handle)(void *handle,
		           struct nvidia_vgpu_vfio_handle_data *data);
	int (*attach_handle)(void *handle,
		             struct nvidia_vgpu_vfio_handle_data *data);
	void (*detach_handle)(void *handle);
	int (*alloc_gsp_client)(void *handle,
				struct nvidia_vgpu_gsp_client *client);
	void (*free_gsp_client)(struct nvidia_vgpu_gsp_client *client);
	u32 (*get_gsp_client_handle)(struct nvidia_vgpu_gsp_client *client);

	int (*shutdown_vgpu_plugin_task)(struct nvidia_vgpu_gsp_client *client,
					 u32 gfid);
	int (*cleanup_vgpu_plugin)(struct nvidia_vgpu_gsp_client *client,
				   u32 gfid);
	int (*bootload_vgpu_plugin_task)(struct nvidia_vgpu_gsp_client *client,
					 const struct bootload_vgpu *params);
	int (*add_vgpu_info)(struct nvidia_vgpu_gsp_client *client,
			     u32 vgpu_info_count,
			     const void *encoded_a081_infos);
	int (*alloc_chids)(void *handle, int count);
	void (*free_chids)(void *handle, int offset, int count);
	struct nvidia_vgpu_mem *(*alloc_fbmem)(void *handle, u64 size,
					       bool vmmu_aligned);
	void (*free_fbmem)(struct nvidia_vgpu_mem *mem);
	int (*bar1_map_mem)(struct nvidia_vgpu_mem *mem);
	void (*bar1_unmap_mem)(struct nvidia_vgpu_mem *mem);
	void (*get_engine_bitmap)(void *handle, unsigned long *bitmap);
};

struct nvkm_vgpu_mgr_vfio_ops *nvkm_vgpu_mgr_get_vfio_ops(void *handle);

#endif
