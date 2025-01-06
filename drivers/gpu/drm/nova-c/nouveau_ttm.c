// SPDX-License-Identifier: GPL-2.0 OR MIT
/*
 * Copyright (c) 2007-2008 Tungsten Graphics, Inc., Cedar Park, TX., USA,
 * Copyright (c) 2009 VMware, Inc., Palo Alto, CA., USA,
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sub license,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial portions
 * of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT. IN NO EVENT SHALL
 * THE COPYRIGHT HOLDERS, AUTHORS AND/OR ITS SUPPLIERS BE LIABLE FOR ANY CLAIM,
 * DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
 * OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
 * USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

#include <linux/limits.h>

#include <drm/ttm/ttm_range_manager.h>
#include <drm/drm_cache.h>

#include "novac_drv.h"
#include "nouveau_gem.h"
#include "nouveau_mem.h"
#include "nouveau_ttm.h"

static void
nouveau_manager_del(struct ttm_resource_manager *man,
		    struct ttm_resource *reg)
{
  	nouveau_mem_del(man, reg);
}

static bool
nouveau_manager_intersects(struct ttm_resource_manager *man,
			   struct ttm_resource *res,
			   const struct ttm_place *place,
			   size_t size)
{
  	return nouveau_mem_intersects(res, place, size);
}

static bool
nouveau_manager_compatible(struct ttm_resource_manager *man,
			   struct ttm_resource *res,
			   const struct ttm_place *place,
			   size_t size)
{
  	return nouveau_mem_compatible(res, place, size);
}

static int
nouveau_vram_manager_new(struct ttm_resource_manager *man,
			 struct ttm_buffer_object *bo,
			 const struct ttm_place *place,
			 struct ttm_resource **res)
{
	struct nouveau_bo *nvbo = nouveau_bo(bo);
	struct nouveau_drm *drm = nouveau_bdev(bo->bdev);
	int ret;

	//TODO
	//	if (drm->device.impl->ram_size == 0)
	//		return -ENOMEM;

	ret = nouveau_mem_new(drm, nvbo->kind, nvbo->comp, res);
	if (ret)
		return ret;

	ttm_resource_init(bo, place, *res);

	ret = nouveau_mem_vram(*res, nvbo->contig, nvbo->page);
	if (ret) {
		nouveau_mem_del(man, *res);
		return ret;
	}

	return 0;
}

const struct ttm_resource_manager_func nouveau_vram_manager = {
	.alloc = nouveau_vram_manager_new,
	.free = nouveau_manager_del,
	.intersects = nouveau_manager_intersects,
	.compatible = nouveau_manager_compatible,
};

static int
nouveau_gart_manager_new(struct ttm_resource_manager *man,
			 struct ttm_buffer_object *bo,
			 const struct ttm_place *place,
			 struct ttm_resource **res)
{
	struct nouveau_bo *nvbo = nouveau_bo(bo);
	struct nouveau_drm *drm = nouveau_bdev(bo->bdev);
	int ret;

	ret = nouveau_mem_new(drm, nvbo->kind, nvbo->comp, res);
	if (ret)
		return ret;

	ttm_resource_init(bo, place, *res);
	(*res)->start = 0;
	return 0;
}

const struct ttm_resource_manager_func nouveau_gart_manager = {
	.alloc = nouveau_gart_manager_new,
	.free = nouveau_manager_del,
	.intersects = nouveau_manager_intersects,
	.compatible = nouveau_manager_compatible,
};

static int
nouveau_ttm_init_host(struct nouveau_drm *drm, u8 kind)
{
	int typei;

	typei = nvif_mmu_type(&drm->cli.mmu, NVIF_MEM_HOST | NVIF_MEM_MAPPABLE |
			      kind | NVIF_MEM_COHERENT);
	if (typei < 0)
		return -ENOSYS;

	drm->ttm.type_host[!!kind] = typei;

	typei = nvif_mmu_type(&drm->cli.mmu, NVIF_MEM_HOST | NVIF_MEM_MAPPABLE | kind);
	if (typei < 0)
		return -ENOSYS;
	drm->ttm.type_ncoh[!!kind] = typei;
	return 0;
}

static int
nouveau_ttm_init_vram(struct nouveau_drm *drm)
{
	struct ttm_resource_manager *man = kzalloc(sizeof(*man), GFP_KERNEL);

	if (!man)
		return -ENOMEM;

	man->func = &nouveau_vram_manager;

	ttm_resource_manager_init(man, &drm->ttm.bdev,
				  drm->gem.vram_available >> PAGE_SHIFT);
	ttm_set_driver_manager(&drm->ttm.bdev, TTM_PL_VRAM, man);
	ttm_resource_manager_set_used(man, true);
	return 0;
}

static void
nouveau_ttm_fini_vram(struct nouveau_drm *drm)
{
	struct ttm_resource_manager *man = ttm_manager_type(&drm->ttm.bdev, TTM_PL_VRAM);

	ttm_resource_manager_set_used(man, false);
	ttm_resource_manager_evict_all(&drm->ttm.bdev, man);
	ttm_resource_manager_cleanup(man);
	ttm_set_driver_manager(&drm->ttm.bdev, TTM_PL_VRAM, NULL);
	kfree(man);
}

static int
nouveau_ttm_init_gtt(struct nouveau_drm *drm)
{
	struct ttm_resource_manager *man;
	unsigned long size_pages = drm->gem.gart_available >> PAGE_SHIFT;
	const struct ttm_resource_manager_func *func = NULL;

	func = &nouveau_gart_manager;

	man = kzalloc(sizeof(*man), GFP_KERNEL);
	if (!man)
		return -ENOMEM;

	man->func = func;
	man->use_tt = true;
	ttm_resource_manager_init(man, &drm->ttm.bdev, size_pages);
	ttm_set_driver_manager(&drm->ttm.bdev, TTM_PL_TT, man);
	ttm_resource_manager_set_used(man, true);
	return 0;
}

static void
nouveau_ttm_fini_gtt(struct nouveau_drm *drm)
{
	struct ttm_resource_manager *man = ttm_manager_type(&drm->ttm.bdev, TTM_PL_TT);

	ttm_resource_manager_set_used(man, false);
	ttm_resource_manager_evict_all(&drm->ttm.bdev, man);
	ttm_resource_manager_cleanup(man);
	ttm_set_driver_manager(&drm->ttm.bdev, TTM_PL_TT, NULL);
	kfree(man);
}

int
nouveau_ttm_init(struct nouveau_drm *drm)
{
	struct drm_device *dev = &drm->dev;
	int typei, ret;

	ret = nouveau_ttm_init_host(drm, 0);
	if (ret)
		return ret;

	ret = nouveau_ttm_init_host(drm, NVIF_MEM_KIND);
	if (ret)
		return ret;

	typei = nvif_mmu_type(&drm->cli.mmu, NVIF_MEM_VRAM | NVIF_MEM_MAPPABLE |
			      NVIF_MEM_KIND |
			      NVIF_MEM_COMP |
			      NVIF_MEM_DISP);
	if (typei < 0)
		return -ENOSYS;

	drm->ttm.type_vram = typei;

	ret = ttm_device_init(&drm->ttm.bdev, &nouveau_bo_driver, dev->dev->parent,
			      dev->anon_inode->i_mapping,
			      dev->vma_offset_manager,
			      drm_need_swiotlb(drm->cli.mmu.info.dmabits),
			      drm->cli.mmu.info.dmabits <= 32);
	if (ret) {
		NV_ERROR(drm, "error initialising bo driver, %d\n", ret);
		return ret;
	}

	/* VRAM init */
	drm->gem.vram_available = drm->info.ram_user;

	arch_io_reserve_memtype_wc(drm->info.resource_addr[1],
				   drm->info.resource_size[1]);

	ret = nouveau_ttm_init_vram(drm);
	if (ret) {
		NV_ERROR(drm, "VRAM mm init failed, %d\n", ret);
		return ret;
	}

	drm->ttm.mtrr = arch_phys_wc_add(drm->info.resource_addr[1],
					 drm->info.resource_size[1]);

	/* GART init */
	drm->gem.gart_available = drm->cli.vmm.vmm.info.limit;

	ret = nouveau_ttm_init_gtt(drm);
	if (ret) {
		NV_ERROR(drm, "GART mm init failed, %d\n", ret);
		return ret;
	}

	mutex_init(&drm->ttm.io_reserve_mutex);
	INIT_LIST_HEAD(&drm->ttm.io_reserve_lru);

	NV_INFO(drm, "VRAM: %d MiB\n", (u32)(drm->gem.vram_available >> 20));
	NV_INFO(drm, "GART: %d MiB\n", (u32)(drm->gem.gart_available >> 20));
	return 0;
}

void
nouveau_ttm_fini(struct nouveau_drm *drm)
{
	nouveau_ttm_fini_vram(drm);
	nouveau_ttm_fini_gtt(drm);

	ttm_device_fini(&drm->ttm.bdev);

	arch_phys_wc_del(drm->ttm.mtrr);
	drm->ttm.mtrr = 0;
	arch_io_free_memtype_wc(drm->info.resource_addr[1],
				drm->info.resource_size[1]);

}
