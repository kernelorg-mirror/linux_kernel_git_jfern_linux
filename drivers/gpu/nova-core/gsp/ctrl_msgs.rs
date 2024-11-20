#![allow(unused)]
pub(crate) use kernel::macros::versions;
use kernel::prelude::*;

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
