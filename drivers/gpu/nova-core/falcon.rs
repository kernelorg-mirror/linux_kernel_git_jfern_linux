// SPDX-License-Identifier: GPL-2.0

//! Falcon microprocessor base support

use core::time::Duration;
use hal::FalconHal;
use kernel::bindings;
use kernel::device;
use kernel::devres::Devres;
use kernel::prelude::*;
use kernel::sync::Arc;

use crate::driver::Bar0;
use crate::gpu::Chipset;
use crate::regs;
use crate::util;

pub(crate) mod gsp;
mod hal;
pub(crate) mod sec2;

/// Revision number of a falcon core, used in the [`crate::regs::NV_PFALCON_FALCON_HWCFG1`]
/// register.
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

impl TryFrom<u8> for FalconCoreRev {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
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

/// Revision subversion number of a falcon core, used in the
/// [`crate::regs::NV_PFALCON_FALCON_HWCFG1`] register.
#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FalconCoreRevSubversion {
    #[default]
    Subversion0 = 0,
    Subversion1 = 1,
    Subversion2 = 2,
    Subversion3 = 3,
}

impl TryFrom<u8> for FalconCoreRevSubversion {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        use FalconCoreRevSubversion::*;

        let sub_version = match value & 0b11 {
            0 => Subversion0,
            1 => Subversion1,
            2 => Subversion2,
            3 => Subversion3,
            _ => return Err(EINVAL),
        };

        Ok(sub_version)
    }
}

/// Security model of a falcon core, used in the [`crate::regs::NV_PFALCON_FALCON_HWCFG1`]
/// register.
#[repr(u8)]
#[derive(Debug, Default, Copy, Clone)]
/// Security mode of the Falcon microprocessor.
/// See falcon.rst for more details.
pub(crate) enum FalconSecurityModel {
    /// Non-Secure: runs unsigned code without privileges.
    #[default]
    None = 0,
    /// Light-Secured (LS): runs signed code with some privileges
    /// Its signature can only be verified and entered from `Heavy` mode.
    /// Also known as Privilege Level 2 or PL2.
    Light = 2,
    /// Heavy-Secured: runs signed code with full privileges.
    /// Its signature can only be verified by the Falcon Boot ROM (BROM).
    /// Also known as Privilege Level 3 or PL3.
    Heavy = 3,
}

impl TryFrom<u8> for FalconSecurityModel {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
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

/// Signing algorithm for a given firmware, used in the [`crate::regs::NV_PFALCON2_FALCON_MOD_SEL`]
/// register. It is passed to the Falcon Boot ROM (BROM) as a parameter.
#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub(crate) enum FalconModSelAlgo {
    /// AES.
    #[expect(dead_code)]
    Aes = 0,
    /// RSA3K.
    #[default]
    Rsa3k = 1,
}

impl TryFrom<u8> for FalconModSelAlgo {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(FalconModSelAlgo::Rsa3k),
            _ => Err(EINVAL),
        }
    }
}

/// Valid values for the `size` field of the [`crate::regs::NV_PFALCON_FALCON_DMATRFCMD`] register.
#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub(crate) enum DmaTrfCmdSize {
    /// 256 bytes transfer.
    #[default]
    Size256B = 0x6,
}

impl TryFrom<u8> for DmaTrfCmdSize {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x6 => Ok(Self::Size256B),
            _ => Err(EINVAL),
        }
    }
}

/// Currently active core on a dual falcon/riscv (Peregrine) controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeregrineCoreSelect {
    /// Falcon core is active.
    Falcon = 0,
    /// RISC-V core is active.
    Riscv = 1,
}

impl From<bool> for PeregrineCoreSelect {
    fn from(value: bool) -> Self {
        match value {
            false => PeregrineCoreSelect::Falcon,
            true => PeregrineCoreSelect::Riscv,
        }
    }
}

/// Different types of memory present in a falcon core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FalconMem {
    /// Instruction Memory.
    Imem,
    /// Data Memory.
    Dmem,
}

