// SPDX-License-Identifier: GPL-2.0

use crate::{
    driver::Bar0,
    falcon::{Falcon, FalconEngine, PFalcon2Base, PFalconBase},
    regs::{self, macros::RegisterBase},
};

/// Type specifying the `Gsp` falcon engine. Cannot be instantiated.
pub(crate) struct Gsp(());

impl RegisterBase<PFalconBase> for Gsp {
    const BASE: usize = 0x00110000;
}

impl RegisterBase<PFalcon2Base> for Gsp {
    const BASE: usize = 0x00111000;
}

impl FalconEngine for Gsp {
    const ID: Self = Gsp(());
}

impl Falcon<Gsp> {
    /// Enable the GSP Falcon message queue interrupt (SWGEN0 interrupt).
    #[expect(dead_code)]
    pub(crate) fn enable_msgq_interrupt(&self, bar: &Bar0) {
        regs::NV_PFALCON_FALCON_IRQMASK::alter(bar, &Gsp::ID, |r| r.set_swgen0(true));
    }

    /// Check if the message queue interrupt is pending.
    #[expect(dead_code)]
    pub(crate) fn has_msgq_interrupt(&self, bar: &Bar0) -> bool {
        regs::NV_PFALCON_FALCON_IRQSTAT::read(bar, &Gsp::ID).swgen0()
    }

    /// Clears the message queue interrupt to allow GSP to signal CPU
    /// for processing new messages.
    pub(crate) fn clear_msgq_interrupt(&self, bar: &Bar0) {
        regs::NV_PFALCON_FALCON_IRQSCLR::default()
            .set_swgen0(true)
            .write(bar, &Gsp::ID);
    }

    /// Acknowledge all pending GSP interrupts.
    #[expect(dead_code)]
    pub(crate) fn ack_all_interrupts(&self, bar: &Bar0) {
        // Read status and write the raw value to IRQSCLR to clear all pending interrupts.
        let status = regs::NV_PFALCON_FALCON_IRQSTAT::read(bar, &Gsp::ID);
        regs::NV_PFALCON_FALCON_IRQSCLR::from(u32::from(status)).write(bar, &Gsp::ID);
    }
}
