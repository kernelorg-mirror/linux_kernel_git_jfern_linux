// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    dma::CoherentAllocation,
    dma_write,
    io::poll::read_poll_timeout,
    pci,
    prelude::*,
    time::Delta,
    transmute::FromBytes, //
};

use crate::{
    driver::Bar0,
    falcon::{
        fsp::Fsp as FspEngine,
        gsp::Gsp,
        sec2::Sec2,
        Falcon,
        FalconEngine,
        FalconFbifMemType,
        FalconFbifTarget,
        FalconMem,
        FalconModSelAlgo, //
    },
    fb::FbLayout,
    firmware::{
        booter::{
            BooterFirmware,
            BooterKind, //
        },
        fsp::FspFirmware,
        fwsec::{
            bootloader::FwsecFirmwareWithBl,
            FwsecCommand,
            FwsecFirmware, //
        },
        gsp::GspFirmware,
        radix3::Radix3,
        FIRMWARE_VERSION, //
    },
    fsp::{
        FmcBootArgs,
        Fsp, //
    },
    gpu::{
        Architecture,
        Chipset, //
    },
    gsp::{
        cmdq::Cmdq,
        commands::{
            self,
            GetGspStaticInfoReply, //
        },
        fw,
        fw::{
            LibosMemoryRegionInitArgument,
            MsgFunction, //
        },
        GspFwWprMeta, //
    },
    regs,
    vbios::Vbios,
};

/// GSP lockdown pattern written by firmware to mbox0 while RISC-V branch privilege
/// lockdown is active. The low byte varies, the upper 24 bits are fixed.
const GSP_LOCKDOWN_PATTERN: u32 = 0xbadf4100;
const GSP_LOCKDOWN_MASK: u32 = 0xffffff00;

/// GSP falcon mailbox state, used to track lockdown release status.
struct GspMbox {
    mbox0: u32,
    mbox1: u32,
}

impl GspMbox {
    /// Read both mailboxes from the GSP falcon.
    fn read(gsp_falcon: &Falcon<Gsp>, bar: &Bar0) -> Self {
        Self {
            mbox0: gsp_falcon.read_mailbox0(bar),
            mbox1: gsp_falcon.read_mailbox1(bar),
        }
    }

    /// Returns true if the lockdown pattern is present in mbox0.
    fn is_locked_down(&self) -> bool {
        self.mbox0 != 0 && (self.mbox0 & GSP_LOCKDOWN_MASK) == GSP_LOCKDOWN_PATTERN
    }

    /// Combines mailbox0 and mailbox1 into a 64-bit address.
    fn combined_addr(&self) -> u64 {
        (u64::from(self.mbox1) << 32) | u64::from(self.mbox0)
    }

    /// Returns true if GSP lockdown has been released.
    ///
    /// Checks the lockdown pattern, validates the boot params address,
    /// and verifies the HWCFG2 lockdown bit is clear.
    fn lockdown_released(&self, bar: &Bar0, fmc_boot_params_addr: u64) -> bool {
        if self.is_locked_down() {
            return false;
        }

        if self.mbox0 != 0 && self.combined_addr() != fmc_boot_params_addr {
            return true;
        }

        let hwcfg2 = regs::NV_PFALCON_FALCON_HWCFG2::read(bar, &crate::falcon::gsp::Gsp::ID);
        !hwcfg2.riscv_br_priv_lockdown()
    }
}

fn read_gsp_fbif_transcfg(bar: &Bar0, ctx_dma: u8) -> Result<regs::NV_PFALCON_FBIF_TRANSCFG> {
    Ok(match ctx_dma {
        0 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 0),
        1 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 1),
        2 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 2),
        3 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 3),
        4 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 4),
        5 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 5),
        6 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 6),
        7 => regs::NV_PFALCON_FBIF_TRANSCFG::read(bar, &Gsp::ID, 7),
        _ => return Err(EINVAL),
    })
}

