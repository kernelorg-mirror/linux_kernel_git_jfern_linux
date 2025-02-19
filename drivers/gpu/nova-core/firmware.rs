
#![allow(dead_code)]

use kernel::{
    scatterlist::*,
    prelude::*,
    firmware::Firmware,
    page::PAGE_SIZE,
    device,
};

use crate::gsp::GSP_PAGE_SIZE;
use core::alloc::Layout;

use crate::dma::{DmaObject, SGObject};

pub(crate) fn fw_rd32(fw: &Firmware, offset: usize) -> u32 {
    u32::from_le_bytes(fw.data()[offset..offset+4].try_into().unwrap())
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct BinHdr {
    bin_magic: u32,
    bin_ver: u32,
    bin_size: u32,
    pub header_offset: u32,
    pub data_offset: u32,
    pub data_size: u32,
}

macro_rules! rb32 {
    ($bytes: expr, $offset: expr) => {
        u32::from_le_bytes($bytes[$offset..$offset+4].try_into().unwrap())
    }
}

impl BinHdr {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            bin_magic: rb32!(bytes, 0),
            bin_ver: rb32!(bytes, 4),
            bin_size: rb32!(bytes, 8),
            header_offset: rb32!(bytes, 12),
            data_offset: rb32!(bytes, 16),
            data_size: rb32!(bytes, 20),
        }
    }

    pub(crate) fn from_fw(fw: &Firmware) -> Self {
        Self::from_bytes(fw.data())
    }
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct BlDesc {
    pub start_tag: u32,
    pub dmem_load_offset: u32,
    pub code_offset: u32,
    pub code_size: u32,
    pub data_offset: u32,
    pub data_size: u32,
}


impl BlDesc {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            start_tag: rb32!(bytes, 0),
            dmem_load_offset: rb32!(bytes, 4),
            code_offset: rb32!(bytes, 8),
            code_size: rb32!(bytes, 12),
            data_offset: rb32!(bytes, 16),
            data_size: rb32!(bytes, 20),
        }
    }

    pub(crate) fn from_fw(fw: &Firmware, offset: usize) -> Self {
        Self::from_bytes(&fw.data()[offset..])
    }
}

#[repr(C,packed)]
pub(crate) struct FlcnBlDmemDesc_v2 {
    pub reserved: [u32; 4],
    pub signature: [u32; 4],
    pub ctx_dma: u32,
    pub code_dma_base: u64,
    pub non_sec_code_off: u32,
    pub non_sec_code_size: u32,
    pub sec_code_off: u32,
    pub sec_code_size: u32,
    pub code_entry_point: u32,
    pub data_dma_base: u64,
    pub data_size: u32,
    pub argc: u32,
    pub argv: u32,
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct HsHeader_v2 {
    pub sig_prod_offset: u32,
    pub sig_prod_size: u32,
    pub patch_loc: u32,
    pub patch_sig: u32,
    pub meta_data_offset: u32,
    meta_data_size: u32,
    pub num_sig: u32,
    pub(crate) header_offset: u32,
    header_size: u32,
}

impl HsHeader_v2 {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            sig_prod_offset: rb32!(bytes, 0),
            sig_prod_size: rb32!(bytes, 4),
            patch_loc: rb32!(bytes, 8),
            patch_sig: rb32!(bytes, 12),
            meta_data_offset: rb32!(bytes, 16),
            meta_data_size: rb32!(bytes, 20),
            num_sig: rb32!(bytes, 24),
            header_offset: rb32!(bytes, 28),
            header_size: rb32!(bytes, 32),
        }
    }

    pub(crate) fn from_fw(fw: &Firmware, offset: usize) -> Self {
        Self::from_bytes(&fw.data()[offset..])
    }
}


#[repr(C)]
#[derive(Debug)]
pub(crate) struct HsLoadHeader_v2 {
    pub os_code_offset: u32,
    pub os_code_size: u32,
    pub os_data_offset: u32,
    pub os_data_size: u32,
    pub apps: KVec<(u32, u32)>,
}

impl HsLoadHeader_v2 {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let num_apps : u32 = rb32!(bytes, 16);

        let mut apps : KVec<(u32, u32)> = Default::default();
        for p in 0..num_apps {
            let offset: usize = p as usize * 8_usize;
            apps.push((rb32!(bytes, (20 + offset)),
                       rb32!(bytes, (24 + offset))), GFP_KERNEL)?;
        }
        Ok(Self {
            os_code_offset: rb32!(bytes, 0),
            os_code_size: rb32!(bytes, 4),
            os_data_offset: rb32!(bytes, 8),
            os_data_size: rb32!(bytes, 12),
            apps,
        })
    }

    pub(crate) fn from_fw(fw: &Firmware, offset: usize) -> Result<Self> {
        Self::from_bytes(&fw.data()[offset..])
    }
}

