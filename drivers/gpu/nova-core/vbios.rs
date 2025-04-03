use kernel::prelude::*;
use core::convert::TryFrom;
use crate::driver::Bar0;
use kernel::error::Result;
use kernel::devres::Devres;
use crate::regs::RomShadow;

/// Helper function to create u16 from two u8 values (little-endian)
pub(crate) fn u16_from_u8s(high: u8, low: u8) -> u16 {
    ((high as u16) << 8) | (low as u16)
}

/// The offset of the VBIOS ROM in the BAR0 space
const ROM_OFFSET: usize = 0x300000;

/// VBIOS data structure
pub struct Vbios<'a> {
    pub bar0: &'a Devres<Bar0>,
    pub version: u16,
    /// VBIOS data
    pub data: KVec<u8>,
}

impl<'a> Vbios<'a> {
    /// Enable ROM shadowing to access VBIOS ROM
    ///
    /// This enables ROM shadowing by clearing bit 0 of the ROM shadow register,
    /// allowing the VBIOS to be accessible through BAR0.
    fn enable_rom_shadow(&self) -> Result {
        with_bar!(self.bar0, |bar0| {
            // Clear LSB of ROM shadow register
            let reg = RomShadow::read(bar0);
            reg.set_val(reg.val() & !0x00000001);
            reg.write(bar0);
        })
    }

    /// Read bytes from the ROM at the current end of the data vector
    pub(crate) fn read_more(&mut self, bytes: u32) -> Result {
        with_bar!(self.bar0, |bar0| {
            // Get current length
            let current_len = self.data.len();

            // Read ROM data bytes push directly to vector
            for i in 0..bytes as usize {
                // Read a byte from the VBIOS ROM and push it to the data vector
                let rom_addr = ROM_OFFSET + current_len + i;
                let byte = bar0.try_readb(rom_addr)?;
                self.data.push(byte, GFP_KERNEL)?;
            }

            Ok(())
        })?
    }

    /// Read bytes at a specific offset, filling any gap
    pub(crate) fn read_more_at_offset(&mut self, offset: u32, bytes: u32) -> Result {
        // If offset is beyond current data size, fill the gap first
        let current_len = self.data.len();

        if offset as usize > current_len {
            // Calculate bytes to read to fill the gap
            let gap_bytes = offset as usize - current_len;
            self.read_more(gap_bytes as u32)?;
        }

        // Now read the requested bytes at the offset
        self.read_more(bytes)
    }

    pub(crate) fn read_bios_image_at_offset(&mut self, offset: usize, bytes: usize) -> Result<BiosImage<'_>> {
        if offset + bytes > self.data.len() {
            match self.read_more_at_offset(offset as u32, bytes as u32) {
                Ok(_) => {},
                Err(e) => {
                    pr_info!("Failed to read more at offset {:#x}: {:?}\n", offset, e);
                    return Err(e);
                }
            }
        }

