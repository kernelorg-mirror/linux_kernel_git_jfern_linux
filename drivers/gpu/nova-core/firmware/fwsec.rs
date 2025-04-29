// SPDX-License-Identifier: GPL-2.0

//! FWSEC is a High Secure firmware that is extracted from the BIOS and performs the first step of
//! the GSP startup by creating the WPR2 memory region and copying critical areas of the VBIOS into
//! it after authenticating them, ensuring they haven't been tampered with. It runs on the GSP
//! falcon.
//!
//! Before being run, it needs to be patched in two areas:
//!
//! - The command to be run, as this firmware can perform several tasks ;
//! - The ucode signature, so the GSP falcon can run FWSEC in HS mode.

use core::alloc::Layout;

use kernel::bindings;
use kernel::device::{self, Device};
use kernel::devres::Devres;
use kernel::prelude::*;
use kernel::transmute::FromBytes;

use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::falcon::gsp::Gsp;
use crate::falcon::{Falcon, FalconBromParams, FalconFirmware, FalconLoadTarget};
use crate::firmware::FalconUCodeDescV3;
use crate::vbios::Vbios;

const NVFW_FALCON_APPIF_ID_DMEMMAPPER: u32 = 0x4;

#[repr(C)]
#[derive(Debug)]
struct FalconAppifHdrV1 {
    version: u8,
    header_size: u8,
    entry_size: u8,
    entry_count: u8,
}
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for FalconAppifHdrV1 {}

#[repr(C, packed)]
#[derive(Debug)]
struct FalconAppifV1 {
    id: u32,
    dmem_base: u32,
}
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for FalconAppifV1 {}

#[derive(Debug)]
#[repr(C, packed)]
struct FalconAppifDmemmapperV3 {
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
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for FalconAppifDmemmapperV3 {}

#[derive(Debug)]
#[repr(C, packed)]
struct ReadVbios {
    ver: u32,
    hdr: u32,
    addr: u64,
    size: u32,
    flags: u32,
}
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for ReadVbios {}

#[derive(Debug)]
#[repr(C, packed)]
struct FrtsRegion {
    ver: u32,
    hdr: u32,
    addr: u32,
    size: u32,
    ftype: u32,
}
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for FrtsRegion {}

const NVFW_FRTS_CMD_REGION_TYPE_FB: u32 = 2;

#[repr(C, packed)]
struct FrtsCmd {
    read_vbios: ReadVbios,
    frts_region: FrtsRegion,
}
// SAFETY: any byte sequence is valid for this struct.
unsafe impl FromBytes for FrtsCmd {}

const NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS: u32 = 0x15;
const NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB: u32 = 0x19;

/// Command for the [`FwsecFirmware`] to execute.
pub(crate) enum FwsecCommand {
    /// Asks [`FwsecFirmware`] to carve out the WPR2 area and place a verified copy of the VBIOS
    /// image into it.
    Frts { frts_addr: u64, frts_size: u64 },
    /// Asks [`FwsecFirmware`] to load pre-OS apps on the PMU.
    #[expect(dead_code)]
    Sb,
}

/// Reinterpret the area starting from `offset` in `fw` as an instance of `T` (which must implement
/// [`FromBytes`]) and return a reference to it.
///
/// # Safety
///
/// Callers must ensure that the region of memory returned is not written for as long as the
/// returned reference is alive.
///
/// TODO: Remove this and `transmute_mut` once we have a way to transmute objects implementing
/// FromBytes, e.g.:
/// https://lore.kernel.org/lkml/20250330234039.29814-1-christiansantoslima21@gmail.com/
unsafe fn transmute<'a, 'b, T: Sized + FromBytes>(
    fw: &'a DmaObject,
    offset: usize,
) -> Result<&'b T> {
    if offset + core::mem::size_of::<T>() > fw.size() {
        return Err(EINVAL);
    }
    if (fw.start_ptr() as usize + offset) % core::mem::align_of::<T>() != 0 {
        return Err(EINVAL);
    }

