#![allow(dead_code)]

pub(crate) use kernel::macros::versions;

use crate::gsp::*;

use core::sync::atomic::{fence, Ordering};
use core::time::Duration;
use kernel::delay::sleep;

use crate::accel::fifo::EventHandler;

#[allow(non_snake_case)]
#[repr(C)]
struct MsgQTxHeader {
    version: u32,
    size: u32,
    msg_size: u32,
    msg_count: u32,
    write_ptr: u32,
    flags: u32,
    rx_hdr_off: u32,
    entry_off: u32,
}

#[repr(C)]
struct MsgQRxHeader {
    read_ptr: u32,
}

#[repr(C)]
struct CmdQ {
    tx: MsgQTxHeader,
    rx: MsgQRxHeader,
}

// This struct represents the shared memory queues between GSP and CPU.
// There are two of these one for sending cmds to GSP from CPU
// One to send msgs and events from GSP to CPU.
// They operate on shared memory area, split into 4k pages, each message
// takes a page, and there are read and write ptrs on what page in the
// range to use next.

pub(crate) struct GSPSharedq {
    pub size: u32,
    pub cnt: u32,
    seq: u32,
    wptr: *mut u32,
    rptr: *mut u32,
    mem_ptr: *mut u8,
}

unsafe impl Send for GSPSharedq {}

impl GSPSharedq {
    pub(crate) fn new(size: u32,
               cnt: u32,
               wptr: *mut u32,
               rptr: *mut u32,
               mem_ptr: *mut u8) -> Self {
        Self {
            size,
            cnt,
            seq: 0,
            wptr,
            rptr,
            mem_ptr
        }
    }

    pub(crate) fn wait_for_write_slot(&self, wptr: u32) -> Result<isize> {
        let mut time = 1000000;
        let mut free: isize;
        loop {
            let rptr = self.read_rptr();
            free = (rptr + self.cnt - wptr - 1) as isize;
            if free >= self.cnt as isize {
                free -= self.cnt as isize;
            }
            if free >= 1 {
                break;
            }

            sleep(Duration::from_micros(2));

            time -= 1;
            if time == 0 {
                break;
            }
        }
        if time == 0 {
            pr_info!("Failed to get cmdq space\n");
            return Err(EINVAL);
        }
        Ok(free)
    }

    pub(crate) fn wait_for_read_slot(&self, wait_size: u32, ptime: &mut i32) -> Result<u32> {
        let rptr : u32 = self.read_rptr();
        let mut qused: u32;

        loop {
            let wptr : u32 = self.read_wptr();

            qused = wptr + self.cnt - rptr;
            if qused >= self.cnt {
                qused -= self.cnt;
            }
            if qused >= wait_size {
                break;
            }

            sleep(Duration::from_micros(2));

            *ptime -= 1;
            if *ptime == 0 {
                break;
            }
        }

        if *ptime == 0 {
            return Err(ETIME);
        }
        Ok(rptr)
    }

    pub(crate) fn read_wptr(&self) -> u32 {
        let val;
        unsafe {
            fence(Ordering::Acquire);
            val = *self.wptr;
        }
        val
    }
    pub(crate) fn read_rptr(&self) -> u32 {
        let val;
        unsafe {
            val = *self.rptr;
        }
        val
    }

    pub(crate) fn write_wptr(&self, val: u32) {
        unsafe {
            fence(Ordering::Acquire);
            *self.wptr = val;
            fence(Ordering::Release);
        }
    }

    pub(crate) fn write_rptr(&self, val: u32) {
        unsafe {
            fence(Ordering::Acquire);
            *(self.rptr) = val;
        }
    }

    pub(crate) fn get_slot_ptr(&self, slot: u32) -> *mut u8 {
        let ptr;
        unsafe {
            ptr = self.mem_ptr.byte_offset((0x1000 + slot * 0x1000) as isize);
        }
        ptr
    }

    pub(crate) fn queue_empty(&self) -> bool {
        let ret: bool;
        unsafe {
            ret = *self.rptr == *self.wptr;
        }
        ret
    }

