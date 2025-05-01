// SPDX-License-Identifier: GPL-2.0

use core::marker::PhantomData;
use core::time::Duration;

use kernel::devres::Devres;
use kernel::prelude::*;

use crate::driver::Bar0;
use crate::falcon::{FalconBromParams, FalconEngine, FalconModSelAlgo, PeregrineCoreSelect};
use crate::regs;
use crate::util;

use super::FalconHal;

fn select_core_ga102<E: FalconEngine>(bar: &Devres<Bar0>) -> Result<()> {
    let bcr_ctrl = with_bar!(bar, |b| regs::NV_PRISCV_RISCV_BCR_CTRL::read(b, E::BASE))?;
    if bcr_ctrl.core_select() != PeregrineCoreSelect::Falcon {
        with_bar!(bar, |b| regs::NV_PRISCV_RISCV_BCR_CTRL::default()
            .set_core_select(PeregrineCoreSelect::Falcon)
            .write(b, E::BASE))?;

        util::wait_on(Duration::from_millis(10), || {
            bar.try_access_with(|b| regs::NV_PRISCV_RISCV_BCR_CTRL::read(b, E::BASE))
                .and_then(|v| if v.valid() { Some(()) } else { None })
        })?;
    }

    Ok(())
}

fn get_signature_reg_fuse_version_ga102(
    bar: &Devres<Bar0>,
    engine_id_mask: u16,
    ucode_id: u8,
) -> Result<u32> {
    // The ucode fuse versions are contained in the FUSE_OPT_FPF_<ENGINE>_UCODE<X>_VERSION
    // registers, which are an array. Our register definition macros do not allow us to manage them
    // properly, so we need to hardcode their addresses for now.

    // Each engine has 16 ucode version registers numbered from 1 to 16.
    if ucode_id == 0 || ucode_id > 16 {
        dev_warn!(bar.as_ref(), "invalid ucode id {:#x}", ucode_id);
        return Err(EINVAL);
    }
    let reg_fuse = if engine_id_mask & 0x0001 != 0 {
        // NV_FUSE_OPT_FPF_SEC2_UCODE1_VERSION
        0x824140
    } else if engine_id_mask & 0x0004 != 0 {
        // NV_FUSE_OPT_FPF_NVDEC_UCODE1_VERSION
        0x824100
    } else if engine_id_mask & 0x0400 != 0 {
        // NV_FUSE_OPT_FPF_GSP_UCODE1_VERSION
        0x8241c0
    } else {
        dev_warn!(
            bar.as_ref(),
            "unexpected engine_id_mask {:#x}",
            engine_id_mask
        );
        return Err(EINVAL);
    } + ((ucode_id - 1) as usize * core::mem::size_of::<u32>());

    let reg_fuse_version = with_bar!(bar, |b| { b.read32(reg_fuse) })?;

    // Equivalent of Find Last Set bit.
    Ok(u32::BITS - reg_fuse_version.leading_zeros())
}

fn program_brom_ga102<E: FalconEngine>(
    bar: &Devres<Bar0>,
    params: &FalconBromParams,
) -> Result<()> {
    with_bar!(bar, |b| {
        regs::NV_PFALCON2_FALCON_BROM_PARAADDR::default()
            .set_value(params.pkc_data_offset)
            .write(b, E::BASE);
        regs::NV_PFALCON2_FALCON_BROM_ENGIDMASK::default()
            .set_value(params.engine_id_mask as u32)
            .write(b, E::BASE);
        regs::NV_PFALCON2_FALCON_BROM_CURR_UCODE_ID::default()
            .set_ucode_id(params.ucode_id)
            .write(b, E::BASE);
        regs::NV_PFALCON2_FALCON_MOD_SEL::default()
            .set_algo(FalconModSelAlgo::Rsa3k)
            .write(b, E::BASE)
    })
}

pub(super) struct Ga102<E: FalconEngine>(PhantomData<E>);

impl<E: FalconEngine> Ga102<E> {
    pub(super) fn new() -> Self {
        Self(PhantomData)
    }
}

impl<E: FalconEngine> FalconHal<E> for Ga102<E> {
    fn select_core(&self, bar: &Devres<Bar0>) -> Result<()> {
        select_core_ga102::<E>(bar)
    }

    fn get_signature_reg_fuse_version(
        &self,
        bar: &Devres<Bar0>,
        engine_id_mask: u16,
        ucode_id: u8,
    ) -> Result<u32> {
        get_signature_reg_fuse_version_ga102(bar, engine_id_mask, ucode_id)
    }

    fn program_brom(&self, bar: &Devres<Bar0>, params: &FalconBromParams) -> Result<()> {
        program_brom_ga102::<E>(bar, params)
    }
}
