// SPDX-License-Identifier: GPL-2.0

mod boot;

use kernel::{
    c_str,
    debugfs,
    device,
    dma::{
        CoherentAllocation,
        DmaAddress, //
    },
    dma_write,
    fs::file,
    pci,
    prelude::*,
    transmute::AsBytes,
    uaccess::UserSliceWriter, //
};

pub(crate) mod cmdq;
pub(crate) mod commands;
mod fw;

pub(crate) use fw::{
    GspFwWprMeta,
    LibosParams, //
};

use crate::{
    firmware::BuildId,
    gpu::Chipset,
    gsp::cmdq::Cmdq,
    gsp::fw::{
        GspArgumentsPadded,
        LibosMemoryRegionInitArgument, //
    },
    num,
};

pub(crate) const GSP_PAGE_SHIFT: usize = 12;
pub(crate) const GSP_PAGE_SIZE: usize = 1 << GSP_PAGE_SHIFT;

/// Number of GSP pages to use in a RM log buffer.
const RM_LOG_BUFFER_NUM_PAGES: usize = 0x10;

/// Array of page table entries, as understood by the GSP bootloader.
#[repr(C)]
struct PteArray<const NUM_ENTRIES: usize>([u64; NUM_ENTRIES]);

/// SAFETY: arrays of `u64` implement `AsBytes` and we are but a wrapper around one.
unsafe impl<const NUM_ENTRIES: usize> AsBytes for PteArray<NUM_ENTRIES> {}

impl<const NUM_PAGES: usize> PteArray<NUM_PAGES> {
    /// Returns the page table entry for `index`, for a mapping starting at `start`.
    // TODO: Replace with `IoView` projection once available.
    fn entry(start: DmaAddress, index: usize) -> Result<u64> {
        start
            .checked_add(num::usize_as_u64(index) << GSP_PAGE_SHIFT)
            .ok_or(EOVERFLOW)
    }
}

/// Size of the header prepended to debugfs log buffer dumps.
///
/// This header makes each dump self-describing so that decoding tools can
/// identify the firmware build, GPU architecture, and metadata format without
/// out-of-band information.
const LOG_BUFFER_HEADER_SIZE: usize = 0x48;

/// Build a log buffer header from GPU and firmware metadata.
///
/// Layout (all little-endian):
///   0x00  gpuArch (u32)
///   0x04  gpuImpl (u32)
///   0x08  version (u32) = 2
///   0x0C  buildIdLength (u32)
///   0x10  taskPrefix[8]
///   0x18  localToGlobalTimerDelta (u64) = 0
///   0x20  buildId[32]
///   0x40  flags (u32) = 1 (packed metadata)
///   0x44  reserved (u32) = 0
fn build_log_buffer_header(
    chipset: Chipset,
    build_id: &BuildId,
    task_prefix: &str,
) -> [u8; LOG_BUFFER_HEADER_SIZE] {
    let mut h = [0u8; LOG_BUFFER_HEADER_SIZE];
    let chipset_val = chipset as u32;

    h[0x00..0x04].copy_from_slice(&(chipset_val >> 4).to_le_bytes());
    h[0x04..0x08].copy_from_slice(&(chipset_val & 0xF).to_le_bytes());
    h[0x08..0x0C].copy_from_slice(&2u32.to_le_bytes());

    let bid = build_id.as_bytes();
    h[0x0C..0x10].copy_from_slice(&(bid.len() as u32).to_le_bytes());

    let prefix = task_prefix.as_bytes();
    let prefix_len = prefix.len().min(8);
    h[0x10..0x10 + prefix_len].copy_from_slice(&prefix[..prefix_len]);

    h[0x20..0x20 + bid.len()].copy_from_slice(bid);
    h[0x40..0x44].copy_from_slice(&1u32.to_le_bytes());

    h
}

/// The logging buffers are byte queues that contain encoded printf-like
/// messages from GSP-RM.  They need to be decoded by a special application
/// that can parse the buffers.
///
/// The 'loginit' buffer contains logs from early GSP-RM init and
/// exception dumps.  The 'logrm' buffer contains the subsequent logs. Both are
/// written to directly by GSP-RM and can be any multiple of GSP_PAGE_SIZE.
///
/// The physical address map for the log buffer is stored in the buffer
/// itself, starting with offset 1. Offset 0 contains the "put" pointer (pp).
/// Initially, pp is equal to 0. If the buffer has valid logging data in it,
/// then pp points to index into the buffer where the next logging entry will
/// be written. Therefore, the logging data is valid if:
///   1 <= pp < sizeof(buffer)/sizeof(u64)
///
/// When a build ID is available, the debugfs file for this buffer prepends
/// a header so the dump is self-describing.
struct LogBuffer {
    header: [u8; LOG_BUFFER_HEADER_SIZE],
    header_len: usize,
    buffer: CoherentAllocation<u8>,
}

