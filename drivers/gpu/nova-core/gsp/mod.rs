#![allow(dead_code)]

pub(crate) use kernel::macros::versions;

use kernel::bindings;
use core::sync::atomic::{AtomicU16, Ordering};
use kernel::prelude::*;
use kernel::sync::{Arc, Mutex, new_mutex};
use kernel::sync::lock::Guard;
use kernel::sync::lock::mutex::MutexBackend;

use crate::{align, div_round_up};
use crate::chipsets_before;
use crate::devinit;
use crate::dma::DmaObject;
use crate::falcon::Falcon;
use crate::gsp::fwsec::Fwsec;
use crate::gsp::fwsec::{NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
                        NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB};
use crate::gsp::gsp_falcon::GspFalcon;
use crate::gsp::sharedq::*;

use crate::gpu::NOVA_ENABLE_VGPU;
use crate::gpu::Chipset;
use crate::gpu::FBInfo;
use crate::gpu::FifoDeviceInfoTable;
use crate::gpu::Firmware;
use crate::gpu::GpuBase;

use crate::gsp::alloc_msgs::*;
use crate::gsp::ctrl_msgs::*;
use crate::gsp::msgs::*;
use crate::gsp::rpc_msgs::*;
use crate::vfn::{Vfn, VfnHandler};
use crate::mmu::mm::MemRange;
use crate::nvfw::*;
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

#[versions(GSP)]
#[allow(unused)]
pub(crate) struct GSPSharedMemObjects {
    pub libos: DmaObject,
    pub loginit: DmaObject,
    pub logintr: DmaObject,
    pub logrm: DmaObject,
    pub rmargs: DmaObject,
    pub kern: Option<DmaObject>,
    pub shm: Arc<DmaObject>,
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

        let flcn = gsp_falcon.falcon.clone();
        let intr: u32 = flcn.rd32(0x0008)?;
        let inte: u32 = flcn.rd32(flcn.addr2 + flcn.riscv_irqmask)?;
        let mut stat = intr & inte;

        if stat == 0 {
            pr_info!("GSP IRQ fired with nothing set {:#x} {:#x}", intr, inte);
            return Ok(0);
        }

