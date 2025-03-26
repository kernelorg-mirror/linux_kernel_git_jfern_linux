// SPDX-License-Identifier: GPL-2.0

//! Falcon microprocessor base support

use core::hint::unreachable_unchecked;
use core::marker::PhantomData;
use core::time::Duration;
use kernel::bindings;
use kernel::devres::Devres;
use kernel::{pci, prelude::*};

use crate::dma::DmaObject;
use crate::driver::Bar0;
use crate::gpu::Chipset;
use crate::regs;
use crate::timer::Timer;

pub(crate) mod gsp;

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FalconCoreRev {
    #[default]
    Rev1 = 1,
    Rev2 = 2,
    Rev3 = 3,
    Rev4 = 4,
    Rev5 = 5,
    Rev6 = 6,
    Rev7 = 7,
}

impl TryFrom<u32> for FalconCoreRev {
    type Error = Error;

    fn try_from(value: u32) -> core::result::Result<Self, Self::Error> {
        use FalconCoreRev::*;

        let rev = match value {
            1 => Rev1,
            2 => Rev2,
            3 => Rev3,
            4 => Rev4,
            5 => Rev5,
            6 => Rev6,
            7 => Rev7,
            _ => return Err(EINVAL),
        };

        Ok(rev)
    }
}

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone)]
pub(crate) enum FalconSecurityModel {
    #[default]
    None = 0,
    Light = 2,
    Heavy = 3,
}

impl TryFrom<u32> for FalconSecurityModel {
    type Error = Error;

    fn try_from(value: u32) -> core::result::Result<Self, Self::Error> {
        use FalconSecurityModel::*;

        let sec_model = match value {
            0 => None,
            2 => Light,
            3 => Heavy,
            _ => return Err(EINVAL),
        };

        Ok(sec_model)
    }
}

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FalconCoreRevSubversion {
    #[default]
    Subversion0 = 0,
    Subversion1 = 1,
    Subversion2 = 2,
    Subversion3 = 3,
}

impl From<u32> for FalconCoreRevSubversion {
    fn from(value: u32) -> Self {
        use FalconCoreRevSubversion::*;

        match value & 0b11 {
            0 => Subversion0,
            1 => Subversion1,
            2 => Subversion2,
            3 => Subversion3,
            // SAFETY: the `0b11` mask limits the possible values to `0..=3`.
            4..=u32::MAX => unsafe { unreachable_unchecked() },
        }
    }
}

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub(crate) enum FalconModSelAlgo {
    #[default]
    Rsa3k = 1,
}

impl TryFrom<u32> for FalconModSelAlgo {
    type Error = Error;

