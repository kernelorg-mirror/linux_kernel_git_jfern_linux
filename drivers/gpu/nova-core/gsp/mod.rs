#![allow(dead_code)]

pub(crate) use kernel::macros::versions;

use crate::order_base_2;
use kernel::prelude::*;
use kernel::bindings;
use kernel::sync::{Arc, Mutex, new_mutex};
use kernel::sync::lock::Guard;
use kernel::sync::lock::mutex::MutexBackend;

use crate::nvfw::*;
use crate::{align, div_round_up};
use crate::chipsets_before;
use crate::devinit;
use crate::dma::DmaObject;
use crate::falcon::Falcon;
use crate::accel::fifo::{FifoRunList, EngineType, GpuPromoteBufferEntry};
use crate::accel::gr::{CtxBufSize, CtxBufInfo};
use crate::gsp::fwsec::Fwsec;
use crate::gsp::fwsec::{NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
                        NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB};
use crate::gsp::gsp_falcon::GspFalcon;
use crate::gsp::sharedq::*;

use crate::gpu::NOVA_ENABLE_VGPU;
use crate::gpu::Chipset;
use crate::gpu::FBInfo;
use crate::gpu::Firmware;
use crate::gpu::GpuBase;
use crate::gpu::IntrInfo;

use crate::accel::fifo::EventHandler;
use crate::gsp::alloc_msgs::*;
use crate::gsp::ctrl_msgs::*;
use crate::gsp::msgs::*;
use crate::gsp::rpc_msgs::*;
use crate::vfn::{Vfn, VfnHandler};
use crate::mmu::mm::MemRange;
use crate::mmu::memory::{InstObj, VramObj};
use crate::mmu::vmm::Vmm;
use crate::sec2::{Sec2, Sec2Fw};
use crate::timer::TimerWait;
use crate::{timer_msec, timer_nsec};

mod alloc_msgs;
mod boot_structs;
mod ctrl_msgs;
mod fwsec;
pub(crate) mod gsp_falcon;
mod msgs;
mod notifiers;
mod rpc_msgs;
mod sharedq;

const GSP_PAGE_SHIFT: u32 = 12;
pub(crate) const GSP_PAGE_SIZE: u32 = 1 << GSP_PAGE_SHIFT;
pub(crate) const GSP_HEAP_SHIFT: u64 = 1 << 20;

const MAX_GPC_COUNT: u32 = 32;

#[versions(GSP)]
#[allow(unused)]
pub(crate) struct GSPSharedMemObjects {
    pub libos: DmaObject,
    pub loginit: DmaObject,
    pub logintr: DmaObject,
    pub logrm: DmaObject,
    pub rmargs: DmaObject,
    pub kern: Option<DmaObject>,
    pub shm: DmaObject,
    pub wpr_meta: DmaObject,
    pub queues: GSPSharedQueues::ver,
}

#[versions(GSP)]
pub(crate) struct GSPSharedMemObjectsOuter {
    pub inner: Pin<KBox<Mutex<GSPSharedMemObjects::ver>>>
}

#[versions(GSP)]
impl VfnHandler for GSPSharedMemObjects::ver {
    fn handle_vfn(&self) -> Result<u32> {
        let gsp_falcon;
        match self.queues.gsp_falcon.as_ref() {
            Some(x) => {
                gsp_falcon = x;
            },
            None => {
                return Ok(0);
            }
        }

        let flcn = &gsp_falcon.falcon;
        let intr: u32 = flcn.rd32(0x0008)?;
        let inte: u32 = flcn.rd32(flcn.addr2 + flcn.riscv_irqmask)?;
        let mut stat = intr & inte;

        if stat == 0 {
            pr_info!("GSP IRQ fired with nothing set {:#x} {:#x}", intr, inte);
            return Ok(0);
        }

        if stat & 0x00000040 != 0 {
            flcn.wr32(0x4, 0x40)?;
            self.queues.msg_irq_work();
            stat &= !0x00000040;
        }

        if stat != 0 {
            pr_info!("GSP intr unhandled {:#x}", stat);
            flcn.wr32(0x14, stat)?;
            flcn.wr32(0x04, stat)?;
        }

        flcn.intr_retrigger()?;
        Ok(1)
    }
}

#[versions(GSP)]
impl VfnHandler for GSPSharedMemObjectsOuter::ver {
    fn handle_vfn(&self) -> Result<u32> {

        let gsp_objs = self.inner.lock();

        gsp_objs.handle_vfn()
    }
}

#[versions(GSP)]
impl GSPSharedMemObjects::ver {

