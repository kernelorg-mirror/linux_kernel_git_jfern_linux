// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    pci,
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use crate::{
    gpu::{
        Architecture,
        Chipset, //
    },
    gsp::GSP_PAGE_SIZE,
    num::IntoSafeCast, //
};

use super::{
    r000,
    r570, //
};

/// Payload of the `GspSetSystemInfo` command.
#[repr(transparent)]
pub(crate) struct GspSetSystemInfo {
    inner: r570::GspSystemInfo,
}
static_assert!(size_of::<GspSetSystemInfo>() < GSP_PAGE_SIZE);

impl GspSetSystemInfo {
    /// Returns an in-place initializer for the `GspSetSystemInfo` command.
    #[allow(non_snake_case)]
    pub(crate) fn init<'a>(
        dev: &'a pci::Device<device::Bound>,
        chipset: Chipset,
    ) -> impl Init<Self, Error> + 'a {
        type InnerGspSystemInfo = r570::GspSystemInfo;
        let init_inner = try_init!(InnerGspSystemInfo {
            gpuPhysAddr: dev.resource_start(0)?,
            gpuPhysFbAddr: dev.resource_start(1)?,
            gpuPhysInstAddr: dev.resource_start(3)?,
            nvDomainBusDeviceFunc: u64::from(dev.dev_id()),

            // Using TASK_SIZE in r535_gsp_rpc_set_system_info() seems wrong because
            // TASK_SIZE is per-task. That's probably a design issue in GSP-RM though.
            maxUserVa: (1 << 47) - 4096,
            // Hopper, Blackwell, and later moved the PCI config mirror window to 0x092000.
            // Older architectures continue to use the legacy window at 0x088000.
            pciConfigMirrorBase: match chipset.arch() {
                Architecture::Turing | Architecture::Ampere | Architecture::Ada => 0x088000,
                Architecture::Hopper | Architecture::Blackwell => 0x092000,
            },
            pciConfigMirrorSize: 0x001000,

            PCIDeviceID: (u32::from(dev.device_id()) << 16) | u32::from(dev.vendor_id().as_raw()),
            PCISubDeviceID: (u32::from(dev.subsystem_device_id()) << 16)
                | u32::from(dev.subsystem_vendor_id()),
            PCIRevisionID: u32::from(dev.revision_id()),
            bIsPrimary: 0,
            bPreserveVideoMemoryAllocations: 0,
            ..Zeroable::init_zeroed()
        });

        try_init!(GspSetSystemInfo {
            inner <- init_inner,
        })
    }
}

// SAFETY: These structs don't meet the no-padding requirements of AsBytes but
//         that is not a problem because they are not used outside the kernel.
unsafe impl AsBytes for GspSetSystemInfo {}

// SAFETY: These structs don't meet the no-padding requirements of FromBytes but
//         that is not a problem because they are not used outside the kernel.
unsafe impl FromBytes for GspSetSystemInfo {}

#[repr(transparent)]
pub(crate) struct PackedRegistryEntry(r570::PACKED_REGISTRY_ENTRY);

impl PackedRegistryEntry {
    pub(crate) fn new(offset: u32, value: u32) -> Self {
        Self({
            r570::PACKED_REGISTRY_ENTRY {
                nameOffset: offset,

                // We only support DWORD types for now. Support for other types
                // will come later if required.
                type_: r570::REGISTRY_TABLE_ENTRY_TYPE_DWORD as u8,
                __bindgen_padding_0: Default::default(),
                data: value,
                length: 0,
            }
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for PackedRegistryEntry {}

/// Payload of the `SetRegistry` command.
#[repr(transparent)]
pub(crate) struct PackedRegistryTable {
    inner: r570::PACKED_REGISTRY_TABLE,
}

impl PackedRegistryTable {
    #[allow(non_snake_case)]
    pub(crate) fn init(num_entries: u32, size: u32) -> impl Init<Self> {
        type InnerPackedRegistryTable = r570::PACKED_REGISTRY_TABLE;
        let init_inner = init!(InnerPackedRegistryTable {
            numEntries: num_entries,
            size,
            entries: Default::default()
        });

        init!(PackedRegistryTable { inner <- init_inner })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for PackedRegistryTable {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for PackedRegistryTable {}

/// Payload of the `GetGspStaticInfo` command and message.
#[repr(transparent)]
#[derive(Zeroable)]
pub(crate) struct GspStaticConfigInfo(r000::GspStaticConfigInfo_t);

impl GspStaticConfigInfo {
    /// Returns a bytes array containing the (hopefully) zero-terminated name of this GPU.
    pub(crate) fn gpu_name_str(&self) -> [u8; 64] {
        self.0.gpuNameString
    }

    /// Returns an iterator over valid FB regions from GSP firmware data.
    fn fb_regions(
        &self,
    ) -> impl Iterator<Item = &bindings::NV2080_CTRL_CMD_FB_GET_FB_REGION_FB_REGION_INFO> {
        let fb_info = &self.0.fbRegionInfoParams;
        fb_info
            .fbRegion
            .iter()
            .take(fb_info.numFBRegions.into_safe_cast())
            .filter(|reg| reg.limit >= reg.base)
    }

    /// Extracts the first usable FB region from GSP firmware data.
    ///
    /// Returns the first region suitable for driver memory allocation as a `(base, size)` tuple.
    /// Usable regions are those that:
    /// - Are not reserved for firmware internal use.
    /// - Are not protected (hardware-enforced access restrictions).
    /// - Support compression (can use GPU memory compression for bandwidth).
    /// - Support ISO (isochronous memory for display requiring guaranteed bandwidth).
    ///
    /// TODO: Multiple discontinuous usable regions of RAM are possible in
    /// special cases. We need to support it (to also match Nouveau's behavior).
    pub(crate) fn first_usable_fb_region(&self) -> Option<(u64, u64)> {
        self.fb_regions().find_map(|reg| {
            // Filter: not reserved, not protected, supports compression and ISO.
            if reg.reserved == 0
                && reg.bProtected == 0
                && reg.supportCompressed != 0
                && reg.supportISO != 0
            {
                let size = reg.limit - reg.base + 1;
                Some((reg.base, size))
            } else {
                None
            }
        })
    }

    /// Compute the end of physical VRAM from all FB regions.
    pub(crate) fn total_fb_end(&self) -> Option<u64> {
        self.fb_regions()
            .map(|reg| reg.limit.saturating_add(1))
            .max()
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for GspStaticConfigInfo {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for GspStaticConfigInfo {}
