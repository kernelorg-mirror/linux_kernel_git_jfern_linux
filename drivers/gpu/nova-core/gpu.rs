// SPDX-License-Identifier: GPL-2.0

#![allow(dead_code)]

use kernel::{
    device,
    device::Device,
    devres::Devres,
    elf::Elf,
    error::code::*,
    firmware,
    fmt,
    pci,
    prelude::*,
    str::CString,
    sync::Arc,
    types::ARef,
};

use crate::bar::Bar;
use crate::bios::Bios;
use crate::devinit;
use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::firmware::{BLFirmware, NvkmFirmware, RadixFirmware};
use crate::gsp::gsp_falcon::GspFalcon;
use crate::gsp::*;
use crate::mmu::memory::InstMem;
use crate::mmu::mm::MemRange;
use crate::timer::Timer;
use crate::vfn::Vfn;
use crate::rm_riscv::RiscvFw;
use crate::sec2::{Sec2, Sec2Fw};
use core::fmt::Debug;

pub(crate) struct GpuConsts {
    sig_section: &'static str,
    pub sec2_addr: u32,
    need_bl_fw: bool,
}

/// Enum representing the GPU chipset.
#[derive(Debug,PartialOrd,PartialEq,Clone,Copy)]
pub(crate) enum Chipset {
    TU102 = 0x162,
    TU104 = 0x164,
    TU106 = 0x166,
    TU117 = 0x167,
    TU116 = 0x168,
    GA100 = 0x170,
    GA102 = 0x172,
    GA103 = 0x173,
    GA104 = 0x174,
    GA106 = 0x176,
    GA107 = 0x177,
    AD102 = 0x192,
    AD103 = 0x193,
    AD104 = 0x194,
    AD106 = 0x196,
    AD107 = 0x197,
}

impl Chipset {
    pub(crate) fn val(chipset: &Chipset) -> u32 {
        *chipset as u32
    }
}

/// Checks if a chipset is inside a certain range
#[macro_export]
macro_rules! chipset_range {
    ($var:expr, $start:ident, $end:ident) => {
        Chipset::val($var) >= Chipset::val(&Chipset::$start) && $var <= Chipset::val(&Chipset::$end)
    }
}

/// Checks if a chipset if before a certain point
#[macro_export]
macro_rules! chipsets_before {
    ($var:expr, $end:ident) => {
        Chipset::val($var) <= Chipset::val(&Chipset::$end)
    }
}

/// Checks if a chipset if after a certain point
#[macro_export]
macro_rules! chipsets_after {
    ($var:expr, $start:ident) => {
        Chipset::val($var) >= Chipset::val(&Chipset::$start)
    }
}

/// Enum representing the GPU generation.
#[derive(Debug)]
pub(crate) enum CardType {
    /// Turing
    TU100 = 0x160,
    /// Ampere
    GA100 = 0x170,
    /// Ada Lovelace
    AD100 = 0x190,
}

/// Structure holding the metadata of the GPU.
#[allow(dead_code)]
pub(crate) struct GpuSpec {
    /// Contents of the boot0 register.
    boot0: u64,
    card_type: CardType,
    pub(crate) chipset: Chipset,
    /// The revision of the chipset.
    chiprev: u8,
    pub gpu_consts: GpuConsts,
}

/// Structure encapsulating the firmware blobs required for the GPU to operate.
#[allow(dead_code)]
pub(crate) struct Firmware {
    pub loader_fw: Sec2Fw,
    pub unload_fw: Sec2Fw,
    pub bootloader_fw: RiscvFw,
    pub gsp_fw: RadixFirmware,
    pub gsp_sigs: NvkmFirmware,
    pub bl_fw: Option<BLFirmware>,
}

#[derive(Default,Debug)]
pub(crate) struct SizeAddr {
    pub addr: u64,
    pub size: u64,
}

#[derive(Default,Debug)]
#[allow(unused)]
pub(crate) struct FBInfo {
    pub vga_workspace: SizeAddr,
    pub bios: SizeAddr,
    pub frts: SizeAddr,
    pub boot: SizeAddr,
    pub elf: SizeAddr,
    pub wpr2_heap: SizeAddr,
    pub wpr2: SizeAddr,
    pub heap: SizeAddr,
    pub fb: SizeAddr,
    pub region: KVec<SizeAddr>,
    pub rsvd_size: u32,
    pub wpr_size: u32,
    pub vf_partition_count: u8,
}

#[derive(Debug)]
#[allow(unused)]
pub(crate) struct IntrInfo {
    pub inst: u32,
    pub stall: u32,
    pub nonstall: u32,
}

#[allow(unused)]
pub(crate) struct FifoDeviceEntry {
    pub addr: u32,
    pub rmid: u32,
    pub id: u32,
    pub eng_desc: u32,
}

