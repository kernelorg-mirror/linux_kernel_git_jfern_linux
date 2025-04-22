// SPDX-License-Identifier: GPL-2.0

// Required to retain the original register names used by OpenRM, which are all capital snake case
// but are mapped to types.
#![allow(non_camel_case_types)]

use kernel::error::Error;

#[macro_use]
mod macros;

use crate::falcon::{
    DmaTrfCmdSize, FalconCoreRev, FalconCoreRevSubversion, FalconFbifMemType, FalconFbifTarget,
    FalconModSelAlgo, FalconSecurityModel, PeregrineCoreSelect,
};
use crate::gpu::{Architecture, Chipset};

/* PMC */

register!(NV_PMC_BOOT_0 @ 0x00000000, "Basic revision information about the GPU" {
    3:0     minor_revision as u8, "Minor revision of the chip";
    7:4     major_revision as u8, "Major revision of the chip";
    8:8     architecture_1 as u8, "MSB of the architecture";
    23:20   implementation as u8, "Implementation version of the architecture";
    28:24   architecture_0 as u8, "Lower bits of the architecture";
});

impl NV_PMC_BOOT_0 {
    /// Combines `architecture_0` and `architecture_1` to obtain the architecture of the chip.
    pub(crate) fn architecture(self) -> Result<Architecture, Error> {
        Architecture::try_from(
            self.architecture_0() | (self.architecture_1() << Self::ARCHITECTURE_0.len()),
        )
    }

    /// Combines `architecture` and `implementation` to obtain a code unique to the chipset.
    pub(crate) fn chipset(self) -> Result<Chipset, Error> {
        self.architecture()
            .map(|arch| {
                ((arch as u32) << Self::IMPLEMENTATION.len()) | self.implementation() as u32
            })
            .and_then(Chipset::try_from)
    }
}

/* PBUS */

// TODO: this is an array of registers.
register!(NV_PBUS_SW_SCRATCH_0E@0x00001438  {
    31:16   frts_err_code as u16;
});

/* PFB */

// These two registers together hold the physical system memory address
// that is used by the GPU for perform sysmembar operation (see [`crate::gpu`]).
register!(NV_PFB_NISO_FLUSH_SYSMEM_ADDR @ 0x00100c10 {
    31:0    adr_39_08 as u32;
});

register!(NV_PFB_NISO_FLUSH_SYSMEM_ADDR_HI @ 0x00100c40 {
    23:0    adr_63_40 as u32;
});

register!(NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE @ 0x00100ce0 {
    3:0     lower_scale as u8;
    9:4     lower_mag as u8;
    30:30   ecc_mode_enabled as bool;
});

register!(NV_PFB_PRI_MMU_WPR2_ADDR_LO@0x001fa824  {
    31:4    lo_val as u32;
});

register!(NV_PFB_PRI_MMU_WPR2_ADDR_HI@0x001fa828  {
    31:4    hi_val as u32;
});

/*
 * PGC6 register space.
 *
 * GC6 is a GPU low-power state where VRAM is in self-refresh and the GPU
 * is powered down (except for power rails needed to keep self-refresh working
 * and important registers and hardware blocks).
 *
 * These scratch registers are "always-on" even in a low-power state and have a
 * designated group number.
 */

// Privilege level mask register.
// It dictates whether the host CPU has privilege to access the
// PGC6_AON_SECURE_SCRATCH_GROUP_05 register (which it needs to read GFW_BOOT).
register!(NV_PGC6_AON_SECURE_SCRATCH_GROUP_05_PRIV_LEVEL_MASK @ 0x00118128 {
    0:0     read_protection_level0 as bool;
});

// TODO: This is an array of registers.
register!(NV_PGC6_AON_SECURE_SCRATCH_GROUP_05 @ 0x00118234 {
    31:0    value as u32;
});

register!(NV_PGC6_AON_SECURE_SCRATCH_GROUP_42 @ 0x001183a4 {
    31:0    value as u32;
});

/* PDISP */

register!(NV_PDISP_VGA_WORKSPACE_BASE @ 0x00625f04 {
    3:3     status_valid as bool;
    31:8    addr as u32;
});

/* FUSE */

register!(NV_FUSE_STATUS_OPT_DISPLAY_MAXWELL @ 0x00021c04 {
    0:0     display_disabled as bool;
});

register!(NV_FUSE_STATUS_OPT_DISPLAY_AMPERE @ 0x00820c04 {
    0:0     display_disabled as bool;
});

