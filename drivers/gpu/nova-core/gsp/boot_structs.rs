pub(crate) use kernel::macros::versions;

use crate::gpu::Chipset;
use crate::gpu::FBInfo;
use crate::gpu::GpuBase;
use crate::gpu::GpuSpec;
use crate::gpu::Firmware;

use crate::rm_riscv::RiscvFw;
use crate::firmware::Radix3;
use kernel::device;
use kernel::prelude::*;
use crate::dma::DmaObject;
use crate::gsp::{GSP_PAGE_SIZE,GSP_PAGE_SHIFT};
use crate::gsp::GSP_HEAP_SHIFT;

use crate::{align64, align_down, div_round_up64, div_round_up};
use crate::nvfw::*;

use crate::chipsets_after;

fn id8(name: &'static str) -> u64 {
    let mut id: u64 = 0;
    for (_i, byte) in name.bytes().enumerate().take(core::mem::size_of::<u64>()) {
        id = (id << 8) | byte as u64;
    }
    id
}

fn create_pte_array(dma: &mut DmaObject) {
    let num_pages = div_round_up(dma.len as usize, GSP_PAGE_SIZE as usize);

    unsafe {
	let ptes : *mut u64 = dma.dma.start_ptr_mut().offset(size_of::<u64>() as isize) as *mut u64;

	for i in 0..num_pages {
	    *ptes.wrapping_add(i) = dma.dma.dma_handle() + (i << GSP_PAGE_SHIFT) as u64;
	}
    }
}

#[versions(GSP)]
pub(crate) struct Wpr {}

#[versions(GSP)]
#[allow(non_upper_case_globals)]
const LIBOS3_CARVEOUT: u64 = {
    #[ver(r == r535_113_01)]
    {
        fw::ver::gen::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS3 as u64
    }

    #[ver(r == r570_86_16)]
    {
        fw::ver::gen::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS3_BAREMETAL as u64
    }
};