    pub(crate) fn rd32(&self, slot: u32, offset: usize) -> u32 {
        let ptr = self.get_slot_ptr(slot);
        unsafe { *(ptr.byte_offset(offset as isize) as *mut u32) }
    }

    pub(crate) fn inc_seq(&mut self) -> u32 {
        let ret = self.seq;
        self.seq += 1;
        ret
    }

    pub(crate) fn copy_data_to_slot(&self, slot: u32, size: usize, src_offset: isize, src: *mut u8) {
        unsafe {
            core::ptr::copy_nonoverlapping(src.offset(src_offset), self.get_slot_ptr(slot), size);
        }
    }

    pub(crate) fn copy_data_from_slot(&self, slot: u32, size: usize, src_offset: isize, dst_offset: isize, dst: *mut u8) {
        unsafe {
            core::ptr::copy_nonoverlapping(self.get_slot_ptr(slot).byte_offset(src_offset), dst.byte_offset(dst_offset), size);
        }
    }
}

pub(crate) struct LockedQueues {
    pub cmdq: GSPSharedq,
    msgq: GSPSharedq,
}

#[versions(GSP)]
#[pin_data]
pub(crate) struct GSPSharedQueues {
    pub ptes_size: u32,
    pub ptes_nr: u32,
    pub cmdq_size: u32,
    #[pin]
    lq: Arc<Pin<KBox<Mutex<LockedQueues>>>>,
    pub gsp_falcon: Option<gsp_falcon::GspFalcon>,
    pub sec2_falcon: Option<Arc<Falcon>>,
    pub kill_handler: Option<Arc<EventHandler>>,
}

#[versions(GSP)]
impl GSPSharedQueues::ver {

    fn fill_cmdq(cmdq: *mut CmdQ, cmdq_size: usize) {
        unsafe {
            (*cmdq).tx.version = 0;
            (*cmdq).tx.size = cmdq_size as u32;
            (*cmdq).tx.entry_off = GSP_PAGE_SIZE;
            (*cmdq).tx.msg_size = GSP_PAGE_SIZE;
            (*cmdq).tx.msg_count = ((cmdq_size - GSP_PAGE_SIZE as usize) / GSP_PAGE_SIZE as usize) as u32;
            (*cmdq).tx.write_ptr = 0;
            (*cmdq).tx.flags = 1;

            (*cmdq).tx.rx_hdr_off = (core::mem::offset_of!(CmdQ, rx) + core::mem::offset_of!(MsgQRxHeader, read_ptr)) as u32;
        }
    }

    pub(crate) fn new(shm: &mut DmaObject, cmdq_size: u32, msgq_size: u32, ptes_size: u32, ptes_nr: u32) -> Result<Self> {
        let cmdq: GSPSharedq;
        let msgq: GSPSharedq;
        unsafe {
            let cmdq_raw_ptr: *mut u8 = shm.dma.start_ptr_mut().offset(ptes_size as isize) as *mut u8;
            let msgq_raw_ptr: *mut u8 = shm.dma.start_ptr_mut().offset(ptes_size as isize + cmdq_size as isize);
            let cmdq_ptr: *mut CmdQ = cmdq_raw_ptr as *mut CmdQ;
            let msgq_ptr: *mut CmdQ = msgq_raw_ptr as *mut CmdQ;

            Self::fill_cmdq(cmdq_ptr, cmdq_size as usize);
            cmdq = GSPSharedq::new(cmdq_size, (*cmdq_ptr).tx.msg_count,
                                   core::ptr::addr_of_mut!((*cmdq_ptr).tx.write_ptr),
                                   core::ptr::addr_of_mut!((*msgq_ptr).rx.read_ptr),
                                   cmdq_raw_ptr as *mut u8);
            msgq = GSPSharedq::new(msgq_size, (*cmdq_ptr).tx.msg_count,
                                   core::ptr::addr_of_mut!((*msgq_ptr).tx.write_ptr),
                                   core::ptr::addr_of_mut!((*cmdq_ptr).rx.read_ptr),
                                   msgq_raw_ptr as *mut u8);
        }

        let lock = Arc::new(KBox::pin_init(new_mutex!(LockedQueues { cmdq, msgq }), GFP_KERNEL)?, GFP_KERNEL)?;

        Ok(Self {
            ptes_size,
            ptes_nr,
            cmdq_size,
            lq: lock,
            gsp_falcon: None,
            sec2_falcon: None,
            kill_handler: None,
        })
    }

