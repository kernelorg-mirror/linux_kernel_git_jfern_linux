// SPDX-License-Identifier: GPL-2.0
use core::time::Duration;
use kernel::devres::Devres;
use kernel::prelude::*;

use crate::{
    driver::Bar0,
    falcon::{Falcon, FalconEngine},
    regs,
    util::wait_on,
};

pub(crate) struct Gsp;
impl FalconEngine for Gsp {
    const BASE: usize = 0x00110000;
}

impl Falcon<Gsp> {
    /// Clears the SWGEN0 bit in the Falcon's IRQ status clear register to
    /// allow GSP to signal CPU for processing new messages in message queue.
    pub(crate) fn clear_swgen0_intr(&self, bar: &Devres<Bar0>) -> Result<()> {
        with_bar!(bar, |b| regs::NV_PFALCON_FALCON_IRQSCLR::default()
            .set_swgen0(true)
            .write(b, Gsp::BASE))
    }

    /// Function to check if GSP reload/resume has completed during the boot process.
    pub(crate) fn check_reload_completed(
        &self,
        bar: &Devres<Bar0>,
        timeout: Duration,
    ) -> Result<bool> {
        wait_on(timeout, || {
            // Note that try_read32() is only available for fixed-offset registers.
            with_bar!(?bar, |b| {
                Ok(regs::NV_PGC6_BSI_SECURE_SCRATCH_14::read(b))
            }).ok().and_then(|val| {
                if val.boot_stage_3_handoff() {
                    Some(true)
                } else {
                    None
                }
            })
        })
    }
}