        if stat & 0x00000040 != 0 {
            flcn.wr32(0x4, 0x40)?;
            pr_info!("GSP WORK");
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

        gsp_objs.handle_vfn()?;
        Ok(0)
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

    pub(crate) fn new(gpu_base: Arc<GpuBase>) -> Result<Self> {
        let cmdq_size = 0x40000;
        let msgq_size = 0x40000;
        let mut ptes_nr = (cmdq_size + msgq_size) >> GSP_PAGE_SHIFT;
        ptes_nr += div_round_up((ptes_nr * size_of::<u64>()) as usize, GSP_PAGE_SIZE as usize);
        let ptes_size = align(ptes_nr * size_of::<u64>() as usize, GSP_PAGE_SIZE as usize);
        let shmem_size = cmdq_size + msgq_size + ptes_size;

        let mut shm = DmaObject::new_cleared(&gpu_base.dev, shmem_size, "shm")?;

        Self::fill_shm_ptes(&mut shm, ptes_nr);

        let num_logs : usize = fw::ver::gen::LOGIDX_SIZE as usize;
        let mut kern: Option<DmaObject> = None;
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
        let shm = Arc::new(shm, GFP_KERNEL)?;
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

#[versions(GSP)]
pub(crate) struct GspManager {
    gpu_base: Arc<GpuBase>,
    sysmem_flush: DmaObject,
    fw: Firmware,
    fb_addr_info: FBInfo,
    gsp_objs: Arc<GSPSharedMemObjectsOuter::ver>,
    bar1_pdb: u64,
    bar2_pdb: u64,
    vmmu_segment_size: u64,
    alloc_id: AtomicU16,
    fifo_info: FifoDeviceInfoTable,
}

pub(crate) trait GspManager: Send + Sync {
    fn alloc_client_device(&self) -> Result<(Arc<GspClient>,
                                             Arc<GspDevice>)>;
    fn free_client(&self, client: Arc<GspClient>) -> Result<()>;
    fn free_device(&self, device: Arc<GspDevice>) -> Result<()>;

    fn update_bar_pde(&self, bar: u32, addr: u64, shift: u32) -> Result<()>;
    fn get_bar_pdb(&self, bar: u8) -> u64;

    fn get_vmmu_segment_size(&self) -> u64;
    fn get_engine_bitmap(&self) -> u64;

    fn cleanup_vgpu_plugin(&self, device: Arc<GspDevice>, gfid: u32) -> i32;
    fn shutdown_vgpu_plugin_task(&self, device: Arc<GspDevice>, gfid: u32) -> i32;
    fn bootload_vgpu_plugin_task(&self, device: Arc<GspDevice>, params: *const bindings::bootload_vgpu) -> i32;
    fn add_vgpu_type(&self, device: Arc<GspDevice>, count: u32, ptr: *const core::ffi::c_void) -> i32;
}

#[versions(GSP)]
impl GspManager for GspManager::ver {

    fn alloc_client_device(&self) -> Result<(Arc<GspClient>,
                                             Arc<GspDevice>)> {
        let client = Arc::new(self.alloc_client()?, GFP_KERNEL)?;
        let device = Arc::new(self.alloc_device(client.clone())?, GFP_KERNEL)?;
        Ok((client, device))
    }

    fn free_client(&self, client: Arc<GspClient>) -> Result<()> {
        let mut msg = FreeMsg::ver::get(&client.object)?;

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        self.free_client_id(client.object.handle & 0xffff);
        Ok(())
    }

    fn free_device(&self, device: Arc<GspDevice>) -> Result<()>{
        let mut msg = FreeMsg::ver::get(&device.subdevice)?;

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;

        let mut msg = FreeMsg::ver::get(&device.object)?;

        msg.push(&mut gsp_objs.queues)?;
        Ok(())
    }

    fn update_bar_pde(&self, bar: u32, addr: u64, shift: u32) -> Result<()> {
        let mut msg = UpdateBarPdeMsg::ver::get(bar, addr, shift)?;

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        msg.push(&mut gsp_objs.queues)?;
        Ok(())
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

    fn get_engine_bitmap(&self) -> u64 {
        let mut mask: u64 = 0;
        for entry in &self.fifo_info.table {
            mask |= 1_u64 << entry.id;
        }
        mask
    }

    fn cleanup_vgpu_plugin(&self, device: Arc<GspDevice>, gfid: u32) -> i32 {
        let mut msg = CleanupVgpuPlugin::ver::new(&device, gfid).unwrap();

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn shutdown_vgpu_plugin_task(&self, device: Arc<GspDevice>, gfid: u32) -> i32 {
        let mut msg = ShutdownVgpuPluginTask::ver::new(&device, gfid).unwrap();

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn bootload_vgpu_plugin_task(&self, device: Arc<GspDevice>, params: *const bindings::bootload_vgpu) -> i32 {
        let mut msg = BootloadVgpuPluginTask::ver::new(&device, params).unwrap();
        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }

    fn add_vgpu_type(&self, device: Arc<GspDevice>, count: u32, ptr: *const core::ffi::c_void) -> i32 {
        let mut msg = PgpuAddVgpuType::ver::new(&device, count, ptr).unwrap();
        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
        match msg.push(&mut gsp_objs.queues) {
            Err(x) => { x.to_errno() }
            _ => { 0 }
        }
    }
}

#[versions(GSP)]
impl GspManager::ver {

    fn get_new_client_id(&self) -> u16 {
        self.alloc_id.fetch_add(1, Ordering::SeqCst)
    }

    fn free_client_id(&self, _handle: u32) {
//        self.ids.lock().unwrap().free(handle);
    }

    fn alloc_client(&self) -> Result<GspClient> {
        let id = self.get_new_client_id();
        let mut msg = AllocClient::ver::new(id, 0xffffffff)?;

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
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

        let gsp_objs = self.gsp_objs.clone();
        let mut gsp_objs = gsp_objs.inner.lock();
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
            if (gsp_objs.queues.gsp_falcon.as_ref().unwrap().falcon.rd32(0x40)? & 0x80000000) != 0 {
                break;
            }
        }, 2000, &gpu_base.timer);

        gsp_objs.queues.gsp_falcon.as_ref().unwrap().reset()?;

        // Boot fwsec into SB mode.
        let mut fwsec = Fwsec::new_from_bios(&gpu_base,
                                             gsp_objs.queues.gsp_falcon.as_ref().unwrap(),
                                             NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB, 0, 0, &fw.bl_fw)?;

        fwsec.boot()?;

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

    pub(crate) fn new(gpu_base: Arc<GpuBase>,
                      vfn: &Arc<Vfn>,
                      mm: &mut MemRange,
                      mut gsp_falcon: GspFalcon,
                      sec2: Sec2,
                      fw: Firmware) -> Result<Arc<GspManager::ver>> {
        let display_disabled = devinit::check_display_disable(&gpu_base)?;
        let fb_size = devinit::vidmem_size(&gpu_base)?;
        let vga_base = devinit::vga_workspace_addr(&gpu_base, fb_size, display_disabled)?;
        let vga_size = fb_size - vga_base;

        let sysmem_flush = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "sysmem flush page")?;

        let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;

        if chipsets_before!(&gpu_base.spec.chipset, GA102) {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
        } else {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
            bar.writel((sysmem_flush.dma.dma_handle() >> 40) as u32, 0x100c40);
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

        let mut gsp_objs = GSPSharedMemObjects::ver::new(gpu_base.clone())?;

        boot_structs::Wpr::ver::fill_wpr_meta(gsp_objs.wpr_meta.dma.start_ptr_mut(), &fw.gsp_fw.radix3, &fw.bootloader_fw, &fw.gsp_sigs.dma, &fb_addr_info);

        gsp_falcon.set_libos_addr(gsp_objs.libos.dma.dma_handle());

        gsp_objs.queues.bind_falcon(gsp_falcon, sec2.falcon);

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
        let table = fifo_table.push(&mut gsp_objs.queues)?;

        let mut fault_buffer_size = CEGetFaultMethodBufferSize::ver::new(&internal_device)?;
        let _ = fault_buffer_size.push(&mut gsp_objs.queues)?;

        let mut vmmu_segment_size_msg = GetVmmuSegmentSize::ver::new(&internal_device)?;
        let _ = vmmu_segment_size_msg.push(&mut gsp_objs.queues)?;

        let gsp_objs = KBox::pin_init(new_mutex!(gsp_objs), GFP_KERNEL)?;

        let gsp_outer  = GSPSharedMemObjectsOuter::ver { inner: gsp_objs };
        let gsp_outer = Arc::new(gsp_outer, GFP_KERNEL)?;

        vfn.add_handler(intr_table[0].stall, gsp_outer.clone() as Arc<dyn VfnHandler>)?;
        vfn.intr_allow(intr_table[0].stall)?;
        vfn.rearm()?;

        let mgr = GspManager::ver {
            gpu_base,
            sysmem_flush,
            fw,
            fb_addr_info,
            gsp_objs: gsp_outer,
            bar1_pdb: gsp_static_config.bar1_pdb(),
            bar2_pdb: gsp_static_config.bar2_pdb(),
            vmmu_segment_size: vmmu_segment_size_msg.get_segment_size(),
            alloc_id: AtomicU16::new(0xab00),
            fifo_info: table,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}

#[versions(GSP)]
impl Drop for GspManager::ver {
    fn drop(&mut self) {
        let gsp_objs = self.gsp_objs.clone();
        let locked_gsp_objs = gsp_objs.inner.lock();
        let mut inner_gsp_objs = locked_gsp_objs;
        let _ = Self::fini(&self.gpu_base, &self.fw, &mut inner_gsp_objs, true, 0xff, 0xff);
    }
}
