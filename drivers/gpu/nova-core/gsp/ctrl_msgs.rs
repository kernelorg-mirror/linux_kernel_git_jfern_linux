#![allow(unused)]
pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use kernel::bindings;
use crate::gsp::*;
use crate::accel::fifo::EngineType;
use crate::gsp::rpc_msgs::*;
use crate::accel::fifo::GpuPromoteBufferEntry;
use crate::gpu::IntrInfo;
use crate::accel::gr::CtxBufSize;
use crate::accel::fifo::{FifoDeviceEntry, FifoDeviceInfoTable};
use crate::nvfw::*;
use crate::gsp::EventSetNotificationAction;

use crate::mmu::vmm::{Vmm, Vma};
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

    pub(crate) fn convert_eng_to_nv2080(et: EngineType, inst: u32) -> u32 {
        match et {
            EngineType::GR => { fw::ver::gen::NV2080_ENGINE_TYPE_GR0 },
            EngineType::CE => { fw::ver::gen::NV2080_ENGINE_TYPE_COPY0 + inst },
            EngineType::NVDEC => { fw::ver::gen::NV2080_ENGINE_TYPE_NVDEC0 + inst },
            EngineType::NVENC => { fw::ver::gen::NV2080_ENGINE_TYPE_NVENC0 + inst },
            _ => { 0 }
        }
    }

    fn convert_rmid_to_engine_inst(rmid: u32) -> Result<(EngineType, u32)> {
        // TODO autogenerate
        match rmid {
            fw::ver::gen::RM_ENGINE_TYPE_GR0 => { Ok((EngineType::GR, 0)) },
            r @ fw::ver::gen::RM_ENGINE_TYPE_COPY0..=fw::ver::gen::RM_ENGINE_TYPE_COPY9 => Ok((EngineType::CE, r - fw::ver::gen::RM_ENGINE_TYPE_COPY0)),
            r @ fw::ver::gen::RM_ENGINE_TYPE_NVDEC0..=fw::ver::gen::RM_ENGINE_TYPE_NVDEC7 => Ok((EngineType::NVDEC, r - fw::ver::gen::RM_ENGINE_TYPE_NVDEC0)),
            r @ fw::ver::gen::RM_ENGINE_TYPE_NVENC0..=fw::ver::gen::RM_ENGINE_TYPE_NVENC2 => Ok((EngineType::NVENC, r - fw::ver::gen::RM_ENGINE_TYPE_NVENC0)),
            other => Err(EINVAL)
        }
    }

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
            let (eng_type, inst) = match Self::convert_rmid_to_engine_inst(eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RM_ENGINE_TYPE as usize]) {
                Err(x) => { continue; }
                Ok((e, i)) => (e, i)
            };

            pr_info!("fifo device {} {:?} {}\n", eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RM_ENGINE_TYPE as usize], eng_type, inst);

            tbl.push(FifoDeviceEntry {
                addr: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RUNLIST_PRI_BASE as usize],
                eng_type,
                inst,
                id: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_RUNLIST as usize] as i32,
                eng_desc: eng_data[fw::ver::gen::ENGINE_INFO_TYPE_ENG_DESC as usize],
                desc_size: 0,
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

    pub(crate) fn fill_sizes(&mut self, table: &mut FifoDeviceInfoTable) {
        let mut msg = fw::ver::gen::s_NV2080_CTRL_INTERNAL_GET_CONSTRUCTED_FALCON_INFO_PARAMS::new(self.ctrl.get_data_ptr());

        for i in 0..msg.get_numConstructedFalcons() {
            let tbl = msg.new_S_constructedFalconsTable(i as isize);
            let desc = tbl.get_engDesc();
            for ent in &mut table.table {
                if ent.eng_desc == desc {
                    ent.desc_size = tbl.get_ctxBufferSize();
                }
            }
        }
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

#[versions(GSP)]
pub(crate) struct VASpaceCopyServerReservedPdes {
    pub ctrl: ControlMsg::ver
}

#[versions(GSP)]
impl VASpaceCopyServerReservedPdes::ver {
    pub(crate) fn new(gsp_va: &GspVa, vmm: &Vmm) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV90F1_CTRL_VASPACE_COPY_SERVER_RESERVED_PDES_PARAMS::str_size();

        let mut ctrl = ControlMsg::ver::get(&gsp_va.object, fw::ver::gen::NV90F1_CTRL_CMD_VASPACE_COPY_SERVER_RESERVED_PDES, msg_size, false)?;

        let promote_info = vmm.get_promote_info()?;
        let lvl2_size = 0x1000;
        let lvl2_aperture = 1;
        let lvl2_page_shift = 0x1d;
        let mut msg = fw::ver::gen::s_NV90F1_CTRL_VASPACE_COPY_SERVER_RESERVED_PDES_PARAMS::new(ctrl.get_data_ptr())
            .virtAddrLo(promote_info.rsvd_lo)
            .virtAddrHi(promote_info.rsvd_hi)
            .pageSize(0x20000000)
            .numLevelsToCopy(promote_info.num_levels)
            .levels_0_physAddress(promote_info.level0_phys_addr)
            .levels_0_size(0x20)
            .levels_0_aperture(1)
            .levels_0_pageShift(0x2f)
            .levels_1_physAddress(promote_info.level1_phys_addr)
            .levels_1_size(0x1000)
            .levels_1_aperture(1)
            .levels_1_pageShift(0x26)
            .levels_2_physAddress(promote_info.level2_phys_addr)
            .levels_2_size(lvl2_size)
            .levels_2_aperture(1)
            .levels_2_pageShift(0x1d);

        Ok(Self {
            ctrl
        })
    }
    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct GpuPromoteCtx {
    pub ctrl: ControlMsg::ver
}

#[versions(GSP)]
impl GpuPromoteCtx::ver {
    pub(crate) fn new_promote_gr(device: &GspDevice, channel: &GspChannel,
                                 entries: &KVec<GpuPromoteBufferEntry>) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS::str_size();
        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_GPU_PROMOTE_CTX, msg_size, false)?;

        let num_ents = entries.len();

        let mut msg = fw::ver::gen::s_NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS::new(ctrl.get_data_ptr())
            .engineType(1)
            .hChanClient(device.object.client.as_ref().unwrap().object.handle)
            .hObject(channel.object.handle)
            .entryCount(num_ents as u32);

        pr_info!("promote_gr: dev:{:#x} chan:{:#x} ents:{}\n",
                 device.object.client.as_ref().unwrap().object.handle,
                 channel.object.handle,
                 entries.len());

        for i in 0..num_ents {
            pr_info!("promote {}: pa:{:#x}/{:#x} sz {:#x} va {:#x} init:{} nm:{}\n",
                     entries[i].buffer_id,
                     entries[i].gpu_phys_addr,
                     entries[i].physattr,
                     entries[i].size,
                     entries[i].gpu_virt_addr,
                     entries[i].initialize,
                     entries[i].nonmapped);
            let _ent = msg.new_S_promoteEntry(i as isize)
                .gpuPhysAddr(entries[i].gpu_phys_addr)
                .gpuVirtAddr(entries[i].gpu_virt_addr)
                .physAttr(entries[i].physattr)
                .size(entries[i].size)
                .bufferId(entries[i].buffer_id)
                .bInitialize(entries[i].initialize as u8)
                .bNonmapped(entries[i].nonmapped as u8);
        }

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn new_falcon(device: &GspDevice, channel: &GspChannel,
                             addr: u64, size: u64, engine_id: u32) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS::str_size();
        let mut ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_GPU_PROMOTE_CTX, msg_size, false)?;

        let mut msg = fw::ver::gen::s_NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS::new(ctrl.get_data_ptr())
            .hClient(device.object.client.as_ref().unwrap().object.handle)
            .hObject(channel.object.handle)
            .hChanClient(device.object.client.as_ref().unwrap().object.handle)
            .virtAddress(addr)
            .size(size)
            .engineType(engine_id)
            .ChID(channel.chid);

        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct GpFifoSchedule {
    pub ctrl:  ControlMsg::ver
}