fn write_gsp_fbif_transcfg(bar: &Bar0, ctx_dma: u8, reg: regs::NV_PFALCON_FBIF_TRANSCFG) -> Result {
    match ctx_dma {
        0 => reg.write(bar, &Gsp::ID, 0),
        1 => reg.write(bar, &Gsp::ID, 1),
        2 => reg.write(bar, &Gsp::ID, 2),
        3 => reg.write(bar, &Gsp::ID, 3),
        4 => reg.write(bar, &Gsp::ID, 4),
        5 => reg.write(bar, &Gsp::ID, 5),
        6 => reg.write(bar, &Gsp::ID, 6),
        7 => reg.write(bar, &Gsp::ID, 7),
        _ => return Err(EINVAL),
    }

    Ok(())
}

fn update_gsp_fbif_transcfg(
    bar: &Bar0,
    ctx_dma: u8,
    f: impl FnOnce(regs::NV_PFALCON_FBIF_TRANSCFG) -> regs::NV_PFALCON_FBIF_TRANSCFG,
) -> Result {
    match ctx_dma {
        0 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 0, f),
        1 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 1, f),
        2 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 2, f),
        3 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 3, f),
        4 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 4, f),
        5 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 5, f),
        6 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 6, f),
        7 => regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 7, f),
        _ => return Err(EINVAL),
    }

    Ok(())
}

impl super::Gsp {
    /// Helper function to load and run the FWSEC-FRTS firmware and confirm that it has properly
    /// created the WPR2 region.
    fn run_fwsec_frts(
        dev: &device::Device<device::Bound>,
        chipset: Chipset,
        falcon: &Falcon<Gsp>,
        bar: &Bar0,
        bios: &Vbios,
        fb_layout: &FbLayout,
    ) -> Result<()> {
        // Check that the WPR2 region does not already exists - if it does, we cannot run
        // FWSEC-FRTS until the GPU is reset.
        if regs::NV_PFB_PRI_MMU_WPR2_ADDR_HI::read(bar).higher_bound() != 0 {
            dev_err!(
                dev,
                "WPR2 region already exists - GPU needs to be reset to proceed\n"
            );
            return Err(EBUSY);
        }

        // FWSEC-FRTS will create the WPR2 region.
        let fwsec_frts = FwsecFirmware::new(
            dev,
            falcon,
            bar,
            bios,
            FwsecCommand::Frts {
                frts_addr: fb_layout.frts.start,
                frts_size: fb_layout.frts.len(),
            },
        )?;

        if chipset.needs_fwsec_bootloader() {
            let fwsec_frts_bl = FwsecFirmwareWithBl::new(fwsec_frts, dev, chipset)?;
            // Load and run the bootloader, which will load FWSEC-FRTS and run it.
            fwsec_frts_bl.run(dev, falcon, bar)?;
        } else {
            // Load and run FWSEC-FRTS directly.
            fwsec_frts.run(dev, falcon, bar)?;
        }

        // SCRATCH_E contains the error code for FWSEC-FRTS.
        let frts_status = regs::NV_PBUS_SW_SCRATCH_0E_FRTS_ERR::read(bar).frts_err_code();
        if frts_status != 0 {
            dev_err!(
                dev,
                "FWSEC-FRTS returned with error code {:#x}\n",
                frts_status
            );

            return Err(EIO);
        }

        // Check that the WPR2 region has been created as we requested.
        let (wpr2_lo, wpr2_hi) = (
            regs::NV_PFB_PRI_MMU_WPR2_ADDR_LO::read(bar).lower_bound(),
            regs::NV_PFB_PRI_MMU_WPR2_ADDR_HI::read(bar).higher_bound(),
        );

        match (wpr2_lo, wpr2_hi) {
            (_, 0) => {
                dev_err!(dev, "WPR2 region not created after running FWSEC-FRTS\n");

                Err(EIO)
            }
            (wpr2_lo, _) if wpr2_lo != fb_layout.frts.start => {
                dev_err!(
                    dev,
                    "WPR2 region created at unexpected address {:#x}; expected {:#x}\n",
                    wpr2_lo,
                    fb_layout.frts.start,
                );

                Err(EIO)
            }
            (wpr2_lo, wpr2_hi) => {
                dev_dbg!(dev, "WPR2: {:#x}-{:#x}\n", wpr2_lo, wpr2_hi);
                dev_dbg!(dev, "GPU instance built\n");

                Ok(())
            }
        }
    }

