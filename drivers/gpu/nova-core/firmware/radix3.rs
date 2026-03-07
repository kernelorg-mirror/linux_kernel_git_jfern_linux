// SPDX-License-Identifier: GPL-2.0

//! 3-level radix page table for GSP firmware data.
//!
//! The GSP bootloader expects data to be mapped via a 3-level page table:
//!
//! ```text
//! Level 0:  1 page, 1 entry         -> points to first level 1 page
//! Level 1:  Multiple pages/entries  -> each entry points to a level 2 page
//! Level 2:  Multiple pages/entries  -> each entry points to a data page
//! ```
//!
//! Each page is 4KB, each entry is 8 bytes (64-bit DMA address).

use core::mem::size_of_val;

use kernel::{
    device,
    dma::{
        DataDirection,
        DmaAddress, //
    },
    kvec,
    prelude::*,
    scatterlist::{
        Owned,
        SGTable, //
    },
};

use crate::{
    dma::DmaObject,
    gsp::GSP_PAGE_SIZE,
    num::FromSafeCast, //
};

/// 3-level radix page table mapping arbitrary data for the GSP.
#[pin_data]
pub(crate) struct Radix3 {
    /// The data mapped via a SG table.
    #[pin]
    data: SGTable<Owned<VVec<u8>>>,
    /// Level 2 page table whose entries contain DMA addresses of data pages.
    #[pin]
    level2: SGTable<Owned<VVec<u8>>>,
    /// Level 1 page table whose entries contain DMA addresses of level 2 pages.
    #[pin]
    level1: SGTable<Owned<VVec<u8>>>,
    /// Level 0 page table (single 4KB page) with one entry: DMA address of first level 1 page.
    level0: DmaObject,
    /// Size in bytes of the data contained in [`Self::data`].
    pub(crate) size: usize,
}

impl Radix3 {
    /// Build a 3-level radix page table for the given data, mapped into `dev`'s DMA address space.
    pub(crate) fn new<'a>(
        dev: &'a device::Device<device::Bound>,
        src: &[u8],
    ) -> impl PinInit<Self, Error> + 'a {
        let size = src.len();

        let data_result = VVec::with_capacity(size, GFP_KERNEL).and_then(|mut v| {
            v.extend_from_slice(src, GFP_KERNEL)?;
            Ok(v)
        });

        pin_init::pin_init_scope(move || {
            let data_vvec = data_result.map_err(|_| ENOMEM)?;

            Ok(try_pin_init!(Self {
                data <- SGTable::new(dev, data_vvec, DataDirection::ToDevice, GFP_KERNEL),
                level2 <- {
                    VVec::<u8>::with_capacity(
                        data.iter().count() * core::mem::size_of::<u64>(),
                        GFP_KERNEL,
                    )
                    .map_err(|_| ENOMEM)
                    .and_then(|level2| map_into_lvl(&data, level2))
                    .map(|level2| SGTable::new(dev, level2, DataDirection::ToDevice, GFP_KERNEL))?
                },
                level1 <- {
                    VVec::<u8>::with_capacity(
                        level2.iter().count() * core::mem::size_of::<u64>(),
                        GFP_KERNEL,
                    )
                    .map_err(|_| ENOMEM)
                    .and_then(|level1| map_into_lvl(&level2, level1))
                    .map(|level1| SGTable::new(dev, level1, DataDirection::ToDevice, GFP_KERNEL))?
                },
                level0: {
                    let mut level0_data = kvec![0u8; GSP_PAGE_SIZE]?;

                    let level1_entry = level1.iter().next().ok_or(EINVAL)?;
                    let level1_entry_addr = level1_entry.dma_address();
                    let dst = &mut level0_data[..size_of_val(&level1_entry_addr)];
                    dst.copy_from_slice(&level1_entry_addr.to_le_bytes());

                    DmaObject::from_data(dev, &level0_data)?
                },
                size,
            }))
        })
    }

    /// Returns the DMA handle of the radix3 level 0 page table.
    pub(crate) fn dma_handle(&self) -> DmaAddress {
        self.level0.dma_handle()
    }
}

/// Build a page table from a scatter-gather list.
///
/// Takes each DMA-mapped region from `sg_table` and writes page table entries
/// for all 4KB pages within that region. For example, a 16KB SG entry becomes
/// 4 consecutive page table entries.
fn map_into_lvl(sg_table: &SGTable<Owned<VVec<u8>>>, mut dst: VVec<u8>) -> Result<VVec<u8>> {
    for sg_entry in sg_table.iter() {
        let num_pages = usize::from_safe_cast(sg_entry.dma_len()).div_ceil(GSP_PAGE_SIZE);

        for i in 0..num_pages {
            let entry = sg_entry.dma_address()
                + (u64::from_safe_cast(i) * u64::from_safe_cast(GSP_PAGE_SIZE));
            dst.extend_from_slice(&entry.to_le_bytes(), GFP_KERNEL)?;
        }
    }

    Ok(dst)
}
