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
#include "nouveau_vmm.h"
#include "novac_drv.h"
#include "nouveau_bo.h"
//#include "nouveau_svm.h"
#include "nouveau_mem.h"

void
nouveau_vma_unmap(struct nouveau_vma *vma)
{
	if (vma->mem) {
		nova_core_vmm_unmap(&vma->vmm->vmm, vma->addr);
		vma->mem = NULL;
	}
}

int
nouveau_vma_map(struct nouveau_vma *vma, struct nouveau_mem *mem)
{
	int ret = nouveau_mem_map(mem, &vma->vmm->vmm, vma->addr);
	if (ret)
		return ret;
	vma->mem = mem;
	return 0;
}

struct nouveau_vma *
nouveau_vma_find(struct nouveau_bo *nvbo, struct nouveau_vmm *vmm)
{
	struct nouveau_vma *vma;

	list_for_each_entry(vma, &nvbo->vma_list, head) {
		if (vma->vmm == vmm)
			return vma;
	}

	return NULL;
}

void
nouveau_vma_del(struct nouveau_vma **pvma)
{
	struct nouveau_vma *vma = *pvma;
	if (vma && --vma->refs <= 0) {
		if (likely(vma->addr != ~0ULL)) {
			nova_core_vmm_put(&vma->vmm->vmm, vma->addr);
		  //			struct nvif_vma tmp = { .addr = vma->addr, .size = 1 };
		}
		list_del(&vma->head);
		kfree(*pvma);
	}
	*pvma = NULL;
}

int
nouveau_vma_new(struct nouveau_bo *nvbo, struct nouveau_vmm *vmm,
		struct nouveau_vma **pvma)
{
	struct nouveau_mem *mem = nouveau_mem(nvbo->bo.resource);
	struct nouveau_vma *vma;
	int ret;
	u64 addr;

	if ((vma = *pvma = nouveau_vma_find(nvbo, vmm))) {
		vma->refs++;
		return 0;
	}

	if (!(vma = *pvma = kmalloc(sizeof(*vma), GFP_KERNEL)))
		return -ENOMEM;
	vma->vmm = vmm;
	vma->refs = 1;
	vma->addr = ~0ULL;
	vma->mem = NULL;
	vma->fence = NULL;
	list_add_tail(&vma->head, &nvbo->vma_list);

	if (nvbo->bo.resource->mem_type != TTM_PL_SYSTEM &&
	    mem->mem.page == nvbo->page) {

		ret = nova_core_vmm_get(&vmm->vmm, NVIF_VMM_GET_LAZY, false, mem->mem.page, 0,
					mem->mem.size, &addr);
		if (ret)
			goto done;

		vma->addr = addr;
		ret = nouveau_vma_map(vma, mem);
	} else {
		ret = nova_core_vmm_get(&vmm->vmm, NVIF_VMM_GET_PTES, false, mem->mem.page, 0,
					mem->mem.size, &addr);
		if (ret)
			goto done;

		vma->addr = addr;
	}
done:
	if (ret)
		nouveau_vma_del(pvma);
	return ret;
}

void
nouveau_vmm_fini(struct nouveau_vmm *vmm)
{
	nova_core_free_vmm(&vmm->vmm);
  //	nouveau_svmm_fini(&vmm->svmm);
	vmm->cli = NULL;
}

int
nouveau_vmm_init(struct nouveau_cli *cli, struct nouveau_vmm *vmm)
{
	int ret = nova_core_alloc_vmm(cli->drm->auxdev,
				      &cli->gsp,
				      &cli->mmu,
				      NVIF_VMM_TYPE_UNMANAGED,
				      &vmm->vmm);
	if (ret)
		return ret;

	vmm->cli = cli;
	return 0;
}