    fn run_booter(
        dev: &device::Device<device::Bound>,
        bar: &Bar0,
        chipset: Chipset,
        sec2_falcon: &Falcon<Sec2>,
        wpr_meta: &CoherentAllocation<GspFwWprMeta>,
    ) -> Result {
        let booter = BooterFirmware::new(
            dev,
            BooterKind::Loader,
            chipset,
            FIRMWARE_VERSION,
            sec2_falcon,
            bar,
        )?;

        booter.run(dev, bar, sec2_falcon, wpr_meta)
    }

    /// Boot GSP via SEC2 booter firmware (Turing/Ampere/Ada path).
    ///
    /// This path uses FWSEC-FRTS to set up WPR2, then boots GSP directly,
    /// then uses SEC2 to run the booter firmware.
    #[allow(clippy::too_many_arguments)]
    fn boot_via_sec2(
        dev: &device::Device<device::Bound>,
        bar: &Bar0,
        chipset: Chipset,
        gsp_falcon: &Falcon<Gsp>,
        sec2_falcon: &Falcon<Sec2>,
        fb_layout: &FbLayout,
        libos: &CoherentAllocation<LibosMemoryRegionInitArgument>,
        wpr_meta: &CoherentAllocation<GspFwWprMeta>,
    ) -> Result {
        // Run FWSEC-FRTS to set up the WPR2 region
        let bios = Vbios::new(dev, bar)?;
        Self::run_fwsec_frts(dev, chipset, gsp_falcon, bar, &bios, fb_layout)?;

        // Reset and boot GSP before SEC2
        gsp_falcon.reset(bar)?;
        let libos_handle = libos.dma_handle();
        let (mbox0, mbox1) = gsp_falcon.boot(
            bar,
            Some(libos_handle as u32),
            Some((libos_handle >> 32) as u32),
        )?;
        dev_dbg!(dev, "GSP MBOX0: {:#x}, MBOX1: {:#x}\n", mbox0, mbox1);
        dev_dbg!(
            dev,
            "Using SEC2 to load and run the booter_load firmware...\n"
        );

        // Run booter via SEC2
        Self::run_booter(dev, bar, chipset, sec2_falcon, wpr_meta)
    }

    /// Boot GSP via FSP Chain of Trust (Hopper/Blackwell+ path).
    ///
    /// This path uses FSP to establish a chain of trust and boot GSP-FMC. FSP handles
    /// the GSP boot internally - no manual GSP reset/boot is needed.
    fn boot_via_fsp(
        dev: &device::Device<device::Bound>,
        bar: &Bar0,
        chipset: Chipset,
        gsp_falcon: &Falcon<Gsp>,
        wpr_meta: &CoherentAllocation<GspFwWprMeta>,
        libos: &CoherentAllocation<LibosMemoryRegionInitArgument>,
    ) -> Result {
        let fsp_falcon = Falcon::<FspEngine>::new(dev, chipset)?;

        Fsp::wait_secure_boot(dev, bar, chipset.arch())?;

        let fsp_fw = FspFirmware::new(dev, chipset, FIRMWARE_VERSION)?;

        let signatures = Fsp::extract_fmc_signatures(dev, &fsp_fw.fmc_full)?;

        let args = FmcBootArgs::new(
            dev,
            chipset,
            &fsp_fw.fmc_image,
            wpr_meta.dma_handle(),
            core::mem::size_of::<GspFwWprMeta>() as u32,
            libos.dma_handle(),
            false,
            &signatures,
        )?;

        Fsp::boot_fmc(dev, bar, &fsp_falcon, &args)?;

        let fmc_boot_params_addr = args.boot_params_dma_handle();
        Self::wait_for_gsp_lockdown_release(dev, bar, gsp_falcon, fmc_boot_params_addr)?;

        Ok(())
    }

