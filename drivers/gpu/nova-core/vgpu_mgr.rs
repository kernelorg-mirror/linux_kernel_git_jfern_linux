
use kernel::bindings;
use kernel::prelude::*;
use kernel::sync::{Arc, Mutex, UniqueArc, ArcBorrow};
use kernel::new_mutex;
use kernel::types::ForeignOwnable;

use crate::driver::NovaCoreDriver;
use crate::driver::NovaCoreData;
use crate::mmu::memory::NVKM_MM_PAGE_SHIFT;
use crate::mmu::memory::VramObj;
use crate::mmu::memory::Memory;
use crate::mmu::vmm::Vma;
use crate::gpu::{GpuDevice, GpuClient};

use crate::gpu::NOVA_ENABLE_VGPU;
struct VGPUMgrInner {
    vfio_handle_data: bindings::nvidia_vgpu_vfio_handle_data,
}

#[pin_data]
pub(crate) struct VGPUMgr {
    enabled: bool,
    vmmu_segment_size: u64,
    #[pin]
    inner: Mutex<VGPUMgrInner>,
}

#[pin_data]
#[repr(C)]
pub(crate) struct VGPUMem {
    #[pin]
    base: bindings::nvidia_vgpu_mem,
    obj: VramObj,
    bar1_vma: Option<Arc<Vma>>,
    core: Arc<NovaCoreData>,
}

impl VGPUMgr {
    pub(crate) fn new(segment_size: u64) -> Result<Arc<Self>> {

        let vgpu_mgr = UniqueArc::pin_init(pin_init!(Self {
            enabled: NOVA_ENABLE_VGPU,
            vmmu_segment_size: segment_size,
            inner <- new_mutex!(VGPUMgrInner {
                vfio_handle_data: bindings::nvidia_vgpu_vfio_handle_data { priv_: core::ptr::null_mut() }
            }),
        }), GFP_KERNEL)?;
        Ok(vgpu_mgr.into())
    }
}
unsafe extern "C" fn nvkm_vgpu_mgr_is_enabled(handle: *mut core::ffi::c_void) -> bool {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;

    let nova = unsafe { (*drv).data() };
    pr_info!("vgpu mgr is enabled\n");
    nova.gpu.vgpu.enabled
}

unsafe extern "C" fn get_handle(handle: *mut core::ffi::c_void,
                                data: *mut bindings::nvidia_vgpu_vfio_handle_data) {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };

    pr_info!("get handle\n");
    let inner = nova.gpu.vgpu.inner.lock();

    if inner.vfio_handle_data.priv_ != core::ptr::null_mut() {
        unsafe { core::ptr::copy_nonoverlapping(&inner.vfio_handle_data as *const bindings::nvidia_vgpu_vfio_handle_data, data, core::mem::size_of::<bindings::nvidia_vgpu_vfio_handle_data>()); }
    }
}

unsafe extern "C" fn attach_handle(handle: *mut core::ffi::c_void,
                                   data: *mut bindings::nvidia_vgpu_vfio_handle_data) -> i32 {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };

    pr_info!("attach handle\n");
    let mut inner = nova.gpu.vgpu.inner.lock();

    unsafe { core::ptr::copy_nonoverlapping(data, &mut inner.vfio_handle_data as *mut bindings::nvidia_vgpu_vfio_handle_data, core::mem::size_of::<bindings::nvidia_vgpu_vfio_handle_data>()); }
    0
}

unsafe extern "C" fn detach_handle(handle: *mut core::ffi::c_void) {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };
    pr_info!("detach handle\n");
    let mut inner = nova.gpu.vgpu.inner.lock();

    inner.vfio_handle_data.priv_ = core::ptr::null_mut();
}

unsafe extern "C" fn alloc_gsp_client(handle: *mut core::ffi::c_void,
                                      client: *mut bindings::nvidia_vgpu_gsp_client) -> i32
{
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };
    pr_info!("alloc GSP client\n");
    let (gpu_client, gpu_device) = match nova.gpu.alloc_client_device() {
        Err(x) => {
            return x.to_errno();
        },
        Ok((x, y)) => { (x, y) }
    };

    let cli_clone = gpu_client.clone();
    let dev_clone = gpu_device.clone();
    unsafe {
        (*client).gsp_client = cli_clone.into_foreign() as *mut core::ffi::c_void;
        (*client).gsp_device = dev_clone.into_foreign() as *mut core::ffi::c_void;
    }
    0
}

