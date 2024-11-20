
#![allow(dead_code)]

use kernel::{
    prelude::*,
    firmware::Firmware,
};

use crate::dma::DmaObject;

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

