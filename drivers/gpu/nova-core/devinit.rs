// SPDX-License-Identifier: GPL-2.0

//! Methods for device initialization.

use kernel::bindings;
use kernel::devres::Devres;
use kernel::prelude::*;

use crate::driver::Bar0;
use crate::regs;

/// Wait for devinit FW completion.
///
/// Upon reset, the GPU runs some firmware code to setup its core parameters. Most of the GPU is
/// considered unusable until this step is completed, so it must be waited on very early during
/// driver initialization.
pub(crate) fn wait_gfw_boot_completion(bar: &Devres<Bar0>) -> Result<()> {
    let mut timeout = 2000;

    loop {
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
