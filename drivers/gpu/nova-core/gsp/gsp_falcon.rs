#![allow(dead_code)]

use crate::gpu::GpuBase;
use crate::gpu::Chipset;
use crate::falcon::{Falcon, FalconFw};
use crate::chipsets_after;
use kernel::prelude::*;
use kernel::sync::Arc;

pub(crate) struct GspFalcon {
    pub falcon: Arc<Falcon>,
    libos_addr: u64,
    app_version: u32,
}

pub(crate) struct GspFalconFw {
    pub fw: FalconFw
}

impl GspFalcon {
    pub(crate) fn new(gpu_base: &Arc<GpuBase>) -> Result<Self> {
        let riscv_irqmask: u32 = if chipsets_after!(&gpu_base.spec.chipset, GA102) {
            0x528
        } else {
            0x2b4
        };

        Ok(Self {
            falcon: Arc::new(Falcon::new(gpu_base,
                                         0x110000, 0x1000, 0x0, riscv_irqmask, false)?, GFP_KERNEL)?,
            libos_addr: 0,
            app_version: 0,
        })
    }

    pub(crate) fn reset(&self) -> Result<()> {
        self.falcon.reset_eng()?;

        if chipsets_after!(&self.falcon.base.spec.chipset, GA102) {
            self.falcon.mask(0x1668, 0x00000111, 0x00000111)?;
        }
        Ok(())
    }

    pub(crate) fn set_libos_addr(&mut self, addr: u64) {
        self.libos_addr = addr;
    }

    pub(crate) fn set_app_version(&mut self, version: u32) {
        self.app_version = version;
    }

    pub(crate) fn write_app_version(&self) -> Result<()> {
        self.falcon.wr32(0x80, self.app_version)
    }

    pub(crate) fn write_libos_addr(&self) -> Result<()> {
        self.falcon.wr32(0x40, (self.libos_addr & 0xffffffff) as u32)?;
        self.falcon.wr32(0x44, (self.libos_addr >> 32) as u32)
    }

    pub(crate) fn cmdq_push(&self) -> Result<()> {
        self.falcon.wr32(0xc00, 0x00000000)
    }
}

impl GspFalconFw {
    fn ga102_fw_signature(fw: &FalconFw) -> Result<u32> {
        let bar = fw.falcon.base.bar.try_access().ok_or(ENXIO)?;
        let mut sig_fuse_version = fw.info.fuse_ver;
        let mut reg_fuse_version : u32 = 0;
        let mut idx : u32 = 0;

        pr_info!("brom: {:#x} {:#x}", fw.info.engine_id, fw.info.ucode_id);
        pr_info!("sig_fuse_version: {}", sig_fuse_version);

        if fw.info.engine_id & 0x00000400 != 0 {
            reg_fuse_version = bar.try_readl((0x8241c0 + (fw.info.ucode_id - 1) * 4) as usize)?;
        }
        // WARN_ON
        reg_fuse_version = 1 << (32 - reg_fuse_version.leading_zeros());
        pr_info!("reg_fuse_version: {:#x}", reg_fuse_version);
        if (reg_fuse_version & fw.info.fuse_ver) == 0 {
            return Err(EINVAL);
        }

        while (reg_fuse_version & sig_fuse_version & 1) == 0 {
            idx += sig_fuse_version & 1;
            reg_fuse_version >>= 1;
            sig_fuse_version >>= 1;
        }
        Ok(idx)
    }

    fn signature(fw: &FalconFw, sig_base_src: &mut u32) -> Result<u32> {
        if chipsets_after!(&fw.falcon.base.spec.chipset, GA102) {
            Self::ga102_fw_signature(fw)
        } else {
            fw.tu102_fw_signature(sig_base_src)
        }
    }

    pub(crate) fn boot(&mut self, mbox0: u32, mbox1: Option<u32>) -> Result<()> {
        self.fw.boot(mbox0, mbox1)
    }

    pub(crate) fn new(mut fw: FalconFw) -> Result<Self> {

        let mut sig_base_src : u32 = fw.sigs.sig_base_prd as u32;
        let idx = Self::signature(&mut fw, &mut sig_base_src)?;

        if fw.sigs.sig_nr_prd != 0 {
            fw.patch(idx, sig_base_src)?;
        }
        Ok(Self {
            fw
        })
    }
}