    // SAFETY: we have checked that the pointer is properly aligned that its pointed memory is
    // large enough the contains an instance of `T`, which implements `FromBytes`.
    Ok(unsafe { &*(fw.start_ptr().add(offset) as *const T) })
}

/// Reinterpret the area starting from `offset` in `fw` as a mutable instance of `T` (which must
/// implement [`FromBytes`]) and return a reference to it.
///
/// # Safety
///
/// Callers must ensure that the region of memory returned is not read or written for as long as
/// the returned reference is alive.
unsafe fn transmute_mut<'a, 'b, T: Sized + FromBytes>(
    fw: &'a mut DmaObject,
    offset: usize,
) -> Result<&'b mut T> {
    if offset + core::mem::size_of::<T>() > fw.size() {
        return Err(EINVAL);
    }
    if (fw.start_ptr_mut() as usize + offset) % core::mem::align_of::<T>() != 0 {
        return Err(EINVAL);
    }

    // SAFETY: we have checked that the pointer is properly aligned that its pointed memory is
    // large enough the contains an instance of `T`, which implements `FromBytes`.
    Ok(unsafe { &mut *(fw.start_ptr_mut().add(offset) as *mut T) })
}

/// Patch the Fwsec firmware image in `fw` to run the command `cmd`.
fn patch_command(fw: &mut DmaObject, v3_desc: &FalconUCodeDescV3, cmd: FwsecCommand) -> Result<()> {
    let hdr_offset = (v3_desc.imem_load_size + v3_desc.interface_offset) as usize;
    // SAFETY: we have an exclusive reference to `fw`, and no caller should have shared `fw` with
    // the hardware yet.
    let hdr: &FalconAppifHdrV1 = unsafe { transmute(fw, hdr_offset) }?;

    if hdr.version != 1 {
        return Err(EINVAL);
    }

    // Find the DMEM mapper section in the firmware.
    for i in 0..hdr.entry_count as usize {
        let app: &FalconAppifV1 =
            // SAFETY: we have an exclusive reference to `fw`, and no caller should have shared
            // `fw` with the hardware yet.
            unsafe {
                transmute(
                    fw,
                    hdr_offset + hdr.header_size as usize + i * hdr.entry_size as usize
                )
            }?;

        if app.id != NVFW_FALCON_APPIF_ID_DMEMMAPPER {
            continue;
        }

        let dmem_mapper: &mut FalconAppifDmemmapperV3 =
            // SAFETY: we have an exclusive reference to `fw`, and no caller should have shared
            // `fw` with the hardware yet.
            unsafe { transmute_mut(fw, (v3_desc.imem_load_size + app.dmem_base) as usize) }?;

        // SAFETY: we have an exclusive reference to `fw`, and no caller should have shared `fw`
        // with the hardware yet.
        let frts_cmd: &mut FrtsCmd = unsafe {
            transmute_mut(
                fw,
                (v3_desc.imem_load_size + dmem_mapper.cmd_in_buffer_offset) as usize,
            )
        }?;

        frts_cmd.read_vbios = ReadVbios {
            ver: 1,
            hdr: core::mem::size_of::<ReadVbios>() as u32,
            addr: 0,
            size: 0,
            flags: 2,
        };

        dmem_mapper.init_cmd = match cmd {
            FwsecCommand::Frts {
                frts_addr,
                frts_size,
            } => {
                frts_cmd.frts_region = FrtsRegion {
                    ver: 1,
                    hdr: core::mem::size_of::<FrtsRegion>() as u32,
                    addr: (frts_addr >> 12) as u32,
                    size: (frts_size >> 12) as u32,
                    ftype: NVFW_FRTS_CMD_REGION_TYPE_FB,
                };

                NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS
            }
            FwsecCommand::Sb => NVFW_FALCON_APPIF_DMEMMAPPER_CMD_SB,
        };

        // Return early as we found and patched the DMEMMAPPER region.
        return Ok(());
    }

    Err(ENOTSUPP)
}

