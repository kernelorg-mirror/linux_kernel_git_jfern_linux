#![allow(unused)]
pub(crate) use kernel::macros::versions;
use kernel::prelude::*;

use crate::nvfw::*;
use crate::gsp::rpc_msgs::*;
use crate::gsp::GspClient;
use crate::gsp::*;
use crate::accel::fifo::EngineType;
use crate::mmu::memory::Memory;
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

        let client = devobj.client.as_ref().unwrap();
        let msg = AllocMsg::ver::get(Some(&client),
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
    pub(crate) fn new(device: &GspDevice, handle: u32, id: u32) -> Result<Self> {
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
    pub(crate) fn new(device: &GspDevice, id: u32) -> Result<Self> {

        let client = device.object.client.as_ref().unwrap();
        let oclass = fw::ver::gen::FERMI_VASPACE_A;
        let handle = 0x90f10000 | id;
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
    pub(crate) fn new(channel: &GspChannel, handle: u32, oclass: u32, inst: u32) -> Result<Self> {
        let mut msg = AllocMsg::ver::get(Some(&channel.object.client.as_ref().unwrap()),
                                         Some(&channel.object),
                                         handle,
                                         oclass,
                                         fw::ver::gen::s_NVC0B5_ALLOCATION_PARAMETERS::str_size())?;
        let mut _msg = fw::ver::gen::s_NVC0B5_ALLOCATION_PARAMETERS::new(msg.get_data_ptr())
            .version(1)
            .engineType(fw::ver::gen::NV2080_ENGINE_TYPE_COPY0 + inst);

        Ok(Self {
            msg,
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}

#[versions(GSP)]
pub(crate) struct FifoAlloc {
    pub msg: AllocMsg::ver,
    pub handle: u32,
}

#[versions(GSP)]
impl FifoAlloc::ver {
    pub(crate) fn new(device: &GspDevice,
                      engine_type: EngineType, engine_inst: u32,
                      va: &GspVa,
                      instobj: &InstObj,
                      userd: &VramObj,
                      mthdbuf: &DmaObject,
                      chan_id: u32, oclass: u32,
                      offset: u64,
                      length: u64,
                      chan_priv: bool) -> Result<Self> {
        let client = device.object.client.as_ref().unwrap();
        let handle = 0xf1f00000 | chan_id;
        let mut msg = AllocMsg::ver::get(Some(&client),
                                         Some(&device.object),
                                         handle,
                                         oclass,
                                         fw::ver::gen::s_NV_CHANNEL_ALLOC_PARAMS::str_size())?;

        let mut flags = 0;

        let nv2080_et = FifoGetDeviceInfoTable::ver::convert_eng_to_nv2080(engine_type, engine_inst);
        //PHYSICAL 0
        //VPR false
        //CHANNEL_SKIP_MAP_REFCOUNTING FALSE

        // runq 1 bit?
        flags |= if chan_priv { 1 << 5 } else { 0 };
        // delay channel scheduling FLASE
        // deny physical mode CE FALSE
        // userd index value 3 - bits
        let userd_i = chan_id % 8;
        let userd_p = chan_id / 8;
        flags |= userd_i << 8;
        // userd index fixed
        // userd page value
        flags |= userd_p << 12;
        // used page fixed
        flags |= 1 << 21;
        // deny auth level priv FALSE
        // skip scrubber FALSE
        // client map fifo FALSE
        // set evict last CE prefetch FALSE
        // vgpu plugin context FALSE
        // pbdma acquire timeout FALSE
        // channel thread DEFAULT
        // map channel FALSE
        // skip ctxbuffer alloc FALSE

        let mut internal_flags = 0;

        // PRIV
        internal_flags |= if chan_priv { 0x1 } else { 0x0 };
        // ERROR NOTIFIER NONE
        internal_flags |= 1 << 2;
        // ECC ERROR NOTIFIER TYPE NONE
        internal_flags |= 1 << 4;

        let mut msg_params = fw::ver::gen::s_NV_CHANNEL_ALLOC_PARAMS::new(msg.get_data_ptr())
            .gpFifoOffset(offset)
            .gpFifoEntries(length as u32 / 8)
            .flags(flags)
            .hVASpace(va.object.handle)
            .engineType(nv2080_et)
            .internalFlags(internal_flags);

        let _ = msg_params.new_S_instanceMem()
            .base(instobj.addr()?)
            .size(instobj.size()?)
            .addressSpace(2)
            .cacheAttrib(1);

        let _ = msg_params.new_S_userdMem()
            .base(userd.addr()?)
            .size(0x200)
            .addressSpace(2)
            .cacheAttrib(1);

        let ramfc_size = 0x200;
        let _ = msg_params.new_S_ramfcMem()
            .base(instobj.addr()?)
            .size(ramfc_size)
            .addressSpace(2)
            .cacheAttrib(1);

        let _ = msg_params.new_S_mthdbufMem()
            .base(mthdbuf.dma.dma_handle())
            .size(mthdbuf.len as u64)
            .addressSpace(1)
            .cacheAttrib(0);

        Ok(Self {
            msg,
            handle,
        })

    }

    // Create the message used in gr golden context creation
    pub(crate) fn golden(device: &GspDevice,
                         va: &GspVa,
                         instobj: &InstObj,
                         rsvd_chids: u32,
                         oclass: u32) -> Result<Self> {
        let chan_id = rsvd_chids;
        let client = device.object.client.as_ref().unwrap();
        let handle = 0xf1f00000 | chan_id;
        let mut msg = AllocMsg::ver::get(Some(&client),
                                         Some(&device.object),
                                         handle,
                                         oclass,
                                         fw::ver::gen::s_NV_CHANNEL_ALLOC_PARAMS::str_size())?;

        let mut flags = 0;
        let nv2080_et = 1;
        //PHYSICAL 0
        //VPR false
        //CHANNEL_SKIP_MAP_REFCOUNTING FALSE
        flags |= 1 << 5;
        // runq 0
        // delay channel scheduling FLASE
        // deny physical mode CE FALSE
        let userd_i = chan_id % 8;
        let userd_p = chan_id / 8;
        flags |= userd_i << 8;
        // userd index fixed
        // userd page value
        flags |= userd_p << 12;
        flags |= 1 << 21;
        // deny auth level priv FALSE
        // skip scrubber FALSE
        // client map fifo FALSE
        // set evict last CE prefetch FALSE
        // vgpu plugin context FALSE
        // pbdma acquire timeout FALSE
        // channel thread DEFAULT
        // map channel FALSE
        // skip ctxbuffer alloc FALSE

        let mut internal_flags = 0;

        // PRIV
        internal_flags |= 1;
        // ERROR NOTIFIER NONE
        internal_flags |= 1 << 2;
        // ECC ERROR NOTIFIER TYPE NONE
        internal_flags |= 1 << 4;

        pr_info!("alloc flags {:#x} {:#x}\n", flags, internal_flags);
        let mut msg_params = fw::ver::gen::s_NV_CHANNEL_ALLOC_PARAMS::new(msg.get_data_ptr())
            .gpFifoOffset(0)
            .gpFifoEntries(0x1000 / 8)
            .flags(flags)
            .hVASpace(va.object.handle)
            .engineType(nv2080_et)
            .internalFlags(internal_flags);

        pr_info!("instobj {:#x} {:#x}\n", instobj.addr()?, instobj.size()?);
        let _ = msg_params.new_S_instanceMem()
            .base(instobj.addr()?)
            .size(0x1000)
            .addressSpace(2)
            .cacheAttrib(1);

        let _ = msg_params.new_S_userdMem()
            .base(instobj.addr()? + 0x1000)
            .size(0x200)
            .addressSpace(2)
            .cacheAttrib(1);

        let ramfc_size = 0x200;
        let _ = msg_params.new_S_ramfcMem()
            .base(instobj.addr()?)
            .size(ramfc_size)
            .addressSpace(2)
            .cacheAttrib(1);

        let _ = msg_params.new_S_mthdbufMem()
            .base(instobj.addr()? + 0x2000)
            .size(0x5000)
            .addressSpace(2)
            .cacheAttrib(1);

        Ok(Self {
            msg,
            handle,
        })

    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.msg.push(queues)
    }
}