/* PFALCON */

register!(NV_PFALCON_FALCON_IRQSCLR @ +0x00000004 {
    4:4     halt as bool;
    6:6     swgen0 as bool;
});

register!(NV_PFALCON_FALCON_MAILBOX0 @ +0x00000040 {
    31:0    value as u32;
});

register!(NV_PFALCON_FALCON_MAILBOX1 @ +0x00000044 {
    31:0    value as u32;
});

register!(NV_PFALCON_FALCON_RM @ +0x00000084 {
    31:0    value as u32;
});

register!(NV_PFALCON_FALCON_HWCFG2 @ +0x000000f4 {
    10:10   riscv as bool;
    12:12   mem_scrubbing as bool;
    31:31   reset_ready as bool, "Signal indicating that reset is completed (GA102+)";
});

register!(NV_PFALCON_FALCON_CPUCTL @ +0x00000100 {
    1:1     startcpu as bool;
    4:4     halted as bool;
    6:6     alias_en as bool;
});

register!(NV_PFALCON_FALCON_BOOTVEC @ +0x00000104 {
    31:0    value as u32;
});

register!(NV_PFALCON_FALCON_DMACTL @ +0x0000010c {
    0:0     require_ctx as bool;
    1:1     dmem_scrubbing as bool;
    2:2     imem_scrubbing as bool;
    6:3     dmaq_num as u8;
    7:7     secure_stat as bool;
});

register!(NV_PFALCON_FALCON_DMATRFBASE @ +0x00000110 {
    31:0    base as u32;
});

register!(NV_PFALCON_FALCON_DMATRFMOFFS @ +0x00000114 {
    23:0    offs as u32;
});

register!(NV_PFALCON_FALCON_DMATRFCMD @ +0x00000118 {
    0:0     full as bool;
    1:1     idle as bool;
    3:2     sec as u8;
    4:4     imem as bool;
    5:5     is_write as bool;
    10:8    size as u8 ?=> DmaTrfCmdSize;
    14:12   ctxdma as u8;
    16:16   set_dmtag as u8;
});

register!(NV_PFALCON_FALCON_DMATRFFBOFFS @ +0x0000011c {
    31:0    offs as u32;
});

register!(NV_PFALCON_FALCON_DMATRFBASE1 @ +0x00000128 {
    8:0     base as u16;
});

register!(NV_PFALCON_FALCON_HWCFG1 @ +0x0000012c {
    3:0     core_rev as u8 ?=> FalconCoreRev, "Core revision";
    5:4     security_model as u8 ?=> FalconSecurityModel, "Security model";
    7:6     core_rev_subversion as u8 ?=> FalconCoreRevSubversion, "Core revision subversion";
});

register!(NV_PFALCON_FALCON_CPUCTL_ALIAS @ +0x00000130 {
    1:1     startcpu as bool;
});

// Actually known as `NV_PSEC_FALCON_ENGINE` and `NV_PGSP_FALCON_ENGINE` depending on the falcon
// instance.
register!(NV_PFALCON_FALCON_ENGINE @ +0x000003c0 {
    0:0     reset as bool;
});

// TODO: this is an array of registers.
register!(NV_PFALCON_FBIF_TRANSCFG @ +0x00000600 {
    1:0     target as u8 ?=> FalconFbifTarget;
    2:2     mem_type as bool => FalconFbifMemType;
});

register!(NV_PFALCON_FBIF_CTL @ +0x00000624 {
    7:7     allow_phys_no_ctx as bool;
});

register!(NV_PFALCON2_FALCON_MOD_SEL @ +0x00001180 {
    7:0     algo as u8 ?=> FalconModSelAlgo;
});

register!(NV_PFALCON2_FALCON_BROM_CURR_UCODE_ID @ +0x00001198 {
    7:0    ucode_id as u8;
});

register!(NV_PFALCON2_FALCON_BROM_ENGIDMASK @ +0x0000119c {
    31:0    value as u32;
});

// TODO: this is an array of registers.
register!(NV_PFALCON2_FALCON_BROM_PARAADDR @ +0x00001210 {
    31:0    value as u32;
});

/* PRISCV */

register!(NV_PRISCV_RISCV_BCR_CTRL @ +0x00001668 {
    0:0     valid as bool;
    4:4     core_select as bool => PeregrineCoreSelect;
    8:8     br_fetch as bool;
});
