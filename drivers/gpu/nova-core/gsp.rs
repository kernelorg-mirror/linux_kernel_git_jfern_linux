// SPDX-License-Identifier: GPL-2.0

// Shut up silly warnings for now
#![allow(dead_code, unused)]

use core::ffi::{c_char, c_int, c_void};

use kernel::device;
use kernel::prelude::*;

use kernel::dma::CoherentAllocation;
use kernel::transmute::{AsBytes, FromBytes};
use kernel::{asm, dma_read, dma_write, pr_info};

use crate::dma::DmaObject;
use crate::firmware::Firmware;
use crate::gsp::fb::FbLayout;
use crate::nvfw::r570_144 as fw;

pub(crate) mod fb;

pub(crate) const GSP_PAGE_SHIFT: usize = 12;
pub(crate) const GSP_PAGE_SIZE: usize = 1 << GSP_PAGE_SHIFT;
pub(crate) const GSP_HEAP_SHIFT: u64 = 1 << 20;

extern "C" {
    fn iowrite32(val: u32, addr: *mut c_void);
    fn print_hex_dump(
        level: *const c_char,
        prefix_str: *const c_char,
        prefix_type: c_int,
        rowsize: c_int,
        groupsize: c_int,
        buf: *const c_char,
        len: usize,
        ascii: c_int,
    );
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

    unsafe fn new(ptr: *mut c_void) -> Self
    where
        Self: Sized,
    {
        unsafe { core::ptr::read(ptr as *const Self) }
    }
}

// Not all message sizes are known at compile time, so we need a ?Sized version.
trait UnsizedGspMessageElement: GspMessageElement {
    fn size(&self) -> usize;
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
    // pdev: *mut c_void,
    // drvdata: *mut c_void,

    // HACK: We only need this until nova-core can boot the GSP as well
    // cmdq_info: GspCmdqInfo,
    msg_count: u32,
    seq: u32,
    gsp_mem: CoherentAllocation<GspMem>,
    cpu_ptr: *mut c_void,
    gsp_ptr: *mut c_void,
}

impl GspCmdq {
    // This is equivalent to gsp_shared_init()
    fn new(dev: &device::Device<device::Bound>) -> Result<GspCmdq> {
        // TODO: At the moment we assume 4096 PTEs will cover the GspMem object.
        // Seems reasonable, see the definition for the struct, but we probalby
        // should calculate it.
        let mut gsp_mem =
            CoherentAllocation::<GspMem>::alloc_coherent(dev, 1, GFP_KERNEL | __GFP_ZERO)?;

        // Basically the same as create_pte_array() but we don't skip the first
        // PTE.
        // TODO: Also we're creating PTEs for the RMARGS struct that follows
        // this one which is a bit ugly. Nouveau r535 doesn't seem to explicitly mention
        // this, so no idea if that was intentional.
        let ptes = unsafe {
            let ptr = gsp_mem.start_ptr_mut() as *mut u64;
            core::slice::from_raw_parts_mut(ptr, size_of::<GspMem>() >> GSP_PAGE_SHIFT)
        };

        for (i, pte) in ptes.iter_mut().enumerate() {
            *pte = gsp_mem.dma_handle() as u64 + ((i as u64) << GSP_PAGE_SHIFT);
        }

        let msg_count = ((0x40000 - GSP_PAGE_SIZE) / GSP_PAGE_SIZE) as u32;
        dma_write!(gsp_mem[0].cpuq.tx.version = 0);
        dma_write!(gsp_mem[0].cpuq.tx.size = 0x40000);
        dma_write!(gsp_mem[0].cpuq.tx.entry_off = GSP_PAGE_SIZE as u32);
        dma_write!(gsp_mem[0].cpuq.tx.msg_size = GSP_PAGE_SIZE as u32);
        dma_write!(gsp_mem[0].cpuq.tx.msg_count = msg_count);
        dma_write!(gsp_mem[0].cpuq.tx.write_ptr = 0);
        dma_write!(gsp_mem[0].cpuq.tx.flags = 1);

        // TODO: Hard-coded for now because offset_of!() isn't stable for nested types
        dma_write!(gsp_mem[0].cpuq.tx.rx_hdr_off = 32);

        // Add 0x1000 for the ptes and another 0x1000 for the message queue header
        let cpu_ptr = unsafe { (gsp_mem.start_ptr_mut() as *mut u8).add(0x2000) as *mut c_void };

        // And another 0x40000 for the gsp queue
        let gsp_ptr = unsafe { (gsp_mem.start_ptr_mut() as *mut u8).add(0x42000) as *mut c_void };

        Ok(GspCmdq {
            msg_count,
            seq: 0,
            gsp_mem,
            cpu_ptr,
            gsp_ptr,
        })
    }

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
        let mut free = rptr + self.msg_count - wptr - 1;

