/*
 * Copyright 2017 Red Hat Inc.
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
 */
#include <drm/ttm/ttm_tt.h>

#include "nouveau_mem.h"
#include "novac_drv.h"
#include "nouveau_bo.h"

int
nouveau_mem_map(struct nouveau_mem *mem,
		struct nova_core_vmm *vmm, u64 addr)
{
	struct nova_core_map_args args = {};

	args.vol = !(mem->mem.mem_type & NVIF_MEM_VRAM);
	args.ro = 0;
	args.private = 0;
	args.kind = mem->kind;
	args.addr = addr;
	args.size = mem->mem.size;
	args.offset = 0;

	return nova_core_vmm_map(mem->drm->auxdev,
				 vmm, &args, &mem->mem);
}

void
nouveau_mem_fini(struct nouveau_mem *mem)
{
	nova_core_free_mem(&mem->mem);
	nova_core_vmm_put(&mem->drm->cli.vmm.vmm, mem->vma_addr[1]);
	nova_core_vmm_put(&mem->drm->cli.vmm.vmm, mem->vma_addr[0]);
}

int
nouveau_mem_host(struct ttm_resource *reg, struct ttm_tt *tt)
{
	struct nouveau_mem *mem = nouveau_mem(reg);
	struct nouveau_drm *drm = mem->drm;
	struct nova_core_mmu *mmu = &drm->cli.mmu;
	u8 type;
	int ret;

	if (!nouveau_drm_use_coherent_gpu_mapping(drm))
		type = drm->ttm.type_ncoh[!!mem->kind];
	else
		type = drm->ttm.type_host[0];

	if (mem->kind && !(mmu->info.mmu_type[type].mmu_type & NVIF_MEM_KIND))
		mem->comp = mem->kind = 0;
	if (mem->comp && !(mmu->info.mmu_type[type].mmu_type & NVIF_MEM_COMP)) {
		mem->kind = mmu->info.kind[mem->kind];
		mem->comp = 0;
	}

//	if (tt->sg)
//		args.sgl = tt->sg->sgl;

	ret = nova_core_alloc_mem(drm->auxdev,
				  mmu,
				  "ttmHostMem",
				  type, false,
				  tt->dma_address,
				  PAGE_SHIFT, reg->size,
				  &mem->mem);
	return ret;
}

int
nouveau_mem_vram(struct ttm_resource *reg, bool contig, u8 page)
{
	struct nouveau_mem *mem = nouveau_mem(reg);
	struct nouveau_drm *drm = mem->drm;	
	u64 size = ALIGN(reg->size, 1 << page);
	int ret;

	ret = nova_core_alloc_mem(drm->auxdev,
				  &drm->cli.mmu,
				  "vramMem",
				  drm->ttm.type_vram,
				  contig, NULL,
				  page, size,
				  &mem->mem);
	if (ret == 0)
		reg->start = mem->mem.addr >> PAGE_SHIFT;
	return ret;
}

void
nouveau_mem_del(struct ttm_resource_manager *man, struct ttm_resource *reg)
{
	struct nouveau_mem *mem = nouveau_mem(reg);

	nouveau_mem_fini(mem);
	ttm_resource_fini(man, reg);
	kfree(mem);
}

int
nouveau_mem_new(struct nouveau_drm *drm, u8 kind, u8 comp,
		struct ttm_resource **res)
{
	struct nouveau_mem *mem;

	if (!(mem = kzalloc(sizeof(*mem), GFP_KERNEL)))
		return -ENOMEM;

	mem->drm = drm;
	mem->kind = kind;
	mem->comp = comp;

	*res = &mem->base;
	return 0;
}

bool
nouveau_mem_intersects(struct ttm_resource *res,
		       const struct ttm_place *place,
		       size_t size)
{
	u32 num_pages = PFN_UP(size);

	/* Don't evict BOs outside of the requested placement range */
	if (place->fpfn >= (res->start + num_pages) ||
	    (place->lpfn && place->lpfn <= res->start))
		return false;

	return true;
}

bool
nouveau_mem_compatible(struct ttm_resource *res,
		       const struct ttm_place *place,
		       size_t size)
{
	u32 num_pages = PFN_UP(size);

	if (res->start < place->fpfn ||
	    (place->lpfn && (res->start + num_pages) > place->lpfn))
		return false;

	return true;
}
