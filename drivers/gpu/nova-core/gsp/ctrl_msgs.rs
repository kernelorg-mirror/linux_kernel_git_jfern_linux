#![allow(unused)]
pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::bindings;
use crate::gsp::*;
use crate::gsp::rpc_msgs::*;

use crate::gpu::IntrInfo;
use crate::gpu::{FifoDeviceEntry, FifoDeviceInfoTable};
use crate::nvfw::*;
use crate::gsp::EventSetNotificationAction;

#[versions(GSP)]
pub(crate) struct InternalIntrGetKernelTableParams {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl InternalIntrGetKernelTableParams::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS::str_size();

        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE, msg_size, true)?;
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<KVec<IntrInfo>> {
        self.ctrl.push(queues)?;

        let mut msg = fw::ver::gen::s_NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS::new(self.ctrl.get_data_ptr());
        let mut intr_table: KVec<IntrInfo> = KVec::new();

        pr_info!("kernel table params {}", msg.get_tableLen());
        for i in 0..msg.get_tableLen() {
            let tbl = msg.new_S_table(i as isize);

            if tbl.get_engineIdx() != fw::ver::gen::MC_ENGINE_IDX_GSP as u16 {
                continue;
            }

            intr_table.push(IntrInfo {
                inst: 0,
                stall: tbl.get_vectorStall(),
                nonstall: tbl.get_vectorNonStall(),
            }, GFP_KERNEL)?;
        }
        Ok(intr_table)
    }
}

#[versions(GSP)]
pub(crate) struct EventSetNotification {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl EventSetNotification::ver {

    pub(crate) fn conv_action(val: EventSetNotificationAction) -> u32 {
        match val {
            EventSetNotificationAction::DISABLE => { fw::ver::gen::NV2080_CTRL_EVENT_SET_NOTIFICATION_ACTION_DISABLE },
            EventSetNotificationAction::SINGLE => { fw::ver::gen::NV2080_CTRL_EVENT_SET_NOTIFICATION_ACTION_SINGLE },
            EventSetNotificationAction::REPEAT => { fw::ver::gen::NV2080_CTRL_EVENT_SET_NOTIFICATION_ACTION_REPEAT },
        }
    }
    pub(crate) fn new(object: &GspObject, event: u32, action: EventSetNotificationAction) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_EVENT_SET_NOTIFICATION_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(object, fw::ver::gen::NV2080_CTRL_CMD_EVENT_SET_NOTIFICATION, msg_size, true)?;

        let mut _msg = fw::ver::gen::s_NV2080_CTRL_EVENT_SET_NOTIFICATION_PARAMS::new(ctrl.get_data_ptr())
            .event(event)
            .action(Self::conv_action(action));

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct FifoGetDeviceInfoTable {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl FifoGetDeviceInfoTable::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_PARAMS::str_size();
        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE, msg_size, true)?;
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<FifoDeviceInfoTable> {
        self.ctrl.push(queues)?;

        let mut msg = fw::ver::gen::s_NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_PARAMS::new(self.ctrl.get_data_ptr());

        let mut tbl : KVec<FifoDeviceEntry> = KVec::new();
        for i in 0..msg.get_numEntries() {
            let mut ent = msg.new_S_entries(i as isize);
            let eng_data: [u32; 16] = ent.get_engineData();
            tbl.push(FifoDeviceEntry {
                addr: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RUNLIST_PRI_BASE as usize],
                rmid: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RM_ENGINE_TYPE as usize],
                id: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RUNLIST as usize],
                eng_desc: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_ENG_DESC as usize],
            }, GFP_KERNEL)?;
        }
        Ok(FifoDeviceInfoTable { table: tbl })
    }
}

#[versions(GSP)]
pub(crate) struct CEGetFaultMethodBufferSize {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl CEGetFaultMethodBufferSize::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS::str_size();
        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE, msg_size, true)?;

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<u32> {
        self.ctrl.push(queues)?;

        let msg = fw::ver::gen::s_NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS::new(self.ctrl.get_data_ptr());
        pr_info!("fault buffer size {}", msg.get_size());
        Ok(msg.get_size())
    }
}


#[versions(GSP)]
pub(crate) struct GetConstructedFalconInfo {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl GetConstructedFalconInfo::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_INTERNAL_GET_CONSTRUCTED_FALCON_INFO_PARAMS::str_size();

        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_INTERNAL_GET_CONSTRUCTED_FALCON_INFO, msg_size, true)?;

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.push(queues)
    }

    pub(crate) fn find_desc_size(&mut self, desc: u32) -> u32 {
        let mut msg = fw::ver::gen::s_NV2080_CTRL_INTERNAL_GET_CONSTRUCTED_FALCON_INFO_PARAMS::new(self.ctrl.get_data_ptr());

        for i in 0..msg.get_numConstructedFalcons() {
            let tbl = msg.new_S_constructedFalconsTable(i as isize);
            if tbl.get_engDesc() == desc {
                return tbl.get_ctxBufferSize();
            }
        }
        0
    }
}

