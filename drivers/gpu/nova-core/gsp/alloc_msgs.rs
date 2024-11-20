#![allow(unused)]
pub(crate) use kernel::macros::versions;
use kernel::prelude::*;

use crate::nvfw::*;
use crate::gsp::rpc_msgs::*;
use crate::gsp::GspClient;
use crate::gsp::*;

#[versions(GSP)]
pub(crate) struct AllocClient {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl AllocClient::ver {
    pub(crate) fn new(id: u16, process_id: u32) -> Result<Self> {
        let handle = 0xc1d00000 | id as u32;
        let oclass = fw::ver::gen::NV01_ROOT;

        let mut msg = AllocMsg::ver::get(None, None, handle,
                                         oclass,
                                         fw::ver::gen::s_NV0000_ALLOC_PARAMETERS::str_size())?;

        let _msg = fw::ver::gen::s_NV0000_ALLOC_PARAMETERS::new(msg.get_data_ptr())
            .hClient(handle)
            .processID(process_id);

        Ok(Self {
            msg,
            handle,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct AllocDevice {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl AllocDevice::ver {
    pub(crate) fn new(client: &GspClient) -> Result<Self> {
        let handle = 0xde1d0000;
        let oclass = fw::ver::gen::NV01_DEVICE_0;

        let mut msg = AllocMsg::ver::get(Some(client), None, handle,
                                         oclass,
                                         fw::ver::gen::s_NV0080_ALLOC_PARAMETERS::str_size())?;

        let _msg = fw::ver::gen::s_NV0080_ALLOC_PARAMETERS::new(msg.get_data_ptr())
            .hClientShare(client.object.handle);

        Ok(Self {
            msg,
            handle,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct AllocSubdevice {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl AllocSubdevice::ver {
    pub(crate) fn new(devobj: &GspObject) -> Result<Self> {
        let handle = 0x5d1d0000;
        let oclass = fw::ver::gen::NV20_SUBDEVICE_0;

        let msg = AllocMsg::ver::get(devobj.client.clone().as_deref(),
                                     Some(devobj),
                                     handle,
                                     oclass,
                                     fw::ver::gen::s_NV2080_ALLOC_PARAMETERS::str_size())?;

        Ok(Self {
            msg,
            handle,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct AllocEvent {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl AllocEvent::ver {
    pub(crate) fn new(handle: u32, id: u32, device: &GspDevice) -> Result<Self> {
        let oclass = fw::ver::gen::NV01_EVENT_KERNEL_CALLBACK_EX;

        let client = device.object.client.as_ref().unwrap();
        let mut msg = AllocMsg::ver::get(Some(&client),
                                         Some(&device.subdevice),
                                         handle,
                                         oclass,
                                         fw::ver::gen::s_NV0005_ALLOC_PARAMETERS::str_size())?;
        let mut _msg = fw::ver::gen::s_NV0005_ALLOC_PARAMETERS::new(msg.get_data_ptr())
            .hParentClient(client.object.handle)
            .hClass(fw::ver::gen::NV01_EVENT_KERNEL_CALLBACK_EX)
            .notifyIndex(fw::ver::gen::NV01_EVENT_CLIENT_RM | id);
        Ok(Self {
            msg,
            handle,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct AllocVMM {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl AllocVMM::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {

        let client = device.object.client.as_ref().unwrap();
        let oclass = fw::ver::gen::FERMI_VASPACE_A;
        let handle = 0x90f10000;
        let mut msg = AllocMsg::ver::get(Some(&client),
                                         Some(&device.object),
                                         handle,
                                         oclass,
                                         fw::ver::gen::s_NV_VASPACE_ALLOCATION_PARAMETERS::str_size())?;
        let mut _msg = fw::ver::gen::s_NV_VASPACE_ALLOCATION_PARAMETERS::new(msg.get_data_ptr())
            .index(fw::ver::gen::NV_VASPACE_ALLOCATION_INDEX_GPU_NEW);

        Ok(Self {
            msg,
            handle,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct CEAlloc {
    pub msg: AllocMsg::ver,
}

#[versions(GSP)]
impl CEAlloc::ver {


}