    /// Wait for GSP lockdown to be released after FSP Chain of Trust.
    fn wait_for_gsp_lockdown_release(
        dev: &device::Device<device::Bound>,
        bar: &Bar0,
        gsp_falcon: &Falcon<Gsp>,
        fmc_boot_params_addr: u64,
    ) -> Result {
        dev_dbg!(dev, "Waiting for GSP lockdown release\n");

        let mbox = read_poll_timeout(
            || Ok(GspMbox::read(gsp_falcon, bar)),
            |mbox| mbox.lockdown_released(bar, fmc_boot_params_addr),
            Delta::from_millis(10),
            Delta::from_millis(30000),
        )
        .inspect_err(|_| {
            dev_err!(dev, "GSP lockdown release timeout\n");
        })?;

        if mbox.mbox0 != 0 {
            dev_err!(dev, "GSP-FMC boot failed (mbox: {:#x})\n", mbox.mbox0);
            return Err(EIO);
        }

        dev_dbg!(dev, "GSP lockdown released\n");
        Ok(())
    }

    /// Attempt to boot the GSP.
    ///
    /// This is a GPU-dependent and complex procedure that involves loading firmware files from
    /// user-space, patching them with signatures, and building firmware-specific intricate data
    /// structures that the GSP will use at runtime.
    ///
    /// Upon return, the GSP is up and running, and its runtime object given as return value.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn boot(
        mut self: Pin<&mut Self>,
        pdev: &pci::Device<device::Bound>,
        bar: &Bar0,
        chipset: Chipset,
        gsp_falcon: &Falcon<Gsp>,
        sec2_falcon: &Falcon<Sec2>,
        gsp_firmware: &kernel::firmware::Firmware,
        gsp_fw_path: kernel::str::CString,
    ) -> Result<GetGspStaticInfoReply> {
        let dev = pdev.as_ref();
        let uses_sec2 = matches!(
            chipset.arch(),
            Architecture::Turing | Architecture::Ampere | Architecture::Ada
        );

        let gsp_fw = KBox::pin_init(
            GspFirmware::new(dev, chipset, FIRMWARE_VERSION, gsp_firmware, gsp_fw_path),
            GFP_KERNEL,
        )?;

        dev_info!(
            dev,
            "GSP firmware: {} (internal version: {})\n",
            gsp_fw.fw_path.to_str().unwrap_or("unknown"),
            gsp_fw.fw_version.to_str().unwrap_or("unknown")
        );

        // Load the optional ucodes (bindata) firmware and build a radix3 page table.
        // GSP-RM uses this to load additional microcode at runtime.
        let ucodes_radix3 =
            match crate::firmware::request_ucodes_firmware(dev, chipset, FIRMWARE_VERSION) {
                Ok(ucodes_fw) => Some(KBox::pin_init(
                    Radix3::new(dev, ucodes_fw.data()),
                    GFP_KERNEL,
                )?),
                Err(e) if e == ENOENT => {
                    dev_dbg!(dev, "ucodes firmware not found; bindataArgs will be zero\n");
                    None
                }
                Err(e) => return Err(e),
            };
        let fb_layout = FbLayout::new(chipset, bar, &gsp_fw)?;
        dev_dbg!(dev, "{:#x?}\n", fb_layout);

        let wpr_meta =
            CoherentAllocation::<GspFwWprMeta>::alloc_coherent(dev, 1, GFP_KERNEL | __GFP_ZERO)?;
        dma_write!(wpr_meta, [0]?, GspFwWprMeta::new(&gsp_fw, &fb_layout));

        let this = self.as_mut().project();

        // Rewrite the RM arguments with bindata info now that we have it.
        let bindata_opt = ucodes_radix3.as_ref().map(|r| fw::BindataArgs {
            radix3: r.dma_handle(),
            size: r.size as u64,
        });
        dma_write!(
            this.rmargs,
            [0]?.inner,
            fw::GspArgumentsCached::new(this.cmdq, bindata_opt.as_ref(), this.rm_state_monitor)
        );

        // Architecture-specific boot path
        if uses_sec2 {
            Self::boot_via_sec2(
                dev,
                bar,
                chipset,
                gsp_falcon,
                sec2_falcon,
                &fb_layout,
                &this.libos,
                &wpr_meta,
            )?;
        } else {
            Self::boot_via_fsp(dev, bar, chipset, gsp_falcon, &wpr_meta, &this.libos)?;
        }

        // Common post-boot initialization
        gsp_falcon.write_os_version(bar, gsp_fw.bootloader.app_version);

        // Poll for RISC-V to become active
        read_poll_timeout(
            || Ok(gsp_falcon.is_riscv_active(bar)),
            |val: &bool| *val,
            Delta::from_millis(10),
            Delta::from_secs(5),
        )?;

        dev_dbg!(dev, "RISC-V active? {}\n", gsp_falcon.is_riscv_active(bar));

        // Send system info and registry RPCs now that GSP is active.
        this.cmdq
            .send_command(bar, commands::SetSystemInfo::new(pdev, chipset))?;
        this.cmdq.send_command(bar, commands::SetRegistry::new())?;

        // Wait for GSP-RM to complete initialization, handling boot events inline.
        Self::wait_gsp_boot_events(
            this.cmdq,
            gsp_falcon,
            sec2_falcon,
            bar,
            dev,
            gsp_fw.bootloader.app_version,
            this.libos.dma_handle(),
        )?;

        // Obtain and display basic GPU information.
        let info = commands::get_gsp_info(this.cmdq, bar)?;
        match info.gpu_name() {
            Ok(name) => dev_info!(dev, "GPU name: {}\n", name),
            Err(e) => dev_warn!(dev, "GPU name unavailable: {:?}\n", e),
        }

        Ok(info)
    }

    /// Wait for GSP boot to complete, handling load-and-execute events inline.
    ///
    /// r000 firmware replaces the CPU sequencer with structured boot events.
    /// When the GSP sends a `LOAD_EXEC_GENERIC_BOOTLOADER` or `LOAD_EXEC_HS_BINARY`
    /// event, the driver loads the requested firmware onto the GSP falcon, executes
    /// it, and performs a core resume (reset GSP into RISCV, start SEC2-RTOS to
    /// resume GSP-RM). The loop runs until `INIT_DONE` arrives.
    #[allow(clippy::too_many_arguments)]
    fn wait_gsp_boot_events(
        cmdq: &mut Cmdq,
        gsp_falcon: &Falcon<Gsp>,
        sec2_falcon: &Falcon<Sec2>,
        bar: &Bar0,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        loop {
            let done = cmdq.receive_and_dispatch(
                Delta::from_secs(10),
                |function, payload_0, _payload_1| -> Result<bool> {
                    match function {
                        MsgFunction::GspInitDone => Ok(true),
                        MsgFunction::GspLoadExecGenericBootloader => {
                            Self::handle_load_exec_bootloader(
                                payload_0,
                                gsp_falcon,
                                sec2_falcon,
                                bar,
                                dev,
                                bootloader_app_version,
                                libos_dma_handle,
                            )?;
                            Ok(false)
                        }
                        MsgFunction::GspLoadExecHsBinary => {
                            Self::handle_load_exec_hs_binary(
                                payload_0,
                                gsp_falcon,
                                sec2_falcon,
                                bar,
                                dev,
                                bootloader_app_version,
                                libos_dma_handle,
                            )?;
                            Ok(false)
                        }
                        _ => Ok(false),
                    }
                },
            )??;

            if done {
                return Ok(());
            }
        }
    }

    /// Handle a `GSP_LOAD_EXEC_GENERIC_BOOTLOADER` event.
    ///
    /// The GSP firmware sends this event with a bootloader descriptor and the
    /// aperture metadata needed to DMA the firmware from CPU-controlled memory
    /// into GSP IMEM and DMEM. Once the image has run to completion, the driver
    /// performs a core resume to restart GSP-RM.
    #[allow(clippy::too_many_arguments)]
    fn handle_load_exec_bootloader(
        payload: &[u8],
        gsp_falcon: &Falcon<Gsp>,
        sec2_falcon: &Falcon<Sec2>,
        bar: &Bar0,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        let params = LoadExecGenericBootloaderParams::from_bytes_prefix(payload)
            .ok_or(EINVAL)?
            .0;
        let dmem_desc_size =
            u32::try_from(core::mem::size_of::<FalconBlDmemDesc>()).map_err(|_| EOVERFLOW)?;
        if params.dmem_desc_size != dmem_desc_size {
            dev_err!(
                dev,
                "Unexpected load-exec descriptor size: {} (expected {})\n",
                params.dmem_desc_size,
                dmem_desc_size
            );
            return Err(EINVAL);
        }

        let ctx_dma = params.ctx_dma().inspect_err(|_| {
            dev_err!(
                dev,
                "Unsupported load-exec DMA context: {}\n",
                params.dmem_desc.ctx_dma
            );
        })?;
        let fbif_target = params.fbif_target().inspect_err(|_| {
            dev_err!(
                dev,
                "Unsupported load-exec aperture: addr_space={}, cpu_cache_attrib={}\n",
                params.addr_space,
                params.cpu_cache_attrib
            );
        })?;
        let dmem_desc = params.dmem_desc;

        let code_dma_base =
            dmem_desc.code_dma_base_lo as u64 | ((dmem_desc.code_dma_base_hi as u64) << 32);
        let data_dma_base =
            dmem_desc.data_dma_base_lo as u64 | ((dmem_desc.data_dma_base_hi as u64) << 32);
        dev_dbg!(
            dev,
            "Load-exec bootloader: ctx_dma={}, addr_space={}, cpu_cache_attrib={}, code_dma_base={:#x}, data_dma_base={:#x}\n",
            ctx_dma,
            params.addr_space,
            params.cpu_cache_attrib,
            code_dma_base,
            data_dma_base
        );

        gsp_falcon
            .wait_for_processor_suspend(bar)
            .inspect_err(|_| {
                dev_err!(
                    dev,
                    "Timeout waiting for GSP suspend (mbox0={:#x})\n",
                    gsp_falcon.read_mailbox0(bar)
                );
            })?;

        gsp_falcon.reset(bar)?;

        gsp_falcon.dma_reset(bar);
        let saved_fbif_transcfg = read_gsp_fbif_transcfg(bar, ctx_dma)?;
        update_gsp_fbif_transcfg(bar, ctx_dma, |v| {
            v.set_target(fbif_target)
                .set_mem_type(FalconFbifMemType::Physical)
        })?;

        let load_result = (|| -> Result {
            if dmem_desc.non_secure_code_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    bar,
                    ctx_dma,
                    code_dma_base,
                    FalconMem::ImemNonSecure,
                    dmem_desc.non_secure_code_off,
                    dmem_desc.non_secure_code_off,
                    dmem_desc.non_secure_code_size,
                )?;
            }

            if dmem_desc.secure_code_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    bar,
                    ctx_dma,
                    code_dma_base,
                    FalconMem::ImemSecure,
                    dmem_desc.secure_code_off,
                    dmem_desc.secure_code_off,
                    dmem_desc.secure_code_size,
                )?;
            }

            if dmem_desc.data_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    bar,
                    ctx_dma,
                    data_dma_base,
                    FalconMem::Dmem,
                    0,
                    0,
                    dmem_desc.data_size,
                )?;
            }

            regs::NV_PFALCON_FALCON_BOOTVEC::default()
                .set_value(dmem_desc.code_entry_point)
                .write(bar, &Gsp::ID);

            gsp_falcon.start(bar)?;

            gsp_falcon.wait_till_halted(bar).inspect_err(|_| {
                dev_err!(
                    dev,
                    "Timeout waiting for firmware to halt (mbox0={:#x})\n",
                    gsp_falcon.read_mailbox0(bar)
                );
            })?;

            Ok(())
        })();
        write_gsp_fbif_transcfg(bar, ctx_dma, saved_fbif_transcfg)?;
        load_result?;

        // Core resume: restart GSP-RM via SEC2.
        gsp_falcon.reset(bar)?;

        gsp_falcon.write_mailboxes(
            bar,
            Some(libos_dma_handle as u32),
            Some((libos_dma_handle >> 32) as u32),
        );

        sec2_falcon.start(bar)?;

        gsp_falcon
            .check_reload_completed(bar, Delta::from_secs(2))
            .inspect_err(|_| {
                let mbox0 = sec2_falcon.read_mailbox0(bar);
                dev_err!(
                    dev,
                    "Timeout waiting for SEC2 to resume GSP-RM (SEC2 mbox0={:#x})\n",
                    mbox0
                );
            })?;

        let sec2_mbox0 = sec2_falcon.read_mailbox0(bar);
        if sec2_mbox0 != 0 {
            dev_err!(
                dev,
                "SEC2 reported error during core resume: {:#x}\n",
                sec2_mbox0
            );
            return Err(EIO);
        }

        gsp_falcon.write_os_version(bar, bootloader_app_version);

        if !gsp_falcon.is_riscv_active(bar) {
            dev_err!(dev, "GSP RISC-V not active after core resume\n");
            return Err(EIO);
        }

        Ok(())
    }

    /// Handle a `GSP_LOAD_EXEC_HS_BINARY` event.
    ///
    /// Similar to the generic bootloader handler, but loads a high-security (HS)
    /// binary from framebuffer memory. The HS binary requires PKC signature
    /// validation via BROM registers before execution.
    #[allow(clippy::too_many_arguments)]
    fn handle_load_exec_hs_binary(
        payload: &[u8],
        gsp_falcon: &Falcon<Gsp>,
        sec2_falcon: &Falcon<Sec2>,
        bar: &Bar0,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        let params = HsBinaryParams::from_bytes_prefix(payload).ok_or(EINVAL)?.0;

        gsp_falcon
            .wait_for_processor_suspend(bar)
            .inspect_err(|_| {
                dev_err!(
                    dev,
                    "Timeout waiting for GSP suspend (mbox0={:#x})\n",
                    gsp_falcon.read_mailbox0(bar)
                );
            })?;

        gsp_falcon.reset(bar)?;

        gsp_falcon.dma_reset(bar);
        regs::NV_PFALCON_FBIF_TRANSCFG::update(bar, &Gsp::ID, 0, |v| {
            v.set_target(FalconFbifTarget::LocalFb)
                .set_mem_type(FalconFbifMemType::Physical)
                .set_engine_id_flag(true)
        });

        if params.ucode_imem_size > 0 {
            gsp_falcon.raw_dma_transfer(
                bar,
                0,
                params.imem_phys_addr,
                FalconMem::ImemSecure,
                params.ucode_imem_va,
                params.ucode_imem_pa,
                params.ucode_imem_size,
            )?;
        }

        if params.ucode_dmem_size > 0 {
            let dmem_mem_off = if params.ucode_dmem_va == 0xFFFFFFFF {
                0
            } else {
                params.ucode_dmem_va
            };
            gsp_falcon.raw_dma_transfer(
                bar,
                0,
                params.dmem_phys_addr,
                FalconMem::Dmem,
                dmem_mem_off,
                params.ucode_dmem_pa,
                params.ucode_dmem_size,
            )?;
        }

        regs::NV_PFALCON2_FALCON_BROM_PARAADDR::default()
            .set_value(params.hs_sig_dmem_addr)
            .write(bar, &Gsp::ID, 0);
        regs::NV_PFALCON2_FALCON_BROM_ENGIDMASK::default()
            .set_value(params.engine_id_mask)
            .write(bar, &Gsp::ID);
        regs::NV_PFALCON2_FALCON_BROM_CURR_UCODE_ID::default()
            .set_ucode_id(params.ucode_id as u8)
            .write(bar, &Gsp::ID);
        regs::NV_PFALCON2_FALCON_MOD_SEL::default()
            .set_algo(FalconModSelAlgo::Rsa3k)
            .write(bar, &Gsp::ID);

        gsp_falcon.write_mailboxes(bar, Some(0xdead), None);

        regs::NV_PFALCON_FALCON_BOOTVEC::default()
            .set_value(params.ucode_imem_va)
            .write(bar, &Gsp::ID);

        gsp_falcon.start(bar)?;
        gsp_falcon.wait_till_halted(bar).inspect_err(|_| {
            dev_err!(
                dev,
                "Timeout waiting for HS binary to halt (mbox0={:#x})\n",
                gsp_falcon.read_mailbox0(bar)
            );
        })?;

        // Core resume: restart GSP-RM via SEC2.
        gsp_falcon.reset(bar)?;
        gsp_falcon.write_mailboxes(
            bar,
            Some(libos_dma_handle as u32),
            Some((libos_dma_handle >> 32) as u32),
        );
        sec2_falcon.start(bar)?;

        gsp_falcon
            .check_reload_completed(bar, Delta::from_secs(2))
            .inspect_err(|_| {
                let mbox0 = sec2_falcon.read_mailbox0(bar);
                dev_err!(
                    dev,
                    "Timeout waiting for SEC2 to resume GSP-RM (SEC2 mbox0={:#x})\n",
                    mbox0
                );
            })?;

        let sec2_mbox0 = sec2_falcon.read_mailbox0(bar);
        if sec2_mbox0 != 0 {
            dev_err!(
                dev,
                "SEC2 reported error during core resume: {:#x}\n",
                sec2_mbox0
            );
            return Err(EIO);
        }

        gsp_falcon.write_os_version(bar, bootloader_app_version);

        if !gsp_falcon.is_riscv_active(bar) {
            dev_err!(dev, "GSP RISC-V not active after core resume\n");
            return Err(EIO);
        }

        Ok(())
    }
}

