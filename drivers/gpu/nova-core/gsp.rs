// SPDX-License-Identifier: GPL-2.0

use core::ffi::c_void;

use kernel::device;
use kernel::prelude::*;

use kernel::dma::CoherentAllocation;
use kernel::transmute::{AsBytes, FromBytes};
use kernel::{asm, dma_read, dma_write, pr_info};

use crate::dma::DmaObject;
use crate::firmware::Firmware;
use crate::gsp::fb::FbLayout;
use crate::nvfw::r570_133_07 as fw;

pub(crate) mod fb;

pub(crate) const GSP_PAGE_SHIFT: usize = 12;
pub(crate) const GSP_PAGE_SIZE: usize = 1 << GSP_PAGE_SHIFT;
pub(crate) const GSP_HEAP_SHIFT: u64 = 1 << 20;

extern "C" {
    fn iowrite32(val: u32, addr: *mut c_void);
}

trait GspMessageElement {
    fn byte_slice(&self) -> &[u8]
    where
        Self: Sized,
    {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }

    unsafe fn copy_to(&self, ptr: *mut c_void) -> *mut c_void
    where
        Self: Sized,
    {
        unsafe {
            core::ptr::copy_nonoverlapping(self as *const Self, ptr as *mut Self, 1);
            (ptr as *const Self).add(1) as *mut c_void
        }
    }

    unsafe fn new_from_raw(ptr: *mut c_void, size: u32) -> Result<(*mut c_void, u32, Self)>
    where
        Self: Sized,
    {
        if size < size_of::<Self>() as u32 {
            return Err(ENOMEM);
        }
        // SAFETY: We've checked the size and know the pointer is valid
        Ok((
            unsafe { (ptr as *const Self).add(1) as *mut c_void },
            size - size_of::<Self>() as u32,
            unsafe { core::ptr::read(ptr as *const Self) },
        ))
    }
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct GspCmdqInfo {
    falcon: *mut c_void,
    shm_ptr: *mut c_void,
    ptr: *mut c_void,
    size: u32,
    cnt: u32,
    seq: u32,
    wptr: *mut u32,
    rptr: *mut u32,
    msgq_ptr: *mut c_void,
    msgq_wptr: *mut u32,
    msgq_rptr: *mut u32,
}

// --- This next section contains constants and structures hand-coded from the GSP headers ---
// These could probably be replaced with bindgen generated versions but leaving them here for now

// A GSP RPC header
#[repr(C)]
#[derive(Debug)]
struct GspRpcHeader {
    header_version: u32,
    signature: u32,
    length: u32,
    function: u32,
    rpc_result: u32,
    rpc_result_private: u32,
    sequence: u32,
    cpu_rm_gfid: u32,
}
impl GspMessageElement for GspRpcHeader {}

// A GSP message element header
#[repr(C)]
#[derive(Debug)]
struct GspMsgHeader {
    auth_tag_buffer: [u8; 16],
    aad_buffer: [u8; 16],
    checksum: u32,
    sequence: u32,
    elem_count: u32,
    pad: u32,
}
impl GspMessageElement for GspMsgHeader {}

// These next two structs come from r535 msgq_priv.h. Hopefully the will never
// need updating once the ABI is stabalised.
#[repr(C)]
#[derive(Debug)]
struct MsgqTxHeader {
    version: u32,    // queue version
    size: u32,       // bytes, page aligned
    msg_size: u32,   // entry size, bytes, must be power-of-2, 16 is minimum
    msg_count: u32,  // number of entries in queue
    write_ptr: u32,  // message id of next slot
    flags: u32,      // if set it means "i want to swap RX"
    rx_hdr_off: u32, // Offset of msgqRxHeader from start of backing store
    entry_off: u32,  // Offset of entries from start of backing store
}

#[repr(C)]
#[derive(Debug)]
struct MsgqRxHeader {
    read_ptr: u32, // message id of last message read
}

#[repr(C)]
#[derive(Debug)]
struct Msgq {
    tx: MsgqTxHeader,
    rx: MsgqRxHeader,
    _pad: [u8; 0xfdc],
    msgq: [u8; 0x3f000],
}

// --- End of GSP header structs

#[repr(C)]
#[derive(Debug)]
struct GspMem {
    ptes: [u8; 4096], // GSP_PAGE_SIZE is 4K for now
    cpuq: Msgq,
    gspq: Msgq,
}

struct NoArgs {}
impl GspMessageElement for NoArgs {}

impl GspMessageElement for fw::GspStaticConfigInfo_t {}

// Needed for CoherentAllocation
unsafe impl FromBytes for GspMem {}
unsafe impl AsBytes for GspMem {}

// SAFETY: this hack isn't :-) Only required until Nova core can boot GSP.
unsafe impl Send for GspCmdq {}

pub(crate) struct GspCmdq {
    // HACK: We only need the next two fields until nova-core can initialise the GSP itself, so make the lifetime checks go away
    pdev: *mut c_void,
    drvdata: *mut c_void,

