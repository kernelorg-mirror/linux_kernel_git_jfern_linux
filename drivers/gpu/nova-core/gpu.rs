// SPDX-License-Identifier: GPL-2.0

#![allow(dead_code)]

use kernel::{
    device,
    device::Device,
    devres::Devres,
    elf::Elf,
    error::code::*,
    firmware,
    new_mutex,
    fmt,
    pci,
    prelude::*,
    str::CString,
    sync::{Arc, Mutex},
    types::ARef,
};

use crate::accel::fifo::BitVec;
use crate::accel::fifo::EngineType;
use crate::bar::Bar;
use crate::bios::Bios;
use crate::devinit;
use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::accel::fifo::{Channel};
use crate::accel::gr::Gr;
use crate::firmware::{BLFirmware, NvkmFirmware, RadixFirmware};
use crate::gsp::gsp_falcon::GspFalcon;
use crate::gsp::*;
use crate::nvfw::fwr535_113_01::gen::*;
use crate::mmu::memory::{InstMem, InstObj, VramObj};
use crate::mmu::mm::MemRange;
use crate::mmu::mmu::Mmu;
use crate::mmu::memory::NVKM_MM_PAGE_SHIFT;
use crate::mmu::vmm::Vmm;
use crate::timer::Timer;
use crate::vfn::Vfn;
use crate::rm_riscv::RiscvFw;
use crate::sec2::{Sec2, Sec2Fw};
use core::fmt::Debug;

#[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
use crate::vgpu_mgr::VGPUMgr;

#[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
pub(crate) const NOVA_ENABLE_VGPU: bool = true;
#[cfg(not(CONFIG_NOVA_CORE_VGPU_SUPPORT))]
pub(crate) const NOVA_ENABLE_VGPU: bool = false;

