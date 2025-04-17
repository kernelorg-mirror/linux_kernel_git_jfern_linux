// SPDX-License-Identifier: GPL-2.0

//! Support for loading High Secure firmware for the SEC2 falcon, the most notable of which  are
//! the Booter load and unload firmwares which are provided by the [`kernel::firmware`] interface
//! and load or unload the GSP RM.

use kernel::bindings;
use kernel::device;
use kernel::devres::Devres;
use kernel::firmware::Firmware;
use kernel::prelude::*;

use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::falcon::sec2::Sec2;
use crate::falcon::{Falcon, FalconBromParams, FalconFirmware, FalconLoadTarget};
use crate::firmware::{BinHdr, HsHeaderV2, HsLoadHeaderV2, HsLoadHeaderV2App};

pub(crate) struct Sec2Firmware {
    load_hdr: HsLoadHeaderV2,
    app0: HsLoadHeaderV2App,
    pkc_data_offset: u32,
    engine_id_mask: u16,
    ucode_id: u8,
    ucode: DmaObject,
}

fn read_u32(src: &[u8], offset: usize) -> Result<u32> {
    src.get(offset..offset + size_of::<u32>())
        .ok_or(EINVAL)
        .map(|i| u32::from_le_bytes([i[0], i[1], i[2], i[3]]))
}

impl Sec2Firmware {
    pub(crate) fn new(
        falcon: &Falcon<Sec2>,
        dev: &device::Device<device::Bound>,
        bar: &Devres<Bar0>,
        fw: &Firmware,
    ) -> Result<Self> {
        let hdr = fw
            .data()
            .get(0..size_of::<BinHdr>())
            .ok_or(EINVAL)
            .and_then(BinHdr::from_bytes)?;

        pr_info!("sec2 hdr: {:?}\n", hdr);

        if hdr.bin_magic != 0x10de {
            pr_err!("invalid sec2 firmware header\n");
            return Err(EINVAL);
        }

        // Object containing the firmware to be signature-patched.
        let mut ucode = {
            let fw_start = hdr.data_offset as usize;
            let fw_size = hdr.data_size as usize;

            DmaObject::from_data(
                dev,
                fw.data().get(fw_start..fw_start + fw_size).ok_or(EINVAL)?,
            )?
        };

        let hs_hdr = {
            let offset = hdr.header_offset as usize;

            fw.data()
                .get(offset..offset + size_of::<HsHeaderV2>())
                .ok_or(EINVAL)
                .and_then(HsHeaderV2::from_bytes)?
        };

        pr_info!("sec2 hs hdr: {:?}\n", hs_hdr);

        let patch_loc = read_u32(fw.data(), hs_hdr.patch_loc as usize)?;
        let patch_sig = read_u32(fw.data(), hs_hdr.patch_sig as usize)?;
        let num_sig = read_u32(fw.data(), hs_hdr.num_sig as usize)?;

        let meta_offset = hs_hdr.meta_data_offset as usize;
        let fuse_ver = read_u32(fw.data(), meta_offset)?;
        let engine_id_mask = read_u32(fw.data(), meta_offset + size_of::<u32>())? as u16;
        let ucode_id = read_u32(fw.data(), meta_offset + 2 * size_of::<u32>())? as u8;

        let load_hdr = {
            let offset = hs_hdr.header_offset as usize;

            fw.data()
                .get(offset..offset + size_of::<HsLoadHeaderV2>())
                .ok_or(EINVAL)
                .and_then(HsLoadHeaderV2::from_bytes)?
        };

        pr_info!("load header: {:?}\n", load_hdr);

        let app0 = {
            let offset = hs_hdr.header_offset as usize + size_of::<HsLoadHeaderV2>();

            fw.data()
                .get(offset..offset + size_of::<HsLoadHeaderV2App>())
                .ok_or(EINVAL)
                .and_then(HsLoadHeaderV2App::from_bytes)?
        };

        pr_info!("app header: {:?}\n", app0);

        pr_info!(
            "loc {} sig {} num_sig {} fuse {} engine {} ucode {}\n",
            patch_loc,
            patch_sig,
            num_sig,
            fuse_ver,
            engine_id_mask,
            ucode_id
        );

        let reg_fuse_version =
            falcon.get_signature_reg_fuse_version(bar, engine_id_mask, ucode_id)?;
        if fuse_ver < reg_fuse_version {
            pr_info!("invalid fuse version\n");
            return Err(EINVAL);
        }

        if num_sig != 0 {
            let idx = if reg_fuse_version != 0 {
                fuse_ver - reg_fuse_version
            } else {
                num_sig - 1
            };

            pr_info!("reg_fuse {} idx {}\n", reg_fuse_version, idx);

            let sig_size = (hs_hdr.sig_prod_size / num_sig) as usize;
            let signatures_start = (hs_hdr.sig_prod_offset + patch_sig) as usize;
            let signatures = &fw
                .data()
                .get(signatures_start..signatures_start + hs_hdr.sig_prod_size as usize)
                .ok_or(EINVAL)?;
            let signature_start = idx as usize * sig_size;
            let signature = signatures
                .get(signature_start..signature_start + sig_size)
                .ok_or(EINVAL)?;
            super::patch_signature(&mut ucode, signature, patch_loc as usize)?;

            pr_info!(
                "signatures_start {} sig_prod_size {} signature_start {} sig_size {}\n",
                signatures_start,
                hs_hdr.sig_prod_size,
                signature_start,
                sig_size
            );
        }

        let pkc_data_offset = patch_loc - load_hdr.os_data_offset;
        Ok(Self {
            load_hdr,
            app0,
            pkc_data_offset,
            engine_id_mask,
            ucode_id,
            ucode,
        })
    }
}

impl FalconFirmware for Sec2Firmware {
    type Target = Sec2;

    fn dma_handle(&self) -> bindings::dma_addr_t {
        self.ucode.dma_handle()
    }

    fn imem_load(&self) -> FalconLoadTarget {
        FalconLoadTarget {
            src_start: self.app0.offset,
            dst_start: 0,
            len: self.app0.len,
        }
    }

    fn dmem_load(&self) -> FalconLoadTarget {
        FalconLoadTarget {
            src_start: self.load_hdr.os_data_offset,
            dst_start: 0,
            len: self.load_hdr.os_data_size,
        }
    }

    fn brom_params(&self) -> FalconBromParams {
        FalconBromParams {
            pkc_data_offset: self.pkc_data_offset,
            engine_id_mask: self.engine_id_mask,
            ucode_id: self.ucode_id,
        }
    }

    fn boot_addr(&self) -> u32 {
        self.app0.offset
    }
}
