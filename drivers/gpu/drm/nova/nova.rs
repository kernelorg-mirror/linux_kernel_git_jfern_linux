// SPDX-License-Identifier: GPL-2.0

//! Nova GPU Driver

mod driver;
mod file;
mod gem;
mod gpu;

use crate::driver::NovaDriver;

kernel::module_pci_driver! {
    type: NovaDriver,
    name: "Nova",
    author: "Danilo Krummrich",
    description: "Nova GPU driver",
    license: "GPL v2",
}
