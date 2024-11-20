#![allow(unused)]

use kernel::dma::CoherentAllocation;
use kernel::scatterlist::{*, DmaDataDirection::*};
use kernel::device;
use kernel::prelude::*;
use kernel::page::{PageSlice, Page, PAGE_SIZE, PAGE_SHIFT};
use core::alloc::Layout;
use core::ptr;

pub (crate) struct DmaObject {
    pub dma: CoherentAllocation<u8>,
    pub len: usize,
    pub name: &'static str,
}

impl DmaObject {
     pub(crate) fn new_from_data(dev: &device::Device,
				 data: &[u8], name: &'static str) -> Result<DmaObject> {
         let len = core::alloc::Layout::from_size_align(data.len(), PAGE_SIZE)?.pad_to_align().size();

         let mut dma: CoherentAllocation<u8> = CoherentAllocation::alloc_coherent(dev, len, GFP_KERNEL | __GFP_ZERO)?;

	 pr_info!("vma: {} {:#x} {:#x}", name, dma.dma_handle(), data.len());
	 unsafe {
	     ptr::copy_nonoverlapping(data.as_ptr(),
				      dma.start_ptr_mut(),
				      data.len());
	 }

	Ok(Self { dma, len, name })
     }

     pub(crate) fn new_cleared(dev: &device::Device,
			       len: usize, name: &'static str) -> Result<DmaObject> {
         let len = core::alloc::Layout::from_size_align(len, PAGE_SIZE)?.pad_to_align().size();
         let dma: CoherentAllocation<u8> = CoherentAllocation::alloc_coherent(dev, len, GFP_KERNEL | __GFP_ZERO)?;
	 Ok(Self { dma, len, name })
     }

    pub(crate) fn wr32(&mut self, value: u32, offset: usize) -> Result<()> {
	let bytes = value.to_le_bytes();

	unsafe {
	    let ptr = self.dma.start_ptr_mut().byte_offset(offset as isize);
	    ptr::copy_nonoverlapping(bytes.as_ptr(),
				     ptr,
				     4);
	}
	Ok(())
    }

    pub(crate) fn wr64(&mut self, value: u64, offset: usize) -> Result<()> {
	let bytes = value.to_le_bytes();

	unsafe {
	    let ptr = self.dma.start_ptr_mut().byte_offset(offset as isize);
	    ptr::copy_nonoverlapping(bytes.as_ptr(),
				     ptr,
				     8);
	}
	Ok(())
    }

    pub(crate) fn get_slice(&self, start: usize, size: usize) -> &[u8] {
	unsafe {
	    core::slice::from_raw_parts(self.dma.start_ptr().byte_offset(start as isize), size)
	}
    }
}

impl Drop for DmaObject {
    fn drop(&mut self) {
	pr_info!("vma drop {} {:#x} {:#x}", self.name, self.dma.dma_handle(), self.len);
    }
}

pub(crate) struct SGObject {
    pub sgt: SGTable,
    pub vec: VVec<PageSlice>,
    pub len: usize,
}

impl SGObject {
    fn create_sgt_from_data(dev: &device::Device,
			    sgt: &mut SGTable,
			    data: VVec<u8>) -> Result<(VVec<PageSlice>, SGTableInit)>
    {
	let pages = Layout::from_size_align(data.len(),
                                            PAGE_SIZE)?.pad_to_align().size() >> PAGE_SHIFT;
	let (ptr, _len, _) = data.into_raw_parts();
	let buf: VVec<PageSlice> = unsafe { VVec::from_raw_parts(ptr as *mut PageSlice, pages, pages) };
	let mut s_init = sgt.alloc_table(pages as u32, GFP_KERNEL)?;

	for (i, sg) in s_init.iter().enumerate() {
	    sg.set_page(Page::page_slice_to_page(&buf[i])?, PAGE_SIZE as u32, 0);
	}
	s_init.dma_map(sgt, &dev, DMA_TO_DEVICE)?;
	Ok((buf, s_init))
    }

    pub(crate) fn new_from_data(dev: &device::Device, vec: VVec<u8>) -> Result<(Self, SGTableInit)> {
	let mut sgt = SGTable::uninit()?;
	let len = vec.len();
	let (vec, s_init) = Self::create_sgt_from_data(dev, &mut sgt, vec)?;
	Ok((Self {
	    sgt,
	    vec,
	    len,
	}, s_init))
    }

    pub(crate) fn new_cleared(dev: &device::Device, size: usize) -> Result<(Self, SGTableInit)> {
	let mut sgt = SGTable::uninit()?;
	let vec : VVec<u8> = VVec::with_capacity(size, GFP_KERNEL | __GFP_ZERO)?;

	let (vec, s_init) = Self::create_sgt_from_data(dev, &mut sgt, vec)?;
	Ok((Self {
	    sgt,
	    vec,
	    len: size,
	}, s_init))
    }
}
