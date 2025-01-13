
use kernel::prelude::*;
use kernel::c_str;
use kernel::bindings;
use kernel::sync::{Arc, SpinLock};
use kernel::new_spinlock;

use crate::dma::DmaObject;
use crate::gpu::{Gpu, GpuClient, GpuDevice, GpuDeviceVmm, GpuChanObject};
use crate::mmu::memory::{InstObj, VramObj};
use crate::gsp::{GspChannel, GspManager};

#[derive(Clone,Copy,Debug)]
pub(crate) enum EngineType {
    GR,
    CE,
    NVDEC,
    NVENC,
    SW,
    SEC2,
    JPEG,
}

impl EngineType {
    pub(crate) fn to_core(&self) -> u8 {
        match self {
            EngineType::GR => bindings::NOVA_CORE_ENGINE_GR,
            EngineType::CE => bindings::NOVA_CORE_ENGINE_CE,
            EngineType::SW => bindings::NOVA_CORE_ENGINE_SW,
            EngineType::NVDEC => bindings::NOVA_CORE_ENGINE_NVDEC,
            EngineType::NVENC => bindings::NOVA_CORE_ENGINE_NVENC,
            _ => 0,
        }
    }

    pub(crate) fn from_core(core_type: u8) -> Result<Self> {
        Ok(match core_type {
            bindings::NOVA_CORE_ENGINE_GR => EngineType::GR,
            bindings::NOVA_CORE_ENGINE_CE => EngineType::CE,
            bindings::NOVA_CORE_ENGINE_SW => EngineType::SW,
            bindings::NOVA_CORE_ENGINE_NVDEC => EngineType::NVDEC,
            bindings::NOVA_CORE_ENGINE_NVENC => EngineType::NVENC,
            _ => return { Err(EINVAL) },
        })
    }
}

// make this generic
pub(crate) struct BitVec {
    bitmask_vec: KVec<u64>,
}

impl BitVec {
    pub(crate) fn new(num_bits: usize) -> Result<BitVec> {
        let vec_size = num_bits / 64;
        Ok(BitVec {
            bitmask_vec: KVec::from_elem(0, vec_size, GFP_KERNEL)?
        })
    }

    pub(crate) fn set_bit(&mut self, index: usize, value: bool) {
        let word = index / 64;
        let bit = index % 64;

        if value {
            self.bitmask_vec[word] |= 1_u64 << bit;
        } else {
            self.bitmask_vec[word] &= !(1_u64 << bit);
        }
    }

    fn ffzbit(word: u64) -> u32 {
        (!word).trailing_zeros()
    }

    pub(crate) fn ffz(&self) -> usize {
        for i in 0..self.bitmask_vec.len() {
            let first = Self::ffzbit(self.bitmask_vec[i]);

            if first >= 64 {
                continue;
            }

            return i * 64 + first as usize;
        }
        0
    }

}

pub(crate) struct ChidInner {
    bits: BitVec,
    resv_bits: BitVec
}

#[pin_data]
#[repr(C)]
pub(crate) struct ChId {
    pub nr: u32,
    mask: u32,
    #[pin]
    inner: SpinLock<ChidInner>,
}

impl ChId {
    pub(crate) fn new(nr: u32, first: u32, count: u32) -> Result<Pin<KBox<ChId>>> {

        let mut bits = BitVec::new(nr as usize)?;
        let resv_bits = BitVec::new(nr as usize)?;

        for id in 0..first {
            bits.set_bit(id as usize, true);
        }

        for id in (first + count)..nr {
            bits.set_bit(id as usize, true);
        }

        Ok(KBox::pin_init(pin_init!(ChId {
            nr,
            mask: (nr - 1) as u32,
            inner <- new_spinlock!(ChidInner {
                bits,
                resv_bits
            })
        }), GFP_KERNEL)?)
    }


    pub(crate) fn get(&self) -> Result<usize> {
        let mut locked = self.inner.lock();

        let res = locked.bits.ffz();

        locked.bits.set_bit(res, true);
        Ok(res)

    }

    pub(crate) fn put(&self, bitidx: usize) {
        let mut locked = self.inner.lock();

        locked.bits.set_bit(bitidx, false);
    }

    pub(crate) fn reserve(&self, first: u32, count: u32) {
        let mut locked = self.inner.lock();
        for id in 0..first {
            locked.resv_bits.set_bit(id as usize, true);
        }
        for id in first + count..self.nr {
            locked.resv_bits.set_bit(id as usize, true);
        }
        for id in first..count {
            locked.bits.set_bit(id as usize, true);
        }
    }
}


#[allow(unused)]
pub(crate) struct FifoDeviceEntry {
    pub addr: u32,
    pub eng_type: EngineType,
    pub inst: u32,
    pub id: i32,
    pub eng_desc: u32,
    pub desc_size: u32,
}

pub(crate) struct FifoDeviceInfoTable {
    pub table: KVec<FifoDeviceEntry>
}

pub(crate) struct RunListEngine {
    pub eng_type: EngineType,
    pub inst: u32,
    desc: u32,
    rm_size: u32,
}

