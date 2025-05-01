// SPDX-License-Identifier: GPL-2.0

//! Contains structures and functions dedicated to the parsing, building and patching of firmwares
//! to be loaded into a given execution unit.

use kernel::device;
use kernel::firmware;
use kernel::prelude::*;
use kernel::str::CString;

use crate::dma::DmaObject;
use crate::gpu;
use crate::gpu::Chipset;

pub(crate) mod fwsec;

pub(crate) const FIRMWARE_VERSION: &str = "535.113.01";

/// Structure encapsulating the firmware blobs required for the GPU to operate.
#[expect(dead_code)]
pub(crate) struct Firmware {
    pub booter_load: firmware::Firmware,
    pub booter_unload: firmware::Firmware,
    pub bootloader: firmware::Firmware,
    pub gsp: firmware::Firmware,
}

impl Firmware {
    pub(crate) fn new(
        dev: &device::Device<device::Bound>,
        chipset: Chipset,
        ver: &str,
    ) -> Result<Firmware> {
        let mut chip_name = CString::try_from_fmt(fmt!("{}", chipset))?;
        chip_name.make_ascii_lowercase();

        let request = |name_| {
            CString::try_from_fmt(fmt!("nvidia/{}/gsp/{}-{}.bin", &*chip_name, name_, ver))
                .and_then(|path| firmware::Firmware::request(&path, dev))
        };

        Ok(Firmware {
            booter_load: request("booter_load")?,
            booter_unload: request("booter_unload")?,
            bootloader: request("bootloader")?,
            gsp: request("gsp")?,
        })
    }
}

/// Structure used to describe some firmwares, notably FWSEC-FRTS.
#[repr(C)]
#[derive(Debug, Clone)]
pub(crate) struct FalconUCodeDescV3 {
    /// Header defined by `NV_BIT_FALCON_UCODE_DESC_HEADER_VDESC*` in OpenRM.
    ///
    /// Bits `31:16` contain the size of the header, after which the actual ucode data starts.
    hdr: u32,
    /// Stored size of the ucode after the header.
    stored_size: u32,
    /// Offset in `DMEM` at which the signature is expected to be found.
    pub(crate) pkc_data_offset: u32,
    /// Offset after the code segment at which the app headers are located.
    pub(crate) interface_offset: u32,
    /// Base address at which to load the code segment into `IMEM`.
    pub(crate) imem_phys_base: u32,
    /// Size in bytes of the code to copy into `IMEM`.
    pub(crate) imem_load_size: u32,
    /// Virtual `IMEM` address (i.e. `tag`) at which the code should start.
    pub(crate) imem_virt_base: u32,
    /// Base address at which to load the data segment into `DMEM`.
    pub(crate) dmem_phys_base: u32,
    /// Size in bytes of the data to copy into `DMEM`.
    pub(crate) dmem_load_size: u32,
    /// Mask of the falcon engines on which this firmware can run.
    pub(crate) engine_id_mask: u16,
    /// ID of the ucode used to infer a fuse register to validate the signature.
    pub(crate) ucode_id: u8,
    /// Number of signatures in this firmware.
    pub(crate) signature_count: u8,
    /// Versions of the signatures, used to infer a valid signature to use.
    pub(crate) signature_versions: u16,
    _reserved: u16,
}

impl FalconUCodeDescV3 {
    pub(crate) fn size(&self) -> usize {
        ((self.hdr & 0xffff0000) >> 16) as usize
    }
}

/// Patch the `ucode_dma` firmware at offset `sig_base_img` with `signature`.
fn patch_signature(ucode_dma: &mut DmaObject, signature: &[u8], sig_base_img: usize) -> Result<()> {
    if sig_base_img + signature.len() > ucode_dma.size() {
        return Err(ERANGE);
    }

    // SAFETY: we are the only user of this object, so there cannot be any race.
    let dst = unsafe { ucode_dma.start_ptr_mut().add(sig_base_img) };

    // SAFETY: `signature` and `dst` are valid, properly aligned, and do not overlap.
    unsafe { core::ptr::copy_nonoverlapping(signature.as_ptr(), dst, signature.len()) };

    Ok(())
}

pub(crate) struct ModInfoBuilder<const N: usize>(firmware::ModInfoBuilder<N>);

impl<const N: usize> ModInfoBuilder<N> {
    const VERSION: &'static str = FIRMWARE_VERSION;

    const fn make_entry_file(self, chipset: &str, fw: &str) -> Self {
        ModInfoBuilder(
            self.0
                .new_entry()
                .push("nvidia/")
                .push(chipset)
                .push("/gsp/")
                .push(fw)
                .push("-")
                .push(Self::VERSION)
                .push(".bin"),
        )
    }

    const fn make_entry_chipset(self, chipset: &str) -> Self {
        self.make_entry_file(chipset, "booter_load")
            .make_entry_file(chipset, "booter_unload")
            .make_entry_file(chipset, "bootloader")
            .make_entry_file(chipset, "gsp")
    }

    pub(crate) const fn create(
        module_name: &'static kernel::str::CStr,
    ) -> firmware::ModInfoBuilder<N> {
        let mut this = Self(firmware::ModInfoBuilder::new(module_name));
        let mut i = 0;

        while i < gpu::Chipset::NAMES.len() {
            this = this.make_entry_chipset(gpu::Chipset::NAMES[i]);
            i += 1;
        }

        this.0
    }
}