    pub(crate) fn fill_shm_ptes(dma: &mut DmaObject, nr_ptes: usize) {
        unsafe {
            let ptes : *mut u64 = dma.dma.start_ptr_mut().offset(0) as *mut u64;
            for i in 0..nr_ptes {
                *ptes.wrapping_add(i) = dma.dma.dma_handle() + (i << GSP_PAGE_SHIFT) as u64;
            }
        }
    }

    pub(crate) fn new(gpu_base: &GpuBase) -> Result<Self> {
        let cmdq_size = 0x40000;
        let msgq_size = 0x40000;
        let mut ptes_nr = (cmdq_size + msgq_size) >> GSP_PAGE_SHIFT;
        ptes_nr += div_round_up((ptes_nr * size_of::<u64>()) as usize, GSP_PAGE_SIZE as usize);
        let ptes_size = align(ptes_nr * size_of::<u64>() as usize, GSP_PAGE_SIZE as usize);
        let shmem_size = cmdq_size + msgq_size + ptes_size;

        let mut shm = DmaObject::new_cleared(&gpu_base.dev, shmem_size, "shm")?;

        Self::fill_shm_ptes(&mut shm, ptes_nr);

        let mut num_logs : usize = fw::ver::gen::LOGIDX_SIZE as usize;
        let mut kern: Option<DmaObject> = None;
        #[ver(r == r570_86_16)]
        {
           /* LIBOS 2 on 560 is same are prior */
            if chipsets_before!(&gpu_base.spec.chipset, GA102) {
                num_logs -= 1;
            } else {
                kern = Some(DmaObject::new_cleared(&gpu_base.dev, 0x10000, "kern")?);
            }
        }
        let mut loginit = DmaObject::new_cleared(&gpu_base.dev, 0x10000, "loginit")?;
        let mut logintr = DmaObject::new_cleared(&gpu_base.dev, 0x10000, "logintr")?;
        let mut logrm = DmaObject::new_cleared(&gpu_base.dev, 0x10000, "logrm")?;
        let mut rmargs = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "rmargs")?;

        boot_structs::Wpr::ver::fill_rmargs(rmargs.dma.start_ptr_mut(), shm.dma.dma_handle(), ptes_nr as u32,
                                             ptes_size as u64,
                                             ptes_size as u64 + cmdq_size as u64, false);

        let libos = boot_structs::Wpr::ver::libos_fill_data(&gpu_base.dev,
                                                            num_logs,
                                                            &mut loginit, &mut logintr,
                                                            &mut logrm, kern.as_mut(), &rmargs)?;

        let wpr_meta = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "wpr_meta")?;

        let queues = GSPSharedQueues::ver::new(&mut shm, cmdq_size as u32, msgq_size as u32, ptes_size as u32, ptes_nr as u32)?;
        Ok(Self {
            libos,
            loginit,
            logintr,
            logrm,
            rmargs,
            kern,
            shm,
            wpr_meta,
            queues,
        })
    }
}

#[derive(Default)]
pub(crate) struct GspObject {
    client: Option<Arc<GspClient>>,
    parent: Option<Arc<GspObject>>,
    handle: u32,
}

pub(crate) struct GspClient {
    object: Arc<GspObject>
}

pub(crate) struct GspVa {
    object: Arc<GspObject>
}

pub(crate) struct GspChannel {
    object: Arc<GspObject>,
    chid: u32,
    engine_type: EngineType,
    engine_inst: u32,
}

impl GspClient {
    pub(crate) fn get_client_handle(&self) -> Result<u32> {
        Ok(self.object.handle)
    }
}

pub(crate) struct GspDevice {
    object: Arc<GspObject>,
    subdevice: Arc<GspObject>,
}

pub(crate) enum EventSetNotificationAction {
    DISABLE,
    SINGLE,
    REPEAT,
}

pub(crate) struct GspEvent {
    object: Arc<GspObject>,
    id: u32,
}

#[versions(GSP)]
pub(crate) struct GspManager {
    gpu_base: Arc<GpuBase>,
    sysmem_flush: DmaObject,
    fw: Firmware,
    fb_addr_info: FBInfo,
    gsp_objs: Arc<GSPSharedMemObjectsOuter::ver>,
    bar1_pdb: u64,
    bar2_pdb: u64,
    gpcs: u8,
    tpcs: u8,
    vmmu_segment_size: u64,
    runl: FifoRunList,
    rsvd_chids: u32,
    mthdbuf_size: u32,
    internal_client: Arc<GspClient>,
    internal_device: Arc<GspDevice>,
    gr_ctx_info: KVec<CtxBufInfo>,
    intr_info: KVec<IntrInfo>,
}

pub(crate) trait GspManager: Send + Sync {
    fn alloc_client_device(&self, client_id: u32) -> Result<(Arc<GspClient>,
                                                             Arc<GspDevice>)>;
    fn free_client(&self, client: &GspClient) -> Result<()>;
    fn free_device(&self, device: &GspDevice) -> Result<()>;

