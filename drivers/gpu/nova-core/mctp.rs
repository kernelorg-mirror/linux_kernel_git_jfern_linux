// SPDX-License-Identifier: GPL-2.0

//! MCTP/NVDM protocol support for inter-processor communication.
//!
//! MCTP (Management Component Transport Protocol, DMTF DSP0236) provides a
//! transport-agnostic message exchange format. NVDM (NVIDIA Data Model) is a
//! vendor-defined message layer on top of MCTP.
//!
//! Multiple GPU subsystems (FSP, GSP, SEC2) use MCTP/NVDM framing. This module
//! provides the shared header types and helpers for all of them.
//!
//! Open RM reference:
//!   `arch/nvalloc/common/inc/mctp_format.h`
//!   `arch/nvalloc/common/inc/nvdm_format.h`

// MCTP transport header bit layout (DMTF DSP0236):
//
//  31  30  29:28  27  26:24  23:16  15:8  7:4   3:0
// [SOM|EOM| SEQ  | TO| TAG | SEID | DEID| RSVD| VER]
//
// TAG, TO, and RSVD are always zero in current usage.
bitfield! {
    pub(crate) struct TransportHeader(u32), "MCTP transport header (first DWORD)." {
        31:31 som as bool, "Start of Message flag";
        30:30 eom as bool, "End of Message flag";
        29:28 seq as u8, "Packet sequence number (0-3, wraps modulo 4)";
        23:16 seid as u8, "Source Endpoint ID";
        15:8  deid as u8, "Destination Endpoint ID";
        3:0   version as u8, "Header version (1 for MCTP 1.0)";
    }
}

impl TransportHeader {
    /// MCTP specification version 1.0.
    const VERSION: u8 = 1;

    /// Create a transport header from a raw u32 value (for parsing received messages).
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Create a new MCTP transport header.
    ///
    /// TAG and TO fields are always zero (not needed when messages are
    /// single-packet and do not use request/response tag correlation).
    pub(crate) fn new(som: bool, eom: bool, seid: u8, deid: u8, seq: u8) -> Self {
        Self::default()
            .set_version(Self::VERSION)
            .set_som(som)
            .set_eom(eom)
            .set_seid(seid)
            .set_deid(deid)
            .set_seq(seq)
    }
}

// NVDM over MCTP message header bit layout:
//
//  31:24      23:8        7     6:0
// [NVDM_TYPE | VENDOR_ID | IC | MSG_TYPE]
//
// IC (Instance ID) is always zero in current usage.
bitfield! {
    pub(crate) struct NvdmHeader(u32), "NVDM over MCTP message header (second DWORD)." {
        31:24 nvdm_type as u8, "NVDM message type (subsystem-specific)";
        23:8  vendor_id as u16, "PCI Vendor ID";
        6:0   msg_type as u8, "MCTP message type";
    }
}

/// MCTP message type for vendor-defined PCI messages (DMTF DSP0236).
pub(crate) const MSG_TYPE_VENDOR_PCI: u8 = 0x7e;

/// NVIDIA PCI Vendor ID.
pub(crate) const VENDOR_ID_NV: u16 = 0x10de;

impl NvdmHeader {
    /// Create an NVDM header from a raw u32 value (for parsing received messages).
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Create a new NVDM header for NVIDIA vendor-defined PCI messages.
    ///
    /// Sets MCTP message type to Vendor Defined PCI (0x7E) and vendor ID to
    /// NVIDIA (0x10DE). The IC (Instance ID) field is always zero.
    pub(crate) fn new(nvdm_type: u8) -> Self {
        Self::default()
            .set_msg_type(MSG_TYPE_VENDOR_PCI)
            .set_vendor_id(VENDOR_ID_NV)
            .set_nvdm_type(nvdm_type)
    }
}

/// NVDM message type constants.
///
/// These values come from a shared namespace across all GPU subsystems.
/// Open RM reference: `arch/nvalloc/common/inc/nvdm_format.h`
pub(crate) mod nvdm_type {
    /// Chain of Trust (FSP boot).
    pub(crate) const COT: u8 = 0x14;
    /// FSP/SEC2 command response.
    pub(crate) const FSP_RESPONSE: u8 = 0x15;
}
