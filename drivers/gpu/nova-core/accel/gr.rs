
use kernel::prelude::*;
use kernel::sync::Arc;
use crate::gpu::{Gpu, GpuBase};
use crate::gsp::GspManager;
use crate::mmu::memory::{InstMem, InstObj, Memory};
use crate::mmu::vmm::{Vmm, VmmMap};
use crate::accel::fifo::GpuPromoteBufferEntry;

const GR_MAX_CTXBUFS: usize = 9;

pub(crate) struct CtxBufSize {
    pub size: u32,
    pub align: u8,
}

pub(crate) struct CtxBufInfo {
    pub buffer_id: u16,
    pub page: u8,
    pub align: u8,
    pub size: u32,
    pub global: bool,
    pub init: bool,
    pub ro: bool,
    pub nonmapped: bool,
}

pub(crate) struct Gr {
    golden_bufs: KVec<Arc<InstObj>>,
    ctxbufmem: [InstObj; GR_MAX_CTXBUFS],
}

impl Gr {
    pub(crate) fn alloc_ctx_bufs(instmem: Arc<InstMem>, golden: Option<&KVec<Arc<InstObj>>>, ctxbufinfo: &KVec<CtxBufInfo>) -> Result<KVec<Arc<InstObj>>> {
        let mut vec: KVec<Arc<InstObj>> = KVec::with_capacity(ctxbufinfo.len(), GFP_KERNEL)?;

        for info_idx in 0..ctxbufinfo.len() {
            let info = &ctxbufinfo[info_idx];
            let mut do_alloc = false;

            if golden.is_none() {
                do_alloc = true;
            }

            if !info.global {
                do_alloc = true;
            }

            let inst;
            if do_alloc {
                inst = Arc::new(InstObj::new(instmem.clone(), info.size as usize,
                                             1 << info.page, info.init, info.init)?, GFP_KERNEL)?;
            } else {
                inst = golden.unwrap()[info_idx].clone();
            }
            vec.push(inst, GFP_KERNEL)?;
        }
        Ok(vec)
    }

    pub(crate) fn golden_init(instmem: Arc<InstMem>, gsp: Arc<dyn GspManager>) -> Result<KVec<Arc<InstObj>>> {
        let base = &instmem.base;
        let (internal_client, internal_device) = gsp.get_internals()?;
        let gold_inst = InstObj::new(instmem.clone(), 0x12000, 0, true, false)?;

        let gold_vmm = Vmm::new(instmem.clone(), 0x1000, 0, 0, false,
                                false, None, None, true, "grGoldenVmm")?;

        let gold_va = gsp.alloc_vaspace(internal_device.clone(), &gold_vmm)?;

        let gold_chan = gsp.alloc_golden_chan(internal_device.clone(),
                                              &gold_va,
                                              &gold_inst,
                                              base.spec.gpu_consts.fifo_class)?;
        /* engine buffers */
        let ctxbufinfo = gsp.get_gr_ctx_info();
        let mem_vec = Self::alloc_ctx_bufs(instmem.clone(), None, ctxbufinfo)?;
        let mut buf_ent_vec : KVec<GpuPromoteBufferEntry> = KVec::new();
        for info_idx in 0..ctxbufinfo.len() {
            let info = &ctxbufinfo[info_idx];
            let mem = &mem_vec[info_idx];

            let mut ent = GpuPromoteBufferEntry {
                buffer_id: info.buffer_id,
                initialize: info.init,
                size: 0,
                gpu_phys_addr: 0,
                gpu_virt_addr: 0,
                physattr: 0,
                nonmapped: info.nonmapped,
            };

            if !info.nonmapped {
                let vma = gold_vmm.get(false, true, false, 0, info.align, mem.size()? as u64)?;
                let mut vmmmap = VmmMap {
                    memory: (*mem).as_ref(),
                    offset: 0,
                    kind: 0,
                    ro: info.ro as u8,
                    private: 1,
                    vol: 0,
                };

                ent.gpu_virt_addr = vma.addr();
                let _ = gold_vmm.map(vma, &mut vmmmap)?;
            }

            if info.init {
                ent.gpu_phys_addr = mem.addr()?;
                ent.size = info.size as u64;
                ent.physattr = 4;
            }

            buf_ent_vec.push(ent, GFP_KERNEL)?;
        }

        gsp.promote_gr_ctx(internal_device, gold_chan.clone(), &buf_ent_vec)?;

        let gold_obj = gsp.alloc_chan_obj(gold_chan.clone(), 0x97000000, base.spec.gpu_consts.gr_classes[2])?;

        gsp.free_chan_obj(&gold_obj);

        gsp.free_fifo_chan(&gold_chan);

        gsp.free_vaspace(&gold_va);

        for buf in &buf_ent_vec {
            if buf.gpu_virt_addr != 0 {
                gold_vmm.unmap_addr(buf.gpu_virt_addr);
                gold_vmm.put_addr(buf.gpu_virt_addr);
            }
        }
        Ok(mem_vec)
    }
}