    fn alloc_vaspace(&self, device: &GspDevice, vmm: &Vmm, vmm_type: u8) -> Result<GspVa>;
    fn free_vaspace(&self, va: &GspVa) -> Result<()>;

    fn alloc_event(&self, device: &GspDevice, handle: u32, id: u32) -> Result<GspEvent>;
    fn free_event(&self, event: &GspEvent) -> Result<()>;

    fn alloc_chid(&self) -> Result<usize>;
    fn free_chid(&self, chid: usize);
    fn alloc_fifo_chan(&self,
                       device: &GspDevice,
                       runl_id: u32,
                       va: &GspVa,
                       inst: &InstObj,
                       userd: &VramObj,
                       mthdbuf: &DmaObject,
                       fifo_class: u32,
                       chid: u32,
                       offset: u64,
                       length: u64,
                       chan_priv: bool) -> Result<Arc<GspChannel>>;
    fn free_fifo_chan(&self, channel: &GspChannel) -> Result<()>;

    fn bind_fifo(&self,
                 channel: &GspChannel) -> Result<()>;
    fn schedule_fifo(&self,
                     channel: &GspChannel,
                     enable: bool) -> Result<()>;


    fn alloc_golden_chan(&self,
                         device: &GspDevice,
                         va: &GspVa,
                         inst: &InstObj,
                         fifo_class: u32) -> Result<Arc<GspChannel>>;

    fn alloc_ce_obj(&self, channel: &GspChannel, handle: u32, oclass: u32, inst: u8) -> Result<Arc<GspObject>>;
    fn alloc_chan_obj(&self, channel: &GspChannel, handle: u32, oclass: u32) -> Result<Arc<GspObject>>;
    fn free_chan_obj(&self, object: &GspObject) -> Result<()>;

    fn update_bar_pde(&self, bar: u32, addr: u64, shift: u32) -> Result<()>;
    fn get_mthdbuf_size(&self) -> u32;
    fn get_bar_pdb(&self, bar: u8) -> u64;
    fn get_gr_info(&self) -> (u8, u8);
    fn get_internals(&self) -> Result<(Arc<GspClient>,
                                       Arc<GspDevice>)>;

    fn get_vmmu_segment_size(&self) -> u64;
    fn get_runlist(&self) -> &FifoRunList;
    fn get_engine_bitmap(&self) -> u64;
    fn get_gr_ctx_info(&self) -> &KVec<CtxBufInfo>;
    fn promote_gr_ctx(&self, device: &GspDevice, channel: &GspChannel,
                      buffer_entries: &KVec<GpuPromoteBufferEntry>) -> Result<()>;

    fn cleanup_vgpu_plugin(&self, device: &GspDevice, gfid: u32) -> i32;
    fn shutdown_vgpu_plugin_task(&self, device: &GspDevice, gfid: u32) -> i32;
    fn bootload_vgpu_plugin_task(&self, device: &GspDevice, params: *const bindings::bootload_vgpu) -> i32;
    fn add_vgpu_type(&self, device: &GspDevice, count: u32, ptr: *const core::ffi::c_void) -> i32;

    fn find_nonstall(&self, runl_id: u32) -> Result<u32>;
}

#[versions(GSP)]
impl GspManager for GspManager::ver {

    fn alloc_client_device(&self, client_id: u32) -> Result<(Arc<GspClient>,
                                             Arc<GspDevice>)> {
        let client = Arc::new(self.alloc_client(client_id)?, GFP_KERNEL)?;
        let device = Arc::new(self.alloc_device(client.clone())?, GFP_KERNEL)?;
        Ok((client, device))
    }

    fn free_client(&self, client: &GspClient) -> Result<()> {
        let mut msg = FreeMsg::ver::get(&client.object)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        Ok(())
    }

