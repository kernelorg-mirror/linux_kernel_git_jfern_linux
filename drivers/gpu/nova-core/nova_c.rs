// SPDX-License-Identifier: GPL-2.0

//! Nova GPU Driver

mod bios;
mod devinit;
mod dma;
mod driver;
mod firmware;
mod falcon;
mod gpu;
mod gsp;
mod nvfw;
mod rm_riscv;
mod sec2;
mod timer;
mod vfn;

use crate::driver::NovaCoreDriver;

kernel::module_pci_driver! {
    type: NovaCoreDriver,
    name: "NovaCore",
    author: "Danilo Krummrich",
    description: "Nova Core GPU driver",
    license: "GPL v2",
}

pub(crate) fn align(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