/// Firmware extracted from the VBIOS and responsible for e.g. carving out the WPR2 region as the
/// first step of the GSP bootflow.
pub(crate) struct FwsecFirmware {
    desc: FalconUCodeDescV3,
    ucode: DmaObject,
}

impl FalconFirmware for FwsecFirmware {
    type Target = Gsp;

    fn dma_handle(&self) -> bindings::dma_addr_t {
        self.ucode.dma_handle()
    }

    fn imem_load(&self) -> FalconLoadTarget {
        FalconLoadTarget {
            src_start: 0,
            dst_start: self.desc.imem_phys_base,
            len: self.desc.imem_load_size,
        }
    }

    fn dmem_load(&self) -> FalconLoadTarget {
        FalconLoadTarget {
            src_start: self.desc.imem_load_size,
            dst_start: self.desc.dmem_phys_base,
            len: Layout::from_size_align(self.desc.dmem_load_size as usize, 256)
                // Cannot panic, as 256 is non-zero and a power of 2.
                .unwrap()
                .pad_to_align()
                .size() as u32,
        }
    }

    fn brom_params(&self) -> FalconBromParams {
        FalconBromParams {
            pkc_data_offset: self.desc.pkc_data_offset,
            engine_id_mask: self.desc.engine_id_mask,
            ucode_id: self.desc.ucode_id,
        }
    }

    fn boot_addr(&self) -> u32 {
        0
    }
}

impl FwsecFirmware {
    /// Extract the Fwsec firmware from `bios` and patch it to run with the `cmd` command.
    pub(crate) fn new(
        falcon: &Falcon<Gsp>,
        dev: &Device<device::Bound>,
        bar: &Devres<Bar0>,
        bios: &Vbios,
        cmd: FwsecCommand,
    ) -> Result<Self> {
        let v3_desc = bios.fwsec_header(dev)?;
        let ucode = bios.fwsec_ucode(dev)?;

        let mut ucode_dma = DmaObject::from_data(dev, ucode)?;
        patch_command(&mut ucode_dma, v3_desc, cmd)?;

        const SIG_SIZE: usize = 96 * 4;
        let signatures = bios.fwsec_sigs(dev)?;
        let sig_base_img = (v3_desc.imem_load_size + v3_desc.pkc_data_offset) as usize;

        if v3_desc.signature_count != 0 {
            // Patch signature.
            let desc_sig_versions = v3_desc.signature_versions as u32;
            let reg_fuse_version = falcon.get_signature_reg_fuse_version(
                bar,
                v3_desc.engine_id_mask,
                v3_desc.ucode_id,
            )?;
            dev_dbg!(
                dev,
                "desc_sig_versions: {:#x}, reg_fuse_version: {}\n",
                desc_sig_versions,
                reg_fuse_version
            );
            let signature_idx = {
                let reg_fuse_version_bit = 1 << reg_fuse_version;

                // Check if the fuse version is supported by the firmware.
                if desc_sig_versions & reg_fuse_version_bit == 0 {
                    dev_warn!(
                        dev,
                        "no matching signature: {:#x} {:#x}\n",
                        reg_fuse_version_bit,
                        v3_desc.signature_versions
                    );
                    return Err(EINVAL);
                }

                // `desc_sig_versions` has one bit set per included signature. Thus, the index of
                // the signature to patch is the number of bits in `desc_sig_versions` set to `1`
                // before `reg_fuse_version_bit`.

                // Mask of the bits of `desc_sig_versions` to preserve.
                let reg_fuse_version_mask = reg_fuse_version_bit.wrapping_sub(1);

                (desc_sig_versions & reg_fuse_version_mask).count_ones()
            };

            dev_dbg!(dev, "patching signature with index {}\n", signature_idx);
            let signature_start = signature_idx as usize * SIG_SIZE;
            let signature = &signatures[signature_start..signature_start + SIG_SIZE];
            super::patch_signature(&mut ucode_dma, signature, sig_base_img)?;
        }

        Ok(FwsecFirmware {
            desc: v3_desc.clone(),
            ucode: ucode_dma,
        })
    }
}
