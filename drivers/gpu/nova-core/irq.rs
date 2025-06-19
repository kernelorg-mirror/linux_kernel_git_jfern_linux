// SPDX-License-Identifier: GPL-2.0

//! Nova Core IRQ (Interrupt) Module

use kernel::{
    prelude::*,
    error::Result,
};

use core::mem::size_of;

use crate::gsp::{GspCmdq, ControlResponse, GspMessageElement};
use crate::falcon::{gsp::Gsp, Falcon};
use crate::nvfw::r570_144 as fw;

/// Control command, need to get from fw.
pub const NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE: u32 = 0x20800a5c;

/// Hardcoded GSP internal handle values for testing on GA102 from Nouveau
/// (should be getting from an RM call)
pub const GSP_INTERNAL_CLIENT_HANDLE: u32 = 0xc2000005;
pub const GSP_INTERNAL_DEVICE_HANDLE: u32 = 0xabcd0080;
pub const GSP_INTERNAL_SUBDEVICE_HANDLE: u32 = 0xabcd2080;

/// Interrupt table entry
#[derive(Debug, Clone)]
pub struct InterruptTableEntry {
    pub engine_idx: u16,
    pub pmc_intr_mask: u32,
    pub vector_stall: u32,
    pub vector_nonstall: u32,
}

/// Control structure for GSP RM control commands
#[repr(C)]
#[derive(Clone, Copy)]
struct RpcGspRmControl {
    hClient: u32,
    hObject: u32,
    cmd: u32,
    status: u32,
    paramsSize: u32,
    flags: u32,
}

/// Individual interrupt table entry from GSP
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct IntrTableEntry {
    engineIdx: u16,
    pmcIntrMask: u32,
    vectorStall: u32,
    vectorNonStall: u32,
}

/// Subtree map for interrupt categories
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct IntrCategorySubtreeMap {
    subtreeStart: u8,
    subtreeEnd: u8,
}

const NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE: usize = 128;
const NV2080_INTR_CATEGORY_ENUM_COUNT: usize = 7;

/// Parameters structure for interrupt table query - matching nouveau's size
#[repr(C)]
#[derive(Clone)]
struct NV2080CtrlInternalIntrGetKernelTableParams {
    tableLen: u32,
    table: [IntrTableEntry; NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE], // 128 entries like nouveau
    subtreeMap: [IntrCategorySubtreeMap; NV2080_INTR_CATEGORY_ENUM_COUNT], // Missing field added
}

impl Default for NV2080CtrlInternalIntrGetKernelTableParams {
    fn default() -> Self {
        Self {
            tableLen: 0,
            table: [IntrTableEntry::default(); NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE],
            subtreeMap: [IntrCategorySubtreeMap::default(); NV2080_INTR_CATEGORY_ENUM_COUNT],
        }
    }
}

/// Control message contains control header and params
#[repr(C)]
struct ControlMessage {
    control: RpcGspRmControl,
    params: NV2080CtrlInternalIntrGetKernelTableParams,
}

impl GspMessageElement for ControlMessage {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    
    fn size(&self) -> usize {
        size_of::<RpcGspRmControl>() + size_of::<NV2080CtrlInternalIntrGetKernelTableParams>()
    }
    
    fn byte_slice(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                self.size(),
            )
        }
    }
}

/// IRQ module implementation
pub struct Irq;