/// FBIF (Framebuffer Interface) aperture type. Used to determine
/// the memory type of the external memory access for a DMA memory
/// transfer (by the Falcon's Framebuffer DMA (FBDMA) engine located
/// inside the falcon). See falcon.rst for more details.
#[derive(Debug, Clone, Default)]
pub(crate) enum FalconFbifTarget {
    /// VRAM.
    #[default]
    /// Local Framebuffer (GPU's VRAM memory)
    LocalFb = 0,
    /// Coherent system memory (System DRAM).
    CoherentSysmem = 1,
    /// Non-coherent system memory (System DRAM).
    NoncoherentSysmem = 2,
}

impl TryFrom<u8> for FalconFbifTarget {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        let res = match value {
            0 => Self::LocalFb,
            1 => Self::CoherentSysmem,
            2 => Self::NoncoherentSysmem,
            _ => return Err(EINVAL),
        };

        Ok(res)
    }
}

/// Type of memory addresses to use.
#[derive(Debug, Clone, Default)]
pub(crate) enum FalconFbifMemType {
    /// Physical memory addresses.
    #[default]
    Virtual = 0,
    /// Virtual memory addresses.
    Physical = 1,
}

impl From<bool> for FalconFbifMemType {
    fn from(value: bool) -> Self {
        match value {
            false => Self::Virtual,
            true => Self::Physical,
        }
    }
}

/// Trait defining the parameters of a given Falcon instance.
pub(crate) trait FalconEngine: Sync {
    /// Base I/O address for the falcon, relative from which its registers are accessed.
    const BASE: usize;
}

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

/// Parameters for the falcon boot ROM.
#[derive(Debug)]
pub(crate) struct FalconBromParams {
    /// Offset in `DMEM`` of the firmware's signature.
    pub(crate) pkc_data_offset: u32,
    /// Mask of engines valid for this firmware.
    pub(crate) engine_id_mask: u16,
    /// ID of the ucode used to infer a fuse register to validate the signature.
    pub(crate) ucode_id: u8,
}

/// Trait for a falcon firmware.
pub(crate) trait FalconFirmware {
    /// Engine on which this firmware is to be loaded.
    type Target: FalconEngine;

    /// Returns the DMA handle of the object containing the firmware.
    fn dma_handle(&self) -> bindings::dma_addr_t;

    /// Returns the load parameters for `IMEM`.
    fn imem_load(&self) -> FalconLoadTarget;

    /// Returns the load parameters for `DMEM`.
    fn dmem_load(&self) -> FalconLoadTarget;

    /// Returns the parameters to write into the BROM registers.
    fn brom_params(&self) -> FalconBromParams;

    /// Returns the start address of the firmware.
    fn boot_addr(&self) -> u32;
}

/// Contains the base parameters common to all Falcon instances.
pub(crate) struct Falcon<E: FalconEngine> {
    hal: Arc<dyn FalconHal<E>>,
}

