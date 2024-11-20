#![allow(dead_code)]

pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::sync::Arc;

use crate::chipsets_before;
use crate::devinit;
use crate::dma::DmaObject;
use crate::gpu::Chipset;
use crate::gpu::Firmware;
use crate::gpu::GpuBase;

pub(crate) mod gsp_falcon;
mod fwsec;


#[versions(GSP)]
pub(crate) struct GspManager {
    gpu_base: Arc<GpuBase>,
    sysmem_flush: DmaObject,
    fw: Firmware,
}

pub(crate) trait GspManager: Send + Sync {
}

#[versions(GSP)]
impl GspManager for GspManager::ver {
}

#[versions(GSP)]
impl GspManager::ver {

    pub(crate) fn new(gpu_base: Arc<GpuBase>,
                      fw: Firmware) -> Result<Arc<GspManager::ver>> {
        let display_disabled = devinit::check_display_disable(&gpu_base)?;
        let fb_size = devinit::vidmem_size(&gpu_base)?;
        let vga_base = devinit::vga_workspace_addr(&gpu_base, fb_size, display_disabled)?;
        let _vga_size = fb_size - vga_base;

        let sysmem_flush = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "sysmem flush page")?;

        let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;

        if chipsets_before!(&gpu_base.spec.chipset, GA102) {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
        } else {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
            bar.writel((sysmem_flush.dma.dma_handle() >> 40) as u32, 0x100c40);
        }

        let mgr = GspManager::ver {
            gpu_base,
            fw,
            sysmem_flush,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}