#[versions(GSP)]
impl Wpr::ver {

fn calc_wpr_heap(spec: &GpuSpec, fb_size_fb: u64) -> u64 {
    let carveout = if chipsets_after!(&spec.chipset, GA102) {
        LIBOS3_CARVEOUT::ver
    } else {
        fw::ver::gen::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS2 as u64
    };

    let heap_min = if chipsets_after!(&spec.chipset, GA102) {
        fw::ver::gen::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB << 20
    } else {
        fw::ver::gen::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS2_MIN_MB << 20
    };

    let size = carveout +
	fw::ver::gen::GSP_FW_HEAP_PARAM_BASE_RM_SIZE_TU10X as u64 +
	align64(fw::ver::gen::GSP_FW_HEAP_PARAM_SIZE_PER_GB_FB as u64 * fb_size_fb, GSP_HEAP_SHIFT) +
	align64(fw::ver::gen::GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE as u64, GSP_HEAP_SHIFT);
    core::cmp::max::<u64>(size, heap_min as u64)
}

pub(crate) fn fill_rmargs(args: *mut u8, shm_addr: u64, ptes_nr: u32, cmdq_offset: u64,
		   statq_offset: u64, resume: bool) {
    let mut msg = fw::ver::gen::s_GSP_ARGUMENTS_CACHED::new(args);
    if resume == false {
	msg.new_S_srInitArguments()
	    .oldLevel(0).flags(0).bInPMTransition(0);
    } else {
	msg.new_S_srInitArguments()
	    .oldLevel(3).flags(0).bInPMTransition(1);
    }
    msg.new_S_messageQueueInitArguments()
	.sharedMemPhysAddr(shm_addr)
	.pageTableEntryCount(ptes_nr)
	.cmdQueueOffset(cmdq_offset)
	.statQueueOffset(statq_offset);

    #[ver(r == r570_86_16)]
    msg.bDmemStack(1);
}

const fn get_wpr_meta_size() -> usize {
    fw::ver::gen::s_GspFwWprMeta::str_size()
}

pub(crate) fn fill_fb_addr_info(gpu_base: &GpuBase,
				fb_size: u64,
				vga_base: u64,
				vga_size: u64,
				fw: &Firmware) -> FBInfo {
    let mut fb_addr_info : FBInfo = Default::default();
    fb_addr_info.fb.size = fb_size as u64;
    fb_addr_info.vga_workspace.size = vga_size as u64;
    fb_addr_info.vga_workspace.addr = vga_base as u64;
    fb_addr_info.bios.addr = fb_addr_info.vga_workspace.addr;
    fb_addr_info.bios.size = fb_addr_info.vga_workspace.size;

    fb_addr_info.frts.size = 0x100000;
    fb_addr_info.frts.addr = align_down(fb_addr_info.bios.addr, 0x20000) - fb_addr_info.frts.size;

    fb_addr_info.boot.size = fw.bootloader_fw.fw.dma.len as u64;
    fb_addr_info.boot.addr = align_down(fb_addr_info.frts.addr - fb_addr_info.boot.size, 0x1000);

    fb_addr_info.elf.size = fw.gsp_fw.len as u64;
    fb_addr_info.elf.addr = align_down(fb_addr_info.boot.addr - fb_addr_info.elf.size, 0x10000);

    let fb_size_fb = div_round_up64(fb_addr_info.fb.size, 1 << 30);
    fb_addr_info.wpr2_heap.size = Self::calc_wpr_heap(&gpu_base.spec, fb_size_fb);

    fb_addr_info.wpr2_heap.addr = align_down(fb_addr_info.elf.addr - fb_addr_info.wpr2_heap.size, 0x100000);
    fb_addr_info.wpr2_heap.size = align_down(fb_addr_info.elf.addr - fb_addr_info.wpr2_heap.addr, 0x100000);

    fb_addr_info.wpr2.addr = align_down(fb_addr_info.wpr2_heap.addr - Self::get_wpr_meta_size() as u64, 0x100000);
    fb_addr_info.wpr2.size = fb_addr_info.frts.addr + fb_addr_info.frts.size - fb_addr_info.wpr2.addr;
    fb_addr_info.heap.size = 0x100000;
    fb_addr_info.heap.addr = fb_addr_info.wpr2.addr - fb_addr_info.heap.size;
    fb_addr_info.wpr_size = (align_down(fb_addr_info.vga_workspace.addr, 0x20000) - fb_addr_info.wpr2.addr) as u32;
    fb_addr_info
}


pub(crate) fn fill_wpr_meta(wpr_meta: *mut u8,
		     gsp_radix3: &Radix3,
		     bootloader_fw: &RiscvFw,
		     sig_dma_obj: &DmaObject,
		     fb_addr_info: &FBInfo) {

    let _wpr = fw::ver::gen::s_GspFwWprMeta::new(wpr_meta)
	.magic(fw::ver::gen::GSP_FW_WPR_META_MAGIC)
	.revision(fw::ver::gen::GSP_FW_WPR_META_REVISION as u64)
	.sysmemAddrOfRadix3Elf(gsp_radix3.lvl0_addr())
	.sizeOfRadix3Elf(fb_addr_info.elf.size)
	.sysmemAddrOfBootloader(bootloader_fw.fw.dma.dma.dma_handle())
	.sizeOfBootloader(bootloader_fw.fw.dma.len as u64)
	.bootloaderCodeOffset(bootloader_fw.code_offset as u64)
	.bootloaderDataOffset(bootloader_fw.data_offset as u64)
	.bootloaderManifestOffset(bootloader_fw.manifest_offset as u64)
	.sysmemAddrOfSignature(sig_dma_obj.dma.dma_handle())
	.sizeOfSignature(sig_dma_obj.len as u64)
	.gspFwRsvdStart(fb_addr_info.heap.addr)
	.nonWprHeapOffset(fb_addr_info.heap.addr)
	.nonWprHeapSize(fb_addr_info.heap.size)
	.gspFwWprStart(fb_addr_info.wpr2.addr)
	.gspFwHeapOffset(fb_addr_info.wpr2_heap.addr)
	.gspFwHeapSize(fb_addr_info.wpr2_heap.size)
	.gspFwOffset(fb_addr_info.elf.addr)
	.bootBinOffset(fb_addr_info.boot.addr)
	.frtsOffset(fb_addr_info.frts.addr)
	.frtsSize(fb_addr_info.frts.size)
	.gspFwWprEnd(align_down(fb_addr_info.vga_workspace.addr, 0x20000))
	.gspFwHeapVfPartitionCount(fb_addr_info.vf_partition_count)
	.fbSize(fb_addr_info.fb.size)
	.vgaWorkspaceOffset(fb_addr_info.vga_workspace.addr)
	.vgaWorkspaceSize(fb_addr_info.vga_workspace.size);
}

pub(crate) fn fill_sr_meta(sr_meta: *mut u8,
			   sr_radix3: &Radix3,
			   len: u64)
{
    let _sr = fw::ver::gen::s_GspFwSRMeta::new(sr_meta)
	.magic(fw::ver::gen::GSP_FW_SR_META_MAGIC)
	.revision(fw::ver::gen::GSP_FW_SR_META_REVISION as u64)
	.sysmemAddrOfSuspendResumeData(sr_radix3.lvl0_addr())
	.sizeOfSuspendResumeData(len);
}
	    
pub(crate) fn libos_fill_data(dev: &device::Device,
			      num_entries: usize,
			      loginit: &mut DmaObject,
			      logintr: &mut DmaObject,
			      logrm: &mut DmaObject,
			      kernel: Option<&mut DmaObject>,
			      rmargs: &DmaObject) -> Result<DmaObject>
{
    let libos_size = fw::ver::gen::s_LibosMemoryRegionInitArgument::str_size();
    let size_in_bytes = (num_entries + 1) * libos_size;
    let mut obj = DmaObject::new_cleared(dev, size_in_bytes, "libos")?;
    let ptr = obj.dma.start_ptr_mut();
    let mut offset = 0;

    fw::ver::gen::s_LibosMemoryRegionInitArgument::new(ptr as *mut u8)
	.id8(id8("LOGINIT"))
	.pa(loginit.dma.dma_handle())
	.size(loginit.len as u64)
	.kind(fw::ver::gen::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
	.loc(fw::ver::gen::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

    create_pte_array(loginit);

    offset += libos_size;

    fw::ver::gen::s_LibosMemoryRegionInitArgument::new(unsafe { (ptr as *mut u8).byte_offset(offset as isize) })
	.id8(id8("LOGINTR"))
	.pa(logintr.dma.dma_handle())
	.size(logintr.len as u64)
	.kind(fw::ver::gen::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
	.loc(fw::ver::gen::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

    create_pte_array(logintr);

    offset += libos_size;

    fw::ver::gen::s_LibosMemoryRegionInitArgument::new(unsafe { (ptr as *mut u8).byte_offset(offset as isize) })
	.id8(id8("LOGRM"))
	.pa(logrm.dma.dma_handle())
	.size(logrm.len as u64)
	.kind(fw::ver::gen::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
	.loc(fw::ver::gen::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

    create_pte_array(logrm);

    offset += libos_size;
    match kernel {
	Some(k) => {
	    fw::ver::gen::s_LibosMemoryRegionInitArgument::new(unsafe { (ptr as *mut u8).byte_offset(offset as isize) })
		.id8(id8("KERN"))
		.pa(k.dma.dma_handle())
		.size(k.len as u64)
		.kind(fw::ver::gen::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
		.loc(fw::ver::gen::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

	    offset += libos_size;
	}
	_ => {}
    }

    fw::ver::gen::s_LibosMemoryRegionInitArgument::new(unsafe { (ptr as *mut u8).byte_offset(offset as isize) })
	.id8(id8("RMARGS"))
	.pa(rmargs.dma.dma_handle())
	.size(rmargs.len as u64)
	.kind(fw::ver::gen::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
	.loc(fw::ver::gen::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

    Ok(obj)
}
}
