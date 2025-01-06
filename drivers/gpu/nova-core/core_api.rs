
use kernel::{
    bindings,
    container_of,
    prelude::*,
    sync::{Arc, ArcBorrow},
};

use kernel::types::ForeignOwnable;
use crate::gpu::{Gpu, GpuDevice, GpuClient, Chipset, GpuDeviceVmm, GpuChanObject};
use crate::mmu::memory::NVKM_MM_PAGE_SHIFT;
use crate::mmu::mmu::Mmu;
use crate::mmu::vmm::{Vmm, VmmMap, SizeAddr};
use crate::mmu::memory::{VramObj,DmaMemObj,SglMemObj,MemObjType};
use crate::mmu::memory::{Memory};
use crate::mmu::mmu::NVKM_MEM_VRAM;
use crate::vfn::Vfn;

use crate::accel::fifo::Channel;
use crate::driver::NovaCoreData;
use crate::mmu::vmm::VMM_TU102;

#[no_mangle]
#[allow(dead_code)]
/// Provide the API for the VFIO RFC
pub unsafe extern "C" fn nova_core_fill_info(auxdev: *mut bindings::auxiliary_device,
                                             info: *mut bindings::nova_core_info) {
    pr_info!("core: {:?} {:?}\n", auxdev, info);
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };

    let gpu = unsafe { &(*core_driver).gpu };
    let pdev = unsafe { &(*core_driver).pdev };
    let ncinfo = unsafe { &mut (*info) };
    let base = gpu.base.clone();


    ncinfo.boot0 = base.spec.boot0;

    ncinfo.fifo_class = base.spec.gpu_consts.fifo_class;
    ncinfo.ce_class = base.spec.gpu_consts.ce_class;
    let (gpcs, tpcs) = gpu.gsp.get_gr_info();
    ncinfo.gr_units = ((tpcs as u16) << 8) | gpcs as u16;
    ncinfo.mthdbuf_size = gpu.gsp.get_mthdbuf_size();
    ncinfo.chipset = Chipset::val(&base.spec.chipset);

    ncinfo.resource_addr[0] = pdev.resource_start(0).unwrap();
    ncinfo.resource_addr[1] = pdev.resource_start(1).unwrap();
    ncinfo.resource_addr[2] = pdev.resource_start(3).unwrap();
    ncinfo.resource_size[0] = pdev.resource_len(0).unwrap();
    ncinfo.resource_size[1] = pdev.resource_len(1).unwrap();
    ncinfo.resource_size[2] = pdev.resource_len(3).unwrap();
    (ncinfo.pci_vendor_id, ncinfo.pci_device_id) = pdev.device_vendor_id();

    pr_info!("got here\n");
    ncinfo.ram_user = (gpu.instmem.vram_mm.size(0).unwrap() as u64) << NVKM_MM_PAGE_SHIFT;

    let mut runlidx = 0;
    let runl = gpu.gsp.get_runlist();
    let chid_nr: u16 = runl.chids.nr as u16;
    pr_info!("got here runl {}\n", runl.entries.len());
    for ent in &runl.entries {
        let engidx = 0;

        for eng in &ent.engns {
            let core_type = eng.eng_type.to_core();
            let mut idx = 0;

            while idx < ncinfo.engine_nr {
                if ncinfo.engine[idx as usize].eng_type == core_type {
                    break;
                }
                idx += 1;
            }
            if idx == ncinfo.engine_nr {
                ncinfo.engine[idx as usize].eng_type = core_type;
                ncinfo.engine_nr += 1;
            }

            let engi = ncinfo.runl[runlidx].engn_nr as usize;

            ncinfo.runl[runlidx].engn[engi].engine = idx;
            ncinfo.runl[runlidx].engn[engi].inst = eng.inst as u8;
            ncinfo.runl[runlidx].engn_nr = (engi + 1) as u8;
        }

        ncinfo.runl[runlidx].id = (ent.id & 0xff) as u8;
        ncinfo.runl[runlidx].chan_nr = chid_nr;
        runlidx += 1;
    }
    ncinfo.runl_nr = runl.entries.len() as u8;
}

#[no_mangle]
#[allow(dead_code)]
/// Provide the API for the VFIO RFC
pub unsafe extern "C" fn nova_core_timer_time(auxdev: *mut bindings::auxiliary_device) -> u64 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };

    let base = unsafe { (*core_driver).gpu.base.clone() };

    base.timer.read().unwrap()
}