    // HACK: We only need this until nova-core can boot the GSP as well
    cmdq_info: GspCmdqInfo,
    gsp_mem: CoherentAllocation<GspMem>,
}

impl GspCmdq {
    // We need the next four accessors because the dma_read macro is failable
    // and uses `?` which requires any calling function to return a Result<>.
    // However in the first instance a dma_read failure probably needs to be dealt with
    // by the function trying to do the read, so we need the accessors to permit that.
    //
    // Of course at the moment we "deal" with errors by panicing...
    //
    // I think we need to update the dma macro's to return a Result<u32>
    fn cpu_wptr(self: &Self) -> Result<u32> {
        Ok(dma_read!(self.gsp_mem[0].cpuq.tx.write_ptr))
    }

    fn gsp_rptr(self: &Self) -> Result<u32> {
        Ok(dma_read!(self.gsp_mem[0].gspq.rx.read_ptr))
    }

    fn cpu_rptr(self: &Self) -> Result<u32> {
        Ok(dma_read!(self.gsp_mem[0].cpuq.rx.read_ptr))
    }

    fn gsp_wptr(self: &Self) -> Result<u32> {
        Ok(dma_read!(self.gsp_mem[0].gspq.tx.write_ptr))
    }

    // Returns the numbers of bytes free for sending an RPC to GSP.
    fn get_free_tx_bytes(self: &Self) -> u32 {
        let wptr = self.cpu_wptr().unwrap();
        let rptr = self.gsp_rptr().unwrap();
        let mut free = rptr + self.cmdq_info.cnt - wptr - 1;

        if free >= self.cmdq_info.cnt {
            free -= self.cmdq_info.cnt;
        }

        free << GSP_PAGE_SHIFT
    }

    // Returns the number of bytes the GSP has written to the queue.
    fn get_used_rx_bytes(self: &Self) -> u32 {
        let rptr = self.cpu_rptr().unwrap();
        let wptr = self.gsp_wptr().unwrap();
        let mut used = wptr + self.cmdq_info.cnt - rptr;
        if used >= self.cmdq_info.cnt {
            used -= self.cmdq_info.cnt;
        }

        used << GSP_PAGE_SHIFT
    }

    fn calculate_checksum<A: GspMessageElement>(
        msg: &GspMsgHeader,
        rpc: &GspRpcHeader,
        args: &A,
    ) -> u32 {
        let mut sum: u64 = 0;
        let msg_bytes = msg.byte_slice();
        for &byte in msg_bytes.iter().rev() {
            sum = sum.rotate_left(8) ^ (byte as u64);
        }
        let rpc_bytes = rpc.byte_slice();
        for &byte in rpc_bytes.iter().rev() {
            sum = sum.rotate_left(8) ^ (byte as u64);
        }
        let args_bytes = args.byte_slice();
        for &byte in args_bytes.iter().rev() {
            sum = sum.rotate_left(8) ^ (byte as u64);
        }
        ((sum >> 32) as u32) ^ (sum as u32)
    }

    // Returns an uninitialized pointer to the shared memory region with at
    // least `size` bytes free for writing.
    // SAFETY: Caller must initialize and ensure size is repsected.
    unsafe fn alloc_cmd(self: &Self, size: u32) -> *mut c_void {
        while self.get_free_tx_bytes() < size {}
        let wptr = self.cpu_wptr().unwrap();
        let ptr = (self.cmdq_info.ptr as usize + 0x1000 + (wptr as usize) * 0x1000) as *mut c_void;
        ptr
    }

