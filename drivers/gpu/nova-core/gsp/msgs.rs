#![allow(unused)]

pub(crate) use kernel::macros::versions;
use kernel::prelude::*;
use crate::align;

use crate::gsp::GSP_PAGE_SIZE;
use crate::div_round_up;

use crate::nvfw::*;

// There are levels of encapsulated messages passed between GSP and CPU.
// Each level adds a header to the start of the message before passing it down.
//
// At the lowest level this are command messages, these have a header with
// amount of pages the message takes, sequence number and a checksum.
//
// At the next level are RPC messages, these add a length, function, result .
//
// The RpcMsg structure encapsulates both command and rpc headers for sent messages.
// For received message the cmd headers are removed, and only rpc headers + message
// remain.
//
// Then above there is the RPC Control message which encapsulates another set of APIs
// with client and object information

// This file defines the base interfaces for Cmd and Rpc msgs.

#[versions(GSP)]
pub(crate) struct RpcMsg {
    pub data: KVec<u8>,
    orig_size: u32,
    recv_data: KVec<u8>,
    has_reply: bool,
    use_reply_data: bool,
}

#[versions(GSP)]
impl RpcMsg::ver {

    pub(crate) const fn get_gsp_msg_min_size() -> usize {
	GSP_PAGE_SIZE as usize
    }

    pub(crate) const fn get_gsp_msg_hdr_size() -> u32 {
	(fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::str_size() - fw::ver::gen::s_rpc_message_header_v03_00::str_size()) as u32
    }

    pub(crate) const fn get_gsp_rpc_hdr_size() -> u32 {
	fw::ver::gen::s_rpc_message_header_v03_00::str_size() as u32
    }

    // This gets the rpc msg length from the shared memory with cmd header
    pub(crate) fn get_rpc_length_from_ptr(recv_data: *mut u8) -> u32 {
	let mut msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(recv_data);
	let rpc_hdr = msg.new_S_rpc();
	rpc_hdr.get_length()
    }

    // This builds the rpc header from the msg once copied out of the shm
    pub(crate) fn get_rpc_result(recv_data: &mut KVec<u8>) -> (u32, u32) {
	let rpc_hdr = fw::ver::gen::s_rpc_message_header_v03_00::new(recv_data.as_mut_ptr());
	(rpc_hdr.get_function(), rpc_hdr.get_rpc_result())
    }

    fn get_rpc(&mut self) -> fw::ver::gen::s_rpc_message_header_v03_00<'_> {
	let mut msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(self.data.as_mut_ptr());
	msg.new_S_rpc()
    }

    pub(crate) fn csum(&mut self, seq: u32) -> u32 {
	let mut msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(self.data.as_mut_ptr());
	let mut argc = msg.get_checkSum();

	argc = align((Self::get_gsp_msg_hdr_size() + argc) as usize, GSP_PAGE_SIZE as usize) as u32;
	msg.set_seqNum(seq);
	msg.set_elemCount(div_round_up(argc as usize, 0x1000) as u32);
	msg.set_checkSum(0);

	let mut ptr : *const u64 = self.data.as_ptr() as *const u64;
	let mut csum: u64 = 0;

	unsafe {
	    let end : *const u64 = (self.data.as_ptr() as *const u8).byte_offset(argc as isize) as *const u64;

	    while ptr < end {
		csum ^= *ptr;
		ptr = ptr.offset(1);
	    }
	}

	let final_csum = (csum >> 32) as u32 ^ (csum & 0xffffffff) as u32;
	msg.set_checkSum(final_csum);
	argc
    }

    pub(crate) fn new(rpc_fn: u32, has_reply: bool, size: usize) -> Result<Self> {
	let rpc_size = size + Self::get_gsp_rpc_hdr_size() as usize;
	let csum_size = align(rpc_size, core::mem::size_of::<u64>());
	let cmd_size_hdr = size + fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::str_size();
	let cmd_size = align(cmd_size_hdr, core::mem::size_of::<u64>());
	let cmd_align_size = align(cmd_size, Self::get_gsp_msg_min_size());
	let mut data = KVec::with_capacity(cmd_align_size as usize, GFP_KERNEL)?;

	unsafe {
	    core::ptr::write_bytes(data.as_mut_ptr() as *mut u8, 0, cmd_size);
	    data.set_len(cmd_size);
	}

	let mut msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(data.as_mut_ptr())
	    .checkSum(csum_size as u32);

	let _rpc_hdr = msg.new_S_rpc()
	    .header_version(0x03000000)
	    .signature(('C' as u32) << 24 | ('P' as u32) << 16 | ('R' as u32) << 8 | 'V' as u32)
	    .function(rpc_fn)
	    .rpc_result(0xffffffff)
	    .rpc_result_private(0xffffffff)
	    .length(rpc_size as u32);

	Ok(Self {
	    data,
	    orig_size: cmd_size as u32,
	    recv_data: Default::default(),
	    has_reply,
	    use_reply_data: false,
	})
    }

    pub(crate) fn get_data_ptr(&mut self) -> *mut u8 {
	if self.use_reply_data == true {
	    self.recv_data[fw::ver::gen::s_rpc_message_header_v03_00::str_size()..].as_mut_ptr()
	} else {
	    self.data[fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::str_size()..].as_mut_ptr()
	}
    }

    pub(crate) fn get_rpc_fn(&mut self) -> u32 {
	self.get_rpc().get_function()
    }

    pub(crate) fn get_orig_size(&mut self) -> u32 {
	let msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(self.data.as_mut_ptr());
	let argc = msg.get_checkSum();
	argc
    }

    pub(crate) fn get_rpc_length(&mut self) -> u32 {
	self.get_rpc().get_length()
    }

    pub(crate) fn set_lengths(&mut self, length: u32) {
        let mut msg = fw::ver::gen::s_GSP_MSG_QUEUE_ELEMENT::new(self.data.as_mut_ptr());
	self.get_rpc().set_length(length);
	msg.set_checkSum(length);
    }

    pub(crate) fn set_recv(&mut self, reply: Option<KVec<u8>>) {
	if self.has_reply == false {
	    return;
	}

	self.recv_data = reply.unwrap();
	self.use_reply_data = true;
    }

    pub(crate) fn dump(&mut self) {
	let rpc = self.get_rpc();
	pr_info!("msg fn:{} len:{:#x} res:{:#x} resp:{:#x}\n",
		 rpc.get_function(), rpc.get_length(),
		 rpc.get_rpc_result(), rpc.get_rpc_result_private());

	pr_info!("{:x?}\n", self.data);
    }
}
