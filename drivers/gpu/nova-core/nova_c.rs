// SPDX-License-Identifier: GPL-2.0

//! Nova GPU Driver

mod accel;
mod bar;
mod bios;
mod devinit;
mod dma;
mod driver;
mod firmware;
mod falcon;
mod gpu;
mod gsp;
mod mmu;
mod nvfw;
mod rm_riscv;
mod sec2;
mod timer;
mod vfn;

pub mod core_api;

/// VGPU MGR interface
#[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
pub mod vgpu_mgr;

use crate::driver::NovaCoreDriver;

kernel::module_pci_driver! {
    type: NovaCoreDriver,
    name: "NovaCore",
    author: "Danilo Krummrich",
    description: "Nova Core GPU driver",
    license: "GPL v2",
}

pub(crate) fn align64(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

pub(crate) fn align(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

pub(crate) fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

pub(crate) fn div_round_up64(n: u64, d: u64) -> u64 {
    (n + d - 1) / d
}

pub(crate) fn div_round_up(n: usize, d: usize) -> usize {
    (n + d - 1) / d
}

pub(crate) fn rounddown(x: usize, y: usize) -> usize {
    x - (x % y)
}

pub(crate) fn roundup(x: usize, y: usize) -> usize {
    (x + (y - 1) / y) * y
}

pub(crate) fn order_base_2(n: usize) -> u32 {
    if n > 1 {
	(n - 1).ilog2() + 1
    } else {
	0
    }
}

pub(crate) fn is_aligned<T: Into<u64>>(x: T, a: T) -> bool {
    let x = x.into();
    let a = a.into();
    (x & (a - 1)) == 0
}
