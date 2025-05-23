// SPDX-License-Identifier: GPL-2.0

//! GSP Sequencer implementation for Pre-hopper GSP boot sequence.

use core::{mem::size_of, time::Duration};
use kernel::delay::sleep;
use kernel::device;
use kernel::time::Delta;
use kernel::prelude::*;
use kernel::alloc::flags::GFP_KERNEL;

use crate::driver::Bar0;
use crate::falcon::{gsp::Gsp, sec2::Sec2, Falcon};
use crate::firmware::Firmware;
use crate::gsp::GspMessageElement;
use crate::nvfw::r570_144 as fw;
use crate::util::wait_on;

use kernel::{dev_dbg, dev_err};

impl_from_bytes!(fw::GSP_SEQUENCER_BUFFER_CMD);
const CMD_SIZE: usize = size_of::<fw::GSP_SEQUENCER_BUFFER_CMD>();

pub(crate) struct GspSequencerInfo {
    pub info: fw::rpc_run_cpu_sequencer_v17_00,
    pub cmd_data: KVec<u8>,
}

impl GspMessageElement for GspSequencerInfo {
    fn new_from_slices(slice_1: &[u8], slice_2: Option<&[u8]>) -> Result<Self> {
        // First, extract the info field from the beginning of the data
        let info_size = size_of::<fw::rpc_run_cpu_sequencer_v17_00>();

        // Check if we have enough data for the info field
        let total_available = slice_1.len() + slice_2.map_or(0, |s| s.len());
        if total_available < info_size {
            return Err(EINVAL);
        }

        let info = fw::rpc_run_cpu_sequencer_v17_00::new_from_slices(slice_1, slice_2)?;

        if slice_1.len() <= info_size {
            return Err(EINVAL);
        }

        let mut data_len = slice_1.len() - info_size;
        if let Some(slice) = slice_2 {
            data_len += slice.len();
        }

        let mut cmd_data = KVec::with_capacity(data_len, GFP_KERNEL)?;
        cmd_data.extend_from_slice(&slice_1[info_size..], GFP_KERNEL)?;

        if let Some(slice) = slice_2 {
            cmd_data.extend_from_slice(slice, GFP_KERNEL)?;
        }

        Ok(GspSequencerInfo { info, cmd_data })
    }
}

impl GspSeqCmd {
    /// Creates a new GspSeqCmd from a firmware GSP_SEQUENCER_BUFFER_CMD
    pub(crate) fn from_fw_cmd(cmd: &fw::GSP_SEQUENCER_BUFFER_CMD) -> Result<Self> {
        match cmd.opCode {
            _ => Err(EINVAL),
        }
    }

    pub(crate) fn new(data: &[u8], dev: &device::Device<device::Bound>) -> Result<Self> {
        let fw_cmd = fw::GSP_SEQUENCER_BUFFER_CMD::from_bytes(data)?;
        let cmd = Self::from_fw_cmd(&fw_cmd)?;

        if data.len() < cmd.size_bytes() {
            dev_err!(dev, "data is not enough for command.\n");
            return Err(EINVAL);
        }

        Ok(cmd)
    }

    /// Get the size of this command in bytes, the command consists of
    /// a 4-byte opcode, and a variable-sized payload.
    pub(crate) fn size_bytes(&self) -> usize {
        0
    }
}

pub(crate) struct GspSequencer<'a> {
    pub seq_info: GspSequencerInfo,
    pub bar: &'a Bar0,
    pub sec2_falcon: &'a Falcon<Sec2>,
    pub gsp_falcon: &'a Falcon<Gsp>,
    pub libos_dma_handle: u64,
    pub fw: &'a Firmware,
    pub dev: &'a device::Device<device::Bound>,
}

