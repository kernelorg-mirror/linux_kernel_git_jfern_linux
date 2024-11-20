#![allow(dead_code)]

pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::sync::Arc;

use crate::devinit;
use crate::gpu::Firmware;
use crate::gpu::GpuBase;

pub(crate) mod gsp_falcon;
mod fwsec;


#[versions(GSP)]
pub(crate) struct GspManager {
    gpu_base: Arc<GpuBase>,
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

        let mgr = GspManager::ver {
            gpu_base,
            fw,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}