        match BiosImage::try_from(&self.data[offset..offset + bytes]) {
            Ok(image) => Ok(image),
            Err(e) => {
                pr_info!("Failed to create BiosImage at offset {:#x}: {:?}\n", offset, e);
                Err(e)
            }
        }
    }

    /// Probe for VBIOS extraction
    pub(crate) fn probe(bar0: &'a Devres<Bar0>) -> Result<Self> {
        let mut vbios = Self { bar0, version: 0,  data: KVec::new() };

        // Enable ROM shadowing so the ROM is accessible on the BAR
        pr_info!("Enabling ROM shadowing\n");
        vbios.enable_rom_shadow()?;
        pr_info!("ROM shadowing enabled\n");

        // Read the first 32 bytes into the KVec
        vbios.read_more(32)?;

        // Print the bytes from the KVec to verify they match
        pr_info!("ROM header bytes from KVec (first 32 bytes):\n");
        for row in 0..2 {
            let base = row * 16;
            if base + 15 < vbios.data.len() {
                pr_info!("  {:08x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}\n",
                    base,
                    vbios.data[base],     vbios.data[base + 1], vbios.data[base + 2], vbios.data[base + 3],
                    vbios.data[base + 4], vbios.data[base + 5], vbios.data[base + 6], vbios.data[base + 7],
                    vbios.data[base + 8], vbios.data[base + 9], vbios.data[base + 10], vbios.data[base + 11],
                    vbios.data[base + 12], vbios.data[base + 13], vbios.data[base + 14], vbios.data[base + 15]
                );
            }
        }

        // Loop through all the BiosImage and extract relevant ones and relevant data from them
        // let mut images = KVec::new();
        //let mut image_offsets = Vec::new();
        let mut cur_offset = 0;
        let mut first_e0_done = false;
        // Instead of storing references, store the needed data
        let mut pci_at_falcon_data_ptr: Option<u32> = None;
        let mut pci_at_falcon_data_ptr_offset: Option<u32> = None;
        let mut fwsec_image_found = false;
        
        // loop till break
        loop {
            // Try to parse a BIOS image at the current offset
            // This will now check for all valid ROM signatures (0xAA55, 0xBB77, 0x4E56)
            let image_size = match vbios.read_bios_image_at_offset(cur_offset, 1024) {
                Ok(image) => {
                    // Get image size in bytes
                    match image.image_size_bytes() {
                        Ok(size) => size,
                        Err(_) => {
                            pr_info!("Invalid image size at offset {:#x}, stopping scan\n", cur_offset);
                            break;
                        }
                    }
                },
                Err(e) => {
                    pr_info!("Failed to parse BIOS image at offset {:#x}: {:?}\n", cur_offset, e);
                    break;
                }
            };
            
            // Read the full image
            vbios.read_more_at_offset(cur_offset as u32, image_size as u32)?;
            
            // Scope for processing the image
            {
                // Create a new BiosImage with the full image data
                let mut full_image = match vbios.read_bios_image_at_offset(cur_offset, image_size) {
                    Ok(img) => img,
                    Err(e) => {
                        pr_info!("Failed to parse full BIOS image at offset {:#x}: {:?}\n", cur_offset, e);
                        break;
                    }
                };
                
                // Determine the image type
                let image_type = match &full_image {
                    BiosImage::PciAt(_) => "PciAt",
                    BiosImage::Efi(_) => "Efi",
                    BiosImage::Nbsi(_) => "Nbsi",
                    BiosImage::FwSec(_) => "FwSec",
                };
                
                pr_info!("Found BIOS image at offset {:#x}, size: {:#x}, type: {}\n", 
                          cur_offset, image_size, image_type);

                // Process PciAt image
                if let BiosImage::PciAt(image) = &full_image {
                    if let Ok(data_ptr) = image.falcon_data_ptr() {
                        pr_info!("PciAt BIOS Image BIT entry BIT_TOKEN_ID_FALCON_DATA: {:#x}\n", data_ptr);
                        pci_at_falcon_data_ptr = Some(data_ptr);
                    } else {
                        pr_info!("Failed to get falcon data pointer for PciAt BIOS Image\n");
                    }
                    
                    if let Ok(data_ptr_offset) = image.falcon_data_ptr_offset() {
                        pci_at_falcon_data_ptr_offset = Some(data_ptr_offset);
                    }
                }

                // Process FwSec image
                if let BiosImage::FwSec(fwsec_image) = &mut full_image {
                    if !first_e0_done {
                        first_e0_done = true;
                        // Note: original code tracks imaged_addr here
                    }
                    fwsec_image.set_falcon_data_offset(pci_at_falcon_data_ptr_offset)?;
                    fwsec_image_found = true;
                }
                
                // Break if this is the last image
                let is_last = full_image.is_last();
                if is_last {
                    pr_info!("Last image found, stopping scan\n");
                    break;
                }
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

        // Print information about falcon data
        if fwsec_image_found {
            if let (Some(data_ptr), Some(data_ptr_offset)) = (pci_at_falcon_data_ptr, pci_at_falcon_data_ptr_offset) {
                pr_info!("PciAt falcon data ptr: {:#x} and ptr offset: {:#x}\n",
                        data_ptr, data_ptr_offset);
                pr_info!("FwSec BIOS Image falcon data offset: {:#x}\n", data_ptr_offset);
            } else {
                pr_info!("Failed to get PciAt falcon data information for FwSec image\n");
            }
        } else {
            pr_info!("Failed to get FwSec image\n");
        }
        
        // Find the BIT header by scanning for "BIT" signature
        
        // If BIT header found, try to find the 'i' entry for version info
        // Look for BMP signature as in the reference code

        Ok(vbios)
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
#[derive(Debug)]
pub(crate) struct PcirStruct {
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
        if data.len() < 24 { // Updated to match full PCI_DATA_STRUCT size
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
            vendor_id: u16_from_u8s(data[5], data[4]),
            device_id: u16_from_u8s(data[7], data[6]),
            device_list_ptr: u16_from_u8s(data[9], data[8]),
            pci_data_struct_len: u16_from_u8s(data[11], data[10]),
            pci_data_struct_rev: data[12],
            class_code,
            image_len: u16_from_u8s(data[17], data[16]),
            vendor_rom_rev: u16_from_u8s(data[19], data[18]),
            code_type: data[20],
            last_image: data[21],
            max_runtime_image_len: u16_from_u8s(data[23], data[22]),
        })
    }
}

impl PcirStruct {
    /// Check if this is the last image in the ROM
    pub(crate) fn is_last(&self) -> bool {
        self.last_image & 0x80 != 0
    }

    /// Calculate image size in bytes
    pub(crate) fn image_size_bytes(&self) -> Result<usize> {
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
pub(crate) struct BitHeader {
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
        let id = u16_from_u8s(data[1], data[0]);
        if id != 0xB8FF || &signature != b"BIT\0" {
            return Err(EINVAL);
        }

        Ok(BitHeader {
            id,
            signature,
            bcd_version: u16_from_u8s(data[7], data[6]),
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
pub struct BitToken {
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
pub const BIT_TOKEN_ID_FALCON_DATA: u8 = 0x70;

impl BitToken {
    /// Find a BIT token entry by BIT ID in a PciAtBiosImage
    pub fn from_id<'a>(image: &'a PciAtBiosImage, token_id: u8) -> Result<Self> {
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
                    data_size: u16_from_u8s(
                        image.base.data[entry_offset + 3],
                        image.base.data[entry_offset + 2]
                    ),
                    data_offset: u16_from_u8s(
                        image.base.data[entry_offset + 5],
                        image.base.data[entry_offset + 4]
                    ),
                });
            }
        }
        
        // Token not found
        Err(ENOENT)
    }
}

/// PCI ROM Expansion Header as defined in PCI Firmware Specification
#[derive(Debug, Clone, Copy)]
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
pub(crate) struct PciRomHeader {
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
        if data.len() < 26 { // Need at least 26 bytes to read pciDataStrucPtr and sizeOfBlock
            return Err(EINVAL);
        }

        let signature = u16_from_u8s(data[1], data[0]);

        // Check for valid ROM signatures
        match signature {
            0xAA55 | 0xBB77 | 0x4E56 => {},
            _ => {
                pr_info!("ROM signature unknown {:#x}\n", signature);
                return Err(EINVAL);
            }
        }

        // Read the pointer to the PCI Data Structure at offset 0x18
        let pci_data_struct_ptr = u16_from_u8s(data[25], data[24]);

        // Try to read optional fields if enough data
        let mut size_of_block = None;
        let mut nbsi_data_offset = None;

        if data.len() >= 30 {
            // Read size_of_block at offset 0x1A
            size_of_block = Some(
                (data[29] as u32) << 24 |
                (data[28] as u32) << 16 |
                (data[27] as u32) << 8 |
                (data[26] as u32)
            );
        }

        // For NBSI images, try to read the nbsiDataOffset at offset 0x16
        if data.len() >= 24 {
            nbsi_data_offset = Some(u16_from_u8s(data[23], data[22]));
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
#[derive(Debug)]
pub(crate) struct NpdeStruct {
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
            npci_data_ext_rev: u16_from_u8s(data[5], data[4]),
            npci_data_ext_len: u16_from_u8s(data[7], data[6]),
            subimage_len: u16_from_u8s(data[9], data[8]),
            last_image: data[10],
        })
    }
}

impl NpdeStruct {
    /// Check if this is the last image in the ROM
    pub(crate) fn is_last(&self) -> bool {
        self.last_image & 0x80 != 0
    }

    /// Calculate image size in bytes
    pub(crate) fn image_size_bytes(&self) -> Result<usize> {
        if self.subimage_len > 0 {
            // Image size is in 512-byte blocks
            Ok(self.subimage_len as usize * 512)
        } else {
            Err(EINVAL)
        }
    }
    
    /// Try to find NPDE in the data
    pub(crate) fn find_in_data(data: &[u8], pcir_offset: usize, pcir_len: u16) -> Option<Self> {
        // Calculate the offset where NPDE might be located
        // NPDE should be right after the PCIR structure, aligned to 16 bytes
        let npde_start = (pcir_offset + pcir_len as usize + 0x0F) & !0x0F;
        
        // Check if we have enough data
        if npde_start + 11 > data.len() {
            return None;
        }
        
        // Try to create NPDE from the data
        match NpdeStruct::try_from(&data[npde_start..]) {
            Ok(npde) => Some(npde),
            Err(_) => None,
        }
    }
}

// Replace the simple BiosImage enum with a more powerful version
pub(crate) enum BiosImage<'a> {
    PciAt(PciAtBiosImage<'a>),
    Efi(EfiBiosImage<'a>),
    Nbsi(NbsiBiosImage<'a>),    // NBSI (Nvidia Bios System Interface)
    FwSec(FwSecBiosImage<'a>),
}

// The indiviaul image types, when adding a new type, add it
// also to the BiosImage::base() method.
pub(crate) struct PciAtBiosImage<'a> {
    base: BiosImageBase<'a>,
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

pub(crate) struct EfiBiosImage<'a> {
    base: BiosImageBase<'a>,
    // EFI-specific fields can be added here in the future.
}

pub(crate) struct NbsiBiosImage<'a> {
    base: BiosImageBase<'a>,
    // NBSI-specific fields can be added here in the future.
}

pub(crate) struct FwSecBiosImage<'a> {
    base: BiosImageBase<'a>,
    // FWSEC-specific fields
    // The offset of the Falcon data from the start of Fwsec image
    falcon_data_offset: Option<u32>,
}

// Implementation for BiosImage to provide common access methods
impl<'a> BiosImage<'a> {
    /// Get a reference to the common BIOS image data regardless of type
    pub(crate) fn base(&self) -> &BiosImageBase<'a> {
        match self {
            Self::PciAt(img) => &img.base,
            Self::Efi(img) => &img.base,
            Self::Nbsi(img) => &img.base,
            Self::FwSec(img) => &img.base,
        }
    }
    
    /// Check if this is the last image
    pub(crate) fn is_last(&self) -> bool {
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
    pub(crate) fn image_size_bytes(&self) -> Result<usize> {
        let base = self.base();

        // Prefer NPDE image size if available
        if let Some(ref npde) = base.npde {
            return npde.image_size_bytes();
        }

        // Otherwise, fall back to the PCIR image size
        base.pcir.image_size_bytes()
    }
}

// Convert from BiosImageBase to BiosImage
impl<'a> TryFrom<BiosImageBase<'a>> for BiosImage<'a> {
    type Error = Error;

    fn try_from(base: BiosImageBase<'a>) -> Result<Self> {
        pr_info!("BiosImageBase called with: {:?}\n", base);
        match base.pcir.code_type {
            0x00 => { Ok(BiosImage::PciAt(base.try_into()?)) },
            0x03 => Ok(BiosImage::Efi(EfiBiosImage { base })),
            0x70 => Ok(BiosImage::Nbsi(NbsiBiosImage { base })),
            0xE0 => Ok(BiosImage::FwSec(FwSecBiosImage { base, falcon_data_offset: None })),
            _ => {
                pr_info!("Unknown BIOS image type {:#x}\n", base.pcir.code_type);
                Err(EINVAL)
            }
        }
    }
}