        if free >= self.msg_count {
            free -= self.msg_count;
        }

        free << GSP_PAGE_SHIFT
    }

    // Returns the number of bytes the GSP has written to the queue.
    fn get_used_rx_bytes(self: &Self) -> u32 {
        let rptr = self.cpu_rptr().unwrap();
        let wptr = self.gsp_wptr().unwrap();
        let mut used = wptr + self.msg_count - rptr;
        if used >= self.msg_count {
            used -= self.msg_count;
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

    fn calculate_checksum_bytes(msg_bytes: &[u8]) -> u32 {
        let mut sum: u64 = 0;
        for &byte in msg_bytes.iter().rev() {
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
        let ptr = (self.cpu_ptr as usize + (wptr as usize) * 0x1000) as *mut c_void;
        ptr
    }

    fn send<A: GspMessageElement>(self: &mut Self, function: u32, args: A) -> Result<()> {
        let mut msg = GspMsgHeader {
            auth_tag_buffer: [0; 16],
            aad_buffer: [0; 16],
            checksum: 0,
            sequence: self.seq,
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

        self.seq += 1;
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
            // iowrite32(0, self.cmdq_info.falcon);
        };

        Ok(())
    }

    fn send_unsized<A: UnsizedGspMessageElement>(
        self: &mut Self,
        function: u32,
        args: &A,
    ) -> Result<()> {
        let mut msg = GspMsgHeader {
            auth_tag_buffer: [0; 16],
            aad_buffer: [0; 16],
            checksum: 0,
            sequence: self.seq,
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

        self.seq += 1;
        rpc.length = (size_of::<GspMsgHeader>() + size_of::<GspRpcHeader>() + args.size()) as u32;

        unsafe {
            let ptr = self.alloc_cmd(rpc.length);
            let rpc_ptr = msg.copy_to(ptr);
            let args_ptr = rpc.copy_to(rpc_ptr);
            args.copy_to(args_ptr);

            let msg_bytes = core::slice::from_raw_parts(ptr as *const u8, rpc.length as usize);
            let mut msg_ptr = ptr as *mut GspMsgHeader;
            (*msg_ptr).checksum = GspCmdq::calculate_checksum_bytes(msg_bytes);
            print_hex_dump(
                "\0".as_ptr() as *const i8,
                "gsp: \0".as_ptr() as *const i8,
                2,
                16,
                1,
                ptr as *const i8,
                rpc.length as usize,
                1,
            );
        }

        let wptr = self.cpu_wptr().unwrap() + 1;

        // TODO: Figure out Rust barriers
        unsafe {
            asm!("sfence";);
            dma_write!(self.gsp_mem[0].cpuq.tx.write_ptr = wptr);
            asm!("mfence";);
            // iowrite32(0, self.cmdq_info.falcon);
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

        let msg_ptr = (self.gsp_ptr as usize + (rptr as usize) * 0x1000) as *mut c_void;
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

impl GspMessageElement for fw::GspFwWprMeta {}

pub(crate) fn build_wpr_meta(
    dev: &device::Device<device::Bound>,
    fw: &Firmware,
    fb_layout: &FbLayout,
) -> Result<DmaObject> {
    // TODO: Can we use dma_write!() with a properly defined structure instead?
    // Alistair: I think we can and should now that we have proper bindings
    // that we can use to define the structure.
    let mut wpr_meta_dma_object = DmaObject::new(dev, GSP_PAGE_SIZE)?;
    let mut wpr_meta =
        unsafe { fw::GspFwWprMeta::new(wpr_meta_dma_object.start_ptr_mut() as *mut c_void) };
    wpr_meta.magic = fw::GSP_FW_WPR_META_MAGIC as u64;
    wpr_meta.revision = fw::GSP_FW_WPR_META_REVISION as u64;
    wpr_meta.sysmemAddrOfRadix3Elf = fw.gsp.lvl0_dma_handle() as u64;
    wpr_meta.sizeOfRadix3Elf = fw.gsp.size() as u64;
    wpr_meta.sysmemAddrOfBootloader = fw.bootloader.ucode.dma_handle();
    wpr_meta.sizeOfBootloader = fw.bootloader.ucode.size() as u64;
    wpr_meta.bootloaderCodeOffset = fw.bootloader.code_offset as u64;
    wpr_meta.bootloaderDataOffset = fw.bootloader.data_offset as u64;
    wpr_meta.bootloaderManifestOffset = fw.bootloader.manifest_offset as u64;
    wpr_meta
        .__bindgen_anon_1
        .__bindgen_anon_1
        .sysmemAddrOfSignature = fw.gsp_sigs.dma_handle() as u64;
    wpr_meta.__bindgen_anon_1.__bindgen_anon_1.sizeOfSignature = fw.gsp_sigs.size() as u64;
    wpr_meta.gspFwRsvdStart = fb_layout.heap.start;
    wpr_meta.nonWprHeapOffset = fb_layout.heap.start;
    wpr_meta.nonWprHeapSize = fb_layout.heap.end - fb_layout.heap.start;
    wpr_meta.gspFwWprStart = fb_layout.wpr2.start;
    wpr_meta.gspFwHeapOffset = fb_layout.wpr2_heap.start;
    wpr_meta.gspFwHeapSize = fb_layout.wpr2_heap.end - fb_layout.wpr2_heap.start;
    wpr_meta.gspFwOffset = fb_layout.elf.start;
    wpr_meta.bootBinOffset = fb_layout.boot.start;
    wpr_meta.frtsOffset = fb_layout.frts.start;
    wpr_meta.frtsSize = fb_layout.frts.end - fb_layout.frts.start;
    wpr_meta.gspFwWprEnd = fb_layout.vga_workspace.start & !(0x20000 - 1);
    wpr_meta.gspFwHeapVfPartitionCount = fb_layout.vf_partition_count;
    wpr_meta.fbSize = fb_layout.fb.end - fb_layout.fb.start;
    wpr_meta.vgaWorkspaceOffset = fb_layout.vga_workspace.start;
    wpr_meta.vgaWorkspaceSize = fb_layout.vga_workspace.end - fb_layout.vga_workspace.start;
    wpr_meta.bootCount = 0;
    wpr_meta.__bindgen_anon_2.__bindgen_anon_1.partitionRpcAddr = 0;
    wpr_meta
        .__bindgen_anon_2
        .__bindgen_anon_1
        .partitionRpcRequestOffset = 0;
    wpr_meta
        .__bindgen_anon_2
        .__bindgen_anon_1
        .partitionRpcReplyOffset = 0;
    wpr_meta.verified = 0;

    Ok(wpr_meta_dma_object)
}

unsafe impl FromBytes for fw::GSP_ARGUMENTS_CACHED {}
unsafe impl AsBytes for fw::GSP_ARGUMENTS_CACHED {}

#[allow(unused)]
pub(crate) struct GspSharedMemObjects {
    libos: DmaObject,
    loginit: DmaObject,
    logintr: DmaObject,
    logrm: DmaObject,
    rmargs: CoherentAllocation<fw::GSP_ARGUMENTS_CACHED>,
    kern: Option<DmaObject>,
    cmdq: GspCmdq,
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

    let arg_offset = libos_arg_nr * size_of::<fw::LibosMemoryRegionInitArgument>();
    let libos_start_ptr = unsafe { libos.start_ptr_mut().add(arg_offset) };

    let libos_mem_init_args = fw::LibosMemoryRegionInitArgument {
        id8: id8(name),
        pa: obj.dma_handle(),
        size: obj.size() as u64,
        kind: fw::LibosMemoryRegionKind_LIBOS_MEMORY_REGION_CONTIGUOUS as u8,
        loc: fw::LibosMemoryRegionLoc_LIBOS_MEMORY_REGION_LOC_SYSMEM as u8,
    };

    Ok(obj)
}

fn create_coherent_dma_object<A: AsBytes + FromBytes>(
    dev: &device::Device<device::Bound>,
    name: &'static str,
    libos: &mut DmaObject,
    libos_arg_nr: usize,
) -> Result<CoherentAllocation<A>> {
    let mut obj = CoherentAllocation::<A>::alloc_coherent(dev, 1, GFP_KERNEL | __GFP_ZERO)?;

    let arg_offset = libos_arg_nr * size_of::<fw::LibosMemoryRegionInitArgument>();
    let libos_start_ptr = unsafe { libos.start_ptr_mut().add(arg_offset) };

    let libos_mem_init_args = fw::LibosMemoryRegionInitArgument {
        id8: id8(name),
        pa: obj.dma_handle(),
        size: obj.size() as u64,
        kind: fw::LibosMemoryRegionKind_LIBOS_MEMORY_REGION_CONTIGUOUS as u8,
        loc: fw::LibosMemoryRegionLoc_LIBOS_MEMORY_REGION_LOC_SYSMEM as u8,
    };

    Ok(obj)
}

const GSP_REGISTRY_NUM_ENTRIES: usize = 2;
struct RegistryEntry {
    key: &'static str,
    value: u32,
}

struct RegistryTable {
    entries: [RegistryEntry; GSP_REGISTRY_NUM_ENTRIES],
}

impl GspMessageElement for RegistryTable {
    unsafe fn copy_to(&self, ptr: *mut c_void) -> *mut c_void {
        // We need to construct the GSP representation of the RegistryTable which we do in place.
        unsafe {
            let table = ptr as *mut fw::PACKED_REGISTRY_TABLE;
            let mut table_data = (ptr as *const u8).add(
                size_of::<fw::PACKED_REGISTRY_TABLE>()
                    + GSP_REGISTRY_NUM_ENTRIES * size_of::<fw::PACKED_REGISTRY_ENTRY>(),
            ) as *mut u8;
            (*table).numEntries = 2;
            (*table).size = self.size() as u32;

            for i in 0..GSP_REGISTRY_NUM_ENTRIES {
                let entry_ptr = (ptr as *const u8).add(
                    size_of::<fw::PACKED_REGISTRY_TABLE>()
                        + i * size_of::<fw::PACKED_REGISTRY_ENTRY>(),
                ) as *mut fw::PACKED_REGISTRY_ENTRY;

                (*entry_ptr).nameOffset = table_data.byte_offset_from(table) as u32;
                (*entry_ptr).type_ = fw::REGISTRY_TABLE_ENTRY_TYPE_DWORD as u8;
                (*entry_ptr).data = self.entries[i].value;
                (*entry_ptr).length = 0;

                // Copy the key string to table_data and null terminate it
                let key_bytes = self.entries[i].key.as_bytes();
                core::ptr::copy_nonoverlapping(key_bytes.as_ptr(), table_data, key_bytes.len());
                table_data = table_data.add(key_bytes.len());
                *table_data = 0; // Add null terminator
                table_data = table_data.add(1); // Move past null terminator
            }

            (ptr as *const u8).add((*table).size as usize) as *mut c_void
        }
    }
}

impl UnsizedGspMessageElement for RegistryTable {
    fn size(&self) -> usize {
        let mut key_size = 0;
        for i in 0..GSP_REGISTRY_NUM_ENTRIES {
            key_size += self.entries[i].key.len() + 1; // +1 for NULL terminator
        }
        size_of::<fw::PACKED_REGISTRY_TABLE>()
            + GSP_REGISTRY_NUM_ENTRIES * size_of::<fw::PACKED_REGISTRY_ENTRY>()
            + key_size
    }
}

fn build_registry(mut cmdq: GspCmdq) -> Result<RegistryTable> {
    let registry = RegistryTable {
        entries: [
            RegistryEntry {
                key: "RMSecBusResetEnable",
                value: 1,
            },
            RegistryEntry {
                key: "RMForcePcieConfigSave",
                value: 1,
            },
        ],
    };

    cmdq.send_unsized(fw::NV_VGPU_MSG_FUNCTION_SET_REGISTRY, &registry);

    Err(EINVAL)
}

impl GspSharedMemObjects {
    pub(crate) fn new(dev: &device::Device<device::Bound>) -> Result<Self> {
        let mut libos = DmaObject::new(dev, GSP_PAGE_SIZE)?;

        let mut loginit = create_dma_object(dev, "LOGINIT", 0x10000, &mut libos, 0)?;
        create_pte_array(&mut loginit);
        let mut logintr = create_dma_object(dev, "LOGINTR", 0x10000, &mut libos, 1)?;
        create_pte_array(&mut logintr);
        let mut logrm = create_dma_object(dev, "LOGRM", 0x10000, &mut libos, 2)?;
        create_pte_array(&mut logrm);

        // Creates its own PTE array
        let cmdq = GspCmdq::new(dev)?;
        let rmargs =
            create_coherent_dma_object::<fw::GSP_ARGUMENTS_CACHED>(dev, "RMARGS", &mut libos, 3)?;
        dma_write!(
            rmargs[0].messageQueueInitArguments.sharedMemPhysAddr = cmdq.gsp_mem.dma_handle()
        );
        dma_write!(rmargs[0].messageQueueInitArguments.pageTableEntryCount = 4096);
        dma_write!(rmargs[0].messageQueueInitArguments.cmdQueueOffset = 0x2000);
        dma_write!(rmargs[0].messageQueueInitArguments.statQueueOffset = 0x42000);
        dma_write!(
            rmargs[0].srInitArguments.oldLevel = fw::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3
        );
        dma_write!(rmargs[0].srInitArguments.flags = 0);
        dma_write!(rmargs[0].srInitArguments.bInPMTransition = 1);

        build_registry(cmdq);

        // TODO: initialize rmargs and shm as per r535_gsp_rmargs_init.
        // TODO: also kernel from Dave's branch?

        Err(EINVAL)
    }
}
