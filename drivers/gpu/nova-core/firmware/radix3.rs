// SPDX-License-Identifier: GPL-2.0

#![allow(dead_code, unused_variables)]

use kernel::bindings;
use kernel::device;
use kernel::dma::DataDirection;
use kernel::prelude::*;
use kernel::scatterlist::Owned;
use kernel::scatterlist::SGTable;
use kernel::types::ARef;

use crate::dma::DmaObject;
use crate::gsp::GSP_PAGE_SIZE;

pub(crate) struct Radix3 {
    lvl0: DmaObject,
    lvl1: DmaObject,
}

impl Radix3 {
    pub(crate) fn new(dev: &device::Device, size: usize) -> Result<Self> {
        Err(ENOTSUPP)
    }
}

pub(crate) struct RadixFirmware {
    // pub radix3: Radix3,
    dev: ARef<device::Device>,
    fw_sg_table: Pin<KBox<SGTable<Owned<VVec<u8>>>>>,
    lvl2_sg_table: Pin<KBox<SGTable<Owned<VVec<u8>>>>>,
    lvl1_sg_table: Pin<KBox<SGTable<Owned<VVec<u8>>>>>,
    lvl0: DmaObject,
    size: usize,
}

impl RadixFirmware {
    pub(crate) fn new(
        dev: &device::Device<device::Bound>,
        name: &'static str,
        fw: &[u8],
    ) -> Result<Self> {
        // Move the firmware into a vmalloc'd vector.
        let mut fw_vvec = VVec::with_capacity(fw.len(), GFP_KERNEL)?;
        fw_vvec.extend_from_slice(fw, GFP_KERNEL)?;

        let fw_sg_table = KBox::pin_init(
            SGTable::new(dev, fw_vvec, DataDirection::TO_DEVICE, GFP_KERNEL),
            GFP_KERNEL,
        )?;

        let num_pages = fw_sg_table.into_iter().count();
        let mut lvl2 =
            VVec::<u8>::with_capacity(num_pages * core::mem::size_of::<u64>(), GFP_KERNEL)?;

        map_into_lvl(&fw_sg_table, &mut lvl2)?;

        let lvl2_sg_table = KBox::pin_init(
            SGTable::new(dev, lvl2, DataDirection::TO_DEVICE, GFP_KERNEL),
            GFP_KERNEL,
        )?;

        let num_pages = lvl2_sg_table.into_iter().count();
        let mut lvl1 =
            VVec::<u8>::with_capacity(num_pages * core::mem::size_of::<u64>(), GFP_KERNEL)?;

        map_into_lvl(&lvl2_sg_table, &mut lvl1)?;

        let lvl1_sg_table = KBox::pin_init(
            SGTable::new(dev, lvl1, DataDirection::TO_DEVICE, GFP_KERNEL),
            GFP_KERNEL,
        )?;

        let mut lvl0 = DmaObject::new(dev, GSP_PAGE_SIZE)?;
        let lvl0_slice =
            unsafe { core::slice::from_raw_parts_mut(lvl0.start_ptr_mut(), lvl0.size()) };
        lvl0_slice[0..core::mem::size_of::<u64>()].copy_from_slice(
            &(lvl1_sg_table.into_iter().next().unwrap().dma_address() as u64).to_le_bytes(),
        );

        Ok(Self {
            dev: dev.into(),
            fw_sg_table,
            lvl2_sg_table,
            lvl1_sg_table,
            lvl0,
            size: fw.len(),
        })
    }

    pub(crate) fn lvl0_dma_handle(&self) -> bindings::dma_addr_t {
        self.lvl0.dma_handle()
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }
}

fn map_into_lvl(sg_table: &SGTable<Owned<VVec<u8>>>, dst: &mut VVec<u8>) -> Result {
    for sg_entry in sg_table.into_iter() {
        pr_debug!(
            "sl: {:#x} {:#x}\n",
            sg_entry.dma_address(),
            sg_entry.dma_len()
        );
        // Round the size up to the next full page, if needed.
        // TODO: handle the case if GSP_PAGE_SIZE != PAGE_SIZE!
        let rounded_up_length = (sg_entry.dma_len() as usize)
            .checked_next_multiple_of(GSP_PAGE_SIZE)
            .ok_or(EINVAL)?;
        for i in 0..(rounded_up_length / GSP_PAGE_SIZE) {
            let entry = sg_entry.dma_address() + (GSP_PAGE_SIZE as u64 * i as u64);
            let entry_bytes = entry.to_le_bytes();
            dst.extend_from_slice(&entry_bytes, GFP_KERNEL)?;
        }
    }

    Ok(())
}
