pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::pci::Device;

use crate::gpu::{SizeAddr, FBInfo};
use crate::gsp::*;

#[versions(GSP)]
pub(crate) struct GspSystemInfoRpcMsg {
    pub rpc: RpcMsg::ver,
}

#[versions(GSP)]
impl GspSystemInfoRpcMsg::ver {
    pub(crate) fn new(gpu_base: &GpuBase) -> Result<Self> {
        let mut rpc = RpcMsg::ver::new(fw::ver::gen::NV_VGPU_MSG_FUNCTION_GSP_SET_SYSTEM_INFO, false, fw::ver::gen::s_GspSystemInfo::str_size())?;
        let mut bars: [u64; 4] = Default::default();

        let pci_dev = unsafe {Device::from_dev(gpu_base.dev.clone())};

        bars[0] = pci_dev.resource_start(0)?;
        bars[1] = pci_dev.resource_start(1)?;
        bars[2] = pci_dev.resource_start(3)?;

        let pciaddr = pci_dev.dev_id()?;

        let _msg = fw::ver::gen::s_GspSystemInfo::new(rpc.get_data_ptr())
            .gpuPhysAddr(bars[0])
            .gpuPhysFbAddr(bars[1])
            .gpuPhysInstAddr(bars[2])
            .nvDomainBusDeviceFunc(pciaddr as u64)
            .maxUserVa((1 << 47) - 4096)
            .pciConfigMirrorBase(0x88000)
            .pciConfigMirrorSize(0x1000);

        Ok(Self {
            rpc,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        queues.rpc_push(&mut self.rpc, false, 0)
    }
}

struct NvRegistryEntry {
    name: &'static str,
    value: u32
}

static REGISTRY : [NvRegistryEntry; 2] = [
    NvRegistryEntry { name: "RMSecBusResetEnable", value: 1 },
    NvRegistryEntry { name: "RMForcePcieConfigSave", value: 1 },
];

#[versions(GSP)]
pub(crate) struct GspRegistryRpcMsg {
    pub rpc: RpcMsg::ver,
}

#[versions(GSP)]
impl GspRegistryRpcMsg::ver {
    pub(crate) fn new() -> Result<Self> {
        let hdr_size = fw::ver::gen::s_PACKED_REGISTRY_ENTRY::str_size() * REGISTRY.len() +
            fw::ver::gen::s_PACKED_REGISTRY_TABLE::str_size();

        let mut rpc_size = hdr_size;
        for i in &REGISTRY {
            rpc_size += i.name.len() + 1;
        }

        let mut rpc = RpcMsg::ver::new(fw::ver::gen::NV_VGPU_MSG_FUNCTION_SET_REGISTRY, false, rpc_size)?;

        let mut table = fw::ver::gen::s_PACKED_REGISTRY_TABLE::new(rpc.get_data_ptr())
            .numEntries(REGISTRY.len() as u32);

        let mut str_offset = hdr_size;
        let mut idx = 0;
        unsafe {
            let mut str_ptr = rpc.get_data_ptr().offset(str_offset as isize) as *mut u8;
            for i in &REGISTRY {
                let _entry = table.new_S_entries(idx)
                    .nameOffset(str_offset as u32)
                    .rtype(1)
                    .length(4)
                    .data(i.value);

                let this_len : isize = (i.name.len() + 1) as isize;
                core::ptr::copy_nonoverlapping(i.name.as_ptr(), str_ptr, (this_len - 1) as usize);
                *(str_ptr.offset(this_len - 1)) = 0;
                str_ptr = str_ptr.offset(this_len);
                str_offset += this_len as usize;
                idx += 1;
            }
        }
        table.set_size(str_offset as u32);
        Ok(Self {
            rpc,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        queues.rpc_push(&mut self.rpc, false, 0)
    }
}

#[versions(GSP)]
pub(crate) struct GSPStaticConfigRpc {
    pub rpc: RpcMsg::ver,
    rpc_size: usize,
}

#[versions(GSP)]
impl GSPStaticConfigRpc::ver {
    pub(crate) fn new() -> Result<Self> {
        let rpc_size = fw::ver::gen::s_GspStaticConfigInfo::str_size();
        let rpc = RpcMsg::ver::new(fw::ver::gen::NV_VGPU_MSG_FUNCTION_GET_GSP_STATIC_INFO, true, rpc_size)?;

        Ok(Self {
            rpc,
            rpc_size,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        queues.rpc_push(&mut self.rpc, true, self.rpc_size as u32)
    }

    fn msg(&mut self) -> fw::ver::gen::s_GspStaticConfigInfo<'_> {
        fw::ver::gen::s_GspStaticConfigInfo::new(self.rpc.get_data_ptr())
    }

    pub(crate) fn internal_client(&mut self) -> u32 {
        self.msg().get_hInternalClient()
    }

    pub(crate) fn internal_device(&mut self) -> u32 {
        self.msg().get_hInternalDevice()
    }

    pub(crate) fn internal_subdevice(&mut self) -> u32 {
        self.msg().get_hInternalSubdevice()
    }

    pub(crate) fn bar1_pdb(&mut self) -> u64 {
        self.msg().get_bar1PdeBase()
    }

    pub(crate) fn bar2_pdb(&mut self) -> u64 {
        self.msg().get_bar2PdeBase()
    }

    pub(crate) fn fill_fb_regions(&mut self, fb_addr_info: &mut FBInfo) -> Result<()> {
        let mut fb_msg = self.msg().new_S_fbRegionInfoParams();

        for i in 0..fb_msg.get_numFBRegions() {
            let reg = fb_msg.new_S_fbRegion(i as isize);
            let base = reg.get_base();
            let limit = reg.get_limit();
            let reserved = reg.get_reserved() != 0;
            let protected: bool = reg.get_bProtected() != 0;
            let compressed = reg.get_supportCompressed() != 0;
            let iso = reg.get_supportISO() != 0;

            pr_info!("region {}: {:#x} {:#x} {} {} {} {}\n", i, base, limit, reserved, protected, compressed, iso);
            if !reserved && !protected {
                if compressed && iso {
                    let size: u64 = (limit + 1) - base;

                    fb_addr_info.region.push(SizeAddr { addr: base, size }, GFP_KERNEL)?;
                }
            }
        }
        Ok(())
    }
}

#[versions(GSP)]
pub(crate) struct UnloadGuestDriver {
    pub rpc: RpcMsg::ver,
}

#[versions(GSP)]
impl UnloadGuestDriver::ver {
    pub(crate) fn get(suspend: bool) -> Result<Self> {
        let mut rpc = RpcMsg::ver::new(fw::ver::gen::NV_VGPU_MSG_FUNCTION_UNLOADING_GUEST_DRIVER, false,
                                  fw::ver::gen::s_rpc_unloading_guest_driver_v1F_07::str_size())?;

        let inpm: u8;
        let gc6: u8 = 0;
        let new_level: u32;
        if suspend {
            inpm = 1;
            new_level = 3;
        } else {
            inpm = 0;
            new_level = 0;
        }
        let _msg = fw::ver::gen::s_rpc_unloading_guest_driver_v1F_07::new(rpc.get_data_ptr())
            .bInPMTransition(inpm)
            .bGc6Entering(gc6)
            .newLevel(new_level);

        Ok(Self {
            rpc
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        queues.rpc_push(&mut self.rpc, true, 0)
    }
}