pub(crate) struct RunListEntry {
    pub id: i32,
    addr: u32,
    pub engns: KVec<RunListEngine>
}

pub(crate) struct FifoRunList {
    pub chids: Pin<KBox<ChId>>,
    pub cgids: Pin<KBox<ChId>>,
    pub entries: KVec<RunListEntry>
}

impl FifoRunList {

    pub(crate) fn entry_matches(entry: &RunListEntry, id: i32, addr: u32) -> bool {
        (id >= 0 && entry.id == id) || (id < 0 && entry.addr == addr)
    }

    pub(crate) fn create_runlist_from_table(table: &FifoDeviceInfoTable) -> Result<FifoRunList> {
        let mut entries: KVec<RunListEntry> = KVec::new();

        let cgids = ChId::new(2048, 0, 2048)?;
        let mut chids = ChId::new(2048, 0, 2048)?;

        #[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
        chids.reserve(512, 1536);

        //vgpu reserve
        for ent in &table.table {
            let mut runlentry = None;
            for runl in &entries {
                if Self::entry_matches(runl, ent.id, ent.addr) {
                    runlentry = Some(runl);
                    break;
                }
            }
            if runlentry.is_none() {
                entries.push(RunListEntry { id: ent.id, addr: ent.addr, engns: KVec::new() }, GFP_KERNEL)?;
            }
        }

        for ent in &table.table {
            let mut runlentry = None;
            for runl in &mut entries {
                if Self::entry_matches(runl, ent.id, ent.addr) {
                    runlentry = Some(runl);
                    break;
                }
            }

            let runlentry = match runlentry {
                None => { continue; }
                Some(ent) => ent
            };

            match &ent.eng_type {
                EngineType::CE | EngineType::GR | EngineType::NVDEC | EngineType::NVENC => {
                    pr_info!("pushing engine {} {:?} {}\n", ent.id, ent.eng_type, ent.inst);
                    runlentry.engns.push(RunListEngine { eng_type: ent.eng_type, inst: ent.inst, desc: ent.eng_desc, rm_size: ent.desc_size }, GFP_KERNEL)?;
                },
                _ => continue
            }
        }
        Ok(FifoRunList {
            chids,
            cgids,
            entries
        })
    }
}

pub(crate) struct GpuPromoteBufferEntry {
    pub gpu_phys_addr: u64,
    pub gpu_virt_addr: u64,
    pub size: u64,
    pub physattr: u32,
    pub buffer_id: u16,
    pub initialize: bool,
    pub nonmapped: bool,
}

pub(crate) struct Channel {
    name: &'static CStr,
    pub id: u32,
    pub doorbell: u32,
//  userd: &'dyn Memory,
    //  obj: GspObject,
    instbuf: InstObj,
    mthdbuf: DmaObject,
    pub mgr: Arc<dyn GspManager>,
    pub(crate) gsp_chan: Arc<GspChannel>
}

impl Channel {
    pub(crate) fn new(gpu: &Gpu, client: Arc<GpuClient>, device: Arc<GpuDevice>,
                      vmm: &GpuDeviceVmm, userd: &VramObj, runl_id: u32, offset: u64, length: u64, chan_priv: bool) -> Result<Self> {
        let chid = gpu.gsp.alloc_chid()? as u32;
        let mthdbuf_size = gpu.gsp.get_mthdbuf_size();
        /* need a vctx engine rm.size */
        /* with a vmm and mapped into it */
        let mut instbuf = InstObj::new(gpu.instmem.clone(), 0x1000, 0x1000, true, true)?;

        vmm.vmm.join(&mut instbuf);

        let doorbell = (runl_id << 16) | chid;
        let mthdbuf = DmaObject::new_cleared(&gpu.base.dev, mthdbuf_size as usize, "mthdbuf")?;

        let gsp_chan = gpu.gsp.alloc_fifo_chan(device.gsp.clone(), runl_id, &vmm.va, &instbuf,
                                               userd, &mthdbuf, gpu.base.spec.gpu_consts.fifo_class, chid, offset, length, chan_priv)?;


        gpu.gsp.bind_fifo(&gsp_chan);
        gpu.gsp.schedule_fifo(&gsp_chan, true);
        Ok(Self {
            name: c_str!("chan"),
            id: chid,
            doorbell,
            instbuf,
            mthdbuf,
            gsp_chan,
            mgr: gpu.gsp.clone()
        })
    }

    pub(crate) fn alloc_obj(&self, handle: u32, oclass: u32, engine_type: EngineType, engine_inst: u8) -> Result<Arc<GpuChanObject>> {
        let gsp = match engine_type {
            EngineType::CE => {
                self.mgr.alloc_ce_obj(self.gsp_chan.clone(), handle, oclass, engine_inst)?
            }
            _ => {
                self.mgr.alloc_chan_obj(self.gsp_chan.clone(), handle, oclass)?
            }
        };
        Ok(Arc::new(GpuChanObject {
            gsp,
            mgr: self.mgr.clone()
        }, GFP_KERNEL)?)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.mgr.free_fifo_chan(&self.gsp_chan);
        self.mgr.free_chid(self.id as usize);
    }
}
