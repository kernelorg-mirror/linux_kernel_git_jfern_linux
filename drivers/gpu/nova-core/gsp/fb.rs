// SPDX-License-Identifier: GPL-2.0

use core::ops::Range;

use kernel::devres::Devres;
use kernel::prelude::*;

use crate::driver::Bar0;
use crate::firmware::Firmware;
use crate::gpu::Chipset;
use crate::gsp::GSP_HEAP_SHIFT;
use crate::nvfw::r570_133_07 as nvfw;
use crate::regs;

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn calc_wpr_heap(chipset: Chipset, fb_size_fb: u64) -> u64 {
    let (carveout, heap_min) = if chipset >= Chipset::GA102 {
        (
            nvfw::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS3_BAREMETAL as u64,
            nvfw::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB << 20,
        )
    } else {
        (
            nvfw::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS2 as u64,
            nvfw::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS2_MIN_MB << 20,
        )
    };

    let size = carveout
        + nvfw::GSP_FW_HEAP_PARAM_BASE_RM_SIZE_TU10X as u64
        + (nvfw::GSP_FW_HEAP_PARAM_SIZE_PER_GB_FB as u64 * fb_size_fb)
            .next_multiple_of(GSP_HEAP_SHIFT)
        + (nvfw::GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE as u64).next_multiple_of(GSP_HEAP_SHIFT);

    core::cmp::max(size, heap_min as u64)
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
    pub boot: Range<u64>,
    pub elf: Range<u64>,
    pub wpr2_heap: Range<u64>,
    pub vf_partition_count: u8,
    pub wpr2: Range<u64>,

    pub heap: Range<u64>,
    pub region: [Range<u64>; 16],
    pub nr_region: usize,
    pub rsvd_size: u32,
}

impl FbLayout {
    pub(crate) fn new(chipset: Chipset, bar: &Devres<Bar0>, fw: &Firmware) -> Result<Self> {
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

        let boot = {
            const BOOTLOADER_DOWN_ALIGN: u64 = 0x1000;
            let bootloader_size = fw.bootloader.ucode.size() as u64;
            let bootloader_base = align_down(frts.start - bootloader_size, BOOTLOADER_DOWN_ALIGN);

            bootloader_base..bootloader_base + bootloader_size
        };

        let elf = {
            const ELF_DOWN_ALIGN: u64 = 0x10000;
            let elf_size = fw.gsp.size() as u64;
            let elf_addr = align_down(boot.start - elf_size, ELF_DOWN_ALIGN);

            elf_addr..elf_addr + elf_size
        };

        let fb_size_fb = fb_len.div_ceil(1 << 30);
        let wpr2_heap = {
            const WPR2_HEAP_DOWN_ALIGN: u64 = 0x100000;
            let wpr2_heap_size = calc_wpr_heap(chipset, fb_size_fb);
            let wpr2_heap_addr = align_down(elf.start - wpr2_heap_size, WPR2_HEAP_DOWN_ALIGN);

            wpr2_heap_addr..align_down(elf.start, WPR2_HEAP_DOWN_ALIGN)
        };

        let wpr2 = {
            const WPR2_DOWN_ALIGN: u64 = 0x100000;
            let wpr2_addr = align_down(
                wpr2_heap.start - nvfw::s_GspFwWprMeta::str_size() as u64,
                WPR2_DOWN_ALIGN,
            );

            wpr2_addr..frts.end
        };

        let heap = {
            const HEAP_SIZE: u64 = 0x100000;

            wpr2.start - HEAP_SIZE..wpr2.start
        };

        Ok(Self {
            fb,
            vga_workspace,
            bios,
            frts,
            boot,
            elf,
            wpr2_heap,
            wpr2,
            heap,
            vf_partition_count: 0,
            region: Default::default(),
            nr_region: 0,
            rsvd_size: 0,
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
