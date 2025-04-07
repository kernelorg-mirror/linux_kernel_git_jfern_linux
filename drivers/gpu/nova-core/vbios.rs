// SPDX-License-Identifier: GPL-2.0

//! VBIOS extraction and parsing.

use crate::{driver::Bar0, falcon::FalconUCodeDescV3};
use core::convert::TryFrom;
use kernel::devres::Devres;
use kernel::error::Result;
use kernel::prelude::*;

/// The offset of the VBIOS ROM in the BAR0 space
const ROM_OFFSET: usize = 0x300000;

// PMU lookup table entry types. Used to locate the PMU table entry
// in the Fwsec image, corresponding to falcon ucodes.
#[allow(dead_code)]
const FALCON_UCODE_ENTRY_APPID_FIRMWARE_SEC_LIC: u8 = 0x05;
#[allow(dead_code)]
const FALCON_UCODE_ENTRY_APPID_FWSEC_DBG: u8 = 0x45;
const FALCON_UCODE_ENTRY_APPID_FWSEC_PROD: u8 = 0x85;

/// VBIOS data structure
pub(crate) struct Vbios {
    pub fwsec_image: Option<FwSecBiosImage>,
    /// VBIOS data
    #[allow(dead_code)]
    data: KVec<u8>,
}

impl Vbios {
    /// Read bytes from the ROM at the current end of the data vector
    fn read_more(bar0: &Devres<Bar0>, data: &mut KVec<u8>, len: usize) -> Result {
        with_bar_res!(bar0, |bar0_ref| {
            // Get current length
            let current_len = data.len();

            // Read ROM data bytes push directly to vector
            for i in 0..len {
                // Read a byte from the VBIOS ROM and push it to the data vector
                let rom_addr = ROM_OFFSET + current_len + i;
                let byte = bar0_ref.try_readb(rom_addr)?;
                data.push(byte, GFP_KERNEL)?;
            }
            Ok(())
        })?;

        Ok(())
    }

    /// Read bytes at a specific offset, filling any gap
    fn read_more_at_offset(
        bar0: &Devres<Bar0>,
        data: &mut KVec<u8>,
        offset: usize,
        len: usize,
    ) -> Result {
        // If offset is beyond current data size, fill the gap first
        let current_len = data.len();

        if offset > current_len {
            // Calculate bytes to read to fill the gap
            let gap_bytes = offset - current_len;
            Self::read_more(bar0, data, gap_bytes)?;
        }

        // Now read the requested bytes at the offset
        Self::read_more(bar0, data, len)
    }

    fn read_bios_image_at_offset(
        bar0: &Devres<Bar0>,
        data: &mut KVec<u8>,
        offset: usize,
        len: usize,
    ) -> Result<BiosImage> {
        if offset + len > data.len() {
            Self::read_more_at_offset(bar0, data, offset, len).inspect_err(|e| {
                pr_info!("Failed to read more at offset {:#x}: {:?}\n", offset, e)
            })?;
        }

        BiosImage::try_from(&data[offset..offset + len]).inspect_err(|e| {
            pr_info!(
                "Failed to create BiosImage at offset {:#x}: {:?}\n",
                offset,
                e
            )
        })
    }