    pub(crate) fn bind_falcon(&mut self, gsp_falcon: gsp_falcon::GspFalcon, sec2_falcon: Arc<Falcon>) {
        self.gsp_falcon = Some(gsp_falcon);
        self.sec2_falcon = Some(sec2_falcon);
    }

    pub(crate) fn bind_kill_handler(&mut self, kill_handler: Arc<EventHandler>) {
        self.kill_handler = Some(kill_handler);
    }

    fn cmdq_push(&self, lq: &mut Guard<'_, LockedQueues, MutexBackend>, rpc: &mut RpcMsg::ver) -> Result<()> {
        let mut argc = rpc.csum(lq.cmdq.inc_seq());

        let mut off = 0;
        let mut wptr = lq.cmdq.read_wptr();
        loop {
            let free = lq.cmdq.wait_for_write_slot(wptr)?;
            let step = core::cmp::min::<u32>(free as u32, lq.cmdq.cnt - wptr);
            let size = core::cmp::min::<u32>(argc, step * GSP_PAGE_SIZE);

            lq.cmdq.copy_data_to_slot(wptr, size as usize, off, rpc.data.as_mut_ptr() as *mut u8);

            wptr += div_round_up(size as usize, 0x1000 as usize) as u32;
            if wptr == lq.cmdq.cnt {
                wptr = 0;
            }

            off += size as isize;
            argc -= size;

            if argc == 0 {
                break;
            }
        }

        lq.cmdq.write_wptr(wptr);
        self.gsp_falcon.as_ref().unwrap().cmdq_push()?;

        Ok(())
    }

    fn rpc_send(&self, lq: &mut Guard<'_, LockedQueues, MutexBackend>,
                rpc: &mut RpcMsg::ver, wait: bool, repc: u32) -> Result<()> {
        self.cmdq_push(lq, rpc)?;

        if wait {
            let rpc_fn: u32 = rpc.get_rpc_fn();
            let rep_vec = self.msg_recv(lq, rpc_fn, repc)?;

            rpc.set_recv(rep_vec);
        }
        Ok(())
    }