impl Irq {
    /// Send a control command to GSP and get response
    pub fn rm_control(
        _gsp_falcon: &Falcon<Gsp>,
        cmdq: &mut GspCmdq<'_>,
        cmd: u32,
        h_client: u32,
        h_object: u32,
        params: &mut NV2080CtrlInternalIntrGetKernelTableParams,
    ) -> Result<()> {
        pr_info!("Nova IRQ: rm_control called with cmd {:#x}", cmd);
        pr_info!("Nova IRQ: Building control message with:");
        pr_info!("  hClient: {:#x}", h_client);
        pr_info!("  hObject: {:#x}", h_object);
        pr_info!("  cmd: {:#x}", cmd);
        pr_info!("  paramsSize: {} bytes", size_of::<NV2080CtrlInternalIntrGetKernelTableParams>());
        
        // Allocate control message on heap to avoid stack overflow
        let msg = kernel::alloc::KBox::new(ControlMessage {
            control: RpcGspRmControl {
                hClient: h_client,
                hObject: h_object,
                cmd,
                status: 0,
                paramsSize: size_of::<NV2080CtrlInternalIntrGetKernelTableParams>() as u32,
                flags: 0,
            },
            params: params.clone(),
        }, kernel::alloc::flags::GFP_KERNEL)?;
        
        pr_info!("Nova IRQ: Sending control message, size = {}", msg.size());
        
        // Send RM control RPC - KBox implements Deref, so we can use it directly
        match cmdq.send(fw::NV_VGPU_MSG_FUNCTION_GSP_RM_CONTROL, &*msg) {
            Ok(_) => {
                pr_info!("Nova IRQ: rm_control - send completed, waiting for response");
                
                // Receive response from GSP
                let (response_fn, response_data) = cmdq.receive()?;
                pr_info!("Nova IRQ: Received response for function {:#x}", response_fn);
                
                // response should contain control message response
                if response_fn == fw::NV_VGPU_MSG_FUNCTION_GSP_RM_CONTROL {
                    pr_info!("Nova IRQ: Got control response, parsing interrupt table");
                    
                    // Cast response data to ControlResponse
                    let control_response = response_data.as_any().downcast_ref::<ControlResponse>()
                        .ok_or(EINVAL)?;
                    
                    // response contains RpcGspRmControl header followed by params
                    // Skip the RpcGspRmControl header (24 bytes)
                    let params_offset = size_of::<RpcGspRmControl>();
                    
                    // Check status field from RpcGspRmControl header
                    let status_bytes = &control_response.data[12..16];
                    let status = u32::from_le_bytes([
                        status_bytes[0],
                        status_bytes[1],
                        status_bytes[2],
                        status_bytes[3],
                    ]);
                    pr_info!("Nova IRQ: RM Control status: {:#x}", status);
                    
                    // Parse the params from the response
                    unsafe {
                        // Copy the response params back to our params structure
                        core::ptr::copy_nonoverlapping(
                            control_response.data[params_offset..].as_ptr(),
                            params as *mut NV2080CtrlInternalIntrGetKernelTableParams as *mut u8,
                            size_of::<NV2080CtrlInternalIntrGetKernelTableParams>(),
                        );
                    }
                    
                    pr_info!("Nova IRQ: Parsed interrupt table with {} entries", params.tableLen);
                    
                    // If we got 0x1f, let's check if tableLen was populated anyway
                    if status == 0x1f && params.tableLen == 0 {
                        pr_info!("Nova IRQ: Got 0x1f with tableLen=0, params size was {} bytes", size_of::<NV2080CtrlInternalIntrGetKernelTableParams>());
                        pr_info!("Nova IRQ: Nouveau uses 2068 bytes, we need to match that size");
                    }
                } else {
                    pr_err!("Nova IRQ: Unexpected response function: {:#x}", response_fn);
                    return Err(EINVAL);
                }
                
                Ok(())
            },
            Err(e) => {
                pr_err!("Nova IRQ: rm_control - send failed: {:?}", e);
                Err(e)
            },
        }
    }
    