    /// Probe for VBIOS extraction
    /// Once the VBIOS object is built, bar0 is not read for vbios purposes anymore.
    pub(crate) fn probe(bar0: &Devres<Bar0>) -> Result<Self> {
        let mut data = KVec::new();
        // Read the first 32 bytes into the KVec
        Self::read_more(bar0, &mut data, 32)?;

        // Print the bytes from the KVec to verify they match
        pr_info!("ROM header bytes from KVec (first 32 bytes):\n");
        for row in 0..2 {
            let base = row * 16;
            if base + 15 < data.len() {
                pr_info!("  {:08x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}\n",
                    base,
                    data[base],     data[base + 1], data[base + 2], data[base + 3],
                    data[base + 4], data[base + 5], data[base + 6], data[base + 7],
                    data[base + 8], data[base + 9], data[base + 10], data[base + 11],
                    data[base + 12], data[base + 13], data[base + 14], data[base + 15]
                );
            }
        }

        // Loop through all the BiosImage and extract relevant ones and relevant data from them
        let mut cur_offset = 0;
        let mut pci_at_image: Option<PciAtBiosImage> = None;
        let mut first_fwsec_image: Option<FwSecBiosImage> = None;
        let mut second_fwsec_image: Option<FwSecBiosImage> = None;

        // loop till break
        loop {
            // Try to parse a BIOS image at the current offset
            // This will now check for all valid ROM signatures (0xAA55, 0xBB77, 0x4E56)
            let image_size = Self::read_bios_image_at_offset(bar0, &mut data, cur_offset, 1024)
                .and_then(|image| {
                    image.image_size_bytes().inspect_err(|_| {
                        pr_info!(
                            "Invalid get image size at offset {:#x}, stopping scan\n",
                            cur_offset
                        );
                    })
                })
                .inspect_err(|e| {
                    pr_info!(
                        "Failed to parse BIOS image at offset {:#x}: {:?}\n",
                        cur_offset,
                        e
                    );
                })?;

            // Create a new BiosImage with the full image data
            let full_image =
                Self::read_bios_image_at_offset(bar0, &mut data, cur_offset, image_size)
                    .inspect_err(|e| {
                        pr_info!(
                            "Failed to parse full BIOS image at offset {:#x}: {:?}\n",
                            cur_offset,
                            e
                        );
                    })?;

            // Determine the image type
            let image_type = full_image.image_type_str();

            pr_info!(
                "Found BIOS image at offset {:#x}, size: {:#x}, type: {}\n",
                cur_offset,
                image_size,
                image_type
            );

            // Get a reference to the image before we potentially use full_image in all_images
            let is_last = full_image.is_last();

            match full_image {
                BiosImage::PciAt(image) => {
                    pci_at_image = Some(image);
                }
                BiosImage::FwSec(image) => {
                    if first_fwsec_image.is_none() {
                        first_fwsec_image = Some(image);
                    } else {
                        second_fwsec_image = Some(image);
                    }
                }
                // For now we don't need to handle these
                BiosImage::Efi(_image) => {}
                BiosImage::Nbsi(_image) => {}
            }

            // Break if this is the last image
            if is_last {
                pr_info!("Last image found, stopping scan\n");
                break;
            }

            // Move to the next image (aligned to 512 bytes)
            cur_offset += image_size;
            cur_offset = (cur_offset + 511) & !511;

            // Safety check - don't go beyond 1MB
            if cur_offset > 0x100000 {
                pr_info!("Exceeded 1MB limit, stopping BIOS scan\n");
                break;
            }
        }

        // Using all the images, setup the falcon data pointer in Fwsec.
        // We need mutable access here, so we handle the Option manually.
        let final_fwsec_image = {
            let mut second = second_fwsec_image; // Take ownership of the option
            let first_ref = first_fwsec_image.as_ref();
            let pci_at_ref = pci_at_image.as_ref();

            if let (Some(second), Some(first), Some(pci_at)) =
                (second.as_mut(), first_ref, pci_at_ref)
            {
                second
                    .setup_falcon_data(pci_at, first)
                    .inspect_err(|e| pr_info!("Falcon data setup failed: {:?}\n", e))?;
            } else {
                pr_info!("Missing required images for falcon data setup, skipping\n");
            }
            second // Return the potentially modified second image
        };

        Ok(Self {
            fwsec_image: final_fwsec_image,
            data,
        })
    }

    pub(crate) fn fwsec_header(&self) -> Result<&FalconUCodeDescV3> {
        let image = self.fwsec_image.as_ref().ok_or(EINVAL)?;
        image.fwsec_header()
    }

    pub(crate) fn fwsec_ucode(&self) -> Result<&[u8]> {
        let image = self.fwsec_image.as_ref().ok_or(EINVAL)?;
        image.fwsec_ucode(image.fwsec_header()?)
    }
}

/// PCI Data Structure as defined in PCI Firmware Specification
/*
struct PCI_DATA_STRUCT
{
    u32       sig;                //  00h: Signature, the string "PCIR" or NVIDIA's alternate "NPDS"
    u16       vendorID;           //  04h: Vendor Identification
    u16       deviceID;           //  06h: Device Identification
    u16       deviceListPtr;      //  08h: Device List Pointer
    u16       pciDataStructLen;   //  0Ah: PCI Data Structure Length
    u8        pciDataStructRev;   //  0Ch: PCI Data Structure Revision
    u8        classCode[3];       //  0Dh: Class Code
    u16       imageLen;           //  10h: Image Length (units of 512 bytes)
    u16       vendorRomRev;       //  12h: Revision Level of the Vendor's ROM
    u8        codeType;           //  14h: holds NBSI_OBJ_CODE_TYPE (0x70) and others
    u8        lastImage;          //  15h: Last Image Indicator: bit7=1 is lastImage
    u16       maxRunTimeImageLen; //  16h: Maximum Run-time Image Length (units of 512 bytes)
}

and here is NPDE (Nvidia PCI Data Extension to the PCI Data Structure):
struct NV_PCI_DATA_EXT_STRUCT
{
    u32   signature;          //  00h: Signature, the string "NPDE"
    u16   nvPciDataExtRev;    //  04h: NVIDIA PCI Data Extension Revision
    u16   nvPciDataExtLen;    //  06h: NVIDIA PCI Data Extension Length
    u16   subimageLen;        //  08h: Sub-image Length
}
*/
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PcirStruct {
    /// PCI Data Structure signature ("PCIR" or "NPDS")
    pub signature: [u8; 4],
    /// PCI Vendor ID (e.g., 0x10DE for NVIDIA)
    pub vendor_id: u16,
    /// PCI Device ID
    pub device_id: u16,
    /// Device List Pointer
    pub device_list_ptr: u16,
    /// PCI Data Structure Length
    pub pci_data_struct_len: u16,
    /// PCI Data Structure Revision
    pub pci_data_struct_rev: u8,
    /// Class code (3 bytes, 0x03 for display controller)
    pub class_code: [u8; 3],
    /// Size of this image in 512-byte blocks
    pub image_len: u16,
    /// Revision Level of the Vendor's ROM
    pub vendor_rom_rev: u16,
    /// ROM image type (0x00 = PC-AT compatible, 0x03 = EFI, 0x70 = NBSI)
    pub code_type: u8,
    /// Last image indicator (0x00 = Not last image, 0x80 = Last image)
    pub last_image: u8,
    /// Maximum Run-time Image Length (units of 512 bytes)
    pub max_runtime_image_len: u16,
}