    fn free_device(&self, device: &GspDevice) -> Result<()>{
        let mut msg = FreeMsg::ver::get(&device.subdevice)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        let mut msg = FreeMsg::ver::get(&device.object)?;

        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn alloc_chid(&self) -> Result<usize> {
        Ok(self.runl.chids.get()? * 8)
    }

    fn free_chid(&self, chid: usize) {
        self.runl.chids.put(chid / 8)
    }

    fn alloc_fifo_chan(&self,
                       device: &GspDevice,
                       runl_id: u32,
                       va: &GspVa,
                       inst: &InstObj,
                       userd: &VramObj,
                       mthdbuf: &DmaObject,
                       fifo_class: u32,
                       chid: u32,
                       offset: u64,
                       length: u64,
                       chan_priv: bool) -> Result<Arc<GspChannel>> {

        let mut engine_type = EngineType::GR;
        let mut engine_inst = 0;
        for ent in &self.runl.entries {
            if ent.id == runl_id as i32 {
                engine_type = ent.engns[0].eng_type;
                engine_inst = ent.engns[0].inst;
            }
        }
        pr_info!("alloc fifo et:{:?} ei:{} fc:{:#x} chid:{} offset:{:#x} length:{:#x} \n", engine_type, engine_inst, fifo_class, chid, offset, length);

        let mut msg = FifoAlloc::ver::new(device, engine_type, engine_inst, va,
                                          inst, userd, mthdbuf,
                                          chid, fifo_class, offset, length, chan_priv)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(Arc::new(GspChannel {
            object: Arc::new(GspObject {
                client: device.object.client.clone(),
                parent: Some(device.object.clone()),
                handle: msg.handle,
            }, GFP_KERNEL)?,
            chid,
            engine_type,
            engine_inst,
        }, GFP_KERNEL)?)
    }

    fn bind_fifo(&self,
                 channel: &GspChannel) -> Result<()> {
        let mut msg = BindParams::ver::new(channel, channel.engine_type, channel.engine_inst)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn schedule_fifo(&self,
                     channel: &GspChannel,
                     enable: bool) -> Result<()> {
        let mut msg = GpFifoSchedule::ver::new(channel, enable)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn free_fifo_chan(&self, channel: &GspChannel) -> Result<()> {
        let mut msg = FreeMsg::ver::get(&channel.object)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn alloc_golden_chan(&self,
                         device: &GspDevice,
                         va: &GspVa,
                         inst: &InstObj,
                         fifo_class: u32) -> Result<Arc<GspChannel>> {
        let mut msg = FifoAlloc::ver::golden(device, va,
                                             inst, self.rsvd_chids, fifo_class)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(Arc::new(GspChannel {
            object: Arc::new(GspObject {
                client: device.object.client.clone(),
                parent: Some(device.object.clone()),
                handle: msg.handle,
            }, GFP_KERNEL)?,
            chid: 0,
            engine_type: EngineType::GR,
            engine_inst: 0
        }, GFP_KERNEL)?)

    }

    fn alloc_ce_obj(&self, channel: &GspChannel, handle: u32, oclass: u32, inst: u8) -> Result<Arc<GspObject>> {
        let mut msg = CEAlloc::ver::new(channel, handle, oclass, inst as u32)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        Ok(Arc::new(GspObject {
            client: channel.object.client.clone(),
            parent: Some(channel.object.clone()),
            handle: handle,
        }, GFP_KERNEL)?)
    }

    fn alloc_chan_obj(&self, channel: &GspChannel, handle: u32, oclass: u32) -> Result<Arc<GspObject>> {
        let mut msg = AllocMsg::ver::get(Some(&channel.object.client.as_ref().unwrap()),
                                         Some(&channel.object),
                                         handle,
                                         oclass, 0)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        Ok(Arc::new(GspObject {
            client: channel.object.client.clone(),
            parent: Some(channel.object.clone()),
            handle: handle,
        }, GFP_KERNEL)?)
    }


    fn free_chan_obj(&self, object: &GspObject) -> Result<()> {
        let mut msg = FreeMsg::ver::get(object)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn alloc_event(&self, device: &GspDevice, handle: u32, id: u32) -> Result<GspEvent> {
        let mut msg = AllocEvent::ver::new(device, handle, id)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        let gsp_event = GspEvent {
            object: Arc::new(GspObject {
                client: device.object.client.clone(),
                parent: Some(device.object.clone()),
                handle: msg.handle,
            }, GFP_KERNEL)?,
            id
        };

        let mut msg = EventSetNotification::ver::new(&device, id, EventSetNotificationAction::REPEAT)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(gsp_event)
    }

    fn free_event(&self, event: &GspEvent) -> Result<()> {
        let mut msg = FreeMsg::ver::get(&event.object)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }


    fn alloc_vaspace(&self, device: &GspDevice, vmm: &Vmm, vmm_type: u8) -> Result<GspVa> {
        let id = if vmm_type == 3 { 1 } else { 0 };
        let mut msg = AllocVMM::ver::new(device, id)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        let gsp_va = GspVa {
            object: Arc::new(GspObject {
                client: device.object.client.clone(),
                parent: Some(device.object.clone()),
                handle: msg.handle,
            }, GFP_KERNEL)?
        };

        let mut msg = VASpaceCopyServerReservedPdes::ver::new(&gsp_va, vmm)?;
        msg.push(&mut gsp_objs.queues)?;
        Ok(gsp_va)
    }

    fn free_vaspace(&self, va: &GspVa) -> Result<()> {
        let mut msg = FreeMsg::ver::get(&va.object)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }
    fn update_bar_pde(&self, bar: u32, addr: u64, shift: u32) -> Result<()> {
        let mut msg = UpdateBarPdeMsg::ver::get(bar, addr, shift)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn get_mthdbuf_size(&self) -> u32 {
        self.mthdbuf_size
    }

    fn get_gr_info(&self) -> (u8, u8) {
        (self.gpcs, self.tpcs)
    }

    fn get_internals(&self) -> Result<(Arc<GspClient>,
                                       Arc<GspDevice>)> {
        Ok((self.internal_client.clone(), self.internal_device.clone()))
    }


    fn get_bar_pdb(&self, bar: u8) -> u64 {
        if bar == 1 {
            self.bar1_pdb
        } else {
            self.bar2_pdb
        }
    }

    fn get_vmmu_segment_size(&self) -> u64 {
        self.vmmu_segment_size
    }

    fn get_runlist(&self) -> &FifoRunList {
        &self.runl
    }

    fn find_nonstall(&self, runl_id: u32) -> Result<u32> {
        let nonstall = self.runl.find_nonstall(&self.intr_info, runl_id)?;
        Ok(nonstall)
    }

    fn get_engine_bitmap(&self) -> u64 {
        let mut mask: u64 = 0;
        for runl in &self.runl.entries {
            for eng in &runl.engns {
                let nv2080 = FifoGetDeviceInfoTable::ver::convert_eng_to_nv2080(eng.eng_type, eng.inst);

                mask |= 1_u64 << nv2080;
            }
        }
        mask
    }

    fn get_gr_ctx_info(&self) -> &KVec<CtxBufInfo> {
        &self.gr_ctx_info
    }

    fn promote_gr_ctx(&self, device: &GspDevice, channel: &GspChannel,
                      buffer_entries: &KVec<GpuPromoteBufferEntry>) -> Result<()> {
        let mut msg = GpuPromoteCtx::ver::new_promote_gr(device, channel,
                                                         buffer_entries)?;
        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)
    }

    fn cleanup_vgpu_plugin(&self, device: &GspDevice, gfid: u32) -> i32 {
        let mut msg = CleanupVgpuPlugin::ver::new(device, gfid).unwrap();

        let mut gsp_objs = self.gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn shutdown_vgpu_plugin_task(&self, device: &GspDevice, gfid: u32) -> i32 {
        let mut msg = ShutdownVgpuPluginTask::ver::new(device, gfid).unwrap();

        let mut gsp_objs = self.gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn bootload_vgpu_plugin_task(&self, device: &GspDevice, params: *const bindings::bootload_vgpu) -> i32 {
        let mut msg = BootloadVgpuPluginTask::ver::new(device, params).unwrap();

        let mut gsp_objs = self.gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn add_vgpu_type(&self, device: &GspDevice, count: u32, ptr: *const core::ffi::c_void) -> i32 {
        let mut msg = PgpuAddVgpuType::ver::new(&device, count, ptr).unwrap();
        let mut gsp_objs = self.gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }
}

struct CtxBufTable {
    id0: u32,
    id1: u32,
    global: bool,
    init: bool,
    ro: bool
}

const NUM_CTX_BUFS: usize = 8;

#[allow(non_upper_case_globals)]
#[versions(GSP)]
const CTX_BUF_MAP: [CtxBufTable; NUM_CTX_BUFS] = [
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_MAIN,
                  global: false,
                  init: true,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PATCH,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PATCH,
                  global: false,
                  init: true,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_BUNDLE_CB,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_BUFFER_BUNDLE_CB,
                  global: true,
                  init: false,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PAGEPOOL,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PAGEPOOL,
                  global: true,
                  init: false,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_ATTRIBUTE_CB,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_ATTRIBUTE_CB,
                  global: true,
                  init: false,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_RTV_CB_GLOBAL,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_RTV_CB_GLOBAL,
                  global: true,
                  init: false,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_FECS_EVENT,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_FECS_EVENT,
                  global: true,
                  init: true,
                  ro: false },
    CtxBufTable { id0: fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PRIV_ACCESS_MAP,
                  id1: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP,
                  global: true,
                  init: true,
                  ro: true },
];

#[versions(GSP)]
impl GspManager::ver {
    fn alloc_client(&self, client_id: u32) -> Result<GspClient> {
        let mut msg = AllocClient::ver::new(client_id as u16, 0xffffffff)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        Ok(GspClient {
            object: Arc::new(GspObject {
                client: None,
                parent: None,
                handle: msg.handle,
            }, GFP_KERNEL)?
        })
    }

    fn alloc_device(&self, client: Arc<GspClient>) -> Result<GspDevice> {
        let mut msg = AllocDevice::ver::new(&client)?;

        let mut gsp_objs = self.gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        let devobj = Arc::new(GspObject {
            client: Some(client.clone()),
            parent: Some(client.object.clone()),
            handle: msg.handle,
        }, GFP_KERNEL)?;

        let mut sub_msg = AllocSubdevice::ver::new(&devobj)?;

        sub_msg.push(&mut gsp_objs.queues)?;

        Ok(GspDevice {
            object: devobj.clone(),
            subdevice: Arc::new(GspObject {
                client: Some(client),
                parent: Some(devobj),
                handle: sub_msg.handle,
            }, GFP_KERNEL)?
        })
    }

    fn init_gsp(loader_fw: &Sec2Fw,
                gsp_objs: &mut GSPSharedMemObjects::ver,
                wpr_sr_addr: u64) -> Result<()> {

        loader_fw.boot((wpr_sr_addr & 0xffffffff) as u32,
                       Some((wpr_sr_addr >> 32) as u32))?;

        gsp_objs.queues.gsp_falcon.as_mut().unwrap().write_app_version()?;

        if !gsp_objs.queues.gsp_falcon.as_ref().unwrap().falcon.riscv_active()? {
            pr_err!("GSP FALCON LOAD FAILED - RISCV NOT ACTIVE\n");
            return Err(EINVAL);
        }
        /* kick off the event processing */
        gsp_objs.queues.poll_gsp_init_done()
    }

    fn fini(gpu_base: &GpuBase,
            fw: &Firmware,
            gsp_objs: &mut GSPSharedMemObjects::ver,
            unload: bool,
            mbox0: u32,
            mbox1: u32) -> Result<()> {

        if unload {
            let mut unload = UnloadGuestDriver::ver::get(false)?;

            unload.push(&mut gsp_objs.queues)?;
        }

        let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;

        timer_msec!({
            if gsp_objs.queues.gsp_falcon.as_ref().unwrap().falcon.rd32(0x40)? == 0x80000000 {
                break;
            }
        }, 2000, &gpu_base.timer);

        drop(bar);

        gsp_objs.queues.gsp_falcon.as_ref().unwrap().reset()?;

        // Boot fwsec into SB mode.
        let mut fwsec = Fwsec::new_from_bios(&gpu_base,
                                             gsp_objs.queues.gsp_falcon.as_ref().unwrap(),
                                             NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB, 0, 0, &fw.bl_fw)?;

        fwsec.boot()?;

        let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;
        let mut wpr2_hi = bar.readl(0x1fa828);

        if wpr2_hi != 0 {
            fw.unload_fw.boot(mbox0, Some(mbox1))?;
            wpr2_hi = bar.readl(0x1fa828);
            if wpr2_hi != 0 {
                pr_info!("WPR2 still set after unload\n");
            }
        } else {
            pr_info!("WPR2 not set, not unloading fw\n");
        }
        Ok(())
    }

    pub(crate) fn setup_ctx_buf_info(gr_ctx_bufs: KVec<CtxBufSize>) -> Result<KVec<CtxBufInfo>> {

        let mut buf_info = KVec::new();
        for i in 0..gr_ctx_bufs.len() {
            let mut map_idx: usize = 0xffffffff;
            for map in 0..CTX_BUF_MAP::ver.len() {
                if CTX_BUF_MAP::ver[map].id0 == i as u32 {
                    map_idx = map;
                    break;
                }
            }

            if map_idx == 0xffffffff {
                continue;
            }

            let mut size: u32 = gr_ctx_bufs[i].size;

            if CTX_BUF_MAP::ver[map_idx].id1 == fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_MAIN {
                size = (align(size as usize, 0x1000) + 64 * 0x1000) as u32; // per subtx headers
            }

            let page;
            if size >= 1 << 21 {
                page = 21;
            } else if size >= 1 << 16 {
                page = 16;
            } else {
                page = 12;
            }

            let align;
            if CTX_BUF_MAP::ver[map_idx].id1 == fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_ATTRIBUTE_CB {
                align = order_base_2(size as usize) as u8; //TODO;
            } else {
                align = page;
            }

            buf_info.push(CtxBufInfo {
                buffer_id: CTX_BUF_MAP::ver[map_idx].id1 as u16,
                size,
                page,
                align,
                global: CTX_BUF_MAP::ver[map_idx].global,
                init: CTX_BUF_MAP::ver[map_idx].init,
                ro: CTX_BUF_MAP::ver[map_idx].ro,
                priv_access_map: CTX_BUF_MAP::ver[map_idx].id1 == fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP,
                unrestricted_priv_access_map: false,
            }, GFP_KERNEL)?;

            if CTX_BUF_MAP::ver[map_idx].id1 == fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP {
                let last_ent = buf_info.len() - 1;
                buf_info.push(CtxBufInfo {
                    buffer_id: fw::ver::gen::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_UNRESTRICTED_PRIV_ACCESS_MAP as u16,
                    size: buf_info[last_ent].size,
                    page: buf_info[last_ent].page,
                    align: buf_info[last_ent].align,
                    global: buf_info[last_ent].global,
                    init: buf_info[last_ent].init,
                    ro:  buf_info[last_ent].ro,
                    priv_access_map: false,
                    unrestricted_priv_access_map: true,
                }, GFP_KERNEL)?;
            }
        }
        Ok(buf_info)
    }

    pub(crate) fn new(gpu_base: &Arc<GpuBase>,
                      vfn: &Arc<Vfn>,
                      event_handler: Arc<EventHandler>,
                      mm: &mut MemRange,
                      mut gsp_falcon: GspFalcon,
                      sec2: Sec2,
                      fw: Firmware) -> Result<Arc<GspManager::ver>> {
        let display_disabled = devinit::check_display_disable(&gpu_base)?;
        let fb_size = devinit::vidmem_size(&gpu_base)?;
        let vga_base = devinit::vga_workspace_addr(&gpu_base, fb_size, display_disabled)?;
        let vga_size = fb_size - vga_base;

        let sysmem_flush = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "sysmem flush page")?;

        {
            let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;

            if chipsets_before!(&gpu_base.spec.chipset, GA102) {
                bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
            } else {
                bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
                bar.writel((sysmem_flush.dma.dma_handle() >> 40) as u32, 0x100c40);
            }
        }

        let mut fb_addr_info = boot_structs::Wpr::ver::fill_fb_addr_info(
            &gpu_base, fb_size, vga_base, vga_size, &fw);

        let mut fwsec = Fwsec::new_from_bios(&gpu_base,
                                             &gsp_falcon,
                                             NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
                                             fb_addr_info.frts.addr,
                                             fb_addr_info.frts.size,
                                             &fw.bl_fw)?;

        fwsec.boot()?;

        gsp_falcon.set_app_version(fw.bootloader_fw.app_version);

        let mut gsp_objs = GSPSharedMemObjects::ver::new(&gpu_base)?;

        boot_structs::Wpr::ver::fill_wpr_meta(gsp_objs.wpr_meta.dma.start_ptr_mut(), &fw.gsp_fw.radix3, &fw.bootloader_fw, &fw.gsp_sigs.dma, &fb_addr_info);

        gsp_falcon.set_libos_addr(gsp_objs.libos.dma.dma_handle());

        gsp_objs.queues.bind_falcon(gsp_falcon, sec2.falcon);

        gsp_objs.queues.bind_kill_handler(event_handler);
        let mut gsp_system_info = GspSystemInfoRpcMsg::ver::new(&gpu_base, NOVA_ENABLE_VGPU)?;
        gsp_system_info.push(&mut gsp_objs.queues)?;

        let mut gsp_registry = GspRegistryRpcMsg::ver::new()?;
        gsp_registry.push(&mut gsp_objs.queues)?;

        gsp_objs.queues.gsp_falcon.as_ref().unwrap().reset()?;
        gsp_objs.queues.gsp_falcon.as_ref().unwrap().write_libos_addr()?;

        let boot_iova = gsp_objs.wpr_meta.dma.dma_handle();
        let booted = Self::init_gsp(&fw.loader_fw, &mut gsp_objs, boot_iova);

        match booted {
            Err(x) => {
                Self::fini(&gpu_base, &fw, &mut gsp_objs, false, 0xff, 0xff)?;
                return Err(x);
            }
            Ok(_) => {}
        }

        let mut gsp_static_config = GSPStaticConfigRpc::ver::new()?;
        gsp_static_config.push(&mut gsp_objs.queues)?;

        gsp_static_config.fill_fb_regions(&mut fb_addr_info)?;

        mm.init(0, (fb_addr_info.region[0].addr >> GSP_PAGE_SHIFT) as usize, (fb_addr_info.region[0].size >> GSP_PAGE_SHIFT) as usize)?;

        let internal_client: Arc<GspClient> = Arc::new(GspClient {
            object: Arc::new(GspObject { client: None, parent: None, handle: gsp_static_config.internal_client() }, GFP_KERNEL)?,
        }, GFP_KERNEL)?;

        let internal_device_object: Arc<GspObject> = Arc::new(GspObject {
            client: Some(internal_client.clone()),
            parent: Some((*internal_client).object.clone()),
            handle: gsp_static_config.internal_device(),
        }, GFP_KERNEL)?;

        let internal_subdevice_object: Arc<GspObject> = Arc::new(GspObject {
            client: Some(internal_client.clone()),
            parent: Some(internal_device_object.clone()),
            handle: gsp_static_config.internal_subdevice(),
        }, GFP_KERNEL)?;
        let internal_device: Arc<GspDevice> = Arc::new(GspDevice {
            object: internal_device_object,
            subdevice: internal_subdevice_object,
        }, GFP_KERNEL)?;

        let mut intr_kernel_table = InternalIntrGetKernelTableParams::ver::new(&internal_device)?;
        let intr_table = intr_kernel_table.push(&mut gsp_objs.queues)?;

        let mut constructed_table = GetConstructedFalconInfo::ver::new(&internal_device)?;
        let _ = constructed_table.push(&mut gsp_objs.queues)?;

        let mut fifo_table = FifoGetDeviceInfoTable::ver::new(&internal_device)?;
        let mut table = fifo_table.push(&mut gsp_objs.queues)?;

        constructed_table.fill_sizes(&mut table);

        let mut kgr_buffers = InternalStaticKGRGetContextBuffersInfo::ver::new(&internal_device)?;
        let gr_ctx_bufs = kgr_buffers.push(&mut gsp_objs.queues)?;

        let gr_ctx_info = Self::setup_ctx_buf_info(gr_ctx_bufs)?;

        let rsvd_chids;
        {
            #[ver(r == r535_113_01)]
            rsvd_chids = 0;
        }

        {
            #[ver(r == r570_86_16)]
            rsvd_chids = 8;
        }

        let runl = FifoRunList::create_runlist_from_table(&table, rsvd_chids)?;

        let mut fault_buffer_size = CEGetFaultMethodBufferSize::ver::new(&internal_device)?;
        let mthdbuf_size = fault_buffer_size.push(&mut gsp_objs.queues)?;

        let mut vmmu_segment_size_msg = GetVmmuSegmentSize::ver::new(&internal_device)?;
        let _ = vmmu_segment_size_msg.push(&mut gsp_objs.queues)?;

        let gpcs;
        let tpcs;

        #[ver(r == r535_113_01)]
        {
            let (in_gpcs, in_tpcs) = gsp_static_config.get_gr_info();
            gpcs = in_gpcs;
            tpcs = in_tpcs;
        }

        #[ver(r == r570_86_16)]
        {
            let mut gpcmsg = GpcInfo::ver::new(&internal_device)?;
            let gpc_mask = gpcmsg.push(&mut gsp_objs.queues)?;

            let mut in_tpcs: u8 = 0;
            let mut in_gpcs: u8 = 0;
            for gpc in 0..MAX_GPC_COUNT {
                if (gpc_mask & (1 << gpc)) != 0 {
                    let mut tpcmsg = TpcInfo::ver::new(&internal_device, gpc)?;
                    let tpc_mask = tpcmsg.push(&mut gsp_objs.queues)?;

                    in_tpcs += tpc_mask.count_ones() as u8;
                    in_gpcs += 1;
                }
            }
            gpcs = in_gpcs;
            tpcs = in_tpcs;
        }

        let gsp_objs = KBox::pin_init(new_mutex!(gsp_objs), GFP_KERNEL)?;

        let gsp_outer  = GSPSharedMemObjectsOuter::ver { inner: gsp_objs };
        let gsp_outer = Arc::new(gsp_outer, GFP_KERNEL)?;

        let gsp_stall = IntrInfo::find_stall(&intr_table, EngineType::GSP, 0)?;
        vfn.add_handler(gsp_stall, gsp_outer.clone() as Arc<dyn VfnHandler>)?;
        vfn.intr_allow(gsp_stall)?;
        vfn.rearm()?;

        let mgr = GspManager::ver {
            gpu_base: gpu_base.clone(),
            sysmem_flush,
            fw,
            fb_addr_info,
            gsp_objs: gsp_outer,
            bar1_pdb: gsp_static_config.bar1_pdb(),
            bar2_pdb: gsp_static_config.bar2_pdb(),
            gpcs,
            tpcs,
            vmmu_segment_size: vmmu_segment_size_msg.get_segment_size(),
            runl,
            rsvd_chids,
            mthdbuf_size,
            internal_client,
            internal_device,
            gr_ctx_info,
            intr_info: intr_table,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}

#[versions(GSP)]
impl Drop for GspManager::ver {
    fn drop(&mut self) {

        let mut inner_gsp_objs = self.gsp_objs.inner.lock();
        let _ = Self::fini(&self.gpu_base, &self.fw, &mut inner_gsp_objs, true, 0xff, 0xff);
    }
}
