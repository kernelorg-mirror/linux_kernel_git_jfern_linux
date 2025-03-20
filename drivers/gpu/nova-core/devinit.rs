// SPDX-License-Identifier: GPL-2.0

//! Methods for device initialization.

use kernel::error::Result;

use crate::{driver::Bar0, gpu::Chipset};

pub(crate) fn display_disabled(bar: &Bar0, chipset: Chipset) -> bool {
    let val = bar.readl(if chipset >= Chipset::GA100 {
        0x820c04
    } else {
        0x021c04
    });

    (val & 0x1) != 0
}

pub(crate) fn vidmem_size(bar: &Bar0, chipset: Chipset) -> Result<u64> {
    if chipset >= Chipset::GA102 {
        Ok((bar.readl(0x1183a4) as u64) << 20)
    } else {
        let data = bar.readl(0x100ce0);
        let lmag = (data & 0x3f0) >> 4;
        let lsca = data & 0xf;
        let size = (lmag as u64) << (lsca + 20);

        if (data & 0x40000000) != 0 {
            Ok(size / 16 * 15)
        } else {
            Ok(size)
        }
    }
}

pub(crate) fn vga_workspace_addr(bar: &Bar0, fb_size: u64, display_disabled: bool) -> u64 {
    let base = fb_size - 0x100000;
    let addr = if !display_disabled {
        bar.readl(0x625f04) as u64
    } else {
        0
    };

    if (addr & 0x00000008) == 0 {
        return base;
    }

    let addr = (addr & 0xffffff00) << 8;
    if addr < base {
        fb_size - 0x20000
    } else {
        addr
    }
}
