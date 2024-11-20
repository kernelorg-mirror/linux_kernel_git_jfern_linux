// SPDX-License-Identifier: GPL-2.0

//! Nova GPU Driver

mod bios;
mod devinit;
mod dma;
mod driver;
mod gpu;
mod timer;

use crate::driver::NovaCoreDriver;

kernel::module_pci_driver! {
    type: NovaCoreDriver,
    name: "NovaCore",
    author: "Danilo Krummrich",
    description: "Nova Core GPU driver",
    license: "GPL v2",
}


