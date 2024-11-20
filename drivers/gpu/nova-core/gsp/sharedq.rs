#![allow(dead_code)]

pub(crate) use kernel::macros::versions;

use crate::gsp::*;

use core::sync::atomic::{fence, Ordering};
use core::time::Duration;
use kernel::delay::sleep;

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
        })
    }

    pub(crate) fn bind_falcon(&mut self, gsp_falcon: gsp_falcon::GspFalcon, sec2_falcon: Arc<Falcon>) {
        self.gsp_falcon = Some(gsp_falcon);
        self.sec2_falcon = Some(sec2_falcon);
    }
}
