#![allow(dead_code)]

pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::sync::Arc;

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

        let mgr = GspManager::ver {
            gpu_base,
            fw,
        };

        let mgr = Arc::new(mgr, GFP_KERNEL)?;
        Ok(mgr)
    }
}
