// SPDX-License-Identifier: GPL-2.0

//! Scatterlist
//!
//! C header: [`include/linux/scatterlist.h`](srctree/include/linux/scatterlist.h)

use crate::{
    bindings,
    device::Device,
    error::{Error, Result},
    types::{Opaque, ARef},
    page::Page,
    alloc::{AllocError, flags::*, KVec},
    sync::Arc,
};
use core::marker::PhantomData;

/// A single scatter-gather entry, representing a span of pages in the device's DMA address space.
///
/// # Invariants
/// The scatterlist pointer is valid for the lifetime of the SGEntry instance.
#[repr(transparent)]
pub struct SGEntry(Opaque<bindings::scatterlist>);

impl SGEntry {
    /// Returns the starting DMA address of this span
    pub fn dma_address(&self) -> u64 {
        // SAFETY: deref of pointer is ok according to the invariant
        (unsafe { *self.0.get() }).dma_address as u64
    }

    /// Returns the length of this span
    pub fn dma_len(&self) -> usize {
        // SAFETY: deref of pointer is ok according to the invariant
        (unsafe{ *self.0.get() }).length as usize
    }

    /// Set this entry to point at a given page.
    /// The caller has to ensure that offset is bounded within the length of the page
    pub fn set_page(&mut self, page: &Page, length: u32, offset: u32)
    {
        let c: *mut bindings::scatterlist = self.0.get();
        // SAFETY: according to the SGEntry invariant, the scatterlist pointer is valid.
        // Page invariant also ensure the pointer is valid. Hence it is safe to
        // call this macro which just sets the fields of a scatterlist entry.
        unsafe { bindings::sg_set_page(c, page.as_ptr(), length, offset) };
    }

    /// # Safety
    /// This is an internal convenience function used in the `Iterator` trait implementation
    /// where it is ensured that the sg pointer is always valid.
    unsafe fn from_raw<'a>(sg: *mut bindings::scatterlist) -> &'a mut Self
    {
        // SAFETY: Guaranteed by the safety requirements of the function.
        unsafe { &mut (*sg.cast()) }
    }
}

/// Represents sgtable dma_data_direction mapping. Corresponds to the
/// kernel's [`enum dma_data_direction`].
#[derive(Copy, Clone)]
#[allow(non_camel_case_types)]
#[repr(i32)]
pub enum DmaDataDirection {
    /// Direction isn't known
    DMA_BIDIRECTIONAL = bindings::dma_data_direction_DMA_BIDIRECTIONAL,
    /// Data is going from the memory to the device
    DMA_TO_DEVICE = bindings::dma_data_direction_DMA_TO_DEVICE,
    /// Data is coming from the device to the memory
    DMA_FROM_DEVICE = bindings::dma_data_direction_DMA_FROM_DEVICE,
    /// No direction (used for debugging)
    DMA_NONE = bindings::dma_data_direction_DMA_NONE,
}

/// A `SGTable` initializer object.
///
/// This is used for mutable operations when initializing the sg_table.
///
/// # Invariants
/// The sg_table pointer is valid for the lifetime of the `SGTableInit` instance.
pub struct SGTableInit(Arc<Opaque<bindings::sg_table>>);

impl SGTableInit {
    /// Map this sgtable describing a buffer for DMA
    pub fn dma_map(&mut self, sgt: &mut SGTable,
                   dev: &Device, dir: DmaDataDirection) -> Result<> {
        // Assert that this initializer is really tied to the `SGTable` that created it
        ::core::assert!(self.0.get() == sgt.sgt.get());

        // SAFETY: Invariants on `Device` and `SGTableInit` ensures that the device and sgt
        // pointers are valid. We also catch catch the error if this function fails and return
        // it. Hence it is safe to call this method.
        let ret = unsafe {
            bindings::dma_map_sgtable(dev.as_raw(),
                                      self.0.get(),
                                      dir as _,
                                      bindings::DMA_ATTR_NO_WARN.into())
        };
        if ret != 0 {
            return Err(Error::from_errno(ret))
        }
        sgt.mappings.push(SgtDmaMap::new(dev, dir), GFP_KERNEL)?;
        Ok(())
    }

    /// Returns an iterator over the the scather-gather table.
    pub fn iter(&self) -> SGTableIter<'_> {
        SGTableIter {
            // SAFETY: dereferenced pointer is valid due to the type invariants on `SGTable`.
            left: unsafe { (*self.0.get()).nents } as usize,
            // SAFETY: dereferenced pointer is valid due to the type invariants on `SGTable`.
            sg: unsafe { (*self.0.get()).sgl },
            _p: PhantomData,
        }
    }

    fn alloc_table(sgt: &Arc<Opaque<bindings::sg_table>>,
                   nents: u32, flags: kernel::alloc::Flags) -> Result<Self>
    {
        let sgt = sgt.clone();
        // SAFETY: The sgt pointer is from the initialized Opaque reference hence is valid.
        // We also catch the error if this function fails and return it. Hence it is safe to
        // call this method.
        let ret = unsafe { bindings::sg_alloc_table(sgt.get(),
                                                    nents, flags.as_raw()) };
        if ret != 0 {
            return Err(Error::from_errno(ret))
        }

        Ok(SGTableInit(sgt))
    }
}

