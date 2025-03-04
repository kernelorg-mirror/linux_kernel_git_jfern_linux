// SPDX-License-Identifier: GPL-2.0

use kernel::devres::Devres;
use kernel::prelude::*;
use kernel::sync::Arc;

use crate::driver::Bar0;
use crate::falcon::{FalconBromParams, FalconEngine};
use crate::gpu::Chipset;

mod ga102;

/// Hardware Abstraction Layer for Falcon cores.
///
/// Implements chipset-specific low-level operations. The trait is generic against [`FalconEngine`]
/// so its `BASE` parameter can be used in order to avoid runtime bound checks when accessing
/// registers.
pub(crate) trait FalconHal<E: FalconEngine>: Sync {
    // Activates the Falcon core if the engine is a risvc/falcon dual engine.
    fn select_core(&self, _bar: &Devres<Bar0>) -> Result<()> {
        Ok(())
    }

    /// Returns the fused version of the signature to use in order to run a HS firmware on this
    /// falcon instance. `engine_id_mask` and `ucode_id` are obtained from the firmware header.
    fn get_signature_reg_fuse_version(
        &self,
        bar: &Devres<Bar0>,
        engine_id_mask: u16,
        ucode_id: u8,
    ) -> Result<u32>;

    // Program the boot ROM registers prior to starting a secure firmware.
    fn program_brom(&self, bar: &Devres<Bar0>, params: &FalconBromParams) -> Result<()>;
}

/// Returns a boxed falcon HAL adequate for the passed `chipset`.
///
/// We use this function and a heap-allocated trait object instead of statically defined trait
/// objects because of the two-dimensional (Chipset, Engine) lookup required to return the
/// requested HAL.
///
/// TODO: replace the return type with `KBox` once it gains the ability to host trait objects.
pub(crate) fn create_falcon_hal<E: FalconEngine + 'static>(
    chipset: Chipset,
) -> Result<Arc<dyn FalconHal<E>>> {
    let hal = match chipset {
        Chipset::GA102 | Chipset::GA103 | Chipset::GA104 | Chipset::GA106 | Chipset::GA107 => {
            Arc::new(ga102::Ga102::<E>::new(), GFP_KERNEL)? as Arc<dyn FalconHal<E>>
        }
        _ => return Err(ENOTSUPP),
    };

    Ok(hal)
}
