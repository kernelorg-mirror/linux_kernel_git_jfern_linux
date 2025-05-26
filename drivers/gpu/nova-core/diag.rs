// SPDX-License-Identifier: GPL-2.0

//! GSP Diagnostic Module - NOCAT Record Parser
//! 
//! This module handles parsing and displaying NOCAT (NVIDIA Operational Critical 
//! Assessment and Triage) records received from the GSP firmware.

use kernel::prelude::*;
use kernel::{pr_info, pr_err, pr_warn};
use core::ffi::c_void;

/// Maximum string length for NOCAT fields
const NOCAT_JOURNAL_MAX_STR_LEN: usize = 65;

/// Maximum diagnostic buffer size
const NOCAT_JOURNAL_MAX_DIAG_BUFFER: usize = 1024;

/// NOCAT record types
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum NocatRecordType {
    Unknown = 0,
    Bugcheck = 1,
    Engine = 2,
    Tdr = 3,
    Rc = 4,
    Assert = 5,
    Any = 6,
}

impl From<u8> for NocatRecordType {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::Bugcheck,
            2 => Self::Engine,
            3 => Self::Tdr,
            4 => Self::Rc,
            5 => Self::Assert,
            6 => Self::Any,
            _ => Self::Unknown,
        }
    }
}

/// NOCAT Journal Insert Record structure
/// This matches the NV2080CtrlNocatJournalInsertRecord structure from resman
#[repr(C)]
pub struct NocatJournalInsertRecord {
    pub flags: u32,
    pub timestamp: u64,
    pub rec_type: u8,
    pub bugcheck: u32,
    pub source: [u8; NOCAT_JOURNAL_MAX_STR_LEN],
    pub subsystem: u32,
    pub error_code: u64,
    pub faulting_engine: [u8; NOCAT_JOURNAL_MAX_STR_LEN],
    pub tdr_reason: u32,
    pub diag_buffer_len: u32,
    pub diag_buffer: [u8; NOCAT_JOURNAL_MAX_DIAG_BUFFER],
}

/// RPC structure for GSP_POST_NOCAT_RECORD event
#[repr(C)]
pub struct RpcGspPostNocatRecord {
    pub data: u32,  // This is actually the start of the NOCAT record data
}

impl NocatJournalInsertRecord {
    /// Parse a NOCAT record from raw RPC data
    pub unsafe fn from_rpc_data(data: *const c_void) -> Result<&'static Self> {
        if data.is_null() {
            return Err(EINVAL);
        }
        
        // The NOCAT record starts at the data field
        let record = &*(data as *const Self);
        Ok(record)
    }
    
    /// Convert source field to string
    fn source_as_str(&self) -> Result<&str> {
        // Find null terminator or use full length
        let len = self.source.iter()
            .position(|&c| c == 0)
            .unwrap_or(NOCAT_JOURNAL_MAX_STR_LEN);
        
        core::str::from_utf8(&self.source[..len])
            .map_err(|_| EINVAL)
    }
    
    /// Convert faulting engine field to string
    fn faulting_engine_as_str(&self) -> Result<&str> {
        // Find null terminator or use full length
        let len = self.faulting_engine.iter()
            .position(|&c| c == 0)
            .unwrap_or(NOCAT_JOURNAL_MAX_STR_LEN);
        
        core::str::from_utf8(&self.faulting_engine[..len])
            .map_err(|_| EINVAL)
    }
    
    /// Dump the NOCAT record to kernel log
    pub fn dump(&self) {
        pr_info!("=== GSP NOCAT Record ===\n");
        pr_info!("Timestamp: {:#x}\n", self.timestamp);
        pr_info!("Record Type: {:?} ({})\n", 
            NocatRecordType::from(self.rec_type), self.rec_type);
        pr_info!("Flags: {:#x}\n", self.flags);
        
        if self.bugcheck != 0 {
            pr_err!("Bugcheck Code: {:#x}\n", self.bugcheck);
        }
        
        if let Ok(source) = self.source_as_str() {
            if !source.is_empty() {
                pr_info!("Source: {}\n", source);
            }
        }
        
        pr_info!("Subsystem: {:#x}\n", self.subsystem);
        pr_info!("Error Code: {:#x}\n", self.error_code);
        
        if let Ok(engine) = self.faulting_engine_as_str() {
            if !engine.is_empty() {
                pr_err!("Faulting Engine: {}\n", engine);
            }
        }
        
        if self.tdr_reason != 0 {
            pr_info!("TDR Reason: {:#x}\n", self.tdr_reason);
        }
        
        // Dump diagnostic buffer if present
        if self.diag_buffer_len > 0 && self.diag_buffer_len <= NOCAT_JOURNAL_MAX_DIAG_BUFFER as u32 {
            pr_info!("Diagnostic Buffer ({} bytes):\n", self.diag_buffer_len);
            self.dump_hex_buffer();
        }
        
        pr_info!("=== End NOCAT Record ===\n");
    }
    
    /// Dump diagnostic buffer in hex format
    fn dump_hex_buffer(&self) {
        let len = core::cmp::min(self.diag_buffer_len as usize, NOCAT_JOURNAL_MAX_DIAG_BUFFER);
        let mut offset = 0;
        
        while offset < len {
            let mut line = alloc::string::String::new();
            let _ = write!(line, "{:04x}: ", offset);
            
            // Print hex bytes
            for i in 0..16 {
                if offset + i < len {
                    let _ = write!(line, "{:02x} ", self.diag_buffer[offset + i]);
                } else {
                    let _ = write!(line, "   ");
                }
                if i == 7 {
                    let _ = write!(line, " ");
                }
            }
            
            let _ = write!(line, " |");
            
            // Print ASCII representation
            for i in 0..16 {
                if offset + i < len {
                    let byte = self.diag_buffer[offset + i];
                    if byte >= 0x20 && byte <= 0x7e {
                        let _ = write!(line, "{}", byte as char);
                    } else {
                        let _ = write!(line, ".");
                    }
                }
            }
            let _ = write!(line, "|");
            
            pr_info!("{}\n", line);
            offset += 16;
        }
    }
}

/// Parse and display a NOCAT record from GSP RPC event data
pub fn parse_nocat_record(data: *const c_void) -> Result<()> {
    unsafe {
        match NocatJournalInsertRecord::from_rpc_data(data) {
            Ok(record) => {
                pr_warn!("GSP firmware reported NOCAT error record:\n");
                record.dump();
                
                // Additional analysis based on record type
                match NocatRecordType::from(record.rec_type) {
                    NocatRecordType::Bugcheck => {
                        pr_err!("GSP firmware bugcheck detected! Code: {:#x}\n", record.bugcheck);
                    }
                    NocatRecordType::Tdr => {
                        pr_err!("GSP firmware TDR (Timeout Detection and Recovery) event!\n");
                    }
                    NocatRecordType::Assert => {
                        pr_err!("GSP firmware assertion failure!\n");
                    }
                    _ => {}
                }
                
                Ok(())
            }
            Err(e) => {
                pr_err!("Failed to parse NOCAT record: {:?}\n", e);
                Err(e)
            }
        }
    }
} 