impl TryFrom<&[u8]> for PcirStruct {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 24 {
            // Updated to match full PCI_DATA_STRUCT size
            pr_info!("Not enough data for PcirStruct\n");
            return Err(EINVAL);
        }

        let mut signature = [0u8; 4];
        signature.copy_from_slice(&data[0..4]);

        // Signature should be "PCIR" (0x52494350) or "NPDS" (0x5344504e)
        if &signature != b"PCIR" && &signature != b"NPDS" {
            pr_info!("Invalid signature for PcirStruct: {:?}\n", signature);
            return Err(EINVAL);
        }

        let mut class_code = [0u8; 3];
        class_code.copy_from_slice(&data[13..16]);

        Ok(PcirStruct {
            signature,
            vendor_id: u16::from_le_bytes([data[4], data[5]]),
            device_id: u16::from_le_bytes([data[6], data[7]]),
            device_list_ptr: u16::from_le_bytes([data[8], data[9]]),
            pci_data_struct_len: u16::from_le_bytes([data[10], data[11]]),
            pci_data_struct_rev: data[12],
            class_code,
            image_len: u16::from_le_bytes([data[16], data[17]]),
            vendor_rom_rev: u16::from_le_bytes([data[18], data[19]]),
            code_type: data[20],
            last_image: data[21],
            max_runtime_image_len: u16::from_le_bytes([data[22], data[23]]),
        })
    }
}

impl PcirStruct {
    /// Check if this is the last image in the ROM
    fn is_last(&self) -> bool {
        self.last_image & 0x80 != 0
    }

    /// Calculate image size in bytes
    fn image_size_bytes(&self) -> Result<usize> {
        if self.image_len > 0 {
            // Image size is in 512-byte blocks
            Ok(self.image_len as usize * 512)
        } else {
            Err(EINVAL)
        }
    }
}

/*
struct BIT_HEADER
{
    u16 Id;            // BMP=0x7FFF/BIT=0xB8FF
    u32 Signature;     // 0x00544942 - BIT Data Structure Signature
    u16 BCD_Version;   // BIT Version - 0x0100 for 1.00
    u8 HeaderSize;    // This version is 12 bytes long
    u8 TokenSize;     // This version has 6 byte long Tokens
    u8 TokenEntries;  // Number of Entries
    u8 HeaderChksum;  // 0 Checksum of the header
};

struct BIT_TOKEN
{
    u8 TokenId;       // Token identifier
    u8 DataVersion;   // Version of token data
    u16 DataSize;      // Size of token data
    u32 DataPtr;       // Pointer to token data
};
 */
/// BIOS Information Table (BIT) Header
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct BitHeader {
    /// 0h: BIT Header Identifier (0xB8FF)
    pub id: u16,
    /// 2h: BIT Header Signature ("BIT\0")
    pub signature: [u8; 4],
    /// 6h: Binary Coded Decimal Version, ex: 0x0100 is 1.00.
    pub bcd_version: u16,
    /// 8h: Size of BIT Header (in bytes)
    pub header_size: u8,
    /// 9h: Size of BIT Tokens (in bytes)
    pub token_size: u8,
    /// 10h: Number of token entries that follow
    pub token_entries: u8,
    /// 11h: BIT Header Checksum
    pub checksum: u8,
}

impl TryFrom<&[u8]> for BitHeader {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(EINVAL);
        }

        let mut signature = [0u8; 4];
        signature.copy_from_slice(&data[2..6]);

        // Check header ID and signature
        let id = u16::from_le_bytes([data[0], data[1]]);
        if id != 0xB8FF || &signature != b"BIT\0" {
            return Err(EINVAL);
        }

        Ok(BitHeader {
            id,
            signature,
            bcd_version: u16::from_le_bytes([data[6], data[7]]),
            header_size: data[8],
            token_size: data[9],
            token_entries: data[10],
            checksum: data[11],
        })
    }
}

/*
impl BitHeader {
    /// Verify BIT header checksum
    pub fn verify_checksum(&self, data: &[u8]) -> bool {
        if data.len() < self.header_size as usize {
            return false;
        }

        let mut sum: u8 = 0;
        for i in 0..self.header_size as usize {
            sum = sum.wrapping_add(data[i]);
        }
        sum == 0
    }
}
*/

/// BIT Token Entry: Records in the BIT table followed by the BIT header
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct BitToken {
    /// Token identifier
    pub id: u8,
    /// Version of the token data
    pub data_version: u8,
    /// Size of token data in bytes
    pub data_size: u16,
    /// Offset to the token data
    pub data_offset: u16,
}

// Define the token ID for the Falcon data
pub(in crate::vbios) const BIT_TOKEN_ID_FALCON_DATA: u8 = 0x70;

