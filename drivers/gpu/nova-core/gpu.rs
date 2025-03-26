// SPDX-License-Identifier: GPL-2.0

use kernel::{
    bindings, device, devres::Devres, error::code::*, firmware, fmt, pci, prelude::*, str::CString,
};

use crate::vbios::Vbios;
use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::falcon::FalconBromParams;
use crate::falcon::{
    self, gsp::GspFalcon, FalconFirmware, FalconLoadTarget, FalconUCodeDescV3, Sec2Falcon,
    NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
};
use crate::timer::Timer;
use crate::util;
use crate::{devinit, regs};
use core::fmt;
use core::time::Duration;

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

impl Default for Chipset {
    fn default() -> Self {
        Self::TU102
    }
}

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
    Turing,
    Ampere,
    Ada,
}

pub(crate) struct Revision {
    major: u8,
    minor: u8,
}

impl Revision {
    fn from_boot0(boot0: regs::Boot0) -> Self {
        Self {
            major: boot0.major_rev(),
            minor: boot0.minor_rev(),
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
        let boot0 = with_bar!(bar, |b| regs::Boot0::read(b))?;

        Ok(Self {
            chipset: boot0.chipset()?,
            revision: Revision::from_boot0(boot0),
        })
    }
}

/// Structure encapsulating the firmware blobs required for the GPU to operate.
#[expect(dead_code)]
pub(crate) struct Firmware {
    booter_load: firmware::Firmware,
    booter_unload: firmware::Firmware,
    bootloader: firmware::Firmware,
    gsp: firmware::Firmware,
}

impl Firmware {
    fn new(dev: &device::Device, spec: &Spec, ver: &str) -> Result<Firmware> {
        let mut chip_name = CString::try_from_fmt(fmt!("{}", spec.chipset))?;
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

/// Structure holding the resources required to operate the GPU.
#[pin_data]
pub(crate) struct Gpu {
    spec: Spec,
    /// MMIO mapping of PCI BAR 0
    bar: Devres<Bar0>,
    fw: Firmware,
    timer: Timer,
    sysmem_flush: DmaObject,
}

impl Gpu {
    pub(crate) fn new(pdev: &pci::Device, bar: Devres<Bar0>) -> Result<impl PinInit<Self>> {
        let spec = Spec::new(&bar)?;
        let fw = Firmware::new(pdev.as_ref(), &spec, "535.113.01")?;

        pr_info!("-------------------------\n");
        dev_info!(
            pdev.as_ref(),
            "NVIDIA (Chipset: {}, Architecture: {:?}, Revision: {})\n",
            spec.chipset,
            spec.chipset.arch(),
            spec.revision
        );

        with_bar!(bar, |b| b.writel(0x40, 0x110004))?;

        let timer = Timer::new();
        let gsp_falcon = GspFalcon::new(
            pdev,
            spec.chipset,
            &bar,
            if spec.chipset > Chipset::GA100 {
                true
            } else {
                false
            },
        )?;

        let _sec2_falcon = Sec2Falcon::new(pdev, spec.chipset, &bar, false)?;

        let display_disabled = with_bar!(bar, |b| devinit::display_disabled(b, spec.chipset))?;
        let fb_size = with_bar_res!(bar, |b| devinit::vidmem_size(b, spec.chipset))?;
        let vga_base = with_bar!(bar, |b| devinit::vga_workspace_addr(
            &b,
            fb_size,
            display_disabled
        ))?;

        let vga_size = fb_size - vga_base;
        dev_info!(
            pdev.as_ref(),
            "Display disabled: {}, FB size: 0x{:x}, VGA base: 0x{:x}, VGA size: 0x{:x}",
            display_disabled,
            fb_size,
            vga_base,
            vga_size,
        );
        // TODO: make this a constant.
        let frts_size = 0x100000;
        let frts_addr = vga_base - frts_size;

        let vbios = Vbios::probe(&bar)?;

        // TODO: should we write 0x0 back when we drop this object?
        let sysmem_flush = DmaObject::new(pdev, 0x1000, "sysmem flush page")?;
        with_bar!(bar, |b| {
            let handle = sysmem_flush.dma.dma_handle();

            regs::PfbNisoFlushSysmemAddr::default()
                .set_adr_39_08((handle >> 8) as u32)
                .write(b);
            if spec.chipset >= Chipset::GA102 {
                regs::PfbNisoFlushSysmemAddrHi::default()
                    .set_adr_63_40((handle >> 40) as u32)
                    .write(b);
            }
        })?;

        // Now let's load.

        let fwsec_frts = load_fwsec_frts(pdev, &bar, &vbios, frts_addr, frts_size)?;

        gsp_falcon.reset(&bar, &timer)?;
        gsp_falcon.dma_load(&bar, &timer, &fwsec_frts)?;

        let (mbox0, _) = gsp_falcon.boot(&bar, &timer, Some(0), None)?;
        if mbox0 != 0 {
            pr_err!("FWSEC firmware returned error {}\n", mbox0);
            return Err(EINVAL);
        }

        let (scratch_e, wpr2_lo, wpr2_hi) = with_bar!(bar, |b| {
            let scratch_e = regs::PbusSwScratche::read(&*b).field() >> 16;
            let wpr2_lo = (regs::PfbPriMmuWpr2AddrLo::read(&*b).lo_val() as u64) << 12;
            let wpr2_hi = (regs::PfbPriMmuWpr2AddrHi::read(&*b).hi_val() as u64) << 12;

            (scratch_e, wpr2_lo, wpr2_hi)
        })?;
        pr_info!("scratch_e: {:#x}\n", scratch_e);
        dev_info!(pdev.as_ref(), "WPR2: {:#x}-{:#x}\n", wpr2_lo, wpr2_hi);

        if wpr2_hi == 0 {
            dev_err!(
                pdev.as_ref(),
                "WPR2 region not created after running FWSEC-FRTS\n"
            );

            return Err(ENOTTY);
        }

        if wpr2_lo != frts_addr {
            dev_err!(
                pdev.as_ref(),
                "WPR2 region created at unexpected address {:#x} ; expected {:#x}\n",
                wpr2_lo,
                frts_addr,
            );
        }

        pr_info!("GPU instance built!\n");

        Ok(pin_init!(Self {
            spec,
            bar,
            fw,
            timer,
            sysmem_flush,
        }))
    }

    pub(crate) fn test_timer(&self) -> Result<()> {
        pr_info!("testing timer subdev\n");
        with_bar!(self.bar, |b| {
            pr_info!("current timestamp: {}\n", self.timer.read(b))
        })?;

        if !matches!(
            self.timer
                .wait_on(&self.bar, Duration::from_millis(10), || Some(())),
            Ok(())
        ) {
            pr_crit!("timer test failure\n");
        }

        let t1 = with_bar!(self.bar, |b| {
            pr_info!("timestamp after immediate exit: {}\n", self.timer.read(b));
            self.timer.read(b)
        })?;

        if self
            .timer
            .wait_on(&self.bar, Duration::from_millis(10), || Option::<()>::None)
            != Err(ETIMEDOUT)
        {
            pr_crit!("timer test 2 failure\n");
        }

        let t2 = with_bar!(self.bar, |b| self.timer.read(b))?;
        if t2 - t1 < Duration::from_millis(10) {
            pr_crit!("timer test 3 failure\n");
        }

        with_bar!(self.bar, |b| {
            pr_info!(
                "timestamp after timeout: {} ({:?})\n",
                self.timer.read(b),
                t2 - t1
            );
        })?;

        Ok(())
    }
}

pub(crate) struct FwsecFrtsFirmware {
    v3_desc: FalconUCodeDescV3,
    ucode_dma: DmaObject,
}

impl FalconFirmware for FwsecFrtsFirmware {
    type Target = falcon::gsp::Gsp;

    fn dma_handle(&self) -> bindings::dma_addr_t {
        self.ucode_dma.dma.dma_handle()
    }

    fn imem_load(&self) -> FalconLoadTarget {
        FalconLoadTarget {
            src_start: 0,
            dst_start: self.v3_desc.imem_phys_base,
            len: self.v3_desc.imem_load_size,
        }
    }

    fn dmem_load(&self) -> FalconLoadTarget {
        fn align_up(value: u32, align: u32) -> u32 {
            (value + align - 1) & !(align - 1)
        }

        FalconLoadTarget {
            src_start: self.v3_desc.imem_load_size,
            dst_start: self.v3_desc.dmem_phys_base,
            len: align_up(self.v3_desc.dmem_load_size, 256),
        }
    }

    fn brom_params(&self) -> FalconBromParams {
        FalconBromParams {
            pkc_data_offset: self.v3_desc.pkc_data_offset,
            engine_id_mask: self.v3_desc.engine_id_mask,
            ucode_id: self.v3_desc.ucode_id,
        }
    }
}

fn load_fwsec_frts(
    pdev: &pci::Device,
    bar: &Devres<Bar0>,
    vbios: &Vbios,
    frts_addr: u64,
    frts_size: u64,
) -> Result<FwsecFrtsFirmware> {
    let v3_desc = vbios.fwsec_header()?;
    let ucode = vbios.fwsec_ucode()?;

    let mut ucode_dma = DmaObject::from_data(pdev, ucode, "fwsec-frts")?;
    crate::falcon::patch_fw(
        &mut ucode_dma,
        v3_desc,
        NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS,
        frts_addr,
        frts_size,
    )?;

    const SIG_SIZE: usize = 96 * 4;
    let signatures = {
        // TODO: then this can be moved into the bios module, as part of the FW extraction
        // process... Actually signatures can be extracted at the same time as the header since
        // they are right after?
        &ucode[0..(v3_desc.signature_count as usize * SIG_SIZE)]
    };
    let sig_base_img = (v3_desc.imem_load_size + v3_desc.pkc_data_offset) as usize;

    if v3_desc.signature_count != 0 {
        // Some more patching...
        let idx = {
            let mut sig_fuse_version = v3_desc.signature_versions as u32;

            pr_info!(
                "brom: {:#x} {:#x}\n",
                v3_desc.engine_id_mask,
                v3_desc.ucode_id
            );
            pr_info!("sig_fuse_version: {}\n", sig_fuse_version);

            let mut reg_fuse_version = if v3_desc.engine_id_mask & 0x00000400 != 0 {
                bar.try_access()
                    .ok_or(ENXIO)
                    .and_then(|b| b.try_readl(0x8241c0 + ((v3_desc.ucode_id - 1) as usize * 4)))?
            } else {
                pr_warn!("unexpected engine_id_mask {:#x}", v3_desc.engine_id_mask);
                return Err(EINVAL);
            };

            reg_fuse_version = 1 << (32 - reg_fuse_version.leading_zeros());
            pr_info!("reg_fuse_version: {:#x}\n", reg_fuse_version);
            if (reg_fuse_version & sig_fuse_version) == 0 {
                pr_warn!(
                    "no matching signature: {:#x} {:#x}\n",
                    reg_fuse_version,
                    v3_desc.signature_versions
                );
                return Err(EINVAL);
            }

            let mut idx = 0;
            while (reg_fuse_version & sig_fuse_version & 1) == 0 {
                idx += sig_fuse_version & 1;
                reg_fuse_version >>= 1;
                sig_fuse_version >>= 1;
            }

            idx
        };

        pr_info!("patching signature with idx {}\n", idx);
        let signature_start = idx as usize * SIG_SIZE;
        let signature = &signatures[signature_start..signature_start + SIG_SIZE];
        // SAFETY: we are the only user of this object, so there cannot be any race.
        let dst = unsafe { ucode_dma.dma.as_slice_mut(sig_base_img, signature.len()) }?;
        pr_info!(
            "ready to copy: {} {} {} {}\n",
            signature.len(),
            dst.len(),
            ucode_dma.len,
            sig_base_img + SIG_SIZE
        );
        // SAFETY: `signature` and `dst` have the same length, so this cannot panic.
        dst.copy_from_slice(signature);
    }

    Ok(FwsecFrtsFirmware {
        v3_desc: v3_desc.clone(),
        ucode_dma,
    })
}