fn alloc_gsp_client(gpu: &Gpu,
                    ncclient: &mut bindings::nova_core_gsp_client) -> Result<()> {

    let (gpu_client, gpu_device) = gpu.alloc_client_device()?;

    let cli_clone = gpu_client.clone();
    let dev_clone = gpu_device.clone();
    ncclient.gsp_client = cli_clone.into_foreign() as *mut core::ffi::c_void;
    ncclient.gsp_device = dev_clone.into_foreign() as *mut core::ffi::c_void;

    Ok(())
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_alloc_gsp_client(auxdev: *mut bindings::auxiliary_device,
                                       client: *mut bindings::nova_core_gsp_client) -> i32
{
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let ncclient = unsafe { &mut (*client) };

    match alloc_gsp_client(gpu, ncclient) {
        Err(x) => x.to_errno(),
        Ok(()) => 0
    }
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_free_gsp_client(client: *mut bindings::nova_core_gsp_client)
{
    let ncclient = unsafe { &mut (*client) };

    let _gpu_device : Arc<GpuDevice> = unsafe { Arc::from_foreign(ncclient.gsp_device) };
    ncclient.gsp_device = core::ptr::null_mut();

    let _gpu_client : Arc<GpuClient> = unsafe { Arc::from_foreign(ncclient.gsp_client) };
    ncclient.gsp_client = core::ptr::null_mut();
}

#[no_mangle]
#[allow(dead_code)]
/// Provide the API for the VFIO RFC
pub unsafe extern "C" fn nova_core_alloc_mmu(auxdev: *mut bindings::auxiliary_device,
                                             mmu: *mut bindings::nova_core_mmu) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let ncmmu = unsafe { &mut (*mmu) };

    let gpu_mmu = match gpu.alloc_mmu() {
        Err(x) => { return x.to_errno() }
        Ok(x) => { x }
    };

    let mmu_clone = gpu_mmu.clone();

    ncmmu.arc = mmu_clone.into_foreign() as *mut core::ffi::c_void;
    ncmmu.info.dmabits = gpu_mmu.dma_bits;

    ncmmu.info.type_nr = gpu_mmu.types.len() as u8;
    ncmmu.info.heap_nr = gpu_mmu.heaps.len() as u8;
    let mut idx: usize = 0;
    for mmu_type in &gpu_mmu.types {
        ncmmu.info.mmu_type[idx].mmu_type = mmu_type.mmu_type;
        ncmmu.info.mmu_type[idx].heap = mmu_type.heap;
        idx = idx + 1;
    }
    for i in 0..16 {
        ncmmu.info.kind[i] = gpu_mmu.kindinfo.kind[i];
    }
    ncmmu.info.kind_nr = 16;
    ncmmu.info.kind_inv = gpu_mmu.kindinfo.invalid;
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_free_mmu(mmu: *mut bindings::nova_core_mmu) {
    unsafe {
        if (*mmu).arc == core::ptr::null_mut() {
            return;
        }
        let _mmu : Arc<Mmu> = Arc::from_foreign((*mmu).arc);
        (*mmu).arc = core::ptr::null_mut();
    }
}

#[no_mangle]
#[allow(dead_code)]
/// Provide the API for the VFIO RFC
pub unsafe extern "C" fn nova_core_alloc_vmm(auxdev: *mut bindings::auxiliary_device,
                                             client: *mut bindings::nova_core_gsp_client,
                                             mmu_ptr: *mut bindings::nova_core_mmu,
                                             vmm_type: u8,
                                             vmm: *mut bindings::nova_core_vmm) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let ncvmm = unsafe { &mut (*vmm) };

    let mmu: ArcBorrow<'_, Mmu> = unsafe { Arc::borrow((*mmu_ptr).arc) };
    let cli: ArcBorrow<'_, GpuClient>=  unsafe { Arc::borrow((*client).gsp_client) };
    let dev: ArcBorrow<'_, GpuDevice>=  unsafe { Arc::borrow((*client).gsp_device) };

    let dev_arc: Arc<GpuDevice> = Arc::<GpuDevice>::from(dev);

    let gpu_vmm = match gpu.alloc_vmm(dev_arc, kernel::page::PAGE_SIZE as u64, 0, vmm_type) {
        Err(x) => { return x.to_errno() }
        Ok(x) => { x }
    };

    let vmm_clone = gpu_vmm.clone();
    ncvmm.arc = vmm_clone.into_foreign() as *mut core::ffi::c_void;
    ncvmm.info.limit = gpu_vmm.vmm.limit().unwrap();

    let mut page_nr = 0;
    for vmmp in VMM_TU102 {
        ncvmm.info.page[page_nr].shift = vmmp.shift;
        ncvmm.info.page[page_nr].page_flags = vmmp.vmm_page_type;
        page_nr = page_nr + 1;
    }
    ncvmm.info.page_nr = page_nr as u8;

    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_free_vmm(vmm: *mut bindings::nova_core_vmm) {
    unsafe {
        if (*vmm).arc == core::ptr::null_mut() {
            return;
        }
        let _vmm : Arc<GpuDeviceVmm> = Arc::from_foreign((*vmm).arc);
        (*vmm).arc = core::ptr::null_mut();
    }
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_vmm_get(vmm_ptr: *mut bindings::nova_core_vmm,
                                           get_type: u8,
                                           sparse: bool,
                                           page: u8,
                                           align: u8,
                                           size: u64,
                                           addr: *mut u64) -> i32 {
    let vmm: ArcBorrow<'_, GpuDeviceVmm> = unsafe { Arc::borrow((*vmm_ptr).arc) };

    let getref = get_type == bindings::NVIF_VMM_GET_PTES;
    let mapref = get_type == bindings::NVIF_VMM_GET_ADDR;

    let vma = match (*vmm).vmm.get(getref, mapref, sparse, page, align, size) {
        Err(x) => { return x.to_errno(); }
        Ok(x) => x
    };
    unsafe { *addr = vma.addr() };
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_vmm_put(vmm_ptr: *mut bindings::nova_core_vmm,
                                           addr: u64) {
    unsafe {
        let vmm: ArcBorrow<'_, GpuDeviceVmm> = unsafe { Arc::borrow((*vmm_ptr).arc) };
        let _ = (*vmm).vmm.put_addr(addr);
    }
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_alloc_mem(auxdev: *mut bindings::auxiliary_device,
                                             mmu_ptr: *mut bindings::nova_core_mmu,
                                             name: *const core::ffi::c_char,
                                             mmu_type: u8,
                                             contig: bool,
                                             dma: *mut bindings::dma_addr_t,
                                             page: u8,
                                             size: u64,
                                             obj: *mut bindings::nova_core_memory_obj) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let ncobj = unsafe { &mut (*obj) };
    let gpu = unsafe { &(*core_driver).gpu };
    let mmu: ArcBorrow<'_, Mmu> = unsafe { Arc::borrow((*mmu_ptr).arc) };

    if (*mmu).types[mmu_type as usize].mmu_type & NVKM_MEM_VRAM != 0 {
        let vram = match VramObj::new(gpu.instmem.vram_mm.clone(),
                                      0, mmu_type, page, size as usize, contig, false) {
            Err(x) => { return x.to_errno(); }
            Ok(x) => x
        };

        let nodes = vram.nodes.len();
        ncobj.addr = vram.addr().unwrap();
        ncobj.size = vram.size().unwrap();
        ncobj.page = vram.page();
        ncobj.obj_type = bindings::NVIF_MEM_OBJ_VRAM;
        let vram : Pin<KBox<VramObj>> = KBox::pin(vram, GFP_KERNEL).unwrap();

        ncobj.obj = vram.into_foreign() as _;
        pr_info!("allocated uvram {:?} {:?} {} {} {}\n", unsafe { CStr::from_char_ptr(name) }, ncobj.obj,
                 ncobj.addr, ncobj.size, nodes);
    } else {
        let memobj = match DmaMemObj::new(unsafe { *dma } , 0, mmu_type, page, size, contig, true) {
            Err(x) => { return x.to_errno(); }
            Ok(x) => x
        };

        ncobj.page = memobj.page();
        ncobj.size = memobj.size().unwrap();
        ncobj.obj_type = bindings::NVIF_MEM_OBJ_DMA;
        let memobj : Pin<KBox<DmaMemObj>> = KBox::pin(memobj, GFP_KERNEL).unwrap();
        ncobj.obj = memobj.into_foreign() as _;
        pr_info!("allocated udma {:?} {:?} sz:{:#x}\n", unsafe { CStr::from_char_ptr(name) }, ncobj.obj,
                 ncobj.size);
    }
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_free_mem(obj_ptr: *mut bindings::nova_core_memory_obj) {
    unsafe {
        if (*obj_ptr).obj == core::ptr::null_mut() {
            return;
        }

        pr_info!("free mem {:?}\n", (*obj_ptr).obj);
        match (*obj_ptr).obj_type {
            bindings::NVIF_MEM_OBJ_VRAM => {
                let _vramobj : KBox<VramObj> = KBox::from_foreign((*obj_ptr).obj);
            },
            bindings::NVIF_MEM_OBJ_DMA => {
                let _dmaobj : KBox<DmaMemObj> = KBox::from_foreign((*obj_ptr).obj);
            },
            _ => {}
        }
        (*obj_ptr).obj = core::ptr::null_mut();
    }

}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_mem_bar1_map(auxdev: *mut bindings::auxiliary_device,
                                                obj_ptr: *mut bindings::nova_core_memory_obj,
                                                kind: u32) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let pdev = unsafe { &(*core_driver).pdev };
    let memobj = unsafe { &mut (*obj_ptr) };
    let vmm = gpu.bar.bar1_vmm();
    match memobj.obj_type {
        bindings::NVIF_MEM_OBJ_VRAM => {
            let vramobj : &VramObj = unsafe { KBox::borrow(memobj.obj) };

            let size = vramobj.size().unwrap();

            let vma = match vmm.get(false, true, false, 12, 0, size) {
                Err(x) => { return x.to_errno(); }
                Ok(v) => v
            };

            let map = match vramobj.vram_map(0, &vmm, vma.clone(), kind as u8) {
                Err(x) => { return x.to_errno(); }
                Ok(m) => m
            };

            memobj.bar1_vma_addr = vma.addr();

            memobj.bar1_map_handle = unsafe { bindings::ioremap(pdev.resource_start(1).unwrap() + vma.addr() as u64, size as u64) };
        },
        _ => { return EINVAL.to_errno(); }
    }
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_mem_bar1_unmap(auxdev: *mut bindings::auxiliary_device,
                                                  obj_ptr: *mut bindings::nova_core_memory_obj) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let pdev = unsafe { &(*core_driver).pdev };
    let memobj = unsafe { &mut (*obj_ptr) };
    let vmm = gpu.bar.bar1_vmm();

    unsafe { bindings::iounmap(memobj.bar1_map_handle) };
    memobj.bar1_map_handle = core::ptr::null_mut();
    let _ = vmm.put_addr(memobj.bar1_vma_addr);
    memobj.bar1_vma_addr = 0;
    0
}


#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_vmm_map(auxdev: *mut bindings::auxiliary_device,
                                           vmm_ptr: *mut bindings::nova_core_vmm,
                                           args: *const bindings::nova_core_map_args,
                                           obj_ptr: *mut bindings::nova_core_memory_obj) -> i32 {
    let memobj = unsafe { &(*obj_ptr) };
    let ncargs = unsafe { &(*args) };
    let vmm: ArcBorrow<'_, GpuDeviceVmm> = unsafe { Arc::borrow((*vmm_ptr).arc) };

    let vramobj: &VramObj;
    let dmaobj: &DmaMemObj;

    let obj: &dyn Memory;

    match memobj.obj_type {
        bindings::NVIF_MEM_OBJ_VRAM => {
            let _vramobj : &VramObj = unsafe { KBox::borrow(memobj.obj) };
            obj = _vramobj as &dyn Memory;
        },
        bindings::NVIF_MEM_OBJ_DMA => {
            let _dmaobj : &DmaMemObj = unsafe { KBox::borrow(memobj.obj) };
            obj = _dmaobj as &dyn Memory;
        },
        _ => { return EINVAL.to_errno(); }
    }
    let vma = match (*vmm).vmm.get_addr(ncargs.addr) {
        Err(err) => { return err.to_errno(); },
        Ok(vma) => vma
    };

    let mut vmmmap = VmmMap {
        memory: obj,
        offset: ncargs.offset,
        kind: ncargs.kind,
        ro: ncargs.ro,
        private: ncargs.private,
        vol: ncargs.vol,
    };
    let _ = vmm.vmm.map(vma, &mut vmmmap);
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_vmm_unmap(vmm_ptr: *mut bindings::nova_core_vmm,
                                             addr: u64) {
    unsafe {
        let vmm: ArcBorrow<'_, GpuDeviceVmm> = unsafe { Arc::borrow((*vmm_ptr).arc) };
        let _ = (*vmm).vmm.unmap_addr(addr);
    }
}



#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_alloc_chan(auxdev: *mut bindings::auxiliary_device,
                                              client: *mut bindings::nova_core_gsp_client,
                                              runl: u8, chan_priv: bool,
                                              offset: u64, length: u64,
                                              vmm_ptr: *mut bindings::nova_core_vmm,
                                              userd: *mut bindings::nova_core_memory_obj,
                                              name: *const core::ffi::c_char,
                                              chan: *mut bindings::nova_core_chan) -> i32 {

    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let cli: ArcBorrow<'_, GpuClient>=  unsafe { Arc::borrow((*client).gsp_client) };
    let dev: ArcBorrow<'_, GpuDevice>=  unsafe { Arc::borrow((*client).gsp_device) };
    let vmm: ArcBorrow<'_, GpuDeviceVmm> = unsafe { Arc::borrow((*vmm_ptr).arc) };

    let cli_arc: Arc<GpuClient> = Arc::<GpuClient>::from(cli);
    let dev_arc: Arc<GpuDevice> = Arc::<GpuDevice>::from(dev);
    let vmm_arc: Arc<GpuDeviceVmm> = Arc::<GpuDeviceVmm>::from(vmm);

    let ncchan = unsafe { &mut (*chan) };
    let userd_obj : &VramObj = unsafe { KBox::borrow((*userd).obj) };
    let gpu_chan = match gpu.create_channel(cli_arc, dev_arc,
                                            runl as u32, chan_priv,
                                            offset, length,
                                            vmm_arc, userd_obj) {
        Err(x) => { return x.to_errno() }
        Ok(x) => { x }
    };

    ncchan.info.id = gpu_chan.id as u16;
    ncchan.info.doorbell_token = gpu_chan.doorbell;

    let chan_clone = gpu_chan.clone();

    ncchan.arc = chan_clone.into_foreign() as *mut core::ffi::c_void;
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_free_chan(chan: *mut bindings::nova_core_chan) {
    unsafe {
        if (*chan).arc == core::ptr::null_mut() {
            return;
        }
        let _chan : Arc<Channel> = Arc::from_foreign((*chan).arc);
        (*chan).arc = core::ptr::null_mut();
    }
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_chan_alloc_object(auxdev: *mut bindings::auxiliary_device,
                                                     chan: *mut bindings::nova_core_chan,
                                                     obj_ptr: *mut bindings::nova_core_chan_obj) -> i32 {

    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };
    let gpu = unsafe { &(*core_driver).gpu };
    let chan: ArcBorrow<'_, Channel>=  unsafe { Arc::borrow((*chan).arc) };
    let obj_info = unsafe { &mut (*obj_ptr) };

    let obj = match gpu.create_channel_obj(&chan.clone(), obj_info.handle, obj_info.class) {
        Err(x) => { return x.to_errno() }
        Ok(x) => { x }
    };

    obj_info.arc = obj.into_foreign() as *mut core::ffi::c_void;
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_chan_free_object(auxdev: *mut bindings::auxiliary_device,
                                                    chan: *mut bindings::nova_core_chan,
                                                    obj_ptr: *mut bindings::nova_core_chan_obj) {
    let obj_info = unsafe { &mut (*obj_ptr) };
    unsafe {
        if obj_info.arc == core::ptr::null_mut() {
            return;
        }
        let _chan : Arc<GpuChanObject> = Arc::from_foreign(obj_info.arc);
        obj_info.arc = core::ptr::null_mut();
    }
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_map_user(auxdev: *mut bindings::auxiliary_device,
                                            user_ptr: *mut bindings::nova_core_user_info) -> i32 {
    let core_driver = unsafe { container_of!(auxdev, NovaCoreData, auxdev) };

    let pdev = unsafe { &(*core_driver).pdev };
    let user = unsafe { &mut (*user_ptr) };

    let (offset, size) = Vfn::user_info();
    user.size = size;
    user.ptr = unsafe { bindings::ioremap(pdev.resource_start(0).unwrap() + offset as u64, size as u64) };
    0
}

#[no_mangle]
#[allow(dead_code)]
pub unsafe extern "C" fn nova_core_unmap_user(auxdev: *mut bindings::auxiliary_device,
                                              user_ptr: *mut bindings::nova_core_user_info) {
    let user = unsafe { &(*user_ptr) };

    unsafe { bindings::iounmap(user.ptr) };
}