    fn send<A: GspMessageElement>(self: &mut Self, function: u32, args: A) -> Result<()> {
        let mut msg = GspMsgHeader {
            auth_tag_buffer: [0; 16],
            aad_buffer: [0; 16],
            checksum: 0,
            sequence: self.cmdq_info.seq,
            elem_count: 1,
            pad: 0,
        };
        let mut rpc = GspRpcHeader {
            header_version: 0x03000000,
            signature: 0x43505256,
            length: 0,
            function,
            rpc_result: 0xffffffff,
            rpc_result_private: 0xffffffff,
            sequence: 0,
            cpu_rm_gfid: 0,
        };

        self.cmdq_info.seq += 1;
        rpc.length =
            (size_of::<GspMsgHeader>() + size_of::<GspRpcHeader>() + size_of::<A>()) as u32;
        msg.checksum = GspCmdq::calculate_checksum(&msg, &rpc, &args);

        unsafe {
            let ptr = self.alloc_cmd(rpc.length);
            let rpc_ptr = msg.copy_to(ptr);
            let args_ptr = rpc.copy_to(rpc_ptr);
            args.copy_to(args_ptr);
        }

        let wptr = self.cpu_wptr().unwrap() + 1;

        // TODO: Figure out Rust barriers
        unsafe {
            asm!("sfence";);
            dma_write!(self.gsp_mem[0].cpuq.tx.write_ptr = wptr);
            asm!("mfence";);
            iowrite32(0, self.cmdq_info.falcon);
        };

        Ok(())
    }

    fn receive<A: GspMessageElement>(self: &mut Self) -> Result<A> {
        let mut rptr = self.cpu_rptr()?;
        let size = loop {
            let size = self.get_used_rx_bytes();
            if size >= size_of::<A>() as u32 {
                break size;
            }
        };

        let msg_ptr =
            (self.cmdq_info.msgq_ptr as usize + 0x1000 + (rptr as usize) * 0x1000) as *mut c_void;
        let (rpc_ptr, size, _) = unsafe { GspMsgHeader::new_from_raw(msg_ptr, size)? };
        let (args_ptr, size, _) = unsafe { GspRpcHeader::new_from_raw(rpc_ptr, size)? };
        let (_, _, args) = unsafe { A::new_from_raw(args_ptr, size)? };

        // TODO: Increment by what we actually received
        rptr += 1;

        // TODO: Figure out Rust barriers
        unsafe {
            asm!("mfence";);
            dma_write!(self.gsp_mem[0].cpuq.rx.read_ptr = rptr);
        };

        // TODO: Validate checksum, etc.
        Ok(args)
    }