impl BitToken {
    /// Find a BIT token entry by BIT ID in a PciAtBiosImage
    pub(in crate::vbios) fn from_id(image: &PciAtBiosImage, token_id: u8) -> Result<Self> {
        let header = image.bit_header.as_ref().ok_or(EINVAL)?;

        // Offset to the first token entry
        let tokens_start = image.bit_offset.unwrap() + header.header_size as usize;

        for i in 0..header.token_entries as usize {
            let entry_offset = tokens_start + (i * header.token_size as usize);

            // Make sure we don't go out of bounds
            if entry_offset + header.token_size as usize > image.base.data.len() {
                return Err(EINVAL);
            }

            // Check if this token has the requested ID
            if image.base.data[entry_offset] == token_id {
                return Ok(BitToken {
                    id: image.base.data[entry_offset],
                    data_version: image.base.data[entry_offset + 1],
                    data_size: u16::from_le_bytes([
                        image.base.data[entry_offset + 2],
                        image.base.data[entry_offset + 3],
                    ]),
                    data_offset: u16::from_le_bytes([
                        image.base.data[entry_offset + 4],
                        image.base.data[entry_offset + 5],
                    ]),
                });
            }
        }

        // Token not found
        Err(ENOENT)
    }
}

/// PCI ROM Expansion Header as defined in PCI Firmware Specification

// ROM Image Header (PCI Expansion ROM)
/*
struct PCI_EXP_ROM_STANDARD
{
    u16       sig;                //  00h: ROM Signature 0xaa55
    u8        reserved [0x16];    //  02h: Reserved (processor architecture unique data)
    u16       pciDataStrucPtr;    //  18h: Pointer to PCI Data Structure  <--- for first image, this is 0x0170 per my dumps.

    Dump of first 32 bytes of first image:
        [542804.534235] NovaCore:   00000000: 55 aa 7f eb 4b 37 34 30 30 e9 4c 19 77 cc 56 49
        [542804.534238] NovaCore:   00000010: 44 45 4f 20 0d 00 00 00 70 01 a5 15 00 00 49 42

    u32       sizeOfBlock;        //  1Ah: <NBSI-specific appendage>
}

Alternative header format used with NBSI:
struct PCI_EXP_ROM_NBSI
{
    u16       sig;                //  00h: ROM Signature 0xaa55
    u8        reserved [0x14];    //  02h: Reserved (processor architecture unique data)
    u16       nbsiDataOffset;     //  16h: Offset from header to NBSI image
    u16       pciDataStrucPtr;    //  18h: Pointer to PCI Data Structure
    u32       sizeOfBlock;        //  1Ah: <NBSI-specific appendage>
}
 */
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct PciRomHeader {
    /// 00h: Signature (0xAA55)
    pub signature: u16,
    /// 02h: Reserved bytes for processor architecture unique data (22 bytes)
    pub reserved: [u8; 22],
    /// 16h: NBSI Data Offset (NBSI-specific, offset from header to NBSI image)
    pub nbsi_data_offset: Option<u16>,
    /// 18h: Pointer to PCI Data Structure (offset from start of ROM image)
    pub pci_data_struct_ptr: u16,
    /// 1Ah: Size of Block (NBSI-specific)
    pub size_of_block: Option<u32>,
}

impl TryFrom<&[u8]> for PciRomHeader {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 26 {
            // Need at least 26 bytes to read pciDataStrucPtr and sizeOfBlock
            return Err(EINVAL);
        }

        let signature = u16::from_le_bytes([data[0], data[1]]);

        // Check for valid ROM signatures
        match signature {
            0xAA55 | 0xBB77 | 0x4E56 => {}
            _ => {
                pr_info!("ROM signature unknown {:#x}\n", signature);
                return Err(EINVAL);
            }
        }

        // Read the pointer to the PCI Data Structure at offset 0x18
        let pci_data_struct_ptr = u16::from_le_bytes([data[24], data[25]]);

        // Try to read optional fields if enough data
        let mut size_of_block = None;
        let mut nbsi_data_offset = None;

        if data.len() >= 30 {
            // Read size_of_block at offset 0x1A
            size_of_block = Some(
                (data[29] as u32) << 24
                    | (data[28] as u32) << 16
                    | (data[27] as u32) << 8
                    | (data[26] as u32),
            );
        }

        // For NBSI images, try to read the nbsiDataOffset at offset 0x16
        if data.len() >= 24 {
            nbsi_data_offset = Some(u16::from_le_bytes([data[22], data[23]]));
        }

        Ok(PciRomHeader {
            signature,
            reserved: [0u8; 22],
            pci_data_struct_ptr,
            size_of_block,
            nbsi_data_offset,
        })
    }
}

/// NVIDIA PCI Data Extension Structure
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct NpdeStruct {
    /// Signature ("NPDE")
    pub signature: [u8; 4],
    /// NVIDIA PCI Data Extension Revision
    pub npci_data_ext_rev: u16,
    /// NVIDIA PCI Data Extension Length
    pub npci_data_ext_len: u16,
    /// Sub-image Length (in 512-byte units)
    pub subimage_len: u16,
    /// Last image indicator flag
    pub last_image: u8,
}