unsafe extern "C" fn free_gsp_client(client: *mut bindings::nvidia_vgpu_gsp_client)
{
    unsafe {
        pr_info!("free GSP device\n");
        let _gpu_device : Arc<GpuDevice> = Arc::from_foreign((*client).gsp_device);
        (*client).gsp_device = core::ptr::null_mut();
    }

    unsafe {
        pr_info!("free GSP client\n");
        let _gpu_client : Arc<GpuClient> = Arc::from_foreign((*client).gsp_client);
        (*client).gsp_client = core::ptr::null_mut();
    }
}

unsafe extern "C" fn get_gsp_client_handle(client: *mut bindings::nvidia_vgpu_gsp_client) -> u32 {

    let cli_borrow: ArcBorrow<'_, GpuClient> = unsafe { Arc::borrow((*client).gsp_client) };

    pr_info!("get GSP client handle\n");
    cli_borrow.gsp.clone().get_client_handle().unwrap()
}

unsafe extern "C" fn free_fbmem(base: *mut bindings::nvidia_vgpu_mem) {
    pr_info!("free fbmem\n");
    let _fbmem : KBox<VGPUMem> = unsafe { KBox::from_foreign(base as _) };
    // drop it
}

unsafe extern "C" fn alloc_fbmem(handle: *mut core::ffi::c_void, size: u64, vmmu_aligned: bool) -> *mut bindings::nvidia_vgpu_mem {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };

    let shift: u32 = if vmmu_aligned { nova.gpu.vgpu.vmmu_segment_size.ilog2() } else { NVKM_MM_PAGE_SHIFT as u32};

    let vramobj = VramObj::new(nova.gpu.vram_mm.clone(), 0, 0, shift as u8, size as usize, true, true).unwrap();

    let fbmem: Pin<KBox<VGPUMem>> = KBox::new(
        VGPUMem {
            base: bindings::nvidia_vgpu_mem {
                addr: vramobj.addr().unwrap(),
                size: vramobj.size().unwrap(),
                bar1_vaddr: core::ptr::null_mut()
            },
            obj: vramobj,
            bar1_vma: None,
            core: nova.clone(),
        }, GFP_KERNEL).unwrap().into();

    pr_info!("alloc fbmem {} {}\n", fbmem.base.addr, fbmem.base.size);
    fbmem.into_foreign() as _
}

unsafe extern "C" fn bar1_map_mem(base: *mut bindings::nvidia_vgpu_mem) -> i32 {
    let fbmem_ptr: *mut VGPUMem = base as *mut _ as *mut VGPUMem;

    let fbmem = unsafe { &mut (*fbmem_ptr) };
    pr_info!("map bar1 mem {:?}\n", base);
    let vmm = fbmem.core.gpu.bar.bar1_vmm();
    let size = fbmem.base.size;
    let vma = vmm.get(false, true, false, 12, 0, size).unwrap();

    match fbmem.obj.vram_map(0, &vmm, vma.clone(), 0) {
        Err(x) => { return x.to_errno(); }
        _ => {}
    }

    fbmem.base.bar1_vaddr = unsafe { bindings::ioremap((*fbmem).core.pdev.resource_start(1).unwrap() + vma.addr(), size as u64) };

    pr_info!("map fbmem {:#x} {:?}\n", vma.addr(), fbmem.base.bar1_vaddr);
    fbmem.bar1_vma = Some(vma);
    0
}

unsafe extern "C" fn bar1_unmap_mem(base: *mut bindings::nvidia_vgpu_mem) {
    let fbmem_ptr: *mut VGPUMem = base as *mut _ as *mut VGPUMem;
    let fbmem = unsafe { &mut (*fbmem_ptr) };
    pr_info!("unmap bar1 mem\n");

    unsafe { bindings::iounmap(fbmem.base.bar1_vaddr) };
    fbmem.bar1_vma = None;
}