impl LogBuffer {
    /// Creates a new `LogBuffer` of `NUM_PAGES` GSP pages, mapped on `dev`.
    fn with_pages<const NUM_PAGES: usize>(
        dev: &device::Device<device::Bound>,
        chipset: Chipset,
        build_id: Option<&BuildId>,
        task_prefix: &str,
    ) -> Result<Self> {
        let mut buffer = CoherentAllocation::<u8>::alloc_coherent(
            dev,
            NUM_PAGES * GSP_PAGE_SIZE,
            GFP_KERNEL | __GFP_ZERO,
        )?;

        let start_addr = buffer.dma_handle();

        // SAFETY: `buffer` has just been created and we are its sole user.
        let pte_region =
            unsafe { buffer.as_slice_mut(size_of::<u64>(), NUM_PAGES * size_of::<u64>())? };

        // Write values one by one to avoid an on-stack instance of `PteArray`.
        for (i, chunk) in pte_region.chunks_exact_mut(size_of::<u64>()).enumerate() {
            let pte_value = PteArray::<0>::entry(start_addr, i)?;

            chunk.copy_from_slice(&pte_value.to_ne_bytes());
        }

        let (header, header_len) = match build_id {
            Some(bid) => (
                build_log_buffer_header(chipset, bid, task_prefix),
                LOG_BUFFER_HEADER_SIZE,
            ),
            None => ([0u8; LOG_BUFFER_HEADER_SIZE], 0),
        };

        Ok(Self {
            header,
            header_len,
            buffer,
        })
    }

    /// Creates a standard 64KB log buffer (16 GSP pages).
    fn new(
        dev: &device::Device<device::Bound>,
        chipset: Chipset,
        build_id: Option<&BuildId>,
        task_prefix: &str,
    ) -> Result<Self> {
        Self::with_pages::<RM_LOG_BUFFER_NUM_PAGES>(dev, chipset, build_id, task_prefix)
    }

    /// Creates a small 4KB log buffer (1 GSP page).
    fn new_small(
        dev: &device::Device<device::Bound>,
        chipset: Chipset,
        build_id: Option<&BuildId>,
        task_prefix: &str,
    ) -> Result<Self> {
        Self::with_pages::<1>(dev, chipset, build_id, task_prefix)
    }
}

impl debugfs::BinaryWriter for LogBuffer {
    fn write_to_slice(
        &self,
        writer: &mut UserSliceWriter,
        offset: &mut file::Offset,
    ) -> Result<usize> {
        if offset.is_negative() {
            return Err(EINVAL);
        }

        let offset_val: usize = (*offset).try_into().map_err(|_| EINVAL)?;
        let total_len = self.header_len + self.buffer.count();

        if offset_val >= total_len {
            return Ok(0);
        }

        let count = (total_len - offset_val).min(writer.len());
        if count == 0 {
            return Ok(0);
        }

        let mut written = 0;

        if offset_val < self.header_len {
            let hdr_start = offset_val;
            let hdr_count = (self.header_len - hdr_start).min(count);
            writer.write_slice(&self.header[hdr_start..hdr_start + hdr_count])?;
            written += hdr_count;
        }

        if written < count {
            let buf_start = if offset_val > self.header_len {
                offset_val - self.header_len
            } else {
                0
            };
            let buf_count = count - written;
            writer.write_dma(&self.buffer, buf_start, buf_count)?;
            written += buf_count;
        }

        *offset += written as i64;
        Ok(written)
    }
}

/// Log buffers used by GSP-RM for debug logging.
///
/// r000+ firmware expects log buffers for all LIBOS3 tasks. Each buffer is
/// registered as a libos memory region entry, identified by its id8 name.
struct LogBuffers {
    /// Init task log buffer (LOGINIT, 64KB).
    loginit: LogBuffer,
    /// Interrupt task log buffer (LOGINTR, 64KB).
    logintr: LogBuffer,
    /// RM task log buffer (LOGRM, 64KB).
    logrm: LogBuffer,
    /// MNOC task log buffer (LOGMNOC, 64KB).
    logmnoc: LogBuffer,
    /// Root task log buffer (LOGROOT, 4KB).
    logroot: LogBuffer,
    /// RM state monitor task log buffer (LOGRMON, 4KB).
    logrmon: LogBuffer,
}

/// GSP runtime data.
#[pin_data]
pub(crate) struct Gsp {
    /// Libos arguments.
    pub(crate) libos: CoherentAllocation<LibosMemoryRegionInitArgument>,
    /// Command queue.
    pub(crate) cmdq: Cmdq,
    /// RM arguments.
    rmargs: CoherentAllocation<GspArgumentsPadded>,
    /// RM state monitor buffer (4KB, required by r000+ GSP-RM for diagnostics).
    rm_state_monitor: CoherentAllocation<u8>,
}