    pub(crate) fn test(self: &mut Self) -> Result<()> {
        self.send(fw::NV_VGPU_MSG_FUNCTION_GET_GSP_STATIC_INFO, NoArgs {})?;
        let msg: fw::GspStaticConfigInfo_t = self.receive()?;
        pr_info!(
            "GPU Name: {}\n",
            core::str::from_utf8(&msg.gpuNameString).unwrap_or("invalid utf8")
        );
        pr_info!(
            "GPU Short Name: {}\n",
            core::str::from_utf8(&msg.gpuShortNameString).unwrap_or("invalid utf8")
        );
        for i in 0..msg.fbRegionInfoParams.numFBRegions as usize {
            pr_info!(
                "GPU FB region {} Base: 0x{:08x} Size: 0x{:08x}\n",
                i,
                msg.fbRegionInfoParams.fbRegion[i].base,
                msg.fbRegionInfoParams.fbRegion[i].limit
            );
        }
        Ok(())
    }
}

pub(crate) fn build_wpr_meta(
    dev: &device::Device<device::Bound>,
    fw: &Firmware,
    fb_layout: &FbLayout,
) -> Result<DmaObject> {
    // TODO: Can we use dma_write!() with a properly defined structure instead?
    let mut wpr_meta = DmaObject::new(dev, GSP_PAGE_SIZE)?;
    fw::s_GspFwWprMeta::new(wpr_meta.start_ptr_mut())
        .magic(fw::GSP_FW_WPR_META_MAGIC)
        .revision(fw::GSP_FW_WPR_META_REVISION as u64)
        .sysmemAddrOfRadix3Elf(fw.gsp.lvl0_dma_handle() as u64)
        .sizeOfRadix3Elf(fw.gsp.size() as u64)
        .sysmemAddrOfBootloader(fw.bootloader.ucode.dma_handle())
        .sizeOfBootloader(fw.bootloader.ucode.size() as u64)
        .bootloaderCodeOffset(fw.bootloader.code_offset as u64)
        .bootloaderDataOffset(fw.bootloader.data_offset as u64)
        .bootloaderManifestOffset(fw.bootloader.manifest_offset as u64)
        .sysmemAddrOfSignature(fw.gsp_sigs.dma_handle() as u64)
        .sizeOfSignature(fw.gsp_sigs.size() as u64)
        .gspFwRsvdStart(fb_layout.heap.start)
        .nonWprHeapOffset(fb_layout.heap.start)
        .nonWprHeapSize(fb_layout.heap.end - fb_layout.heap.start)
        .gspFwWprStart(fb_layout.wpr2.start)
        .gspFwHeapOffset(fb_layout.wpr2_heap.start)
        .gspFwHeapSize(fb_layout.wpr2_heap.end - fb_layout.wpr2_heap.start)
        .gspFwOffset(fb_layout.elf.start)
        .bootBinOffset(fb_layout.boot.start)
        .frtsOffset(fb_layout.frts.start)
        .frtsSize(fb_layout.frts.end - fb_layout.frts.start)
        .gspFwWprEnd(fb_layout.vga_workspace.start & !(0x20000 - 1))
        .gspFwHeapVfPartitionCount(fb_layout.vf_partition_count)
        .fbSize(fb_layout.fb.end - fb_layout.fb.start)
        .vgaWorkspaceOffset(fb_layout.vga_workspace.start)
        .vgaWorkspaceSize(fb_layout.vga_workspace.end - fb_layout.vga_workspace.start)
        .bootCount(0)
        .partitionRpcAddr(0)
        .partitionRpcRequestOffset(0)
        .partitionRpcReplyOffset(0)
        .verified(0);

    Ok(wpr_meta)
}

#[allow(unused)]
pub(crate) struct GspSharedMemObjects {
    libos: DmaObject,
    loginit: DmaObject,
    logintr: DmaObject,
    logrm: DmaObject,
    rmargs: DmaObject,
    kern: Option<DmaObject>,
    shm: DmaObject,
    wpr_meta: DmaObject,
}

/// Generates the `ID8` identifier required for some GSP objects.
fn id8(name: &str) -> u64 {
    let mut bytes = [0u8; core::mem::size_of::<u64>()];

    for (c, b) in name.bytes().rev().zip(&mut bytes) {
        *b = c;
    }

    u64::from_ne_bytes(bytes)
}

/// Creates a self-mapping page table for `obj` at its beginning.
fn create_pte_array(obj: &mut DmaObject) {
    let num_pages = obj.size().div_ceil(GSP_PAGE_SIZE);
    let handle = obj.dma_handle();

    let ptes = unsafe {
        let ptr = obj.start_ptr_mut().add(core::mem::size_of::<u64>()) as *mut u64;
        core::slice::from_raw_parts_mut(ptr, num_pages)
    };

    for (i, pte) in ptes.iter_mut().enumerate() {
        *pte = handle as u64 + ((i as u64) << GSP_PAGE_SHIFT);
    }
}

/// Creates a new `DmaObject` with `name` of `size`, and register it into the `libos` object at
/// argument position `libos_arg_nr`.
fn create_dma_object(
    dev: &device::Device<device::Bound>,
    name: &'static str,
    size: usize,
    libos: &mut DmaObject,
    libos_arg_nr: usize,
) -> Result<DmaObject> {
    let mut obj = DmaObject::new(dev, size)?;
    create_pte_array(&mut obj);

    let arg_offset = libos_arg_nr * fw::s_LibosMemoryRegionInitArgument::str_size();
    let libos_start_ptr = unsafe { libos.start_ptr_mut().add(arg_offset) };

    fw::s_LibosMemoryRegionInitArgument::new(libos_start_ptr)
        .id8(id8(name))
        .pa(obj.dma_handle())
        .size(obj.size() as u64)
        .kind(fw::LIBOS_MEMORY_REGION_CONTIGUOUS as u8)
        .loc(fw::LIBOS_MEMORY_REGION_LOC_SYSMEM as u8);

    Ok(obj)
}

impl GspSharedMemObjects {
    pub(crate) fn new(dev: &device::Device<device::Bound>) -> Result<Self> {
        let mut libos = DmaObject::new(dev, GSP_PAGE_SIZE)?;

        let loginit = create_dma_object(dev, "LOGINIT", 0x10000, &mut libos, 0);
        let logintr = create_dma_object(dev, "LOGINTR", 0x10000, &mut libos, 1);
        let logrm = create_dma_object(dev, "LOGRM", 0x10000, &mut libos, 2);
        let rmargs = create_dma_object(dev, "RMARGS", 0x1000, &mut libos, 3);

        // TODO: initialize rmargs and shm as per r535_gsp_rmargs_init.
        // TODO: also kernel from Dave's branch?

        Err(EINVAL)
    }
}