    fn rpc_push_locked(&self, lq: &mut Guard<'_, LockedQueues, MutexBackend>, rpc: &mut RpcMsg::ver, wait: bool, repc: u32) -> Result<()> {
        let max_msg_size : u32 = (16 * 0x1000) - RpcMsg::ver::get_gsp_msg_hdr_size();
        let max_rpc_size : u32 = max_msg_size - RpcMsg::ver::get_gsp_rpc_hdr_size();
        let mut rpc_size : u32 = rpc.get_rpc_length() - RpcMsg::ver::get_gsp_rpc_hdr_size();

        if rpc_size > max_rpc_size {
            let mut offset : u32 = 0;

            rpc.set_lengths(RpcMsg::ver::get_gsp_rpc_hdr_size() + max_rpc_size);
            let rpc_fn: u32 = rpc.get_rpc_fn();

            self.rpc_send(lq, rpc, false, 0)?;

            rpc_size -= max_rpc_size;
            offset += max_rpc_size;
            while rpc_size != 0 {
                let size: u32 = core::cmp::min::<u32>(rpc_size, max_rpc_size);

                let mut cont_rpc = RpcMsg::ver::new(fw::ver::gen::NV_VGPU_MSG_FUNCTION_CONTINUATION_RECORD, false, size as usize)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(rpc.get_data_ptr().byte_offset(offset as isize), cont_rpc.get_data_ptr(), size as usize);
                }

                self.rpc_send(lq, &mut cont_rpc, false, 0)?;
                offset += size;
                rpc_size -= size;
            }

            if wait {
                self.msg_recv(lq, rpc_fn, repc)?;
            }
        } else {
            self.rpc_send(lq, rpc, wait, repc)?;
        }
        Ok(())
    }

    fn msgq_wait(&self, lq: &Guard<'_, LockedQueues, MutexBackend>,
                 msg: Option<&mut KVec<u8>>,
                 offset: u32,
                 repc: u32, peeklen: Option<&mut u32>, ptime: &mut i32,
                 skip_copy_rpc_header: bool) -> Result<isize> {
        let size: u32 = div_round_up((RpcMsg::ver::get_gsp_msg_hdr_size() + repc) as usize, GSP_PAGE_SIZE as usize) as u32;
        if size == 0 || size >= lq.msgq.cnt {
            pr_info!("ERROR IN MSGQ WAIT {}", size);
            return Err(EINVAL);
        }

        let mut rptr = match lq.msgq.wait_for_read_slot(size, ptime) {
            Err(_) => {
                pr_info!("Error timedout waiting for msgq read slot");
                return Err(ETIME);
            },
            Ok(x) => { x }
        };

        if peeklen != None {
            let peek_val = peeklen.unwrap();

            *peek_val = RpcMsg::ver::get_rpc_length_from_ptr(lq.msgq.get_slot_ptr(rptr));
            return Ok(0);
        }

        let size = align(repc as usize + RpcMsg::ver::get_gsp_msg_hdr_size() as usize, GSP_PAGE_SIZE as usize);

        let mut len = ((lq.msgq.cnt - rptr) * GSP_PAGE_SIZE) - RpcMsg::ver::get_gsp_msg_hdr_size();
        len = core::cmp::min::<u32>(repc, len);

        let msg = msg.unwrap();
        if !skip_copy_rpc_header {
            lq.msgq.copy_data_from_slot(rptr, len as usize, RpcMsg::ver::get_gsp_msg_hdr_size() as isize,
                                        offset as isize, msg.as_mut_ptr() as *mut u8);
        } else {
            lq.msgq.copy_data_from_slot(rptr, len as usize - RpcMsg::ver::get_gsp_msg_hdr_size() as usize,
                                        RpcMsg::ver::get_gsp_msg_hdr_size() as isize,
                                        offset as isize + RpcMsg::ver::get_gsp_msg_hdr_size() as isize, msg.as_mut_ptr() as *mut u8);
        }

        let new_repc = repc - len;

        if new_repc != 0 {

            // I don't think this code makes any sense - nouveau does this so just copy it for now
            lq.msgq.copy_data_from_slot(0, new_repc as usize, 0, len as isize, msg.as_mut_ptr() as *mut u8);
            // also probably mssing msg set length
        }

        rptr = (rptr + div_round_up(size as usize, GSP_PAGE_SIZE as usize) as u32) % lq.msgq.cnt;

        lq.msgq.write_rptr(rptr);

        Ok((offset + len) as isize)
    }

    fn msgq_recv(&self, lq: &Guard<'_, LockedQueues, MutexBackend>, msg_repc: u32, total_repc: u32, ptime: &mut i32) -> Result<Option<KVec<u8>>> {
        let max_msg_size : u32 = (16 * 0x1000) - RpcMsg::ver::get_gsp_msg_hdr_size();
        let max_rpc_size : u32 = max_msg_size - RpcMsg::ver::get_gsp_rpc_hdr_size();
        let repc: u32 = total_repc;

        let buf_size = core::cmp::max::<u32>(msg_repc, total_repc + RpcMsg::ver::get_gsp_rpc_hdr_size());

        let mut msg = KVec::with_capacity(buf_size as usize, GFP_KERNEL)?;
        unsafe {
            msg.set_len(buf_size as usize);
        }
        let _msg_offset = self.msgq_wait(lq, Some(&mut msg), 0, msg_repc, None, ptime, false)?;

        if total_repc <= max_rpc_size {
            return Ok(Some(msg));
        }

        let mut offset = msg_repc;
        let mut new_repc = repc - msg_repc - RpcMsg::ver::get_gsp_rpc_hdr_size();

        while new_repc != 0 {
            let size = self.msg_recv_continuation(lq, &mut msg, offset, new_repc, ptime)?;

            new_repc -= size as u32;
            offset += size as u32;
        }

        Ok(Some(msg))
    }

    fn msg_recv_continuation(&self, lq: &Guard<'_, LockedQueues, MutexBackend>, msg: &mut KVec<u8>, offset: u32, _repc: u32, ptime: &mut i32) -> Result<isize> {
        let mut peekval: u32 = 0;

        self.msgq_wait(lq, None, 0, RpcMsg::ver::get_gsp_rpc_hdr_size(), Some(&mut peekval), ptime, false)?;

        let msg_length = peekval;

        self.msgq_wait(lq, Some(msg), offset, msg_length, None, ptime, true)
    }

    fn msg_recv(&self, lq: &Guard<'_, LockedQueues, MutexBackend>, rpc_fn: u32, repc: u32) -> Result<Option<KVec<u8>>> {
        loop {
            let mut peekval: u32 = 0;
            let mut time = 4000000;

            self.msgq_wait(lq, None, 0, RpcMsg::ver::get_gsp_rpc_hdr_size(), Some(&mut peekval), &mut time, false)?;

            let msg_length = peekval;

            let msg = self.msgq_recv(lq, msg_length, repc, &mut time)?;

            if msg.is_none() {
                return Err(EINVAL);
            }

            let mut msg = msg.unwrap();

            let (recv_rpc_fn, rpc_result) = RpcMsg::ver::get_rpc_result(&mut msg);

            if rpc_result != 0 {
                pr_info!("MESSAGE INVALID {}\n", rpc_result);
                return Err(EINVAL);
            }

            if rpc_fn != 0 && recv_rpc_fn == rpc_fn {
                if repc != 0 {
                    pr_info!("MSG FUNC MATCHED {}", rpc_fn);
                    return Ok(Some(msg));
                }
                return Ok(None);
            }

            match recv_rpc_fn {
                fw::ver::gen::NV_VGPU_MSG_EVENT_GSP_RUN_CPU_SEQUENCER => {
                    if !self.gsp_falcon.is_none() &&
                        !self.sec2_falcon.is_none() {
                            notifiers::Notifiers::ver::run_cpu_sequencer(self.gsp_falcon.as_ref().unwrap(), self.sec2_falcon.as_ref().unwrap(), &mut msg)?;
                        }
                },
                fw::ver::gen::NV_VGPU_MSG_EVENT_OS_ERROR_LOG => {
                    notifiers::Notifiers::ver::os_error_log(&mut msg);
                },
                fw::ver::gen::NV_VGPU_MSG_EVENT_RC_TRIGGERED => {
                    notifiers::Notifiers::ver::rc_triggered(&mut msg, &self.kill_handler);
                },
                fw::ver::gen::NV_VGPU_MSG_EVENT_MMU_FAULT_QUEUED => {
                    notifiers::Notifiers::ver::mmu_fault_queued(&mut msg);
                },
                fw::ver::gen::NV_VGPU_MSG_EVENT_GPUACCT_PERFMON_UTIL_SAMPLES => {},

                #[ver(r == r535_113_01)]
                fw::ver::gen::NV_VGPU_MSG_EVENT_GSP_SEND_USER_SHARED_DATA => {
                    notifiers::Notifiers::ver::user_shared_data(&mut msg);
                },

                unk => { pr_info!("Unhandled {:#x}\n", unk); },

            }

            if rpc_fn == 0 && lq.msgq.queue_empty() {
                break;
            }
        }
        Ok(None)
    }

    pub(crate) fn rpc_poll(&mut self, rpc_fn: u32) -> Result<()> {
        let mut locked = self.lq.lock();
        self.msg_recv(&mut locked, rpc_fn, 0)?;
        Ok(())
    }

    pub(crate) fn msg_irq_work(&self) {
        let mut locked = self.lq.lock();
        if !locked.msgq.queue_empty() {
            let _ = self.msg_recv(&mut locked, 0, 0);
        }
    }

    pub(crate) fn rpc_push(&mut self, rpc: &mut RpcMsg::ver, wait: bool, repc: u32) -> Result<()> {
        let mut locked = self.lq.lock();
        self.rpc_push_locked(&mut locked, rpc, wait, repc)
    }

    pub(crate) fn poll_gsp_init_done(&mut self) -> Result<()> {
        self.rpc_poll(fw::ver::gen::NV_VGPU_MSG_EVENT_GSP_INIT_DONE)
    }
}
