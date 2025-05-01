// SPDX-License-Identifier: GPL-2.0

use kernel::{device, devres::Devres, error::code::*, pci, prelude::*};

use crate::devinit;
use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::falcon::{gsp::Gsp, sec2::Sec2, Falcon};
use crate::firmware::Firmware;
use crate::regs;
use crate::util;
use crate::vbios::Vbios;
use core::fmt;

macro_rules! define_chipset {
    ({ $($variant:ident = $value:expr),* $(,)* }) =>
    {
        /// Enum representation of the GPU chipset.
        #[derive(fmt::Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
        pub(crate) enum Chipset {
            $($variant = $value),*,
        }

        impl Chipset {
            pub(crate) const ALL: &'static [Chipset] = &[
                $( Chipset::$variant, )*
            ];

            pub(crate) const NAMES: [&'static str; Self::ALL.len()] = [
                $( util::const_bytes_to_str(
                        util::to_lowercase_bytes::<{ stringify!($variant).len() }>(
                            stringify!($variant)
                        ).as_slice()
                ), )*
            ];
        }

        // TODO replace with something like derive(FromPrimitive)
        impl TryFrom<u32> for Chipset {
            type Error = kernel::error::Error;

            fn try_from(value: u32) -> Result<Self, Self::Error> {
                match value {
                    $( $value => Ok(Chipset::$variant), )*
                    _ => Err(ENODEV),
                }
            }
        }
    }
}

define_chipset!({
    // Turing
    TU102 = 0x162,
    TU104 = 0x164,
    TU106 = 0x166,
    TU117 = 0x167,
    TU116 = 0x168,
    // Ampere
    GA100 = 0x170,
    GA102 = 0x172,
    GA103 = 0x173,
    GA104 = 0x174,
    GA106 = 0x176,
    GA107 = 0x177,
    // Ada
    AD102 = 0x192,
    AD103 = 0x193,
    AD104 = 0x194,
    AD106 = 0x196,
    AD107 = 0x197,
});

impl Chipset {
    pub(crate) fn arch(&self) -> Architecture {
        match self {
            Self::TU102 | Self::TU104 | Self::TU106 | Self::TU117 | Self::TU116 => {
                Architecture::Turing
            }
            Self::GA100 | Self::GA102 | Self::GA103 | Self::GA104 | Self::GA106 | Self::GA107 => {
                Architecture::Ampere
            }
            Self::AD102 | Self::AD103 | Self::AD104 | Self::AD106 | Self::AD107 => {
                Architecture::Ada
            }
        }
    }
}

// TODO
//
// The resulting strings are used to generate firmware paths, hence the
// generated strings have to be stable.
//
// Hence, replace with something like strum_macros derive(Display).
//
// For now, redirect to fmt::Debug for convenience.
impl fmt::Display for Chipset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Enum representation of the GPU generation.
#[derive(fmt::Debug)]
pub(crate) enum Architecture {
    Turing = 0x16,
    Ampere = 0x17,
    Ada = 0x19,
}

impl TryFrom<u8> for Architecture {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        match value {
            0x16 => Ok(Self::Turing),
            0x17 => Ok(Self::Ampere),
            0x19 => Ok(Self::Ada),
            _ => Err(ENODEV),
        }
    }
}

pub(crate) struct Revision {
    major: u8,
    minor: u8,
}

impl Revision {
    fn from_boot0(boot0: regs::NV_PMC_BOOT_0) -> Self {
        Self {
            major: boot0.major_revision(),
            minor: boot0.minor_revision(),
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}.{:x}", self.major, self.minor)
    }
}

/// Structure holding the metadata of the GPU.
pub(crate) struct Spec {
    chipset: Chipset,
    /// The revision of the chipset.
    revision: Revision,
}

impl Spec {
    fn new(bar: &Devres<Bar0>) -> Result<Spec> {
        let boot0 = with_bar!(bar, regs::NV_PMC_BOOT_0::read)?;

        Ok(Self {
            chipset: boot0.chipset()?,
            revision: Revision::from_boot0(boot0),
        })
    }
}

/// Structure holding the resources required to operate the GPU.
#[pin_data(PinnedDrop)]
pub(crate) struct Gpu {
    spec: Spec,
    /// MMIO mapping of PCI BAR 0
    bar: Devres<Bar0>,
    fw: Firmware,
    // System memory page required for flushing all pending GPU-side memory writes done through
    // PCIE into system memory.
    sysmem_flush: DmaObject,
}

#[pinned_drop]
impl PinnedDrop for Gpu {
    fn drop(self: Pin<&mut Self>) {
        // Unregister the sysmem flush page before we release it.
        let _ = with_bar!(&self.bar, |b| {
            regs::NV_PFB_NISO_FLUSH_SYSMEM_ADDR::default()
                .set_adr_39_08(0)
                .write(b);
            if self.spec.chipset >= Chipset::GA102 {
                regs::NV_PFB_NISO_FLUSH_SYSMEM_ADDR_HI::default()
                    .set_adr_63_40(0)
                    .write(b);
            }
        });
    }
}

impl Gpu {
    pub(crate) fn new(
        pdev: &pci::Device<device::Bound>,
        bar: Devres<Bar0>,
    ) -> Result<impl PinInit<Self>> {
        let spec = Spec::new(&bar)?;
        let fw = Firmware::new(pdev.as_ref(), spec.chipset, "535.113.01")?;

        dev_info!(
            pdev.as_ref(),
            "NVIDIA (Chipset: {}, Architecture: {:?}, Revision: {})\n",
            spec.chipset,
            spec.chipset.arch(),
            spec.revision
        );

        // We must wait for GFW_BOOT completion before doing any significant setup on the GPU.
        devinit::wait_gfw_boot_completion(&bar)
            .inspect_err(|_| dev_err!(pdev.as_ref(), "GFW boot did not complete"))?;

        // System memory page required for sysmembar to properly flush into system memory.
        let sysmem_flush = {
            let page = DmaObject::new(pdev.as_ref(), kernel::bindings::PAGE_SIZE)?;

            // Register the sysmem flush page.
            with_bar!(bar, |b| {
                let handle = page.dma_handle();

                regs::NV_PFB_NISO_FLUSH_SYSMEM_ADDR::default()
                    .set_adr_39_08((handle >> 8) as u32)
                    .write(b);
                if spec.chipset >= Chipset::GA102 {
                    regs::NV_PFB_NISO_FLUSH_SYSMEM_ADDR_HI::default()
                        .set_adr_63_40((handle >> 40) as u32)
                        .write(b);
                }
            })?;

            page
        };

        let gsp_falcon = Falcon::<Gsp>::new(
            pdev.as_ref(),
            spec.chipset,
            &bar,
            spec.chipset > Chipset::GA100,
        )?;
        gsp_falcon.clear_swgen0_intr(&bar)?;

        let _sec2_falcon = Falcon::<Sec2>::new(pdev.as_ref(), spec.chipset, &bar, true)?;

        let _bios = Vbios::new(pdev, &bar)?;

        Ok(pin_init!(Self {
            spec,
            bar,
            fw,
            sysmem_flush,
        }))
    }
}