impl TryFrom<&[u8]> for NpdeStruct {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 11 {
            pr_info!("Not enough data for NpdeStruct\n");
            return Err(EINVAL);
        }

        let mut signature = [0u8; 4];
        signature.copy_from_slice(&data[0..4]);

        // Signature should be "NPDE" (0x4544504E)
        if &signature != b"NPDE" {
            pr_info!("Invalid signature for NpdeStruct: {:?}\n", signature);
            return Err(EINVAL);
        }

        Ok(NpdeStruct {
            signature,
            npci_data_ext_rev: u16::from_le_bytes([data[4], data[5]]),
            npci_data_ext_len: u16::from_le_bytes([data[6], data[7]]),
            subimage_len: u16::from_le_bytes([data[8], data[9]]),
            last_image: data[10],
        })
    }
}

impl NpdeStruct {
    /// Check if this is the last image in the ROM
    fn is_last(&self) -> bool {
        self.last_image & 0x80 != 0
    }

    /// Calculate image size in bytes
    fn image_size_bytes(&self) -> Result<usize> {
        if self.subimage_len > 0 {
            // Image size is in 512-byte blocks
            Ok(self.subimage_len as usize * 512)
        } else {
            Err(EINVAL)
        }
    }

    /// Try to find NPDE in the data
    fn find_in_data(data: &[u8], pcir_offset: usize, pcir_len: u16) -> Option<Self> {
        // Calculate the offset where NPDE might be located
        // NPDE should be right after the PCIR structure, aligned to 16 bytes
        let npde_start = (pcir_offset + pcir_len as usize + 0x0F) & !0x0F;

        // Check if we have enough data
        if npde_start + 11 > data.len() {
            return None;
        }

        // Try to create NPDE from the data
        NpdeStruct::try_from(&data[npde_start..])
            .inspect_err(|e| {
                pr_info!("Error creating NpdeStruct: {:?}", e);
            })
            .ok()
    }
}

// Use a macro to implement BiosImage enum and methods. This avoids having to
// repeat each enum type when implementing functions like base() as well.
macro_rules! bios_image {
    (
        $($variant:ident $class:ident),* $(,)?
    ) => {
        // BiosImage enum with variants for each image type
        enum BiosImage {
            $($variant($class)),*
        }

        impl BiosImage {
            /// Get a reference to the common BIOS image data regardless of type
            fn base(&self) -> &BiosImageBase {
                match self {
                    $(Self::$variant(img) => &img.base),*
                }
            }

            /// Returns a string representing the type of BIOS image
            fn image_type_str(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => stringify!($variant)),*
                }
            }

            /// Check if this is the last image
            fn is_last(&self) -> bool {
                let base = self.base();

                // For NBSI images (type == 0x70), return true as they're
                // considered the last image
                if matches!(self, Self::Nbsi(_)) {
                    return true;
                }

                // For other image types, check NPDE first if available
                if let Some(ref npde) = base.npde {
                    return npde.is_last();
                }

                // Otherwise, fall back to checking the PCIR last_image flag
                base.pcir.is_last()
            }

            /// Get the image size in bytes
            fn image_size_bytes(&self) -> Result<usize> {
                let base = self.base();

                // Prefer NPDE image size if available
                if let Some(ref npde) = base.npde {
                    return npde.image_size_bytes();
                }

                // Otherwise, fall back to the PCIR image size
                base.pcir.image_size_bytes()
            }
        }
    }
}

bios_image! {
    PciAt PciAtBiosImage,   // PCI-AT compatible BIOS image
    Efi EfiBiosImage,       // EFI (Extensible Firmware Interface)
    Nbsi NbsiBiosImage,     // NBSI (Nvidia Bios System Interface)
    FwSec FwSecBiosImage    // FWSEC (Firmware Security)
}

struct PciAtBiosImage {
    base: BiosImageBase,
    /*
     * The BIT header (BIOS Information Table)
     *
     *  struct BIT_HEADER
     *  {
     *      u16   id;                // Identifier
     *      char  signature[4];      // Signature string
     *      u16   bcd_version;       // BCD-encoded version
     *      u8    header_size;       // Size of header
     *      u8    token_size;        // Size of each token
     *      u8    token_entries;     // Number of tokens
     *      u8    checksum;          // Header checksum
     *  }
     *
     *  struct BIT_TOKEN
     *  {
     *      u8    id;                // Token identifier
     *      u8    data_version;      // Token data version
     *      u16   data_size;         // Token data size
     *      u16   data_offset;       // Offset to token data
     *  }
     */
    bit_header: Option<BitHeader>,
    bit_offset: Option<usize>,
}

struct EfiBiosImage {
    base: BiosImageBase,
    // EFI-specific fields can be added here in the future.
}

struct NbsiBiosImage {
    base: BiosImageBase,
    // NBSI-specific fields can be added here in the future.
}

pub(crate) struct FwSecBiosImage {
    base: BiosImageBase,
    // FWSEC-specific fields
    // The offset of the Falcon data from the start of Fwsec image
    falcon_data_offset: Option<usize>,
    // The PmuLookupTable starts at the offset of the falcon data pointer
    pmu_lookup_table: Option<PmuLookupTable>,
    // The offset of the Falcon ucode
    falcon_ucode_offset: Option<usize>,
}

