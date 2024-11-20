#![allow(dead_code)]

pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::sync::Arc;

use crate::chipsets_before;
use crate::devinit;
use crate::dma::DmaObject;
use crate::gsp::fwsec::Fwsec;
use crate::gsp::fwsec::NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS;
use crate::gsp::gsp_falcon::GspFalcon;

use crate::gpu::Chipset;
use crate::gpu::FBInfo;
use crate::gpu::Firmware;
use crate::gpu::GpuBase;

mod boot_structs;
mod fwsec;
pub(crate) mod gsp_falcon;

const GSP_PAGE_SHIFT: u32 = 12;
pub(crate) const GSP_PAGE_SIZE: u32 = 1 << GSP_PAGE_SHIFT;
pub(crate) const GSP_HEAP_SHIFT: u64 = 1 << 20;

#[versions(GSP)]
pub(crate) struct GspManager {
    gpu_base: Arc<GpuBase>,
    sysmem_flush: DmaObject,
    fw: Firmware,
    fb_addr_info: FBInfo,
}

pub(crate) trait GspManager: Send + Sync {
}

#[versions(GSP)]
impl GspManager for GspManager::ver {
}

#[versions(GSP)]
impl GspManager::ver {

    pub(crate) fn new(gpu_base: Arc<GpuBase>,
                      gsp_falcon: GspFalcon,
                      fw: Firmware) -> Result<Arc<GspManager::ver>> {
        let display_disabled = devinit::check_display_disable(&gpu_base)?;
        let fb_size = devinit::vidmem_size(&gpu_base)?;
        let vga_base = devinit::vga_workspace_addr(&gpu_base, fb_size, display_disabled)?;
        let vga_size = fb_size - vga_base;

        let sysmem_flush = DmaObject::new_cleared(&gpu_base.dev, 0x1000, "sysmem flush page")?;

        let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;

        if chipsets_before!(&gpu_base.spec.chipset, GA102) {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
        } else {
            bar.writel((sysmem_flush.dma.dma_handle() >> 8) as u32, 0x100c10);
            bar.writel((sysmem_flush.dma.dma_handle() >> 40) as u32, 0x100c40);
        }

        let fb_addr_info = boot_structs::Wpr::ver::fill_fb_addr_info(
            &gpu_base, fb_size, vga_base, vga_size, &fw);

        let mut fwsec = Fwsec::new_from_bios(&gpu_base,
                                             &gsp_falcon,
                                             NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
                                             fb_addr_info.frts.addr,
                                             fb_addr_info.frts.size,
                                             &fw.bl_fw)?;

        fwsec.boot()?;

        let mgr = GspManager::ver {
            gpu_base,
            fw,
            fb_addr_info,
            sysmem_flush,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}