pub(crate) struct NvkmFirmware {
    pub dma: DmaObject,
    pub name: &'static str,
}

impl NvkmFirmware {
    pub(crate) fn new(name: &'static str, dma: DmaObject) -> Self
    {
        Self {
            dma,
            name,
        }
    }
}

pub(crate) struct BLFirmware {
    pub fw: Firmware,
    pub boot_addr: u32,
    pub boot_size: u32,
    pub offset: u32
}

impl BLFirmware {
    pub(crate) fn new(bl: Firmware) -> Self {
        let bin_hdr = BinHdr::from_fw(&bl);
        let bl_desc = BlDesc::from_fw(&bl, bin_hdr.header_offset as usize);

        Self {
            fw: bl,
            boot_addr: bl_desc.start_tag << 8,
            boot_size: bl_desc.code_size,
            offset: bin_hdr.data_offset + bl_desc.code_offset,
        }
    }
}

#[allow(unused)]
pub(crate) struct Radix3 {
    lvl2: SGObject,
    lvl1: DmaObject,
    lvl0: DmaObject,
}

impl Radix3 {
    /// Allocate a radix3 table from a scatter-gather list
    pub(crate) fn new(size: usize,
                      sg_init: SGTableInit,
                      dev: &device::Device) -> Result<Radix3>
    {
        let tbl_size: usize =
            Layout::from_size_align((size as usize / GSP_PAGE_SIZE as usize)
                                    * core::mem::size_of::<u64>(),
                                    GSP_PAGE_SIZE as usize)?.pad_to_align().size();

        if tbl_size == 0 {
            return Err(EINVAL);
        }

        let mut lvl0 = DmaObject::new_cleared(dev, GSP_PAGE_SIZE as usize, "lvl0")?;
        let mut lvl1 = DmaObject::new_cleared(dev, GSP_PAGE_SIZE as usize, "lvl1")?;

        let mut lvl2_vec = VVec::<u8>::with_capacity(tbl_size, GFP_KERNEL)?;
        unsafe { lvl2_vec.set_len(tbl_size) };

        /* fill lvl2 with pages from fw sgt */
        let mut index: usize = 0;
        for sg in sg_init.iter() {
            for j in 0..(sg.dma_len() / GSP_PAGE_SIZE as usize) {
                let entry: u64 = sg.dma_address() + (GSP_PAGE_SIZE as u64 * j as u64);

                let bytes = entry.to_le_bytes(); // use to_be_bytes() for big-endian

                // Write the bytes to the Vec at the specified offset
                lvl2_vec[index..index + 8].copy_from_slice(&bytes);

                index += core::mem::size_of::<u64>();
            }
        }

        let (lvl2, s_init) = SGObject::new_from_data(dev, lvl2_vec)?;

        // Write the bus address of level 1 to level 0
        let lvl1_addr = lvl1.dma.dma_handle();
        lvl0.wr64(lvl1_addr, 0)?;

        let mut index: usize = 0;
        // Write the bus address of each page in level 2 to level 1
        for sg in s_init.iter() {
            for j in 0..(sg.dma_len() / GSP_PAGE_SIZE as usize) {
                let entry: u64 = sg.dma_address() + (GSP_PAGE_SIZE as u64 * j as u64);
                lvl1.wr64(entry, index)?;
                index += core::mem::size_of::<u64>();
            }
        }

        Ok(Self {
            lvl2,
            lvl1,
            lvl0
        })
    }

    pub(crate) fn lvl0_addr(&self) -> u64 {
	self.lvl0.dma.dma_handle()
    }
}

#[allow(unused)]
pub(crate) struct RadixFirmware {
    sg_obj: SGObject,
    pub radix3: Radix3,
    pub len: usize,
    name: &'static str,
}

impl RadixFirmware {
    pub(crate) fn new(dev: &device::Device, name: &'static str, data: &[u8]) -> Result<Self>
    {
        let len = Layout::from_size_align(data.len(),
                                          PAGE_SIZE)?.pad_to_align().size();

        let mut newvec : VVec<u8> = VVec::with_capacity(len, GFP_KERNEL)?;

        newvec.extend_from_slice(data, GFP_KERNEL)?;

        let (sg_obj, s_init) = SGObject::new_from_data(dev, newvec)?;

        let radix3 = Radix3::new(data.len(), s_init, dev)?;

        Ok(Self {
            sg_obj,
            radix3,
            len: data.len(),
            name,
        })
    }
}