/// Falcon bootloader DMEM descriptor (RM_FLCN_BL_DMEM_DESC).
///
/// This is nested within the `GSP_LOAD_EXEC_GENERIC_BOOTLOADER` payload and
/// describes the firmware image to be loaded by the host into the GSP falcon.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct FalconBlDmemDesc {
    _reserved: [u32; 4],
    _signature: [u32; 4],
    ctx_dma: u32,
    code_dma_base_lo: u32,
    code_dma_base_hi: u32,
    non_secure_code_off: u32,
    non_secure_code_size: u32,
    secure_code_off: u32,
    secure_code_size: u32,
    code_entry_point: u32,
    data_dma_base_lo: u32,
    data_dma_base_hi: u32,
    data_size: u32,
    _argc: u32,
    _argv: u32,
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for FalconBlDmemDesc {}

/// Parameters for loading and executing the generic bootloader.
///
/// Sent by GSP-RM as the payload of `GSP_LOAD_EXEC_GENERIC_BOOTLOADER`.
/// The descriptor carries the code and data addresses, while `addr_space` and
/// `cpu_cache_attrib` tell the CPU side how to configure the selected FBIF
/// aperture.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct LoadExecGenericBootloaderParams {
    dmem_desc: FalconBlDmemDesc,
    dmem_desc_size: u32,
    addr_space: u32,
    cpu_cache_attrib: u32,
    _reserved: [u32; 4],
}