/// A scatter-gather table of DMA address spans.
///
/// # Invariants
///
/// After initialization, the sg_table pointer is valid for the lifetime of the SGTable instance.
pub struct SGTable {
    sgt: Arc<Opaque<bindings::sg_table>>,
    mappings: KVec<SgtDmaMap>,
}

impl SGTable {
    /// Constructs a new uninitialized `SGTable` object.
    pub fn uninit() -> Result<Self>
    {
        let sgt: Opaque<bindings::sg_table> = Opaque::uninit();
        // Zero-fill the uninitialized sgt struct.
        let sgt_ffi: bindings::sg_table = Default::default();
        // SAFETY: The src pointer is from the successfully allocated Opaque object, hence
        // is valid for writes.
        unsafe { sgt.get().write(sgt_ffi); }

        Ok( Self{sgt: Arc::new(sgt, GFP_KERNEL)?,
                 mappings: KVec::new(),
        })
    }

    /// Allocate and initialize the sg table. Mutable operations can then be performed on the
    /// returned initializer object, which is not thread-safe. Returns an error if the
    /// allocation failed or the `SGTable` is already initialized.
    ///
    /// The `SGTable` object itself should be `Send` and `Sync` across threads since the only
    /// operation allowed on it is during resource deallocation when the object goes out of scope.
    pub fn alloc_table(&self, nents: u32,
                       flags: kernel::alloc::Flags) -> Result<SGTableInit>
    {
        if !self.is_uninit() {
            return Err(AllocError.into());
        }
        Ok(SGTableInit::alloc_table(&self.sgt, nents, flags)?)
    }

    fn is_uninit(&self) -> bool
    {
        // SAFETY: The dereferenced pointer is valid from the successfully allocated Opaque
        // object,
        unsafe { (*self.sgt.get()).sgl.is_null() }
    }
}

/// SAFETY: The `SGTable` should be `Send` across threads since the only operation
/// allowed on it is during resource deallocation when the object goes out of scope.
unsafe impl Send for SGTable {}

/// SAFETY: The `SGTable` should be `Sync` across threads since the only operation
/// allowed on it is during resource deallocation when the object goes out of scope.
unsafe impl Sync for SGTable {}

impl<'a> IntoIterator for &'a SGTableInit {
    type Item = &'a mut SGEntry;
    type IntoIter = SGTableIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Scatter-gather table iterator
pub struct SGTableIter<'a> {
    sg: *mut bindings::scatterlist,
    left: usize,
    _p: PhantomData<&'a ()>,
}

impl<'a> Iterator for SGTableIter<'a> {
    type Item = &'a mut SGEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            None
        } else {
            let sg = self.sg;
            // SAFETY: `sg` is always valid since we don't enter this block if we have no more
            // entries, e.g. sg_next() never returns NULL.
            self.sg = unsafe { bindings::sg_next(self.sg) };
            self.left -= 1;
            // SAFETY: dereferenced pointer is always valid from above invariant.
            Some(unsafe { SGEntry::from_raw(sg) })
        }
    }
}

/// Represents a dma mapping of a scather-gather table to a DMA device.
///
/// # Invariants
/// The pointer to the device is valid for the lifetime of the SgtDmaMap instance.
struct SgtDmaMap {
    dev: ARef<Device>,
    dir: DmaDataDirection,
}

impl SgtDmaMap {
    /// Creates a new mapping object
    fn new(dev: &Device, dir: DmaDataDirection) -> Self {
        Self { dev: dev.into(), dir, }
    }
}

impl Drop for SGTable {
    fn drop(&mut self) {
        // No resource to deallocate.
        if self.is_uninit() {
            return;
        }

        for map in self.mappings.iter() {
            // SAFETY: the `device` and `sgt` pointers are valid due to the type invariants
            // on `SgtDmaMap` and `SGTable`. Hence it is safe to call this method.
            unsafe { bindings::dma_unmap_sgtable(map.dev.as_raw(), self.sgt.get(),
                                                 map.dir as _, 0) };
        }
        // SAFETY: The `sgt` pointer is valid due to the type invariants on `SGTable`.
        // Hence it is safe to call this method.
        unsafe {bindings::sg_free_table(self.sgt.get()) };
    }
}
