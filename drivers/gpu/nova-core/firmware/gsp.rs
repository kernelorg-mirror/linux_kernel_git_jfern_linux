// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    dma::DmaAddress,
    prelude::*,
    str::{
        CStr,
        CString, //
    },
};

use crate::{
    dma::DmaObject,
    firmware::{
        elf,
        radix3::Radix3,
        riscv::RiscvFirmware, //
    },
    gpu::{
        Architecture,
        Chipset, //
    },
};

/// GSP firmware with 3-level radix page tables for the GSP bootloader.
///
/// Also known as "Radix3" firmware.
#[pin_data]
pub(crate) struct GspFirmware {
    /// The GSP firmware image mapped via a 3-level radix page table.
    #[pin]
    radix3: Radix3,
    /// Firmware file path as requested from userspace (e.g. `nvidia/gb202/gsp/gsp-000.bin`).
    pub(crate) fw_path: CString,
    /// Firmware version string extracted from the `.fwversion` ELF section.
    pub(crate) fw_version: CString,
    /// Build ID extracted from the `.note.gnu.build-id` ELF section.
    pub(crate) build_id: Option<elf::BuildId>,
    /// Device-mapped GSP signatures matching the GPU's [`Chipset`].
    pub(crate) signatures: DmaObject,
    /// GSP bootloader, verifies the GSP firmware before loading and running it.
    pub(crate) bootloader: RiscvFirmware,
}

impl GspFirmware {
    fn find_gsp_sigs_section(chipset: Chipset) -> Option<&'static str> {
        match chipset.arch() {
            Architecture::Turing if matches!(chipset, Chipset::TU116 | Chipset::TU117) => {
                Some(".fwsignature_tu11x")
            }
            Architecture::Turing => Some(".fwsignature_tu10x"),
            // GA100 uses the same firmware as Turing
            Architecture::Ampere if chipset == Chipset::GA100 => Some(".fwsignature_tu10x"),
            Architecture::Ampere => Some(".fwsignature_ga10x"),
            Architecture::Ada => Some(".fwsignature_ad10x"),
            Architecture::Hopper => Some(".fwsignature_gh10x"),
            Architecture::Blackwell if matches!(chipset, Chipset::GB100 | Chipset::GB102) => {
                Some(".fwsignature_gb10x")
            }
            Architecture::Blackwell => Some(".fwsignature_gb20x"),
        }
    }

    /// Maps the pre-loaded GSP firmware into `dev`'s address-space and creates the page tables
    /// expected by the GSP bootloader to load it.
    pub(crate) fn new<'a>(
        dev: &'a device::Device<device::Bound>,
        chipset: Chipset,
        ver: &'a str,
        gsp_firmware: &'a kernel::firmware::Firmware,
        fw_path: CString,
    ) -> impl PinInit<Self, Error> + 'a {
        pin_init::pin_init_scope(move || {
            let fw_section = elf::elf_section(gsp_firmware.data(), ".fwimage").ok_or(EINVAL)?;
            let fw_data = VVec::with_capacity(fw_section.len(), GFP_KERNEL)
                .and_then(|mut v| {
                    v.extend_from_slice(fw_section, GFP_KERNEL)?;
                    Ok(v)
                })
                .map_err(|_| ENOMEM)?;

            let sigs_section = Self::find_gsp_sigs_section(chipset).ok_or(ENOTSUPP)?;
            let signatures = elf::elf_section(gsp_firmware.data(), sigs_section)
                .ok_or(EINVAL)
                .and_then(|data| DmaObject::from_data(dev, data))?;

            let fw_version = {
                let version_str = elf::elf_section(gsp_firmware.data(), ".fwversion")
                    .and_then(|data| CStr::from_bytes_until_nul(data).ok())
                    .and_then(|cstr| cstr.to_str().ok())
                    .unwrap_or("unknown");
                CString::try_from_fmt(fmt!("{version_str}"))?
            };

            let build_id = elf::elf_build_id(gsp_firmware.data());

            let (_, bl) = super::request_firmware(dev, chipset, "bootloader", ver)?;
            let bootloader = RiscvFirmware::new(dev, &bl)?;

            Ok(try_pin_init!(Self {
                radix3 <- Radix3::new(dev, &fw_data),
                fw_path,
                fw_version,
                build_id,
                signatures,
                bootloader,
            }))
        })
    }

    /// Returns the size of the GSP firmware image, in bytes.
    pub(crate) fn size(&self) -> usize {
        self.radix3.size
    }

    /// Returns the DMA handle of the radix3 level 0 page table.
    pub(crate) fn radix3_dma_handle(&self) -> DmaAddress {
        self.radix3.dma_handle()
    }
}