pub(crate) struct GpuConsts {
    sig_section: &'static str,
    pub sec2_addr: u32,
    need_bl_fw: bool,
    pub fifo_class: u32,
    pub gr_classes: [u32; 4],
    pub ce_class: u32,
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
    pub boot0: u64,
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
    pub base: Arc<GpuBase>,
    pub vfn: Arc<Vfn>,
    pub gsp: Arc<dyn GspManager>,
    pub bar: Arc<Bar>,
    pub mmu: Arc<Mmu>,
    #[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
    pub vgpu: Arc<VGPUMgr>,
    #[cfg(not(CONFIG_NOVA_CORE_VGPU_SUPPORT))]
    pub vgpu: bool,
    pub instmem: Arc<InstMem>,
    pub alloc_id: Arc<AllocId>,
    pub gr_ctx_bufs: KVec<Arc<InstObj>>,
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
                    fifo_class: TURING_CHANNEL_GPFIFO_A,
                    gr_classes: [FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B, TURING_A, TURING_COMPUTE_A],
                    ce_class: TURING_DMA_COPY_A,
                })
            }
            Chipset::TU106 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_tu10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: true,
                    fifo_class: TURING_CHANNEL_GPFIFO_A,
                    gr_classes: [FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B, TURING_A, TURING_COMPUTE_A],
                    ce_class: TURING_DMA_COPY_A,
                })
            }
            Chipset::TU117 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_tu11x",
                    sec2_addr: 0x840000,
                    need_bl_fw: true,
                    fifo_class: TURING_CHANNEL_GPFIFO_A,
                    gr_classes: [FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B, TURING_A, TURING_COMPUTE_A],
                    ce_class: TURING_DMA_COPY_A,
                })
            }
            Chipset::AD102 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_ad10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: false,
                    fifo_class: AMPERE_CHANNEL_GPFIFO_A,
                    gr_classes: [FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B, ADA_A, ADA_COMPUTE_A],
                    ce_class: AMPERE_DMA_COPY_B,
                })
            }
            Chipset::GA102 => {
                Some(GpuConsts{
                    sig_section: ".fwsignature_ga10x",
                    sec2_addr: 0x840000,
                    need_bl_fw: false,
                    fifo_class: AMPERE_CHANNEL_GPFIFO_A,
                    gr_classes: [FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B, AMPERE_B, AMPERE_COMPUTE_B],
                    ce_class: AMPERE_DMA_COPY_A,
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

struct AllocInner {
    bits: BitVec
}

#[pin_data]
#[repr(C)]
pub(crate) struct AllocId {
    #[pin]
    inner: Mutex<AllocInner>
}

impl AllocId {
    pub(crate) fn new() -> Result<Arc<AllocId>> {
        let bits = BitVec::new(64)?;
        Ok(Arc::pin_init(pin_init!(AllocId {
            inner <- new_mutex!(AllocInner {
                bits
            })
        }), GFP_KERNEL)?)
    }

    pub(crate) fn alloc(&self) -> u32 {
        let mut locked = self.inner.lock();
        let res = locked.bits.ffz();
        locked.bits.set_bit(res, true);
        res as u32
//        (res.wrapping_sub(0)) as u32
    }

    pub(crate) fn free(&self, handle: u32) {
        let mut locked = self.inner.lock();
        locked.bits.set_bit((handle & 0xffff) as usize, false);
    }
}

#[repr(C)]
pub(crate) struct GpuDevice {
    pub gsp: Arc<GspDevice>,
    pub mgr: Arc<dyn GspManager>,
}

impl Drop for GpuDevice {
    fn drop(&mut self) {
        let _ = self.mgr.free_device(self.gsp.clone());
    }
}

#[repr(C)]
pub(crate) struct GpuClient {
    pub gsp: Arc<GspClient>,
    pub allocator: Arc<AllocId>,
    mgr: Arc<dyn GspManager>,
}

impl Drop for GpuClient {
    fn drop(&mut self) {

        self.allocator.free(self.gsp.get_client_handle().unwrap() & 0xffff);
        let _ = self.mgr.free_client(self.gsp.clone());

    }
}

#[repr(C)]
pub(crate) struct GpuChanObject {
    pub gsp: Arc<GspObject>,
    pub mgr: Arc<dyn GspManager>,
}

impl Drop for GpuChanObject {
    fn drop(&mut self) {
        let _ = self.mgr.free_chan_obj(&self.gsp);
    }
}

#[repr(C)]
pub(crate) struct GpuDeviceVmm {
    pub vmm: Vmm,
    pub va: GspVa,
}

impl Gpu {

    pub(crate) fn alloc_mmu(&self) -> Result<Arc<Mmu>> {
        let size = (self.instmem.vram_mm.size(0)? << NVKM_MM_PAGE_SHIFT) as u64;
        let mmu = Mmu::new(self.base.clone(), size)?;
        Ok(Arc::new(mmu, GFP_KERNEL)?)
    }

    pub(crate) fn alloc_vmm(&self, device: Arc<GpuDevice>,
                            addr: u64, size: u64, vmm_type: u8) -> Result<Arc<GpuDeviceVmm>> {
        let vmm = Vmm::new(self.instmem.clone(), addr, size, vmm_type, false, false, None,
                           None, true, "uvmm")?;

        let va = self.gsp.alloc_vaspace(device.gsp.clone(), &vmm)?;

        Ok(Arc::new(GpuDeviceVmm {
            vmm,
            va
        }, GFP_KERNEL)?)
    }

    pub(crate) fn int_alloc_client_device(alloc_id: Arc<AllocId>, gsp: Arc<dyn GspManager>) -> Result<(Arc<GpuClient>, Arc<GpuDevice>)> {

        let client_id = alloc_id.alloc();
        let (gsp_client, gsp_device) = gsp.alloc_client_device(client_id)?;

        Ok((Arc::new(GpuClient { gsp: gsp_client, allocator: alloc_id.clone(), mgr: gsp.clone() }, GFP_KERNEL)?,
            Arc::new(GpuDevice { gsp: gsp_device, mgr: gsp.clone() }, GFP_KERNEL)?))
    }

    pub(crate) fn alloc_client_device(&self) -> Result<(Arc<GpuClient>, Arc<GpuDevice>)> {
        Self::int_alloc_client_device(self.alloc_id.clone(), self.gsp.clone())
    }

    pub(crate) fn create_channel(&self,
                                 client: Arc<GpuClient>,
                                 device: Arc<GpuDevice>,
                                 runl: u32,
                                 chan_priv: bool,
                                 offset: u64,
                                 length: u64,
                                 vmm: Arc<GpuDeviceVmm>,
                                 userd: &VramObj) -> Result<Arc<Channel>> {

        Ok(Arc::new(Channel::new(self, client, device, &vmm, userd, runl, offset, length, chan_priv)?, GFP_KERNEL)?)
    }

    pub(crate) fn create_channel_obj(&self,
                                     channel: &Channel,
                                     handle: u32,
                                     oclass: u32,
                                     engine_type: EngineType,
                                     engine_inst: u8) -> Result<Arc<GpuChanObject>> {
        channel.alloc_obj(handle, oclass, engine_type, engine_inst)
    }

    pub(crate) fn get_engine_bitmap(&self) -> u64 {
        self.gsp.get_engine_bitmap()
    }

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

        let mmu = Arc::new(Mmu::new(base.clone(), (vram_mm.size(0)? << NVKM_MM_PAGE_SHIFT) as u64)?, GFP_KERNEL)?;
        let instmem = InstMem::new(base.clone(), vram_mm.clone(),
                                   pdev.resource_start(3)?)?;

        let bars = Arc::new(Bar::new(instmem.clone(),
                                     gsp.clone(),
                                     pdev.resource_len(1)?,
                                     Some(pdev.resource_len(3)?),
                                     pdev.resource_start(3)?)?, GFP_KERNEL)?;

        let id_allocator = AllocId::new()?;
        instmem.set_bar(bars.clone())?;

	#[cfg(CONFIG_NOVA_CORE_VGPU_SUPPORT)]
	let vgpu = VGPUMgr::new(gsp.get_vmmu_segment_size())?;
	#[cfg(not(CONFIG_NOVA_CORE_VGPU_SUPPORT))]
	let vgpu = false;
        {
            let bar = base.bar.try_access().ok_or(ENXIO)?;
            bar.try_writel(0x40, 0x110004)?;
        }

        let gr_ctx_bufs = Gr::golden_init(instmem.clone(), gsp.clone(), id_allocator.clone())?;
        Ok(pin_init!(Self { base, vfn, gsp, mmu, bar: bars, instmem, vgpu, gr_ctx_bufs, alloc_id: id_allocator }))
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