pub(crate) trait GspSeqCmdRunner {
    fn run(&self, sequencer: &GspSequencer<'_>) ->  Result;
}

impl GspSeqCmdRunner for GspSeqCmd {
    fn run(&self, seq: &GspSequencer<'_>) ->  Result {
        Ok(())
    }
}

pub(crate) struct GspSeqIter<'a> {
    cmd_data: &'a [u8],
    current_offset: usize, // Tracking the current position
    total_cmds: u32,
    cmds_processed: u32,
    dev: &'a device::Device<device::Bound>,
}

impl<'a> Iterator for GspSeqIter<'a> {
    type Item = Result<GspSeqCmd>;

    fn next(&mut self) -> Option<Self::Item> {
        // Stop if we've processed all commands or reached the end of data
        if self.cmds_processed >= self.total_cmds || self.current_offset >= self.cmd_data.len() {
            return None;
        }

        // Check if we have enough data for opcode
        let opcode_size = size_of::<fw::GSP_SEQ_BUF_OPCODE>();
        if self.current_offset + opcode_size > self.cmd_data.len() {
            return Some(Err(EINVAL));
        }

        let offset = self.current_offset;

        // Handle command creation based on available data,
        // zero-pad if necessary (since last command may not be full size).
        let mut buffer = [0u8; CMD_SIZE];
        let copy_len = if offset + CMD_SIZE <= self.cmd_data.len() {
            CMD_SIZE
        } else {
            self.cmd_data.len() - offset
        };
        buffer[..copy_len].copy_from_slice(&self.cmd_data[offset..offset + copy_len]);
        let cmd_result = GspSeqCmd::new(&buffer, self.dev);

        cmd_result.map_or_else(
            |_err| {
                dev_err!(self.dev, "Error parsing command at offset {}", offset);
                None
            },
            |cmd| {
                self.current_offset += cmd.size_bytes();
                self.cmds_processed += 1;
                Some(Ok(cmd))
            },
        )
    }
}

impl<'a, 'b> IntoIterator for &'b GspSequencer<'a> {
    type Item = Result<GspSeqCmd>;
    type IntoIter = GspSeqIter<'b>;

    fn into_iter(self) -> Self::IntoIter {
        let cmd_data = &self.seq_info.cmd_data[..];

        GspSeqIter {
            cmd_data,
            current_offset: 0,
            total_cmds: self.seq_info.info.cmdIndex,
            cmds_processed: 0,
            dev: self.dev,
        }
    }
}

impl<'a> GspSequencer<'a> {
    pub(crate) fn new(
        cmdq: &mut crate::gsp::GspCmdq<'a>,
        fw: &'a Firmware,
        libos_dma_handle: u64,
        gsp_falcon: &'a Falcon<Gsp>,
        sec2_falcon: &'a Falcon<Sec2>,
        dev: &'a device::Device<device::Bound>,
        bar: &'a Bar0,
        timeout: Delta,
    ) -> Result<Self> {
        // Receive the sequencer info from the GSP command queue
        let seq_info = crate::util::wait_on_result(timeout,
            || match cmdq.receive(fw::NV_VGPU_MSG_EVENT_GSP_RUN_CPU_SEQUENCER) {
                Ok(seq_info) => Some(Ok(seq_info)),
                Err(EAGAIN) => None,
                Err(e) => Some(Err(e)),
            })?;

        Ok(GspSequencer {
            seq_info,
            bar,
            sec2_falcon,
            gsp_falcon,
            libos_dma_handle,
            fw,
            dev,
        })
    }

    pub(crate) fn run(&self) ->  Result {
        dev_dbg!(self.dev, "Running CPU Sequencer commands\n");

        for cmd_result in self {
            match cmd_result {
                Ok(cmd) => cmd.run(self)?,
                Err(e) => {
                    dev_err!(
                        self.dev,
                        "Error running command at index {}\n",
                        self.seq_info.info.cmdIndex
                    );
                    return Err(e);
                }
            }
        }

        dev_dbg!(self.dev, "CPU Sequencer commands completed successfully\n");

        Ok(())
    }
}