// Convert from BiosImageBase to BiosImage
impl TryFrom<BiosImageBase> for BiosImage {
    type Error = Error;

    fn try_from(base: BiosImageBase) -> Result<Self> {
        pr_info!("BiosImageBase called with: {:?}\n", base);
        match base.pcir.code_type {
            0x00 => Ok(BiosImage::PciAt(base.try_into()?)),
            0x03 => Ok(BiosImage::Efi(EfiBiosImage { base })),
            0x70 => Ok(BiosImage::Nbsi(NbsiBiosImage { base })),
            0xE0 => Ok(BiosImage::FwSec(FwSecBiosImage {
                base,
                falcon_data_offset: None,
                pmu_lookup_table: None,
                falcon_ucode_offset: None,
            })),
            _ => {
                pr_info!("Unknown BIOS image type {:#x}\n", base.pcir.code_type);
                Err(EINVAL)
            }
        }
    }
}

// BiosImage creation from a byte slice
impl TryFrom<&[u8]> for BiosImage {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        pr_info!(
            "BiosImage try_from called with data length: {:?}\n",
            data.len()
        );
        let base = BiosImageBase::try_from(data)?;
        pr_info!("BiosImageBase created successfully. Calling to_image\n");
        base.to_image()
    }
}

/// BIOS Image structure containing various headers and references
/// fields base to all BIOS images.
#[derive(Debug)]
#[allow(dead_code)]
struct BiosImageBase {
    /// PCI ROM Expansion Header
    pub rom_header: PciRomHeader,
    /// PCI Data Structure
    pub pcir: PcirStruct,
    /// NVIDIA PCI Data Extension (optional)
    pub npde: Option<NpdeStruct>,
    /// Image data (includes ROM header and PCIR)
    pub data: KVec<u8>,
}

impl BiosImageBase {
    fn to_image(self) -> Result<BiosImage> {
        BiosImage::try_from(self)
    }
}

impl TryFrom<&[u8]> for BiosImageBase {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        pr_info!(
            "BiosImageBase try_from called with data length: {:?}\n",
            data.len()
        );
        // Ensure we have enough data for the ROM header
        if data.len() < 26 {
            pr_info!("Not enough data for ROM header\n");
            return Err(EINVAL);
        }

        // Parse the ROM header
        let rom_header = match PciRomHeader::try_from(&data[0..26]) {
            Ok(rom_header) => rom_header,
            Err(e) => {
                pr_info!("Failed to create PciRomHeader: {:?}\n", e);
                return Err(e);
            }
        };

        pr_info!(
            "Found ROM header with PCIR ptr: {:#x}\n",
            rom_header.pci_data_struct_ptr
        );

        // Get the PCI Data Structure using the pointer from the ROM header
        let pcir_offset = rom_header.pci_data_struct_ptr as usize;
        if pcir_offset + 24 > data.len() {
            pr_info!(
                "PCIR offset {:#x} out of bounds (data length: {})\n",
                pcir_offset,
                data.len()
            );
            pr_info!("Consider reading more data for construction of BiosImage\n");
            return Err(EINVAL);
        }

        let pcir_data = &data[pcir_offset..];
        let pcir = match PcirStruct::try_from(pcir_data) {
            Ok(pcir) => pcir,
            Err(e) => {
                pr_info!(
                    "Failed to create PcirStruct at offset {:#x}: {:?}\n",
                    pcir_offset,
                    e
                );
                return Err(e);
            }
        };

        // Look for NPDE structure if this is not an NBSI image (type != 0x70)
        let npde = match NpdeStruct::find_in_data(data, pcir_offset, pcir.pci_data_struct_len) {
            Some(npde) => Some(npde),
            None => None,
        };

        if let Some(ref npde) = npde {
            pr_info!(
                "Found NPDE structure with sub-image length: {:#x}\n",
                npde.subimage_len
            );
        }

        // Create a copy of the data
        let mut data_copy = KVec::new();
        for &byte in data {
            data_copy.push(byte, GFP_KERNEL)?;
        }

        Ok(BiosImageBase {
            rom_header,
            pcir,
            npde,
            data: data_copy,
        })
    }
}

impl PciAtBiosImage {
    /// Find a byte pattern in a slice
    fn find_byte_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Find the BIT header in the PciAtBiosImage
    fn find_bit_header(data: &[u8]) -> Result<(BitHeader, usize)> {
        let bit_pattern = [0xff, 0xb8, b'B', b'I', b'T', 0x00];
        let bit_offset = Self::find_byte_pattern(data, &bit_pattern);
        pr_info!("Bit offset: {}\n", bit_offset.unwrap());
        if bit_offset.is_none() {
            return Err(EINVAL);
        }

        let bit_header = BitHeader::try_from(&data[bit_offset.unwrap()..])?;
        Ok((bit_header, bit_offset.unwrap()))
    }

    /// Get a BIT token entry from the BIT table in the PciAtBiosImage
    fn get_bit_token(&self, token_id: u8) -> Result<BitToken> {
        BitToken::from_id(self, token_id)
    }

