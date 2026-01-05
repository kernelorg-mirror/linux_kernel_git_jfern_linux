// SPDX-License-Identifier: GPL-2.0

//! BAR1 user interface for CPU access to GPU virtual memory. Used for USERD
//! for GPU work submission, and applications to access GPU buffers via mmap().

use kernel::{
    device,
    devres::Devres,
    io::Io,
    new_mutex,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    },
};

use crate::{
    driver::Bar1,
    gpu::Chipset,
    mm::{
        vmm::{
            MappedRange,
            Vmm, //
        },
        GpuMm,
        Pfn,
        Vfn,
        VirtualAddress,
        VramAddress,
        PAGE_SIZE, //
    },
    num::IntoSafeCast,
};

/// BAR1 user interface for virtual memory mappings.
///
/// Owns the [`Vmm`] for the BAR1 address space.
#[pin_data]
pub(crate) struct BarUser {
    #[pin]
    vmm: Mutex<Vmm>,
    mm: Arc<GpuMm>,
    bar1: Arc<Devres<Bar1>>,
}

impl BarUser {
    /// Create a pin-initializer for [`BarUser`].
    pub(crate) fn new(
        pdb_addr: VramAddress,
        chipset: Chipset,
        va_size: u64,
        mm: Arc<GpuMm>,
        bar1: Arc<Devres<Bar1>>,
    ) -> Result<impl PinInit<Self>> {
        let vmm = Vmm::new(pdb_addr, chipset.mmu_version(), va_size)?;
        Ok(pin_init!(Self {
            vmm <- new_mutex!(vmm, "bar_user_vmm"),
            mm,
            bar1,
        }))
    }

    /// Map physical pages to a contiguous BAR1 virtual range.
    pub(crate) fn map(
        self: &Arc<Self>,
        dev: &device::Device<device::Bound>,
        pfns: &[Pfn],
        writable: bool,
    ) -> Result<BarUserAccess> {
        if pfns.is_empty() {
            return Err(EINVAL);
        }
        let mut vmm = self.vmm.lock();
        let mapped = vmm.map_pages(dev, &self.mm, pfns, None, writable)?;

        Ok(BarUserAccess {
            bar_user: self.clone(),
            mapped: Some(mapped),
        })
    }
}

/// Access object for a mapped BAR1 region.
pub(crate) struct BarUserAccess {
    bar_user: Arc<BarUser>,
    /// [`BarUserAccess::release`] [`Option::take`]s this; `Some` at
    /// drop time means `release()` was never called.
    mapped: Option<MappedRange>,
}

impl BarUserAccess {
    /// Tear down the BAR1 mapping using a caller-supplied bound device.
    pub(crate) fn release(mut self, dev: &device::Device<device::Bound>) -> Result {
        let mapped = self.mapped.take().ok_or(EINVAL)?;
        let mut vmm = self.bar_user.vmm.lock();
        vmm.unmap_pages(dev, &self.bar_user.mm, mapped)?;
        Ok(())
    }

    /// Returns the active mapping.
    fn mapped(&self) -> &MappedRange {
        // `mapped` is only `None` after `take()` in `release`; hence unwrap()
        // cannot panic here.
        self.mapped.as_ref().unwrap()
    }

    /// Get the base virtual address of this mapping.
    pub(crate) fn base(&self) -> VirtualAddress {
        VirtualAddress::from(self.mapped().vfn_start)
    }

    /// Get the total size of the mapped region in bytes.
    pub(crate) fn size(&self) -> usize {
        self.mapped().num_pages * PAGE_SIZE
    }

    /// Get the starting virtual frame number.
    pub(crate) fn vfn_start(&self) -> Vfn {
        self.mapped().vfn_start
    }

    /// Get the number of pages in this mapping.
    pub(crate) fn num_pages(&self) -> usize {
        self.mapped().num_pages
    }

    /// Translate an offset within this mapping to a BAR1 aperture offset.
    fn bar_offset(&self, offset: usize) -> Result<usize> {
        if offset >= self.size() {
            return Err(EINVAL);
        }

        let base_vfn: usize = self.mapped().vfn_start.raw().into_safe_cast();
        let base = base_vfn.checked_mul(PAGE_SIZE).ok_or(EOVERFLOW)?;
        base.checked_add(offset).ok_or(EOVERFLOW)
    }

    // Fallible accessors with runtime bounds checking.

    /// Read a 32-bit value at the given offset.
    pub(crate) fn try_read32(
        &self,
        dev: &device::Device<device::Bound>,
        offset: usize,
    ) -> Result<u32> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_read32(off)
    }

    /// Write a 32-bit value at the given offset.
    pub(crate) fn try_write32(
        &self,
        dev: &device::Device<device::Bound>,
        value: u32,
        offset: usize,
    ) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_write32(value, off)
    }

    /// Read a 64-bit value at the given offset.
    pub(crate) fn try_read64(
        &self,
        dev: &device::Device<device::Bound>,
        offset: usize,
    ) -> Result<u64> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_read64(off)
    }

    /// Write a 64-bit value at the given offset.
    pub(crate) fn try_write64(
        &self,
        dev: &device::Device<device::Bound>,
        value: u64,
        offset: usize,
    ) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_write64(value, off)
    }
}

impl Drop for BarUserAccess {
    fn drop(&mut self) {
        if self.mapped.is_some() {
            kernel::pr_warn!(
                "BarUserAccess dropped without calling release(). BarUser address space will leak.\n"
            );
        }
        // The inner `MappedRange`'s own `MustUnmapGuard` will also fire,
        // identifying the leaked VA range.
    }
}