    /// Get interrupt table from GSP
    /// 
    /// Current status: We're getting 0x57 (NV_ERR_OBJECT_NOT_FOUND) from GSP.
    /// This means the hardcoded handles from nouveau are not valid in nova-core's context.
    /// 
    /// Progress made:
    /// 1. Fixed structure size to match nouveau (128 entries + subtreeMap)
    /// 2. Fixed stack overflow by allocating large structures on heap
    /// 3. Now sending correct RPC size (2124 bytes) matching nouveau
    /// 4. Implemented GET_GSP_STATIC_INFO RPC infrastructure
    /// 
    /// GET_GSP_STATIC_INFO findings:
    /// - RPC returns error 0xff100002 (indicates missing prerequisite)
    /// - Response is only 32 bytes (RPC header only, no payload)
    /// - This error means GSP internal client objects must be created first
    /// 
    /// Next steps to get valid handles:
    /// 1. Create GSP internal client (NV_VGPU_MSG_FUNCTION_GSP_RM_ALLOC with specific params)
    /// 2. Create internal device object under the client
    /// 3. Create internal subdevice object under the device
    /// 4. Then GET_GSP_STATIC_INFO should return the allocated handles
    /// 5. Use those handles instead of hardcoded values for interrupt table query
    /// 
    /// The hardcoded handles work in nouveau because nouveau creates these objects
    /// during GSP initialization. Nova-core needs to do the same.
    /// Next steps required:
    /// 1. Implement GSP internal client creation during GSP init (equivalent to nouveau's
    ///    r535_gsp_client_ctor for internal client)
    /// 2. Create internal device object using NV01_DEVICE_0 class
    /// 3. Create internal subdevice object using GF100_SUBDEVICE_MASTER class
    /// 4. Store these object handles in a structure accessible during runtime
    /// 5. Use the actual handles instead of hardcoded values in rm_control()
    /// 6. Consider implementing a proper RM API wrapper for object allocation/management

    pub fn get_interrupt_table<'a>(gsp_falcon: &Falcon<Gsp>, cmdq: &mut GspCmdq<'a>) -> Result<kernel::alloc::Vec<InterruptTableEntry, kernel::alloc::allocator::Kmalloc>> {
        pr_info!("Nova IRQ: Retrieving interrupt table from GSP...\n");
        
        // Step 1: Get GSP static info to retrieve internal handles
        // No prints here to avoid stack overflow when called from deep stack
        // let _static_info_result = cmdq.get_gsp_static_info();
        
        // Allocate params structure on heap to avoid stack overflow
        let mut params = kernel::alloc::KBox::new(NV2080CtrlInternalIntrGetKernelTableParams::default(), kernel::alloc::flags::GFP_KERNEL)?;
        
        // Send control command using hardcoded handles (for now)
        Self::rm_control(
            gsp_falcon,
            cmdq,
            NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE,
            GSP_INTERNAL_CLIENT_HANDLE,
            GSP_INTERNAL_SUBDEVICE_HANDLE,
            &mut *params
        )?;
        
        // Convert to Rust vector
        let mut entries = kernel::alloc::Vec::new();
        
        pr_info!("Nova IRQ: GA102 Interrupt Table (Retrieved from GSP)\n");
        pr_info!("Nova IRQ: | Entry | Engine Index | Engine Type | Stall Vector | Non-Stall Vector |\n");
        pr_info!("Nova IRQ: |-------|-------------|-------------|--------------|------------------|\n");
        
        // Process the returned table
        for i in 0..params.tableLen as usize {
            if i >= params.table.len() {
                break;
            }
            
            let entry = &params.table[i];
            let irq_entry = InterruptTableEntry {
                engine_idx: entry.engineIdx,
                pmc_intr_mask: entry.pmcIntrMask,
                vector_stall: entry.vectorStall,
                vector_nonstall: entry.vectorNonStall,
            };
            entries.push(irq_entry.clone(), kernel::alloc::flags::GFP_KERNEL)?;
            
            // Map engine indices to names based on the report
            let engine_name = match entry.engineIdx {
                2 => "CE0",
                49 => "GSP",
                58 => "Unknown",
                59 => "Unknown", 
                61 => "Unknown",
                72 => "DISP",
                73 => "Unknown",
                82 => "GR0",
                _ => "UNKNOWN",
            };
            
            // Print the table entry
            pr_info!("Nova IRQ: | {:5} | {:11} | {:11} | {:#010x}   | {:#010x}       |",
                i, entry.engineIdx, engine_name, entry.vectorStall, entry.vectorNonStall);
        }
        
        pr_info!("Nova IRQ: Retrieved {} interrupt entries", entries.len());
        
        Ok(entries)
    }
}