pub(crate) struct FifoDeviceInfoTable {
    pub table: KVec<FifoDeviceEntry>
}

/// Structure holding the base pre-GSP boot GPU pieces
#[allow(dead_code)]
pub(crate) struct GpuBase {
    pub dev: ARef<Device>,
    pub spec: GpuSpec,
    /// MMIO mapping of PCI BAR 0
    pub bar: Arc<Devres<Bar0>>,
    pub bios: Bios,
    pub timer: Arc<Timer>,
}

/// Structure holding the resources required to operate the GPU.
#[allow(dead_code)]
#[pin_data]
pub(crate) struct Gpu {
    base: Arc<GpuBase>,
    pub vfn: Arc<Vfn>,
    pub gsp: Arc<dyn GspManager>,
    pub vram_mm: Arc<MemRange>,
    pub bar: Arc<Bar>,
}

// TODO replace with something like derive(FromPrimitive)
impl Chipset {
    fn from_u32(value: u32) -> Option<Chipset> {
        match value {
            0x162 => Some(Chipset::TU102),
            0x164 => Some(Chipset::TU104),
            0x166 => Some(Chipset::TU106),
            0x167 => Some(Chipset::TU117),
            0x168 => Some(Chipset::TU116),
            0x172 => Some(Chipset::GA102),
            0x173 => Some(Chipset::GA103),
            0x174 => Some(Chipset::GA104),
            0x176 => Some(Chipset::GA106),
            0x177 => Some(Chipset::GA107),
            0x192 => Some(Chipset::AD102),
            0x193 => Some(Chipset::AD103),
            0x194 => Some(Chipset::AD104),
            0x196 => Some(Chipset::AD106),
            0x197 => Some(Chipset::AD107),
            _ => None,
        }
    }
}

// TODO replace with something like derive(FromPrimitive)
impl CardType {
    fn from_u32(value: u32) -> Option<CardType> {
        match value {
            0x160 => Some(CardType::TU100),
            0x170 => Some(CardType::GA100),
            0x190 => Some(CardType::AD100),
            _ => None,
        }
    }
}

impl GpuConsts {
    fn get(chipset: &Chipset) -> Option<GpuConsts> {
        match chipset {
            Chipset::TU102 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_tu10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: true,
                })
            }
            Chipset::TU106 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_tu10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: true,
                })
            }
            Chipset::TU117 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_tu11x",
                    sec2_addr: 0x840000,
                    need_bl_fw: true,
                })
            }
            Chipset::AD102 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_ad10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: false,
                })
            }
            Chipset::GA102 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_ga10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: false,
                })
            }
            _ => None,
        }
    }
}

impl GpuSpec {
    fn new(bar: &Devres<Bar0>) -> Result<GpuSpec> {
        let bar = bar.try_access().ok_or(ENXIO)?;
        let boot0 = u64::from_le(bar.readq(0));
        let chip = ((boot0 & 0x1ff00000) >> 20) as u32;

        if boot0 & 0x1f000000 == 0 {
            return Err(ENODEV);
        }

        let chipset = match Chipset::from_u32(chip) {
            Some(x) => x,
            None => return Err(ENODEV),
        };

        let card_type = match CardType::from_u32(chip & 0x1f0) {
            Some(x) => x,
            None => return Err(ENODEV),
        };

        let gpu_consts = match GpuConsts::get(&chipset) {
            Some(x) => x,
            None => return Err(ENODEV),
        };
        Ok(Self {
            boot0,
            card_type,
            chipset,
            chiprev: (boot0 & 0xff) as u8,
            gpu_consts,
        })
    }
}

impl Firmware {

