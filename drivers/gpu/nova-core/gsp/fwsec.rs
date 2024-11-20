#![allow(dead_code)]

use kernel::prelude::*;
use crate::gpu::GpuBase;
use crate::falcon::FalconFw;
use crate::falcon::FalconFwInfo;
use crate::falcon::FalconFwSign;
use crate::firmware::NvkmFirmware;
use crate::firmware::BLFirmware;
use crate::gsp::gsp_falcon::GspFalconFw;
use crate::gsp::gsp_falcon::GspFalcon;
use crate::align;
use crate::dma::DmaObject;

#[allow(non_snake_case)]
#[allow(dead_code)]

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct FalconUCodeDescV2 {
    pub(crate) Hdr: u32,
    StoredSize: u32,
    UncompressedSize: u32,
    VirtualEntry: u32,
    InterfaceOffset: u32,
    IMEMPhysBase: u32,
    IMEMLoadSize: u32,
    IMEMVirtBase: u32,
    IMEMSecBase: u32,
    IMEMSecSize: u32,
    DMEMOffset: u32,
    DMEMPhysBase: u32,
    DMEMLoadSize: u32,
    altIMEMLoadSize: u32,
    altDMEMLoadSize: u32,
}

#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct FalconUCodeDescV3 {
    Hdr: u32,
    StoredSize: u32,
    PKCDataOffset: u32,
    InterfaceOffset: u32,
    IMEMPhysBase: u32,
    IMEMLoadSize: u32,
    IMEMVirtBase: u32,
    DMEMPhysBase: u32,
    DMEMLoadSize: u32,
    EngineIdMask: u16,
    UcodeId: u8,
    SignatureCount: u8,
    SignatureVersions: u16,
    Reserved: u16,
}

#[repr(C)]
pub(crate) union FalconUCodeDesc {
    pub(crate) v2: FalconUCodeDescV2,
    pub(crate) v3: FalconUCodeDescV3,
}

const NVFW_FALCON_APPIF_ID_DMEMMAPPER: u32 = 0x4;

#[repr(C)]
#[derive(Debug)]
struct nvfw_falcon_appif_v1 {
    id: u32,
    dmem_base: u32,
}

pub(crate) const NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS: u32 = 0x15;
pub(crate) const NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB: u32 = 0x19;

#[repr(C)]
#[derive(Debug)]
struct nvfw_falcon_appif_dmemmapper_v3 {
    signature: u32,
    version: u16,
    size: u16,
    cmd_in_buffer_offset: u32,
    cmd_in_buffer_size: u32,
    cmd_out_buffer_offset: u32,
    cmd_out_buffer_size: u32,
    nvf_img_data_buffer_offset: u32,
    nvf_img_data_buffer_size: u32,
    printf_buffer_hdr: u32,
    ucode_build_time_stamp: u32,
    ucode_signature: u32,
    init_cmd: u32,
    ucode_feature: u32,
    ucode_cmd_mask0: u32,
    ucode_cmd_mask1: u32,
    multi_tgt_tbl: u32,
}

#[repr(C)]
struct read_vbios {
    ver: u32,
    hdr: u32,
    addr: u64,
    size: u32,
    flags: u32,
}

#[repr(C)]
struct frts_region {
    ver: u32,
    hdr: u32,
    addr: u32,
    size: u32,
    ftype: u32,
}

const NVFW_FRTS_CMD_REGION_TYPE_FB: u32 = 2;
#[repr(C)]
pub(crate) struct FwsecFrtsCmd {
    read_vbios: read_vbios,
    frts_region: frts_region,
}

#[repr(C)]
#[derive(Debug)]
struct nvfw_falcon_appif_hdr_v1 {
    ver: u8,
    hdr: u8,
    len: u8,
    cnt: u8,
}

pub(crate) struct Fwsec {
    pub fw: GspFalconFw,
    init_cmd: u32,
}

impl Fwsec {