// BiosImage creation from a byte slice
impl<'a> TryFrom<&'a [u8]> for BiosImage<'a> {
    type Error = Error;

    fn try_from(data: &'a [u8]) -> Result<Self> {
        pr_info!("BiosImage try_from called with data length: {:?}\n", data.len());
        let base = BiosImageBase::try_from(data)?;
        pr_info!("BiosImageBase created successfully. Calling to_image\n");
        base.to_image()
    }
}

/// BIOS Image structure containing various headers and references
/// fields base to all BIOS images.
#[derive(Debug)]
pub(crate) struct BiosImageBase<'a> {
    /// PCI ROM Expansion Header
    pub rom_header: PciRomHeader,
    /// PCI Data Structure
    pub pcir: PcirStruct,
    /// NVIDIA PCI Data Extension (optional)
    pub npde: Option<NpdeStruct>,
    /// Slice of the image data (includes ROM header and PCIR)
    pub data: &'a [u8],
}

impl<'a> BiosImageBase<'a> {
    pub(crate) fn to_image(self) -> Result<BiosImage<'a>> {
        BiosImage::try_from(self)
    }
}

impl<'a> TryFrom<&'a [u8]> for BiosImageBase<'a> {
    type Error = Error;

    fn try_from(data: &'a [u8]) -> Result<Self> {
        pr_info!("BiosImageBase try_from called with data length: {:?}\n", data.len());
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

        pr_info!("Found ROM header with PCIR ptr: {:#x}\n", rom_header.pci_data_struct_ptr);

        // Get the PCI Data Structure using the pointer from the ROM header
        let pcir_offset = rom_header.pci_data_struct_ptr as usize;
        if pcir_offset + 24 > data.len() {
            pr_info!("PCIR offset {:#x} out of bounds (data length: {})\n", pcir_offset, data.len());
            pr_info!("Consider reading more data for construction of BiosImage\n");
            return Err(EINVAL);
        }

        let pcir_data = &data[pcir_offset..];
        let pcir = match PcirStruct::try_from(pcir_data) {
            Ok(pcir) => pcir,
            Err(e) => {
                pr_info!("Failed to create PcirStruct at offset {:#x}: {:?}\n", pcir_offset, e);
                return Err(e);
            }
        };

        // Look for NPDE structure if this is not an NBSI image (type != 0x70)
        let npde = match NpdeStruct::find_in_data(data, pcir_offset, pcir.pci_data_struct_len) {
            Some(npde) => Some(npde),
            None => None,
        };

        if let Some(ref npde) = npde {
            pr_info!("Found NPDE structure with sub-image length: {:#x}\n", npde.subimage_len);
        }

        Ok(BiosImageBase {
            rom_header,
            pcir,
            npde,
            data,
        })
    }
}

