
use kernel::prelude::*;
use kernel::bindings;
use kernel::sync::{Arc, SpinLock, Mutex};
use kernel::new_spinlock;
use kernel::new_mutex;

use crate::dma::DmaObject;
use crate::gpu::{Gpu, GpuDevice, GpuDeviceVmm, GpuChanObject};
use crate::gpu::IntrInfo;
use crate::mmu::memory::{InstObj, VramObj};
use crate::gsp::{GspChannel, GspManager};
use crate::vfn::{Vfn, VfnHandler};

#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) enum EngineType {
    GR,
    CE,
    NVDEC,
    NVENC,
    SW,
    NVJPG,
    OFA,
    GSP,
}

impl EngineType {
    pub(crate) fn to_core(&self) -> u8 {
        match self {
            EngineType::GR => bindings::NOVA_CORE_ENGINE_GR,
            EngineType::CE => bindings::NOVA_CORE_ENGINE_CE,
            EngineType::SW => bindings::NOVA_CORE_ENGINE_SW,
            EngineType::NVDEC => bindings::NOVA_CORE_ENGINE_NVDEC,
            EngineType::NVENC => bindings::NOVA_CORE_ENGINE_NVENC,
            EngineType::OFA => bindings::NOVA_CORE_ENGINE_OFA,
            EngineType::NVJPG => bindings::NOVA_CORE_ENGINE_NVJPG,
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
            bindings::NOVA_CORE_ENGINE_NVJPG => EngineType::NVJPG,
            bindings::NOVA_CORE_ENGINE_OFA => EngineType::OFA,
            _ => return Err(EINVAL),
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
    pub entries: KVec<RunListEntry>
}

impl FifoRunList {

    pub(crate) fn entry_matches(entry: &RunListEntry, id: i32, addr: u32) -> bool {
        (id >= 0 && entry.id == id) || (id < 0 && entry.addr == addr)
    }

    pub(crate) fn find_nonstall(&self, info: &KVec<IntrInfo>, runl_id: u32) -> Result<u32> {
        for ent in &self.entries {
            if ent.id as u32 == runl_id {
                let eng = &ent.engns[0];
                let ns = IntrInfo::find_nonstall(info, eng.eng_type, eng.inst)?;
                return Ok(ns);
            }
        }
        return Err(EINVAL);
    }

    pub(crate) fn create_runlist_from_table(table: &FifoDeviceInfoTable, rsvd_chids: u32) -> Result<FifoRunList> {
        let mut entries: KVec<RunListEntry> = KVec::new();

        let chids = ChId::new(2048, rsvd_chids, 2048)?;

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
                EngineType::CE | EngineType::GR | EngineType::NVDEC | EngineType::NVENC | EngineType::OFA | EngineType::NVJPG => {
                    pr_info!("pushing engine {} {:?} {}\n", ent.id, ent.eng_type, ent.inst);
                    runlentry.engns.push(RunListEngine { eng_type: ent.eng_type, inst: ent.inst, desc: ent.eng_desc, rm_size: ent.desc_size }, GFP_KERNEL)?;
                },
                _ => continue
            }
        }
        Ok(FifoRunList {
            chids,
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

// Register one of these per non-stall interrupt source
// Then all channels for that runl get added

pub(crate) struct NonStallTrackerInner {
    num_registered: u32,
    cns_handlers: KVec<Option<Arc<ChannelNonStall>>>,
}

#[pin_data]
pub(crate) struct NonStallTracker {
    nonstall: u32,
    #[pin]
    inner: Mutex<NonStallTrackerInner>,
}

impl VfnHandler for NonStallTracker {
    fn handle_vfn(&self) -> Result<u32> {
        pr_info!("nonstall handler\n");

        let inner = &self.inner.lock();
        for cns in &inner.cns_handlers {
            match cns {
                None => {},
                Some(cns) => { unsafe { cns.cb.cb.unwrap()(cns.cb.data); } }
            }
        }
        Ok(1)
    }
}

impl NonStallTracker {
    fn num_reg(&self) -> u32 {
        self.inner.lock().num_registered
    }

    fn add_cns(&self, cns: Arc<ChannelNonStall>) -> Result<()> {
        let inner = &mut self.inner.lock();
        for channs in &mut inner.cns_handlers {
            if channs.is_none() {
                *channs = Some(cns);
                inner.num_registered += 1;
                return Ok(());
            }
        }
        inner.cns_handlers.push(Some(cns), GFP_KERNEL)?;
        inner.num_registered += 1;
        Ok(())
    }

    fn remove_cns(&self, chan_id: u32) -> Result<()> {
        let inner = &mut self.inner.lock();
        for channs in &mut inner.cns_handlers {
            let in_channs = match channs {
                Some(c) => c,
                None => { continue }
            };
            if in_channs.chan_id == chan_id {
                *channs = None;
                inner.num_registered -= 1;
                break;
            }
        }
        Ok(())
    }
}

pub(crate) struct ChannelCbInfo {
    cb: Option<unsafe extern "C" fn(data: *mut core::ffi::c_void) -> i32>,
    data: *mut core::ffi::c_void,
}

unsafe impl Send for ChannelCbInfo {}
unsafe impl Sync for ChannelCbInfo {}

pub(crate) struct ChannelKilled {
    id: u32,
    killed: ChannelCbInfo,
}

impl ChannelKilled {
    pub(crate) fn killed(&self) {
        pr_info!("killed handler\n");
        unsafe { (self.killed.cb.unwrap())(self.killed.data) };
    }
}

#[allow(dead_code)]
pub(crate) struct Channel {
    pub id: u32,
    pub runl_id: u32,
    pub doorbell: u32,
    instbuf: InstObj,
    mthdbuf: DmaObject,

    nonstall: Option<u32>,

    pub mgr: Arc<dyn GspManager>,
    pub(crate) gsp_chan: Arc<GspChannel>,
}

pub(crate) struct ChannelNonStall {
    nonstall: u32,
    cb: ChannelCbInfo,
    chan_id: u32,
}

impl VfnHandler for ChannelNonStall {
    fn handle_vfn(&self) -> Result<u32> {
        pr_info!("nonstall handler\n");
        unsafe { (self.cb.cb.unwrap())(self.cb.data) };
        Ok(1)
    }
}

impl Channel {
    pub(crate) fn new(gpu: &Gpu, device: &GpuDevice,
                      vmm: &GpuDeviceVmm, userd: &VramObj, runl_id: u32, offset: u64, length: u64, chan_priv: bool) -> Result<Arc<Self>> {
        let chid = gpu.gsp.alloc_chid()? as u32;
        let mthdbuf_size = gpu.gsp.get_mthdbuf_size();
        /* need a vctx engine rm.size */
        /* with a vmm and mapped into it */
        let mut instbuf = InstObj::new(&gpu.instmem, 0x1000, 0x1000, true, true)?;

        vmm.vmm.join(&mut instbuf)?;

        let doorbell = (runl_id << 16) | chid;
        let mthdbuf = DmaObject::new_cleared(&gpu.base.dev, mthdbuf_size as usize, "mthdbuf")?;

        let gsp_chan = gpu.gsp.alloc_fifo_chan(&device.gsp, runl_id, &vmm.va, &instbuf,
                                               userd, &mthdbuf, gpu.base.spec.gpu_consts.fifo_class, chid, offset, length, chan_priv)?;


        gpu.gsp.bind_fifo(&gsp_chan)?;
        gpu.gsp.schedule_fifo(&gsp_chan, true)?;

        let nonstall = match gpu.gsp.find_nonstall(runl_id) {
            Err(x) => { return Err(x); }
            Ok(ns) => Some(ns),
        };

        Ok(Arc::new(Self {
            id: chid,
            runl_id,
            doorbell,
            instbuf,
            mthdbuf,
            gsp_chan,
            nonstall,
            mgr: gpu.gsp.clone()
        }, GFP_KERNEL)?)
    }

    pub(crate) fn alloc_obj(&self, handle: u32, oclass: u32, engine_type: EngineType, engine_inst: u8) -> Result<Arc<GpuChanObject>> {
        let gsp = match engine_type {
            EngineType::CE => {
                self.mgr.alloc_ce_obj(&self.gsp_chan, handle, oclass, engine_inst)?
            }
            _ => {
                self.mgr.alloc_chan_obj(&self.gsp_chan, handle, oclass)?
            }
        };
        Ok(Arc::new(GpuChanObject {
            gsp,
            mgr: self.mgr.clone(),
        }, GFP_KERNEL)?)
    }

    pub(crate) fn register_killed(chan: &Arc<Channel>,
                                  gpu: &Gpu,
                                  cb: Option<unsafe extern "C" fn(data: *mut core::ffi::c_void) -> i32>,
                                  data: *mut core::ffi::c_void) -> i32 {

        let killed = ChannelKilled {
            id: chan.id,
            killed: ChannelCbInfo {
                cb,
                data,
            },
        };

        let _ = gpu.event_handler.add_killed_handler(killed);
        pr_info!("killed registered\n");
        0
    }

    pub(crate) fn unregister_killed(chan: &Arc<Channel>,
                                    gpu: &Gpu) -> i32 {
        gpu.event_handler.remove_killed_handler(chan.id);
        pr_info!("killed unregistered\n");
        0
    }

    pub(crate) fn register_nonstall(chan: &Arc<Channel>,
                                    gpu: &Gpu,
                                    cb: Option<unsafe extern "C" fn(data: *mut core::ffi::c_void) -> i32>,
                                    data: *mut core::ffi::c_void) -> Result<Arc<ChannelNonStall>> {

        let nonstall = match chan.nonstall {
            None => { return Err(ENOENT); }
            Some(ns) => ns,
        };

        let cns = Arc::new(ChannelNonStall {
            nonstall,
            cb: ChannelCbInfo {
                cb,
                data
            },
            chan_id: chan.id,
        }, GFP_KERNEL)?;

        pr_info!("nonstall registered {:#x} {:#x}\n", chan.runl_id, nonstall);

        gpu.event_handler.register_nonstall_handler(gpu, nonstall, cns.clone())?;
        Ok(cns)
    }

    pub(crate) fn unregister_nonstall(event_handler: &EventHandler, vfn: &Vfn, cns: &ChannelNonStall) -> Result<()>{
        event_handler.unregister_nonstall_handler(vfn, cns.nonstall, cns.chan_id)
    }

    pub(crate) fn free(&self) {
        let _ = self.mgr.free_fifo_chan(&self.gsp_chan);
        let _ = self.mgr.free_chid(self.id as usize);
    }
}

#[pin_data]
pub(crate) struct EventHandler {
    #[pin]
    handlers: Mutex<KVec<ChannelKilled>>,
    #[pin]
    nonstall: Mutex<KVec<Arc<NonStallTracker>>>,
}

impl EventHandler {

    pub(crate) fn new() -> Result<Arc<EventHandler>> {
        Arc::pin_init(pin_init!(EventHandler {
            handlers <- new_mutex!(KVec::new()),
            nonstall <- new_mutex!(KVec::new()),
        }), GFP_KERNEL)
    }

    pub(crate) fn add_killed_handler(&self, killed: ChannelKilled) -> Result<()> {
        let mut handlers = self.handlers.lock();
        for handler in &mut *handlers {
            if handler.killed.cb == None {
                *handler = killed;
                return Ok(());
            }
        }
        handlers.push(killed, GFP_KERNEL)?;
        Ok(())
    }

    pub(crate) fn remove_killed_handler(&self, chid: u32) {
        let mut handlers = self.handlers.lock();

        for handler in &mut *handlers {
            if handler.id == chid {
                handler.id = 0;
                handler.killed.cb = None;
                handler.killed.data = core::ptr::null_mut();
            }
        }
    }

    pub(crate) fn handle_channel_killed(&self, chid: u32) {
        let handlers = self.handlers.lock();

        for handler in &*handlers {
            if handler.id == chid && handler.killed.cb.is_some() {
                handler.killed();
                break;
            }
        }
    }

    pub(crate) fn register_nonstall_handler(&self, gpu: &Gpu, nonstall: u32,
                                            cns: Arc<ChannelNonStall>) -> Result<()> {
        let mut nonstall_handlers = self.nonstall.lock();

        for ns in &mut *nonstall_handlers {
            if ns.nonstall == nonstall {
                ns.add_cns(cns);

                if ns.num_reg() == 1 {
                    gpu.vfn.intr_allow(nonstall)?;
                }
                return Ok(())
            }
        }

        let nst_inner = NonStallTrackerInner {
            num_registered: 0,
            cns_handlers: KVec::new(),
        };

        let nst = Arc::pin_init(pin_init!(NonStallTracker {
            nonstall,
            inner <- new_mutex!(nst_inner),
        }), GFP_KERNEL)?;

        nst.add_cns(cns);
        nonstall_handlers.push(nst.clone(), GFP_KERNEL)?;

        let _ = gpu.vfn.add_handler(nonstall, nst.clone() as Arc<dyn VfnHandler>);
        gpu.vfn.intr_allow(nonstall)?;

        Ok(())
    }

    pub(crate) fn unregister_nonstall_handler(&self, vfn: &Vfn, nonstall: u32, chan_id: u32) -> Result<()> {
        let mut nonstall_handlers = self.nonstall.lock();
        for ns in &mut *nonstall_handlers {
            if ns.nonstall == nonstall {
                ns.remove_cns(chan_id);

                if ns.num_reg() == 0 {
                    vfn.intr_block(nonstall);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn unregister(&self, gpu: &Gpu) {
        let mut nonstall_handlers = self.nonstall.lock();
        for ns in &mut *nonstall_handlers {
            gpu.vfn.remove_handler(ns.clone() as Arc<dyn VfnHandler>);
        }
    }
}