    fn find_elf_section<'a>(elf: &'a Elf<'_>, section: &'static str) -> Result<&'a [u8]> {
        for i in 0..elf.section_header_count() {
            if elf.section_name(i)?.to_str() == core::prelude::v1::Ok(section) {
                return elf.section_bytes(i);
            }
        }
        Err(EINVAL)
    }

    fn new(dev: ARef<device::Device>, gpu_base: &GpuBase, sec2: &Sec2, ver: &str) -> Result<Firmware> {
        let mut chip_name = CString::try_from_fmt(fmt!("{:?}", gpu_base.spec.chipset))?;
        chip_name.make_ascii_lowercase();

        let fw_booter_load_path =
            CString::try_from_fmt(fmt!("nvidia/{}/gsp/booter_load-{}.bin", &*chip_name, ver,))?;
        let fw_booter_unload_path =
            CString::try_from_fmt(fmt!("nvidia/{}/gsp/booter_unload-{}.bin", &*chip_name, ver))?;
        let fw_bootloader_path =
            CString::try_from_fmt(fmt!("nvidia/{}/gsp/bootloader-{}.bin", &*chip_name, ver))?;
        let fw_gsp_path =
            CString::try_from_fmt(fmt!("nvidia/{}/gsp/gsp-{}.bin", &*chip_name, ver))?;
        let mut fw_bl_path = None;

        if gpu_base.spec.gpu_consts.need_bl_fw {
            fw_bl_path = Some(CString::try_from_fmt(fmt!("nvidia/{}/acr/bl.bin", &*chip_name))?);
        }
        let booter_load = firmware::Firmware::request(&fw_booter_load_path, &dev)?;
        let booter_unload = firmware::Firmware::request(&fw_booter_unload_path, &dev)?;
        let bootloader = firmware::Firmware::request(&fw_bootloader_path, &dev)?;
        let gsp = firmware::Firmware::request(&fw_gsp_path, &dev)?;

        let mut bl = None;
        if gpu_base.spec.gpu_consts.need_bl_fw {
            bl = Some(firmware::Firmware::request(&fw_bl_path.unwrap(), &dev)?);
        }

        let bootloader_fw = RiscvFw::new_from_fw(&dev, &bootloader, "bootloader")?;
        let loader_fw = Sec2Fw::new(&dev, sec2.falcon.clone(), &booter_load, "booter-load")?;
        let unload_fw = Sec2Fw::new(&dev, sec2.falcon.clone(), &booter_unload, "booter-unload")?;

        let mut bl_fw = None;
        if !bl.is_none() {
            bl_fw = Some(BLFirmware::new(bl.unwrap()));
        }

        let gspvec : VVec<u8> = gsp.copy(GFP_KERNEL)?;
        let elf = Elf::from_bytes(gspvec.as_slice())?;

        let data = Self::find_elf_section(&elf, ".fwimage")?;

        let gsp_fw = RadixFirmware::new(&dev, ".fwimage", data)?;

        let data = Self::find_elf_section(&elf, gpu_base.spec.gpu_consts.sig_section)?;
        let gsp_sigs_dma = DmaObject::new_from_data(&dev, data, gpu_base.spec.gpu_consts.sig_section)?;
        let gsp_sigs = NvkmFirmware::new("gsp sigs", gsp_sigs_dma);

        Ok(Firmware {
            loader_fw,
            unload_fw,
            bootloader_fw,
            gsp_fw,
            gsp_sigs,
            bl_fw,
        })
    }
}

impl Gpu {
    pub(crate) fn new(pdev: &pci::Device, bar: Arc<Devres<Bar0>>) -> Result<impl PinInit<Self>> {
        let spec = GpuSpec::new(&bar)?;
        let mut bios = Bios::new();

        devinit::wait(&bar)?;

        let vfn = Vfn::new(bar.clone())?;

        Vfn::install_irq(&vfn, pdev)?;

        let timer = Arc::new(Timer::new(bar.clone())?, GFP_KERNEL)?;

        bios.probe(&bar)?;

        let base = Arc::new(GpuBase {
            dev: pdev.as_ref().into(),
            spec,
            bar,
            bios,
            timer
        }, GFP_KERNEL)?;

        dev_info!(
            pdev.as_ref(),
            "NVIDIA {:?} ({:#x})",
            base.spec.chipset,
            base.spec.boot0
        );

        let sec2 = Sec2::new(base.clone())?;
        let gsp_falcon = GspFalcon::new(base.clone())?;

        let fw = Firmware::new(pdev.as_dev(), &base, &sec2, "535.113.01")?;

        let mut vram_mm = MemRange::new(1)?;

        let gsp = GspManagerr535_113_01::new(base.clone(), &vfn, &mut vram_mm, gsp_falcon, sec2, fw)? as Arc<dyn GspManager>;

        let vram_mm = Arc::new(vram_mm, GFP_KERNEL)?;

        let instmem = InstMem::new(base.clone(), vram_mm.clone(),
                                   pdev.resource_start(3)?)?;

        let bars = Arc::new(Bar::new(instmem.clone(),
                                     gsp.clone(),
                                     pdev.resource_len(1)?,
                                     Some(pdev.resource_len(3)?),
                                     pdev.resource_start(3)?)?, GFP_KERNEL)?;

        instmem.set_bar(bars.clone())?;

        {
            let bar = base.bar.try_access().ok_or(ENXIO)?;
            bar.try_writel(0x40, 0x110004)?;
        }

        Ok(pin_init!(Self { base, vfn, gsp, bar: bars, vram_mm }))
    }

    pub(crate) fn release(&self) {
        self.vfn.unregister_irq();
    }
}

const PCI_BASE: usize = 0x88000;
pub(crate) fn msi_rearm(bar: &Devres<Bar0>) -> Result<()> {
    let bar = bar.try_access().ok_or(ENXIO)?;
    bar.writel(0, PCI_BASE + 0x704);
    Ok(())
}
