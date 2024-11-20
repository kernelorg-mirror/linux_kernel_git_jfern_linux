
#![allow(dead_code)]

use kernel::prelude::*;
use crate::firmware::NvkmFirmware;
use kernel::firmware::Firmware;
use crate::firmware::BinHdr;
use crate::dma::DmaObject;
use kernel::device;

#[allow(non_snake_case)]
#[repr(C)]
pub(crate) struct RmRiscvUCodeDesc {
    version: u32,
    bootloaderOffset: u32,
    bootloaderSize: u32,
    bootloaderParamOffset: u32,
    bootloaderParamSize: u32,
    riscvElfOffset: u32,
    riscvElfSize: u32,
    appVersion: u32,

    manifestOffset: u32,
    manifestSize: u32,

    monitorDataOffset: u32,
    monitorDataSize: u32,

    monitorCodeOffset: u32,
    monitorCodeSize: u32,
    bIsMonitorEnabled: u32,

    swbromCodeOffset: u32,
    swbromCodeSize: u32,

    swbromDataOffset: u32,
    swbromDataSize: u32,

    fbReservedSize: u32,

    bSignedAsCode: u32,
}

impl RmRiscvUCodeDesc {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            version: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            bootloaderOffset: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            bootloaderSize: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            bootloaderParamOffset: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            bootloaderParamSize: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            riscvElfOffset: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            riscvElfSize: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            appVersion: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            manifestOffset: u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            manifestSize: u32::from_le_bytes(bytes[36..40].try_into().unwrap()),
            monitorDataOffset: u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            monitorDataSize: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
            monitorCodeOffset: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
            monitorCodeSize: u32::from_le_bytes(bytes[52..56].try_into().unwrap()),
            bIsMonitorEnabled: 0,
            swbromCodeOffset: 0,
            swbromCodeSize: 0,
            swbromDataOffset: 0,
            swbromDataSize: 0,
            fbReservedSize: 0,
            bSignedAsCode: 0,
        }
    }
}

pub(crate) struct RiscvFw {
    pub fw: NvkmFirmware,
    pub code_offset: u32,
    pub data_offset: u32,
    pub manifest_offset: u32,
    pub app_version: u32,
}

impl RiscvFw {
    pub(crate) fn new_from_fw(dev: &device::Device, fw: &Firmware, name: &'static str) -> Result<Self> {
        let bl_bin_hdr = BinHdr::from_fw(fw);
        let riscv_desc : RmRiscvUCodeDesc = RmRiscvUCodeDesc::from_bytes(&fw.data()[bl_bin_hdr.header_offset as usize..]);

        let dma = DmaObject::new_from_data(dev,
                                           &fw.data()[(bl_bin_hdr.data_offset as usize)..(bl_bin_hdr.data_offset as usize + bl_bin_hdr.data_size as usize)], name)?;

        Ok(RiscvFw {
            fw: NvkmFirmware::new(name, dma),
            code_offset: riscv_desc.monitorCodeOffset,
            data_offset: riscv_desc.monitorDataOffset,
            manifest_offset: riscv_desc.manifestOffset,
            app_version: riscv_desc.appVersion,
        })
    }
}