unsafe extern "C" fn alloc_chids(_handle: *mut core::ffi::c_void, count: i32) -> i32 {
    pr_info!("alloc chids {}\n", count);
    512
}

unsafe extern "C" fn free_chids(_handle: *mut core::ffi::c_void, offset: i32, count: i32) {
    pr_info!("free chids {} {}\n", offset, count);
}

unsafe extern "C" fn shutdown_vgpu_plugin_task(client: *mut bindings::nvidia_vgpu_gsp_client,
                                               gfid: u32) -> i32 {
    let device_borrow: ArcBorrow<'_, GpuDevice> = unsafe { Arc::borrow((*client).gsp_device) };
    device_borrow.mgr.shutdown_vgpu_plugin_task(device_borrow.gsp.clone(), gfid)
}

unsafe extern "C" fn cleanup_vgpu_plugin(client: *mut bindings::nvidia_vgpu_gsp_client,
                                         gfid: u32) -> i32 {
    let device_borrow: ArcBorrow<'_, GpuDevice> = unsafe { Arc::borrow((*client).gsp_device) };
    device_borrow.mgr.cleanup_vgpu_plugin(device_borrow.gsp.clone(), gfid)
}

unsafe extern "C" fn bootload_vgpu_plugin_task(client: *mut bindings::nvidia_vgpu_gsp_client,
                                               params: *const bindings::bootload_vgpu) -> i32 {
    let device_borrow: ArcBorrow<'_, GpuDevice> = unsafe { Arc::borrow((*client).gsp_device) };
    device_borrow.mgr.bootload_vgpu_plugin_task(device_borrow.gsp.clone(), params)
}

unsafe extern "C" fn add_vgpu_info(client: *mut bindings::nvidia_vgpu_gsp_client,
                                   count: u32,
                                   ptr: *const core::ffi::c_void) -> i32 {
    let device_borrow: ArcBorrow<'_, GpuDevice> = unsafe { Arc::borrow((*client).gsp_device) };
    device_borrow.mgr.add_vgpu_type(device_borrow.gsp.clone(), count, ptr)
}

unsafe extern "C" fn get_engine_bitmap(handle: *mut core::ffi::c_void, bitmap: *mut u64) {
    let drv: *mut NovaCoreDriver = handle as *mut NovaCoreDriver;
    let nova = unsafe { (*drv).data() };
    let eng_bitmap: u64 = nova.gpu.get_engine_bitmap();

    pr_info!("get engine bitmap {:#x}\n", eng_bitmap);

    unsafe { *bitmap = eng_bitmap };
}

const NVKM_FUNCS: bindings::nvkm_vgpu_mgr_vfio_ops = bindings::nvkm_vgpu_mgr_vfio_ops {
    vgpu_mgr_is_enabled: Some(nvkm_vgpu_mgr_is_enabled),
    get_handle: Some(get_handle),
    attach_handle: Some(attach_handle),
    detach_handle: Some(detach_handle),
    alloc_gsp_client: Some(alloc_gsp_client),
    free_gsp_client: Some(free_gsp_client),
    get_gsp_client_handle: Some(get_gsp_client_handle),
    shutdown_vgpu_plugin_task: Some(shutdown_vgpu_plugin_task),
    cleanup_vgpu_plugin: Some(cleanup_vgpu_plugin),
    bootload_vgpu_plugin_task: Some(bootload_vgpu_plugin_task),
    add_vgpu_info: Some(add_vgpu_info),
    alloc_chids: Some(alloc_chids),
    free_chids: Some(free_chids),
    alloc_fbmem: Some(alloc_fbmem),
    free_fbmem: Some(free_fbmem),
    bar1_map_mem: Some(bar1_map_mem),
    bar1_unmap_mem: Some(bar1_unmap_mem),
    get_engine_bitmap: Some(get_engine_bitmap),
};

#[no_mangle]
#[allow(dead_code)]
/// Provide the API for the VFIO RFC
pub unsafe extern "C" fn nvkm_vgpu_mgr_get_vfio_ops(_handle: *mut core::ffi::c_void) -> *const bindings::nvkm_vgpu_mgr_vfio_ops {
    pr_info!("get vfio ops\n");
    &NVKM_FUNCS
}