    pub(crate) fn patch(fw: &mut FalconFw, if_offset: u32, init_cmd: u32, frts_addr: u64, frts_size: u64) -> Result<()> {
        unsafe {
            let hdr: *const nvfw_falcon_appif_hdr_v1 = fw.fw.dma.dma.start_ptr().offset((fw.info.dmem_base_img + if_offset) as isize) as *const nvfw_falcon_appif_hdr_v1;

            let dmem: *mut u8 = fw.fw.dma.dma.start_ptr_mut().offset(fw.info.dmem_base_img as isize);
            if (*hdr).ver != 1 {
                return Err(EINVAL);
            }

            for i in 0..(*hdr).cnt {
                let app: *const nvfw_falcon_appif_v1 = (hdr as *const u8).offset(((*hdr).hdr + i * (*hdr).len) as isize) as *const nvfw_falcon_appif_v1;

                if (*app).id != NVFW_FALCON_APPIF_ID_DMEMMAPPER {
                    continue;
                }

                let dmemmap: *mut nvfw_falcon_appif_dmemmapper_v3 = dmem.offset((*app).dmem_base as isize) as *mut nvfw_falcon_appif_dmemmapper_v3;
                (*dmemmap).init_cmd = init_cmd;

                let frtscmd: *mut FwsecFrtsCmd = dmem.offset((*dmemmap).cmd_in_buffer_offset as isize) as *mut FwsecFrtsCmd;
                (*frtscmd).read_vbios.ver = 1;
                (*frtscmd).read_vbios.hdr = core::mem::size_of::<read_vbios>() as u32;
                (*frtscmd).read_vbios.addr = 0;
                (*frtscmd).read_vbios.size = 0;
                (*frtscmd).read_vbios.flags = 2;

                if init_cmd == NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS {
                    (*frtscmd).frts_region.ver = 1;
                    (*frtscmd).frts_region.hdr = core::mem::size_of::<frts_region>() as u32;
                    (*frtscmd).frts_region.addr = (frts_addr >> 12) as u32;
                    (*frtscmd).frts_region.size = (frts_size >> 12) as u32;
                    (*frtscmd).frts_region.ftype = NVFW_FRTS_CMD_REGION_TYPE_FB;
                }
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn fill_falcon_fw_info_v2(fwinfo: &mut FalconFwInfo, ucode_desc: *const FalconUCodeDesc) {
        unsafe {
            fwinfo.nmem_base = (*ucode_desc).v2.IMEMPhysBase;
            fwinfo.nmem_size = (*ucode_desc).v2.IMEMLoadSize - (*ucode_desc).v2.IMEMSecSize;
            fwinfo.imem_base = (*ucode_desc).v2.IMEMSecBase;
            fwinfo.imem_size = (*ucode_desc).v2.IMEMSecSize;
            fwinfo.dmem_base_img = (*ucode_desc).v2.DMEMOffset;
            fwinfo.dmem_base = (*ucode_desc).v2.DMEMPhysBase;
            fwinfo.dmem_size = (*ucode_desc).v2.DMEMLoadSize;
        }
    }

    pub(crate) fn fill_falcon_fw_info_v3(fwinfo: &mut FalconFwInfo, ucode_desc: *const FalconUCodeDesc) {
        unsafe {
            fwinfo.imem_base = (*ucode_desc).v3.IMEMPhysBase;
            fwinfo.imem_size = (*ucode_desc).v3.IMEMLoadSize;
            fwinfo.dmem_base_img = (*ucode_desc).v3.IMEMLoadSize;
            fwinfo.dmem_base = (*ucode_desc).v3.DMEMPhysBase;
            fwinfo.dmem_size = align((*ucode_desc).v3.DMEMLoadSize as usize, 256) as u32;
            fwinfo.dmem_sign = (*ucode_desc).v3.PKCDataOffset;
            fwinfo.fuse_ver = (*ucode_desc).v3.SignatureVersions as u32;
            fwinfo.ucode_id = (*ucode_desc).v3.UcodeId as u32;
            fwinfo.engine_id = (*ucode_desc).v3.EngineIdMask as u32;
        }
    }

    pub(crate) fn new_from_bios(gpu_base: &GpuBase, gsp_falcon: &GspFalcon, init_cmd: u32, frts_addr: u64, frts_size: u64, bl: &Option<BLFirmware>) -> Result<Self>
    {
        let offset = gpu_base.bios.find_fwsec_offset()?;
        let ucode_ptr = gpu_base.bios.ptr(offset as isize);

        let mut fw: GspFalconFw;
        let mut fwinfo : FalconFwInfo = Default::default();
        let ioffset: u32;
        let sigs: FalconFwSign;
        let ucode_data_start: usize;
        let ucode_len : usize;

        let ucodedesc: *const FalconUCodeDesc = ucode_ptr as *const FalconUCodeDesc;

        unsafe {
            let offset_ptr = gpu_base.bios.ptr(offset as isize).offset_from(gpu_base.bios.ptr(0));
            let vers = ((*ucodedesc).v2.Hdr & 0xff00) >> 8;
            let size = ((*ucodedesc).v2.Hdr & 0xffff0000) >> 16;
            ucode_data_start = (offset_ptr + size as isize) as usize;

            match vers {
                2 => {
                    ucode_len = ((*ucodedesc).v2.IMEMLoadSize + (*ucodedesc).v2.DMEMLoadSize) as usize;
                    sigs = Default::default();
                    Self::fill_falcon_fw_info_v2(&mut fwinfo, ucodedesc);
                    ioffset = (*ucodedesc).v2.InterfaceOffset;

                    let bl = bl.as_ref().unwrap();
                    fwinfo.boot_addr = bl.boot_addr;
                    fwinfo.boot_size = bl.boot_size;
                    fwinfo.boot.reserve(bl.boot_size as usize, GFP_KERNEL)?;
                    core::ptr::copy_nonoverlapping(bl.fw.data().as_ptr().byte_offset(bl.offset as isize), fwinfo.boot.as_mut_ptr(), fwinfo.boot_size as usize);
                    fwinfo.boot.set_len(fwinfo.boot_size as usize);
                }
                3 => {
                    ucode_len = ((*ucodedesc).v3.IMEMLoadSize + (*ucodedesc).v3.DMEMLoadSize) as usize;
                    sigs = FalconFwSign::new(((*ucodedesc).v3.IMEMLoadSize + (*ucodedesc).v3.PKCDataOffset) as usize,
                                             96 * 4,
                                             (*ucodedesc).v3.SignatureCount as usize,
                                             (offset_ptr + 0x2c) as usize,
                                             &gpu_base.bios.bios_vec)?;
                    Self::fill_falcon_fw_info_v3(&mut fwinfo, ucodedesc);
                    ioffset = (*ucodedesc).v3.InterfaceOffset;
                }
                _ => { panic!(); }
            }
        }
        let nv_fw = NvkmFirmware::new("fwsec", DmaObject::new_from_data(&gpu_base.dev,
                                                                        gpu_base.bios.get_range(ucode_data_start..ucode_data_start + ucode_len).unwrap(), "fwsec")?);
        fw = GspFalconFw::new(FalconFw::new_from_info(nv_fw, gsp_falcon.falcon.clone(),
                                                      sigs, fwinfo))?;
        Fwsec::patch(&mut fw.fw, ioffset, init_cmd, frts_addr, frts_size)?;

        Ok(Fwsec {
            fw,
            init_cmd,
        })
    }

    fn verify_frts(&self) -> Result<()> {
        let bar = self.fw.fw.falcon.base.bar.try_access().ok_or(ENXIO)?;
        /* verify */
        let val = bar.readl(0x001400 + (0xe * 4)) >> 16;
        if val != 0 {
            pr_info!("FAILED TO VERIFY FRTS {:#x}\n", val);
            return Err(EINVAL);
        }
        pr_info!("fwsec-frts: WPR2 @ {:#x} - {:#x}", bar.readl(0x1fa824), bar.readl(0x1fa828));
        Ok(())
    }

    fn verify_sb(&self) -> Result<()> {
        let bar = self.fw.fw.falcon.base.bar.try_access().ok_or(ENXIO)?;
        /* verify */
        let val = bar.readl(0x001400 + (0x15 * 4)) & 0x0000ffff;
        if val != 0 {
            pr_info!("FAILED TO VERIFY SB {}", val);
            return Err(EINVAL);
        }
        Ok(())
    }

    pub(crate) fn boot(&mut self) -> Result<()> {
        let mbox0 : u32 = 0;
        pr_info!("booting fwsec");
        self.fw.boot(mbox0, None)?;

        if self.init_cmd == NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS {
            self.verify_frts()
        } else {
            self.verify_sb()
        }
    }
}