impl PciAtBiosImage<'_> {
    /// Find a byte pattern in a slice
    fn find_byte_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len())
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
        pr_info!("Falcon data bit token data offset: {:#x}\n", token.data_offset);
        if token.data_offset as usize + 4 > self.base.data.len() {
            return Err(EINVAL);
        }

        // read the 4 bytes at the offset specified in the token
        let offset = token.data_offset as usize;
        let bytes: [u8; 4] = match self.base.data[offset..offset + 4].try_into() {
            Ok(bytes) => bytes,
            Err(_) => { return Err(EINVAL); }
        };

        let data_ptr = u32::from_le_bytes(bytes);

        pr_info!("Falcon data pointer: {:#x}\n", data_ptr);
        
        Ok(data_ptr)
    }

    // The falcon data pointer assumes that the PciAt and FWSEC images
    // are contiguous in memory. However, testing shows the EFI image sits in
    // between them. So calculate the offset from the end of the PciAt image
    // to the start of data pointer.
    fn falcon_data_ptr_offset(&self) -> Result<u32> {
        let ptr = self.falcon_data_ptr()?;

        if (ptr as usize) < self.base.data.len() {
            return Err(EINVAL);
        }

        Ok(ptr - self.base.data.len() as u32)
    }
}

impl<'a> TryFrom<BiosImageBase<'a>> for PciAtBiosImage<'a> {
    type Error = Error;

    fn try_from(base: BiosImageBase<'a>) -> Result<Self> {
        let (bit_header, bit_offset) = PciAtBiosImage::find_bit_header(&base.data)?;

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
        pr_info!("Bit header token_entries: {:#?}\n", bit_header.token_entries);

        // print the bit header checksum
        pr_info!("Bit header checksum: {:#?}\n", bit_header.checksum);

        Ok(PciAtBiosImage { base, bit_header: Some(bit_header),
                bit_offset: Some(bit_offset) })
    }
}

impl FwSecBiosImage<'_> {
    fn set_falcon_data_offset(&mut self, falcon_offset: Option<u32>) -> Result {
        if let Some(offset) = falcon_offset {
            // The image's len is less than the offset, just return success
            // since we'll wait for an fwsec image that the offset does fit in
            if offset > self.base.data.len() as u32 {
                return Ok(());
            }

            self.falcon_data_offset = Some(offset);
        } else {
            pr_info!("FwSec image falcon offset not set.\n");
            return Err(EINVAL);
        }
        Ok(())
    }
}