impl Gsp {
    // Creates an in-place initializer for a `Gsp` manager for `pdev`.
    pub(crate) fn new<'a>(
        pdev: &'a pci::Device<device::Bound>,
        chipset: Chipset,
        build_id: Option<&'a BuildId>,
    ) -> impl PinInit<Self, Error> + 'a {
        pin_init::pin_init_scope(move || {
            let dev = pdev.as_ref();

            let loginit = LogBuffer::new(dev, chipset, build_id, "INIT")?;
            let logintr = LogBuffer::new(dev, chipset, build_id, "INTR")?;
            let logrm = LogBuffer::new(dev, chipset, build_id, "RM")?;
            let logmnoc = LogBuffer::new(dev, chipset, build_id, "MNOC")?;
            let logroot = LogBuffer::new_small(dev, chipset, build_id, "ROOT")?;
            let logrmon = LogBuffer::new_small(dev, chipset, build_id, "RMON")?;

            Ok(try_pin_init!(Self {
                libos: CoherentAllocation::<LibosMemoryRegionInitArgument>::alloc_coherent(
                    dev,
                    GSP_PAGE_SIZE / size_of::<LibosMemoryRegionInitArgument>(),
                    GFP_KERNEL | __GFP_ZERO,
                )?,
                cmdq: Cmdq::new(dev)?,
                rmargs: CoherentAllocation::<GspArgumentsPadded>::alloc_coherent(
                    dev,
                    1,
                    GFP_KERNEL | __GFP_ZERO,
                )?,
                rm_state_monitor: CoherentAllocation::<u8>::alloc_coherent(
                    dev,
                    GSP_PAGE_SIZE,
                    GFP_KERNEL | __GFP_ZERO,
                )?,
                _: {
                    // Set up libos memory region entries for each LIBOS3 task log buffer,
                    // followed by RMARGS. The order matches Open RM's
                    // _kgspInitLibosLoggingStructures + kgspSetupLibosInitArgs_IMPL.
                    // LOGINIT must be first for early init logging.
                    // RMARGS must be last.
                    dma_write!(
                        libos, [0]?, LibosMemoryRegionInitArgument::new("LOGINIT", &loginit.buffer)
                    );
                    dma_write!(
                        libos, [1]?, LibosMemoryRegionInitArgument::new("LOGINTR", &logintr.buffer)
                    );
                    dma_write!(
                        libos, [2]?, LibosMemoryRegionInitArgument::new("LOGRM", &logrm.buffer)
                    );
                    dma_write!(
                        libos, [3]?, LibosMemoryRegionInitArgument::new("LOGMNOC", &logmnoc.buffer)
                    );
                    dma_write!(
                        libos, [4]?, LibosMemoryRegionInitArgument::new("LOGROOT", &logroot.buffer)
                    );
                    dma_write!(
                        libos, [5]?, LibosMemoryRegionInitArgument::new("LOGRMON", &logrmon.buffer)
                    );
                    dma_write!(rmargs, [0]?.inner, fw::GspArgumentsCached::new(
                        cmdq, None, &rm_state_monitor
                    ));
                    dma_write!(libos, [6]?, LibosMemoryRegionInitArgument::new("RMARGS", rmargs));
                },
                // HACK: Leak the debugfs scope so GSP-RM log buffers survive probe failure.
                // The DMA buffers + debugfs entries will persist for post-mortem debugging.
                _: {
                    let log_buffers = LogBuffers {
                        loginit,
                        logintr,
                        logrm,
                        logmnoc,
                        logroot,
                        logrmon,
                    };

                    #[allow(static_mut_refs)]
                    let log_parent: &debugfs::Dir = unsafe { crate::DEBUGFS_ROOT.as_ref() }
                        .expect("DEBUGFS_ROOT not initialized");

                    let scope = KBox::pin_init(
                        log_parent.scope(log_buffers, dev.name(), |logs, dir| {
                            dir.read_binary_file(c_str!("loginit"), &logs.loginit);
                            dir.read_binary_file(c_str!("logintr"), &logs.logintr);
                            dir.read_binary_file(c_str!("logrm"), &logs.logrm);
                            dir.read_binary_file(c_str!("logmnoc"), &logs.logmnoc);
                            dir.read_binary_file(c_str!("logroot"), &logs.logroot);
                            dir.read_binary_file(c_str!("logrmon"), &logs.logrmon);
                        }),
                        GFP_KERNEL,
                    )?;
                    core::mem::forget(scope);
                },
            }))
        })
    }
}