#[versions(GSP)]
impl GpFifoSchedule::ver {
    pub(crate) fn new(channel: &GspChannel, enable: bool) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS::str_size();
        let mut ctrl = ControlMsg::ver::get(&channel.object, fw::ver::gen::NVA06F_CTRL_CMD_GPFIFO_SCHEDULE, msg_size, false)?;

        let mut msg = fw::ver::gen::s_NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS::new(ctrl.get_data_ptr())
            .bEnable(enable as u8);
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}


#[versions(GSP)]
pub(crate) struct BindParams {
    pub ctrl: ControlMsg::ver
}

#[versions(GSP)]
impl BindParams::ver {
    pub(crate) fn new(channel: &GspChannel, engineType: EngineType, engineInst: u32) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NVA06F_CTRL_BIND_PARAMS::str_size();
        let mut ctrl = ControlMsg::ver::get(&channel.object, fw::ver::gen::NVA06F_CTRL_CMD_BIND, msg_size, false)?;

        let nv2080_et = FifoGetDeviceInfoTable::ver::convert_eng_to_nv2080(engineType, engineInst);
        let mut msg = fw::ver::gen::s_NVA06F_CTRL_BIND_PARAMS::new(ctrl.get_data_ptr())
            .engineType(nv2080_et);
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<()> {
        self.ctrl.wr(queues)
    }
}

#[versions(GSP)]
pub(crate) struct InternalStaticKGRGetContextBuffersInfo {
    pub ctrl: ControlMsg::ver,
}

#[versions(GSP)]
impl InternalStaticKGRGetContextBuffersInfo::ver {
    pub(crate) fn new(device: &GspDevice) -> Result<Self> {
        let msg_size = fw::ver::gen::s_NV2080_CTRL_INTERNAL_STATIC_GR_GET_CONTEXT_BUFFERS_INFO_PARAMS::str_size();

        let ctrl = ControlMsg::ver::get(&device.subdevice, fw::ver::gen::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO, msg_size, true)?;
        Ok(Self {
            ctrl
        })
    }

    pub(crate) fn push(&mut self, queues: &mut GSPSharedQueues::ver) -> Result<KVec<CtxBufSize>> {
        self.ctrl.push(queues)?;

        let mut msg = fw::ver::gen::s_NV2080_CTRL_INTERNAL_STATIC_GR_GET_CONTEXT_BUFFERS_INFO_PARAMS::new(self.ctrl.get_data_ptr());

        // only entry 0 ever seems to be filled out
        //for i in 0..fw::ver::gen::NV2080_CTRL_INTERNAL_GR_MAX_ENGINES {
        let mut eng = msg.new_S_engineContextBuffersInfo(0);

        let mut ctxvec = KVec::new();
        for e in 0..fw::ver::gen::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_COUNT {
            let props = eng.new_S_engine(e as isize);

            ctxvec.push(CtxBufSize { size: props.get_size(), align: props.get_alignment() as u8 }, GFP_KERNEL)?;
            pr_info!("engine {}: {}/{}\n", e, props.get_size(), props.get_alignment());
        }
        Ok(ctxvec)
    }
}
