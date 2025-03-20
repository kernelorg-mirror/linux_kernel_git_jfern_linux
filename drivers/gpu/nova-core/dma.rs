// SPDX-License-Identifier: GPL-2.0

use kernel::dma::{CoherentAllocation, Device};
use kernel::page::PAGE_SIZE;
use kernel::prelude::*;

pub(crate) struct DmaObject {
    pub dma: CoherentAllocation<u8>,
    pub len: usize,
    #[allow(dead_code)]
    pub name: &'static str,
}

impl DmaObject {
    pub(crate) fn new(dev: &dyn Device, len: usize, name: &'static str) -> Result<Self> {
        let len = core::alloc::Layout::from_size_align(len, PAGE_SIZE)
            .map_err(|_| EINVAL)?
            .pad_to_align()
            .size();
        let dma = CoherentAllocation::alloc_coherent(dev, len, GFP_KERNEL | __GFP_ZERO)?;

        Ok(Self { dma, len, name })
    }

    pub(crate) fn from_data(dev: &dyn Device, data: &[u8], name: &'static str) -> Result<Self> {
        Self::new(dev, data.len(), name).and_then(|dma_obj| {
            // SAFETY: We have just created this object and there is no other user at this stage.
            unsafe { dma_obj.dma.write(&data, 0) }?;
            Ok(dma_obj)
        })
    }
}