#[versions(GSP)]
pub(crate) struct GetVmmuSegmentSize {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl GetVmmuSegmentSize::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_GPU_GET_VMMU_SEGMENT_SIZE_PARAMS::str_size();

        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_GPU_GET_VMMU_SEGMENT_SIZE, msg_size, true)?;

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.push(queues)
    }

    pub(crate) fn get_segment_size(&mut self) -> u64 {
        let msg = fw::ver::gen::s_NV2080_CTRL_GPU_GET_VMMU_SEGMENT_SIZE_PARAMS::new(self.ctrl.get_data_ptr());
        msg.get_vmmuSegmentSize()
    }
}

#[versions(GSP)]
pub(crate) struct ShutdownVgpuPluginTask {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl ShutdownVgpuPluginTask::ver {
    pub(crate) fn new(device: &GspDevice, gfid: u32) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_VGPU_MGR_INTERNAL_SHUTDOWN_GSP_VGPU_PLUGIN_TASK, msg_size, false)?;

        let msg = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_PARAMS::new(ctrl.get_data_ptr())
            .gfid(gfid);

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct CleanupVgpuPlugin {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl CleanupVgpuPlugin::ver {
    pub(crate) fn new(device: &GspDevice, gfid: u32) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_VGPU_PLUGIN_CLEANUP_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_VGPU_MGR_INTERNAL_VGPU_PLUGIN_CLEANUP, msg_size, false)?;

        let msg = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_VGPU_PLUGIN_CLEANUP_PARAMS::new(ctrl.get_data_ptr())
            .gfid(gfid);

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct BootloadVgpuPluginTask {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl BootloadVgpuPluginTask::ver {
    pub(crate) fn new(device: &GspDevice, args: *const bindings::bootload_vgpu) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_BOOTLOAD_GSP_VGPU_PLUGIN_TASK_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_VGPU_MGR_INTERNAL_BOOTLOAD_GSP_VGPU_PLUGIN_TASK, msg_size, false)?;

        unsafe {
            let mut fb_phys_addr_list = [0u64; 384];
            let mut fb_length_list = [0u64; 384];
            fb_phys_addr_list[0] = (*args).fbmem_heap_addr;
            fb_length_list[0] = (*args).fbmem_heap_size;

            let msg = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_BOOTLOAD_GSP_VGPU_PLUGIN_TASK_PARAMS::new(ctrl.get_data_ptr())
                .dbdf((*args).dbdf)
                .gfid((*args).gfid)
                .numChannels((*args).num_channels)
                .numGuestFbSegments(1)
                .chidOffset((*args).chid_offset)
                .guestFbPhysAddrList(fb_phys_addr_list)
                .guestFbLengthList(fb_length_list)
                .pluginHeapMemoryPhysAddr((*args).heap_mem_addr)
                .pluginHeapMemoryLength((*args).heap_mem_size)
                .initTaskLogBuffOffset((*args).init_task_log_buf_offset)
                .initTaskLogBuffSize((*args).init_task_log_buf_size)
                .vgpuTaskLogBuffOffset((*args).vgpu_task_log_buf_offset)
                .vgpuTaskLogBuffSize((*args).vgpu_task_log_buf_size);
        }
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct PgpuAddVgpuType {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl PgpuAddVgpuType::ver {
    pub(crate) fn new(device: &GspDevice, vgpu_info_count: u32, args: *const core::ffi::c_void) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_PGPU_ADD_VGPU_TYPE_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_VGPU_MGR_INTERNAL_PGPU_ADD_VGPU_TYPE, msg_size, false)?;

        let mut msg = fw::ver::gen::s_NV2080_CTRL_VGPU_MGR_INTERNAL_PGPU_ADD_VGPU_TYPE_PARAMS::new(ctrl.get_data_ptr())
            .discardVgpuTypes(1)
            .vgpuInfoCount(vgpu_info_count);

        let mut info = msg.new_S_vgpuInfo(0);

        // HACKS
        unsafe {
            core::ptr::copy_nonoverlapping(args,
                                           info.raw() as *mut core::ffi::c_void,
                                           fw::ver::gen::s_NVA081_CTRL_VGPU_INFO::str_size() * vgpu_info_count as usize);
        }
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}