    /// Find the Falcon data pointer structure in the PciAtBiosImage
    /// This is just a 4 byte structure that contains a pointer to the
    /// Falcon data in the FWSEC image.
    fn falcon_data_ptr(&self) -> Result<u32> {
        let token = self.get_bit_token(BIT_TOKEN_ID_FALCON_DATA)?;

        // Make sure we don't go out of bounds
        pr_info!(
            "Falcon data bit token data offset: {:#x}\n",
            token.data_offset
        );
        if token.data_offset as usize + 4 > self.base.data.len() {
            return Err(EINVAL);
        }

        // read the 4 bytes at the offset specified in the token
        let offset = token.data_offset as usize;
        let bytes: [u8; 4] = match self.base.data[offset..offset + 4].try_into() {
            Ok(bytes) => bytes,
            Err(_) => {
                return Err(EINVAL);
            }
        };

        let data_ptr = u32::from_le_bytes(bytes);

        if (data_ptr as usize) < self.base.data.len() {
            pr_info!("Falcon data pointer out of bounds\n");
            return Err(EINVAL);
        }

        pr_info!("Falcon data pointer: {:#x}\n", data_ptr);
        Ok(data_ptr)
    }
}

impl TryFrom<BiosImageBase> for PciAtBiosImage {
    type Error = Error;

    fn try_from(base: BiosImageBase) -> Result<Self> {
        let data_slice = &base.data;
        let (bit_header, bit_offset) = PciAtBiosImage::find_bit_header(data_slice)?;

        // Find the Falcon data pointer
        // let falcon_data_ptr = PciAtBiosImage::find_falcon_data_ptr(&bit_header)?;

        // print the bit header
        pr_info!("Bit header: {:#?}\n", bit_header);

        // print the bit header signature
        pr_info!("Bit header signature: {:#?}\n", bit_header.signature);

        // print the bit header bcd_version
        pr_info!("Bit header bcd_version: {:#?}\n", bit_header.bcd_version);

        // print the bit header header_size
        pr_info!("Bit header header_size: {:#?}\n", bit_header.header_size);

        // print the bit header token_size
        pr_info!("Bit header token_size: {:#?}\n", bit_header.token_size);

        // print the bit header token_entries
        pr_info!(
            "Bit header token_entries: {:#?}\n",
            bit_header.token_entries
        );

        // print the bit header checksum
        pr_info!("Bit header checksum: {:#?}\n", bit_header.checksum);

        Ok(PciAtBiosImage {
            base,
            bit_header: Some(bit_header),
            bit_offset: Some(bit_offset),
        })
    }
}

struct PmuLookupTableEntry {
    application_id: u8,
    target_id: u8,
    data: u32,
}

impl TryFrom<&[u8]> for PmuLookupTableEntry {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 5 {
            return Err(EINVAL);
        }

        Ok(PmuLookupTableEntry {
            application_id: data[0],
            target_id: data[1],
            data: u32::from_le_bytes(data[2..6].try_into().map_err(|_| EINVAL)?),
        })
    }
}

struct PmuLookupTable {
    version: u8,
    header_len: u8,
    entry_len: u8,
    entry_count: u8,
    table_data: KVec<u8>,
}

impl TryFrom<&[u8]> for PmuLookupTable {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 4 {
            return Err(EINVAL);
        }

        let header_len = data[1] as usize;
        let entry_len = data[2] as usize;
        let entry_count = data[3] as usize;

        let required_bytes = header_len + (entry_count * entry_len);

        if data.len() < required_bytes {
            return Err(EINVAL);
        }

        // Create a copy of only the table data
        let mut table_data = KVec::new();

        // "last_entry_bytes" is a debugging aid.
        // let mut last_entry_bytes: Option<KVec<u8>> = Some(KVec::new());

        for &byte in &data[header_len..required_bytes] {
            table_data.push(byte, GFP_KERNEL)?;
            /*
             * Useful for debugging (dumps the table data to dmesg):
             * last_entry_bytes.as_mut().ok_or(EINVAL)?.push(byte, GFP_KERNEL)?;
             *
             * let last_entry_bytes_len = last_entry_bytes.as_ref().ok_or(EINVAL)?.len();
             * if last_entry_bytes_len == entry_len {
             *     pr_info!("Last entry bytes: {:02x?}\n", &last_entry_bytes.as_ref().ok_or(EINVAL)?[..]);
             *     last_entry_bytes = Some(KVec::new());
             * }
             */
        }

        Ok(PmuLookupTable {
            version: data[0],
            header_len: header_len as u8,
            entry_len: entry_len as u8,
            entry_count: entry_count as u8,
            table_data,
        })
    }
}

impl PmuLookupTable {
    fn lookup_index(&self, idx: u8) -> Result<PmuLookupTableEntry> {
        if idx >= self.entry_count {
            return Err(EINVAL);
        }

        let index = (idx as usize) * self.entry_len as usize;
        Ok(PmuLookupTableEntry::try_from(&self.table_data[index..])?)
    }