    fn try_from(value: u32) -> core::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(FalconModSelAlgo::Rsa3k),
            _ => Err(EINVAL),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RiscvCoreSelect {
    Falcon = 0,
    Riscv = 1,
}

impl From<bool> for RiscvCoreSelect {
    fn from(value: bool) -> Self {
        match value {
            false => RiscvCoreSelect::Falcon,
            true => RiscvCoreSelect::Riscv,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FalconMem {
    Imem,
    Dmem,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub(crate) struct FalconUCodeDescV3 {
    pub(crate) hdr: u32,
    pub(crate) stored_size: u32,
    pub(crate) pkc_data_offset: u32, // dmem address for copying signatures
    pub(crate) interface_offset: u32,
    pub(crate) imem_phys_base: u32,
    pub(crate) imem_load_size: u32,
    pub(crate) imem_virt_base: u32,
    pub(crate) dmem_phys_base: u32,
    pub(crate) dmem_load_size: u32,
    pub(crate) engine_id_mask: u16,
    pub(crate) ucode_id: u8,
    pub(crate) signature_count: u8,
    pub(crate) signature_versions: u16,
    _reserved: u16,
}

impl FalconUCodeDescV3 {
    pub(crate) fn size(&self) -> usize {
        ((self.hdr & 0xffff0000) >> 16) as usize
    }
}

/// Trait defining the parameters of a given Falcon instance.
pub(crate) trait FalconEngine {
    /// Base I/O address for the falcon, relative from which its registers are accessed.
    const BASE: usize;
}

pub(crate) struct Sec2;
impl FalconEngine for Sec2 {
    const BASE: usize = 0x00840000;
}
pub(crate) type Sec2Falcon = Falcon<Sec2>;

/// Represents a portion of the firmware to be loaded into a particular memory (e.g. IMEM or DMEM).
#[derive(Debug)]
pub(crate) struct FalconLoadTarget {
    /// Offset from the start of the source object to copy from.
    pub(crate) src_start: u32,
    /// Offset from the start of the destination memory to copy into.
    pub(crate) dst_start: u32,
    /// Number of bytes to copy.
    pub(crate) len: u32,
}

#[derive(Debug)]
pub(crate) struct FalconBromParams {
    pub(crate) pkc_data_offset: u32,
    pub(crate) engine_id_mask: u16,
    pub(crate) ucode_id: u8,
}

// TODO: for DMA, create the DMA object when we actually load? This makes it possible to use PIO
// without relying on DMA objects.

pub(crate) trait FalconFirmware {
    type Target: FalconEngine;

    fn dma_handle(&self) -> bindings::dma_addr_t;

    /// Returns the load parameters for `IMEM`.
    fn imem_load(&self) -> FalconLoadTarget;

    /// Returns the load parameters for `DMEM`.
    fn dmem_load(&self) -> FalconLoadTarget;

    fn brom_params(&self) -> FalconBromParams;
}

/// Contains the base parameters common to all Falcon instances.
#[derive(Debug)]
pub(crate) struct Falcon<I: FalconEngine> {
    /// Chipset this falcon belongs to.
    chipset: Chipset,
    /// Whether this falcon is part of a dual falcon/riscv engine.
    has_riscv: bool,
    _instance: PhantomData<I>,
}

impl<I: FalconEngine> Falcon<I> {
    pub(crate) fn new(
        pdev: &pci::Device,
        chipset: Chipset,
        bar: &Devres<Bar0>,
        need_riscv: bool,
    ) -> Result<Self> {
        let hwcfg1 = with_bar!(bar, |b| regs::FalconHwcfg1::read(b, I::BASE))?;
        let rev = hwcfg1.core_rev()?;
        let subver = hwcfg1.core_rev_subversion();
        let sec_model = hwcfg1.security_model()?;

        if need_riscv {
            let hwcfg2 = with_bar!(bar, |b| regs::FalconHwcfg2::read(b, I::BASE))?;
            if !hwcfg2.riscv() {
                dev_err!(
                    pdev.as_ref(),
                    "riscv support requested on falcon that does not support it\n"
                );
                return Err(EINVAL);
            }
        }

        dev_info!(
            pdev.as_ref(),
            "new falcon: {:?} {:?} {:?}",
            rev,
            subver,
            sec_model
        );

        Ok(Self {
            chipset,
            has_riscv: need_riscv,
            _instance: PhantomData,
        })
    }

    fn select_falcon_core(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        if self.has_riscv {
            let bcr_ctrl = with_bar!(bar, |b| regs::RiscvBcrCtrl::read(b, I::BASE))?;
            if bcr_ctrl.core_select() != RiscvCoreSelect::Falcon {
                with_bar!(bar, |b| regs::RiscvBcrCtrl::default()
                    .set_core_select(RiscvCoreSelect::Falcon)
                    .write(b, I::BASE))?;

                timer.wait_on(bar, Duration::from_millis(10), || {
                    bar.try_access_with(|b| regs::RiscvBcrCtrl::read(b, I::BASE))
                        .and_then(|v| if v.valid() { Some(()) } else { None })
                })?;
            }
        }

        Ok(())
    }

    fn reset_wait_mem_scrubbing(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        /* TODO: is this needed? */
        with_bar!(bar, |b| regs::FalconMailbox0::alter(b, I::BASE, |v| v))?;

        timer.wait_on(bar, Duration::from_millis(20), || {
            bar.try_access_with(|b| regs::FalconHwcfg2::read(b, I::BASE))
                .and_then(|r| if r.mem_scrubbing() { Some(()) } else { None })
        })
    }

    fn reset_prep(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        let _ = with_bar!(bar, |b| regs::FalconHwcfg2::read(b, I::BASE))?;

        // Expected to timeout apparently?
        // TODO: check why with OpenRM.
        let _ = timer.wait_on(bar, Duration::from_micros(150), || {
            bar.try_access_with(|b| regs::FalconHwcfg2::read(b, I::BASE))
                .and_then(|r| if r.unk_31() { Some(()) } else { None })
        });

        Ok(())
    }

    fn reset_eng(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        self.reset_prep(bar, timer)?;

        with_bar!(bar, |b| regs::RiscvUnk3c0::alter(b, I::BASE, |v| v
            .set_unk0(true)))?;

        let _: Result<()> = timer.wait_on(bar, Duration::from_micros(10), || None);

        with_bar!(bar, |b| regs::RiscvUnk3c0::alter(b, I::BASE, |v| v
            .set_unk0(false)))?;

        self.reset_wait_mem_scrubbing(bar, timer)?;

        Ok(())
    }

    fn disable(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        self.select_falcon_core(bar, timer)?;

        with_bar!(bar, |b| {
            regs::FalconUnk0048::alter(b, I::BASE, |r| r.set_val0(0));

            regs::FalconIrqmclr::default()
                .set_val(u32::MAX)
                .write(b, I::BASE)
        })?;

        self.reset_eng(bar, timer)
    }

    fn enable(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        self.reset_eng(bar, timer)?;
        self.select_falcon_core(bar, timer)?;
        self.reset_wait_mem_scrubbing(bar, timer)?;

        with_bar!(bar, |b| {
            // We write Boot0 into FalconRm, for some reason...
            regs::FalconRm::default()
                .set_val(regs::Boot0::read(b).into())
                .write(b, I::BASE)
        })
    }

    pub(crate) fn reset(&self, bar: &Devres<Bar0>, timer: &Timer) -> Result<()> {
        self.disable(bar, timer)?;
        self.enable(bar, timer)
    }

    fn dma_wr(
        &self,
        bar: &Devres<Bar0>,
        timer: &Timer,
        dma_handle: bindings::dma_addr_t,
        target_mem: FalconMem,
        load_offsets: FalconLoadTarget,
        sec: bool,
    ) -> Result<()> {
        const DMA_LEN: u32 = 256;
        const DMA_LEN_ILOG2_MINUS2: u8 = (DMA_LEN.ilog2() - 2) as u8;

        let dma_start = dma_handle + load_offsets.src_start as bindings::dma_addr_t;
        if dma_start % DMA_LEN as bindings::dma_addr_t > 0 {
            pr_err!(
                "DMA transfer start addresses must be a multiple of {}",
                DMA_LEN
            );
            return Err(EINVAL);
        }
        if load_offsets.len % DMA_LEN > 0 {
            pr_err!("DMA transfer length must be a multiple of {}", DMA_LEN);
            return Err(EINVAL);
        }

        pr_info!(
            "dma write {:?}: dma_handle {:x} dma_start {:x} len {:x}\n",
            target_mem,
            dma_handle,
            dma_start,
            load_offsets.len
        );

        // Set up the base source DMA address.
        with_bar!(bar, |b| {
            regs::FalconDmaTrfBase::default()
                .set_base((dma_start >> 8) as u32)
                .write(b, I::BASE);
            regs::FalconDmaTrfBase1::default()
                .set_base((dma_start >> 40) as u16)
                .write(b, I::BASE)
        })?;

        let cmd = regs::FalconDmaTrfCmd::default()
            .set_size(DMA_LEN_ILOG2_MINUS2)
            .set_imem(target_mem == FalconMem::Imem)
            .set_sec(if sec { 1 } else { 0 });

        for pos in (0..load_offsets.len).step_by(DMA_LEN as usize) {
            // Perform a transfer of size `DMA_LEN`.
            with_bar!(bar, |b| {
                regs::FalconDmaTrfMOffs::default()
                    .set_offs(load_offsets.dst_start + pos)
                    .write(b, I::BASE);
                regs::FalconDmaTrfBOffs::default()
                    .set_offs(pos)
                    .write(b, I::BASE);
                cmd.write(b, I::BASE)
            })?;

            // Wait for the transfer to complete.
            timer.wait_on(bar, Duration::from_millis(2000), || {
                bar.try_access_with(|b| regs::FalconDmaTrfCmd::read(b, I::BASE))
                    .and_then(|v| if v.idle() { Some(()) } else { None })
            })?;
        }

        Ok(())
    }

    pub(crate) fn dma_load<F: FalconFirmware<Target = I>>(
        &self,
        bar: &Devres<Bar0>,
        timer: &Timer,
        fw: &F,
    ) -> Result<()> {
        let dma_handle = fw.dma_handle();

        with_bar!(bar, |b| {
            regs::FalconUnk624::alter(b, I::BASE, |v| v.set_unk7(true));
            regs::FalconDmaCtl::default().write(b, I::BASE);
            regs::FalconUnk600::alter(b, I::BASE, |v| v.set_unk16(false).set_unk2((1 << 2) | 1));
        })?;

        self.dma_wr(
            bar,
            timer,
            dma_handle,
            FalconMem::Imem,
            fw.imem_load(),
            true,
        )?;
        self.dma_wr(
            bar,
            timer,
            dma_handle,
            FalconMem::Dmem,
            fw.dmem_load(),
            true,
        )?;

        self.program_brom(bar, &fw.brom_params())?;

        Ok(())
    }

    /// Programs BROM registers for PKC signature validation.
    fn program_brom(&self, bar: &Devres<Bar0>, params: &FalconBromParams) -> Result<()> {
        if self.chipset >= Chipset::GA102 {
            with_bar!(bar, |b| {
                regs::FalconBromParaaddr0::default()
                    .set_addr(params.pkc_data_offset)
                    .write(b, I::BASE);
                regs::FalconBromEngidmask::default()
                    .set_mask(params.engine_id_mask as u32)
                    .write(b, I::BASE);
                regs::FalconBromCurrUcodeId::default()
                    .set_ucode_id(params.ucode_id as u32)
                    .write(b, I::BASE);
                regs::FalconModSel::default()
                    .set_algo(FalconModSelAlgo::Rsa3k)
                    .write(b, I::BASE);
            })?;
        }

        Ok(())
    }

    pub(crate) fn boot(
        &self,
        bar: &Devres<Bar0>,
        timer: &Timer,
        mbox0: Option<u32>,
        mbox1: Option<u32>,
    ) -> Result<(u32, u32)> {
        pr_info!("booting falcon...\n");

        with_bar!(bar, |b| {
            if let Some(mbox0) = mbox0 {
                regs::FalconMailbox0::default()
                    .set_mailbox0(mbox0)
                    .write(b, I::BASE);
            }

            if let Some(mbox1) = mbox1 {
                regs::FalconMailbox1::default()
                    .set_mailbox1(mbox1)
                    .write(b, I::BASE);
            }

            // Set `BootVec` to start of non-secure code.
            // TODO: use boot vector variable - apparently this is 0 on v3 hdr?
            regs::FalconBootVec::default()
                .set_boot_vec(0)
                .write(b, I::BASE);

            regs::FalconCpuCtl::default()
                .set_start_cpu(true)
                .write(b, I::BASE);
        })?;

        pr_info!("booted!\n");
        timer.wait_on(bar, Duration::from_secs(2), || {
            bar.try_access()
                .map(|b| regs::FalconCpuCtl::read(&*b, I::BASE))
                .and_then(|v| if v.halted() { Some(()) } else { None })
        })?;

        let (mbox0, mbox1) = with_bar!(bar, |b| {
            let mbox0 = regs::FalconMailbox0::read(b, I::BASE).mailbox0();
            let mbox1 = regs::FalconMailbox1::read(b, I::BASE).mailbox1();

            (mbox0, mbox1)
        })?;

        pr_info!("successfully returned {} {}\n", mbox0, mbox1);

        Ok((mbox0, mbox1))
    }
}

#[repr(C)]
#[derive(Debug)]
struct FalconAppifHdrV1 {
    ver: u8,
    hdr: u8,
    len: u8,
    cnt: u8,
}

#[repr(C, packed)]
#[derive(Debug)]
struct FalconAppifV1 {
    id: u32,
    dmem_base: u32,
}

const NVFW_FALCON_APPIF_ID_DMEMMAPPER: u32 = 0x4;

#[derive(Debug)]
#[repr(C, packed)]
struct FalconAppifDmemmapperV3 {
    signature: u32,
    version: u16,
    size: u16,
    cmd_in_buffer_offset: u32,
    cmd_in_buffer_size: u32,
    cmd_out_buffer_offset: u32,
    cmd_out_buffer_size: u32,
    nvf_img_data_buffer_offset: u32,
    nvf_img_data_buffer_size: u32,
    printf_buffer_hdr: u32,
    ucode_build_time_stamp: u32,
    ucode_signature: u32,
    init_cmd: u32,
    ucode_feature: u32,
    ucode_cmd_mask0: u32,
    ucode_cmd_mask1: u32,
    multi_tgt_tbl: u32,
}

pub(crate) const NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS: u32 = 0x15;

#[derive(Debug)]
#[repr(C, packed)]
struct ReadVbios {
    ver: u32,
    hdr: u32,
    addr: u64,
    size: u32,
    flags: u32,
}

#[derive(Debug)]
#[repr(C, packed)]
struct FrtsRegion {
    ver: u32,
    hdr: u32,
    addr: u32,
    size: u32,
    ftype: u32,
}

const NVFW_FRTS_CMD_REGION_TYPE_FB: u32 = 2;

#[repr(C, packed)]
struct FrtsCmd {
    read_vbios: ReadVbios,
    frts_region: FrtsRegion,
}

unsafe fn transmute<'a, 'b, T: Sized>(fw: &'a DmaObject, offset: usize) -> Result<&'b T> {
    if offset + core::mem::size_of::<T>() > fw.len {
        return Err(EINVAL);
    }
    // SAFETY: we just ensured that the object was within the bounds of the DMA
    // object.
    Ok(unsafe { &*(fw.dma.start_ptr().offset(offset as isize) as *const T) })
}

unsafe fn transmute_mut<'a, 'b, T: Sized>(
    fw: &'a mut DmaObject,
    offset: usize,
) -> Result<&'b mut T> {
    if offset + core::mem::size_of::<T>() > fw.len {
        return Err(EINVAL);
    }
    // SAFETY: we just ensured that the object was within the bounds of the DMA
    // object.
    Ok(unsafe { &mut *(fw.dma.start_ptr().offset(offset as isize) as *mut T) })
}

// TODO: use iterators for this?
pub(crate) fn patch_fw(
    fw: &mut DmaObject,
    v3_desc: &FalconUCodeDescV3,
    init_cmd: u32,
    frts_addr: u64,
    frts_size: u64,
) -> Result<()> {
    // TODO: check that the ptrs we create remain within the bounds of the DMA object!

    let hdr: &FalconAppifHdrV1 = unsafe {
        transmute(
            fw,
            (v3_desc.imem_load_size + v3_desc.interface_offset) as usize,
        )
    }?;

    pr_info!("{:?}", hdr);

    if hdr.ver != 1 {
        return Err(EINVAL);
    }

    for i in 0..hdr.cnt {
        let app = unsafe {
            &*((hdr as *const _ as *const u8).offset((hdr.hdr + i * hdr.len) as isize)
                as *const FalconAppifV1)
        };

        pr_info!("app: {:?}", app);

        if app.id != NVFW_FALCON_APPIF_ID_DMEMMAPPER {
            continue;
        }

        let dmem_mapper: &mut FalconAppifDmemmapperV3 =
            unsafe { transmute_mut(fw, (v3_desc.imem_load_size + app.dmem_base) as usize) }?;
        pr_info!("dmemmap: {:?}", dmem_mapper);

        dmem_mapper.init_cmd = init_cmd;

        let frts_cmd: &mut FrtsCmd = unsafe {
            transmute_mut(
                fw,
                (v3_desc.imem_load_size + dmem_mapper.cmd_in_buffer_offset) as usize,
            )
        }?;

        frts_cmd.read_vbios = ReadVbios {
            ver: 1,
            hdr: core::mem::size_of::<ReadVbios>() as u32,
            addr: 0,
            size: 0,
            flags: 2,
        };

        if init_cmd == NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS {
            frts_cmd.frts_region = FrtsRegion {
                ver: 1,
                hdr: core::mem::size_of::<FrtsRegion>() as u32,
                addr: (frts_addr >> 12) as u32,
                size: (frts_size >> 12) as u32,
                ftype: NVFW_FRTS_CMD_REGION_TYPE_FB,
            };
        }

        // Break early as we found the DMEMMAPPER region.
        break;
    }

    Ok(())
}
