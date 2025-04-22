// SPDX-License-Identifier: GPL-2.0

//! Methods for device initialization.
//!
//! A clarification about devinit terminology:
//! devinit is a sequence of register read/writes after reset that performs tasks
//! such as:
//! 1. Programming VRAM memory controller timings.
//! 2. Power sequencing.
//! 3. Clock and PLL configuration.
//! 4. Thermal management.
//!
//! devinit itself is a 'script' which is interpreted by the PMU microcontroller of
//! the GPU by an interpreter program.
//!
//! Note that the devinit sequence also needs to run during suspend/resume at runtime.

use kernel::bindings;
use kernel::devres::Devres;
use kernel::prelude::*;

use crate::driver::Bar0;
use crate::regs;

/// Wait for gfw (GPU firmware) boot completion signal (GFW_BOOT).
///
/// Upon reset, several microcontrollers (such as PMU, SEC2, GSP etc) on the GPU run some GPU
/// firmware (gfw) code to setup its core parameters. Most of the GPU is considered unusable until
/// this step is completed, so it must be waited on very early during driver initialization.
///
/// The GPU firmware (gfw) code includes several components that execute before the driver loads.
/// These components are located in the VBIOS ROM and are executed in a sequence on these different
/// microcontrollers. The devinit sequence itself runs on the PMU, and the FWSEC runs on the GSP.
///
/// This function specifically waits for a signal indicating core initialization is complete before
/// which not much can be done. This signal is setup by the FWSEC running on the GSP in Heavy-secured
/// mode.
pub(crate) fn wait_gfw_boot_completion(bar: &Devres<Bar0>) -> Result<()> {
    let mut timeout = 2000;

    loop {
        // Before accessing the completion status in [`crate::regs::NV_PGC6_AON_SECURE_SCRATCH_GROUP_05`],
        // we must first check [`crate::regs::NV_PGC6_AON_SECURE_SCRATCH_GROUP_05_PRIV_LEVEL_MASK`].
        // This is because the register is accessible only after secure firmware (FWSEC) lowers the
        // privilege level to allow CPU (LS/Light-secured) access. We can only safely read the status
        // register from CPU (LS/Light-secured) once mask indicates privilege level has been lowered.
        let gfw_booted =
            with_bar!(
                bar,
                |b| regs::NV_PGC6_AON_SECURE_SCRATCH_GROUP_05_PRIV_LEVEL_MASK::read(b)
                    .read_protection_level0()
                    && (regs::NV_PGC6_AON_SECURE_SCRATCH_GROUP_05::read(b).value() & 0xff) == 0xff
            )?;

        if gfw_booted {
            return Ok(());
        }

        if timeout == 0 {
            return Err(ETIMEDOUT);
        }
        timeout -= 1;

        // TODO: use `read_poll_timeout` once it is available.
        // (https://lore.kernel.org/lkml/20250220070611.214262-8-fujita.tomonori@gmail.com/)
        // SAFETY: msleep should be safe to call with any parameter.
        unsafe { bindings::msleep(2) };
    }
}