impl<E: FalconEngine + 'static> Falcon<E> {
    /// Create a new falcon instance.
    ///
    /// `need_riscv` is set to `true` if the caller expects the falcon to be a dual falcon/riscv
    /// controller.
    pub(crate) fn new(
        dev: &device::Device,
        chipset: Chipset,
        bar: &Devres<Bar0>,
        need_riscv: bool,
    ) -> Result<Self> {
        let hwcfg1 = with_bar!(bar, |b| regs::NV_PFALCON_FALCON_HWCFG1::read(b, E::BASE))?;
        // Ensure that the revision and security model contain valid values.
        let _rev = hwcfg1.core_rev()?;
        let _sec_model = hwcfg1.security_model()?;

        if need_riscv {
            let hwcfg2 = with_bar!(bar, |b| regs::NV_PFALCON_FALCON_HWCFG2::read(b, E::BASE))?;
            if !hwcfg2.riscv() {
                dev_err!(
                    dev,
                    "riscv support requested on a controller that does not support it\n"
                );
                return Err(EINVAL);
            }
        }

        Ok(Self {
            hal: hal::create_falcon_hal(chipset)?,
        })
    }

    /// Wait for memory scrubbing to complete.
    fn reset_wait_mem_scrubbing(&self, bar: &Devres<Bar0>) -> Result<()> {
        util::wait_on(Duration::from_millis(20), || {
            bar.try_access_with(|b| regs::NV_PFALCON_FALCON_HWCFG2::read(b, E::BASE))
                .and_then(|r| if r.mem_scrubbing() { Some(()) } else { None })
        })
    }

    /// Reset the falcon engine.
    fn reset_eng(&self, bar: &Devres<Bar0>) -> Result<()> {
        let _ = with_bar!(bar, |b| regs::NV_PFALCON_FALCON_HWCFG2::read(b, E::BASE))?;

        // According to OpenRM's `kflcnPreResetWait_GA102` documentation, HW sometimes does not set
        // RESET_READY so a non-failing timeout is used.
        let _ = util::wait_on(Duration::from_micros(150), || {
            bar.try_access_with(|b| regs::NV_PFALCON_FALCON_HWCFG2::read(b, E::BASE))
                .and_then(|r| if r.reset_ready() { Some(()) } else { None })
        });

        with_bar!(bar, |b| regs::NV_PFALCON_FALCON_ENGINE::alter(
            b,
            E::BASE,
            |v| v.set_reset(true)
        ))?;

        let _: Result<()> = util::wait_on(Duration::from_micros(10), || None);

        with_bar!(bar, |b| regs::NV_PFALCON_FALCON_ENGINE::alter(
            b,
            E::BASE,
            |v| v.set_reset(false)
        ))?;

        self.reset_wait_mem_scrubbing(bar)?;

        Ok(())
    }

    /// Reset the controller, select the falcon core, and wait for memory scrubbing to complete.
    pub(crate) fn reset(&self, bar: &Devres<Bar0>) -> Result<()> {
        self.reset_eng(bar)?;
        self.hal.select_core(bar)?;
        self.reset_wait_mem_scrubbing(bar)?;

        with_bar!(bar, |b| {
            regs::NV_PFALCON_FALCON_RM::default()
                .set_value(regs::NV_PMC_BOOT_0::read(b).into())
                .write(b, E::BASE)
        })
    }

    /// Perform a DMA write according to `load_offsets` from `dma_handle` into the falcon's
    /// `target_mem`.
    ///
    /// `sec` is set if the loaded firmware is expected to run in secure mode.
    fn dma_wr(
        &self,
        bar: &Devres<Bar0>,
        dma_handle: bindings::dma_addr_t,
        target_mem: FalconMem,
        load_offsets: FalconLoadTarget,
        sec: bool,
    ) -> Result<()> {
        const DMA_LEN: u32 = 256;

        // For IMEM, we want to use the start offset as a virtual address tag for each page, since
        // code addresses in the firmware (and the boot vector) are virtual.
        //
        // For DMEM we can fold the start offset into the DMA handle.
        let (src_start, dma_start) = match target_mem {
            FalconMem::Imem => (load_offsets.src_start, dma_handle),
            FalconMem::Dmem => (
                0,
                dma_handle + load_offsets.src_start as bindings::dma_addr_t,
            ),
        };
        if dma_start % DMA_LEN as bindings::dma_addr_t > 0 {
            dev_err!(
                bar.as_ref(),
                "DMA transfer start addresses must be a multiple of {}",
                DMA_LEN
            );
            return Err(EINVAL);
        }
        if load_offsets.len % DMA_LEN > 0 {
            dev_err!(
                bar.as_ref(),
                "DMA transfer length must be a multiple of {}",
                DMA_LEN
            );
            return Err(EINVAL);
        }

        // Set up the base source DMA address.
        with_bar!(bar, |b| {
            regs::NV_PFALCON_FALCON_DMATRFBASE::default()
                .set_base((dma_start >> 8) as u32)
                .write(b, E::BASE);
            regs::NV_PFALCON_FALCON_DMATRFBASE1::default()
                .set_base((dma_start >> 40) as u16)
                .write(b, E::BASE)
        })?;

        let cmd = regs::NV_PFALCON_FALCON_DMATRFCMD::default()
            .set_size(DmaTrfCmdSize::Size256B)
            .set_imem(target_mem == FalconMem::Imem)
            .set_sec(if sec { 1 } else { 0 });

        for pos in (0..load_offsets.len).step_by(DMA_LEN as usize) {
            // Perform a transfer of size `DMA_LEN`.
            with_bar!(bar, |b| {
                regs::NV_PFALCON_FALCON_DMATRFMOFFS::default()
                    .set_offs(load_offsets.dst_start + pos)
                    .write(b, E::BASE);
                regs::NV_PFALCON_FALCON_DMATRFFBOFFS::default()
                    .set_offs(src_start + pos)
                    .write(b, E::BASE);
                cmd.write(b, E::BASE)
            })?;

            // Wait for the transfer to complete.
            util::wait_on(Duration::from_millis(2000), || {
                bar.try_access_with(|b| regs::NV_PFALCON_FALCON_DMATRFCMD::read(b, E::BASE))
                    .and_then(|v| if v.idle() { Some(()) } else { None })
            })?;
        }

        Ok(())
    }

    /// Perform a DMA load into `IMEM` and `DMEM` of `fw`, and prepare the falcon to run it.
    pub(crate) fn dma_load<F: FalconFirmware<Target = E>>(
        &self,
        bar: &Devres<Bar0>,
        fw: &F,
    ) -> Result<()> {
        let dma_handle = fw.dma_handle();

        with_bar!(bar, |b| {
            regs::NV_PFALCON_FBIF_CTL::alter(b, E::BASE, |v| v.set_allow_phys_no_ctx(true));
            regs::NV_PFALCON_FALCON_DMACTL::default().write(b, E::BASE);
            regs::NV_PFALCON_FBIF_TRANSCFG::alter(b, E::BASE, |v| {
                v.set_target(FalconFbifTarget::CoherentSysmem)
                    .set_mem_type(FalconFbifMemType::Physical)
            });
        })?;

        self.dma_wr(bar, dma_handle, FalconMem::Imem, fw.imem_load(), true)?;
        self.dma_wr(bar, dma_handle, FalconMem::Dmem, fw.dmem_load(), true)?;

        self.hal.program_brom(bar, &fw.brom_params())?;

        with_bar!(bar, |b| {
            // Set `BootVec` to start of non-secure code.
            regs::NV_PFALCON_FALCON_BOOTVEC::default()
                .set_value(fw.boot_addr())
                .write(b, E::BASE);
        })?;

        Ok(())
    }

    /// Start running the loaded firmware.
    ///
    /// `mbox0` and `mbox1` are optional parameters to write into the `MBOX0` and `MBOX1` registers
    /// prior to running.
    ///
    /// Returns `MBOX0` and `MBOX1` after the firmware has stopped running.
    pub(crate) fn boot(
        &self,
        bar: &Devres<Bar0>,
        mbox0: Option<u32>,
        mbox1: Option<u32>,
    ) -> Result<(u32, u32)> {
        with_bar!(bar, |b| {
            if let Some(mbox0) = mbox0 {
                regs::NV_PFALCON_FALCON_MAILBOX0::default()
                    .set_value(mbox0)
                    .write(b, E::BASE);
            }

            if let Some(mbox1) = mbox1 {
                regs::NV_PFALCON_FALCON_MAILBOX1::default()
                    .set_value(mbox1)
                    .write(b, E::BASE);
            }

            match regs::NV_PFALCON_FALCON_CPUCTL::read(b, E::BASE).alias_en() {
                true => regs::NV_PFALCON_FALCON_CPUCTL_ALIAS::default()
                    .set_startcpu(true)
                    .write(b, E::BASE),
                false => regs::NV_PFALCON_FALCON_CPUCTL::default()
                    .set_startcpu(true)
                    .write(b, E::BASE),
            }
        })?;

        util::wait_on(Duration::from_secs(2), || {
            bar.try_access()
                .map(|b| regs::NV_PFALCON_FALCON_CPUCTL::read(&*b, E::BASE))
                .and_then(|v| if v.halted() { Some(()) } else { None })
        })?;

        let (mbox0, mbox1) = with_bar!(bar, |b| {
            let mbox0 = regs::NV_PFALCON_FALCON_MAILBOX0::read(b, E::BASE).value();
            let mbox1 = regs::NV_PFALCON_FALCON_MAILBOX1::read(b, E::BASE).value();

            (mbox0, mbox1)
        })?;

        Ok((mbox0, mbox1))
    }

    /// Returns the fused version of the signature to use in order to run a HS firmware on this
    /// falcon instance. `engine_id_mask` and `ucode_id` are obtained from the firmware header.
    pub(crate) fn get_signature_reg_fuse_version(
        &self,
        bar: &Devres<Bar0>,
        engine_id_mask: u16,
        ucode_id: u8,
    ) -> Result<u32> {
        self.hal
            .get_signature_reg_fuse_version(bar, engine_id_mask, ucode_id)
    }
}