    // find entry by type value
    fn find_entry_by_type(&self, entry_type: u8) -> Result<PmuLookupTableEntry> {
        for i in 0..self.entry_count {
            let entry = self.lookup_index(i)?;
            pr_info!(
                "PmuLookupTableEntry: idx: {:#x}, application_id: {:#x}, target_id: {:#x}\n",
                i,
                entry.application_id,
                entry.target_id
            );
            if entry.application_id == entry_type {
                pr_info!("PmuLookupTableEntry found: idx: {:#x}, application_id: {:#x}, target_id: {:#x}\n", i, entry.application_id, entry.target_id);
                return Ok(entry);
            }
        }

        Err(EINVAL)
    }
}

impl FwSecBiosImage {
    fn setup_falcon_data(
        &mut self,
        pci_at_image: &PciAtBiosImage,
        first_fwsec_image: &FwSecBiosImage,
    ) -> Result<()> {
        let mut offset = pci_at_image.falcon_data_ptr()? as usize;

        // The falcon data pointer assumes that the PciAt and FWSEC images
        // are contiguous in memory. However, testing shows the EFI image sits in
        // between them. So calculate the offset from the end of the PciAt image
        // rather than the start of it. Compensate.
        offset -= pci_at_image.base.data.len();

        // The offset is now from the start of the first Fwsec image, however
        // the offset points to a location in the second Fwsec image. Since
        // the fwsec images are contiguous, subtract the length of the first Fwsec
        // image from the offset to get the offset to the start of the second
        // Fwsec image.
        offset -= first_fwsec_image.base.data.len();

        self.falcon_data_offset = Some(offset);

        // The PmuLookupTable starts at the offset of the falcon data pointer
        self.pmu_lookup_table = Some(PmuLookupTable::try_from(&self.base.data[offset..])?);

        // print the PmuLookupTable fields
        if let Some(pmu_lookup_table) = &self.pmu_lookup_table {
            pr_info!("New 3 PmuLookupTable created with ver: {:#x}, hdr: {:#x}, len: {:#x}, cnt: {:#x}\n",
                pmu_lookup_table.version,
                pmu_lookup_table.header_len,
                pmu_lookup_table.entry_len,
                pmu_lookup_table.entry_count);
        }

        match self
            .pmu_lookup_table
            .as_ref()
            .ok_or(EINVAL)?
            .find_entry_by_type(FALCON_UCODE_ENTRY_APPID_FWSEC_PROD)
        {
            Ok(entry) => {
                let mut ucode_offset = entry.data as usize;
                ucode_offset -= pci_at_image.base.data.len();
                ucode_offset -= first_fwsec_image.base.data.len();
                self.falcon_ucode_offset = Some(ucode_offset);
                pr_info!("PmuLookupTableEntry found: app_id: {:#x}, target_id: {:#x}, data: {:#x}, ucode_offs: {:#x}\n",
                    entry.application_id, entry.target_id, entry.data, ucode_offset);

                /*
                 * for debug: print the v3_desc header
                 * let v3_desc = self.fwsec_header()?;
                 * pr_info!("PmuLookupTableEntry v3_desc: {:#?}\n", v3_desc);
                 */
            }
            Err(e) => {
                pr_info!("PmuLookupTableEntry not found, error: {:?}\n", e);
            }
        }
        Ok(())
    }

    /// Get the FwSec header (FalconUCodeDescV3)
    fn fwsec_header(&self) -> Result<&FalconUCodeDescV3> {
        // Get the falcon ucode offset that was found in setup_falcon_data
        let falcon_ucode_offset = self.falcon_ucode_offset.ok_or(EINVAL)? as usize;

        // Make sure the offset is within the data bounds
        if falcon_ucode_offset + core::mem::size_of::<FalconUCodeDescV3>() > self.base.data.len() {
            pr_err!("fwsec-frts header not contained within BIOS bounds\n");
            return Err(ERANGE);
        }

        // Read the first 4 bytes to get the version
        let hdr_bytes: [u8; 4] = self.base.data[falcon_ucode_offset..falcon_ucode_offset + 4]
            .try_into()
            .map_err(|_| EINVAL)?;
        let hdr = u32::from_le_bytes(hdr_bytes);
        let ver = (hdr & 0xff00) >> 8;

        if ver != 3 {
            pr_err!("invalid fwsec firmware version\n");
            return Err(EINVAL);
        }

        // Return a reference to the FalconUCodeDescV3 structure
        Ok(unsafe {
            &*(self.base.data.as_ptr().add(falcon_ucode_offset) as *const FalconUCodeDescV3)
        })
    }
    /// Get the ucode data as a byte slice
    fn fwsec_ucode(&self, v3_desc: &FalconUCodeDescV3) -> Result<&[u8]> {
        let falcon_ucode_offset = self.falcon_ucode_offset.ok_or(EINVAL)? as usize;

        // The ucode data follows the descriptor
        let ucode_data_offset = falcon_ucode_offset + v3_desc.size();
        let size = (v3_desc.imem_load_size + v3_desc.dmem_load_size) as usize;

        // Make sure the data is within bounds
        if ucode_data_offset + size > self.base.data.len() {
            pr_err!("fwsec ucode data not contained within BIOS bounds\n");
            return Err(ERANGE);
        }

        Ok(&self.base.data[ucode_data_offset..ucode_data_offset + size])
    }
}