impl LoadExecGenericBootloaderParams {
    const ADDR_SYSMEM: u32 = 1;
    const ADDR_FBMEM: u32 = 2;
    const NV_MEMORY_CACHED: u32 = 0;
    const NV_MEMORY_UNCACHED: u32 = 1;
    const NUM_CTX_DMA: usize = 8;

    fn ctx_dma(&self) -> Result<u8> {
        let ctx_dma = u8::try_from(self.dmem_desc.ctx_dma).map_err(|_| EINVAL)?;

        if usize::from(ctx_dma) >= Self::NUM_CTX_DMA {
            return Err(EINVAL);
        }

        Ok(ctx_dma)
    }

    fn fbif_target(&self) -> Result<FalconFbifTarget> {
        match (self.addr_space, self.cpu_cache_attrib) {
            (Self::ADDR_FBMEM, _) => Ok(FalconFbifTarget::LocalFb),
            (Self::ADDR_SYSMEM, Self::NV_MEMORY_CACHED) => Ok(FalconFbifTarget::CoherentSysmem),
            (Self::ADDR_SYSMEM, Self::NV_MEMORY_UNCACHED) => {
                Ok(FalconFbifTarget::NoncoherentSysmem)
            }
            _ => Err(EINVAL),
        }
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for LoadExecGenericBootloaderParams {}

/// Parameters for loading and executing an HS (High-Security) binary.
///
/// Sent by GSP-RM as the payload of `GSP_LOAD_EXEC_HS_BINARY`. The firmware
/// code and data are located in framebuffer memory at the given physical addresses.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct HsBinaryParams {
    imem_phys_addr: u64,
    dmem_phys_addr: u64,
    _reserved64: [u64; 2],
    ucode_imem_va: u32,
    ucode_imem_pa: u32,
    ucode_imem_size: u32,
    ucode_dmem_va: u32,
    ucode_dmem_pa: u32,
    ucode_dmem_size: u32,
    hs_sig_dmem_addr: u32,
    engine_id_mask: u32,
    ucode_id: u32,
    _reserved32: [u32; 3],
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for HsBinaryParams {}
