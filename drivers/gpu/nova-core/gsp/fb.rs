// SPDX-License-Identifier: GPL-2.0

use core::ops::Range;

use kernel::devres::Devres;
use kernel::prelude::*;

use crate::driver::Bar0;
use crate::gpu::Chipset;
use crate::regs;

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// Layout of the GPU framebuffer memory.
///
/// Contains ranges of GPU memory reserved for a given purpose during the GSP bootup process.
#[derive(Debug)]
#[expect(dead_code)]
pub(crate) struct FbLayout {
    pub fb: Range<u64>,

    pub vga_workspace: Range<u64>,
    pub bios: Range<u64>,

    pub frts: Range<u64>,
}

impl FbLayout {
    pub(crate) fn new(chipset: Chipset, bar: &Devres<Bar0>) -> Result<Self> {
        let fb = {
            let fb_size = with_bar!(bar, |b| vidmem_size(b, chipset))?;

            0..fb_size
        };
        let fb_len = fb.end - fb.start;

        let vga_workspace = {
            let vga_base = with_bar!(bar, |b| vga_workspace_addr(b, fb_len, chipset,))?;

            vga_base..fb.end
        };

        let bios = vga_workspace.clone();

        let frts = {
            const FRTS_DOWN_ALIGN: u64 = 0x20000;
            const FRTS_SIZE: u64 = 0x100000;
            let frts_base = align_down(vga_workspace.start, FRTS_DOWN_ALIGN) - FRTS_SIZE;

            frts_base..frts_base + FRTS_SIZE
        };

        Ok(Self {
            fb,
            vga_workspace,
            bios,
            frts,
        })
    }
}

/// Returns `true` if the display is disabled.
fn display_disabled(bar: &Bar0, chipset: Chipset) -> bool {
    if chipset >= Chipset::GA100 {
        regs::NV_FUSE_STATUS_OPT_DISPLAY_MAXWELL::read(bar).display_disabled()
    } else {
        regs::NV_FUSE_STATUS_OPT_DISPLAY_AMPERE::read(bar).display_disabled()
    }
}

/// Returns the video memory size in bytes.
fn vidmem_size(bar: &Bar0, chipset: Chipset) -> u64 {
    if chipset >= Chipset::GA102 {
        (regs::NV_PGC6_AON_SECURE_SCRATCH_GROUP_42::read(bar).value() as u64) << 20
    } else {
        let local_mem_range = regs::NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE::read(bar);
        let size =
            (local_mem_range.lower_mag() as u64) << ((local_mem_range.lower_scale() as u64) + 20);

        if local_mem_range.ecc_mode_enabled() {
            size / 16 * 15
        } else {
            size
        }
    }
}

/// Returns the vga workspace address.
fn vga_workspace_addr(bar: &Bar0, fb_size: u64, chipset: Chipset) -> u64 {
    let base = fb_size - 0x100000;
    let vga_workspace_base = if display_disabled(bar, chipset) {
        regs::NV_PDISP_VGA_WORKSPACE_BASE::read(bar)
    } else {
        return base;
    };

    if !vga_workspace_base.status_valid() {
        return base;
    }

    let addr = (vga_workspace_base.addr() as u64) << 16;
    if addr < base {
        fb_size - 0x20000
    } else {
        addr
    }
}
