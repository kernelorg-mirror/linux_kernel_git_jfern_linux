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
        
        // loop till break
        loop {
            // Try to parse a BIOS image at the current offset
            // This will now check for all valid ROM signatures (0xAA55, 0xBB77, 0x4E56)
            match vbios.read_bios_image_at_offset(cur_offset, 1024) {
                Ok(image) => {
                    // Get image size in bytes
                    let image_size = match image.image_size_bytes() {
                        Ok(size) => size,
                        Err(_) => {
                            pr_info!("Invalid image size at offset {:#x}, stopping scan\n", cur_offset);
                            break;
                        }
                    };
                    
                    // Read the full image
                    vbios.read_more_at_offset(cur_offset as u32, image_size as u32)?;
                    
                    // Create a new BiosImage with the full image data
                    let full_image = match vbios.read_bios_image_at_offset(cur_offset, image_size) {
                        Ok(img) => img,
                        Err(e) => {
                            pr_info!("Failed to parse full BIOS image at offset {:#x}: {:?}\n", cur_offset, e);
                            break;
                        }
                    };
                    
                    // Special handling for FwSec image (type 0xE0)
                    if let BiosImage::FwSec(_) = &full_image {
                        if !first_e0_done {
                            first_e0_done = true;
                            // Note: original code tracks imaged_addr here
                        }
                    }
                    
                    // Store the current offset for this image
                    //image_offsets.push(cur_offset);
                    
                    // Add to our collection
                    // images.push(full_image, GFP_KERNEL)?;
                    
                    // Determine the image type
                    let image_type = match &full_image {
                        BiosImage::PciAt(_) => "PciAt",
                        BiosImage::Efi(_) => "Efi",
                        BiosImage::Nbsi(_) => "Nbsi",
                        BiosImage::FwSec(_) => "FwSec",
                    };
                    
                    pr_info!("Found BIOS image at offset {:#x}, size: {:#x}, type: {}\n", 
                              cur_offset, image_size, image_type);
                    
                    // Break if this is the last image
                    if full_image.is_last() {
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
                },
                Err(e) => {
                    pr_info!("Failed to parse BIOS image at offset {:#x}: {:?}\n", cur_offset, e);
                    break;
                }
            }
        }

        /*
        // Summarize the images found
        pr_info!("Found {} images:\n", images.len());
        for (i, (image, offset)) in images.iter().zip(image_offsets.iter()).enumerate() {
            // Calculate data offset - in this case it's the same as the image offset
            // because each image's data slice starts at that offset in the vbios.data KVec
            let data_offset = offset;
            
            pr_info!("  Image {}: offset {:#x}, data_offset {:#x}, {:?}\n", 
                     i, offset, data_offset, image);
        }
        */
        
        // Attempt to extract version information from BIT if available
        // TODO: Extract version info from BIT entries similar to the original code
        
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

/// BIOS Information Table (BIT) Header
#[derive(Debug, Clone, Copy)]
pub(crate) struct BitHeader {
    /// BIT Header Identifier (0xB8FF)
    pub id: u16,
    /// BIT Header Signature ("BIT\0")
    pub signature: [u8; 4],
    ///Binary Coded Decimal Version, ex: 0x0100 is 1.00.
    pub bcd_version: u16,
    /// Size of BIT Header (in bytes)
    pub header_size: u8,
    /// Size of BIT Tokens (in bytes)
    pub token_size: u8,
    /// Number of token entries that follow
    pub token_entries: u8,
    /// BIT Header Checksum
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

/// BIT Token Entry
#[derive(Debug, Clone, Copy)]
pub struct BitEntry {
    /// Unique identifier indicating data type
    pub id: u8,
    /// Version of the data structure
    pub version: u8,
    /// Size of data structure in bytes
    pub length: u16,
    /// Pointer (offset) to the actual data structure
    pub offset: u16,
}

impl TryFrom<&[u8]> for BitEntry {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 6 {
            return Err(EINVAL);
        }

        Ok(BitEntry {
            id: data[0],
            version: data[1],
            length: u16_from_u8s(data[3], data[2]),
            offset: u16_from_u8s(data[5], data[4]),
        })
    }
}

impl BitEntry {
    /// Find a specific BitEntry by ID in a table of entries
    pub(crate) fn from_id(data: &[u8], token_size: u8, token_entries: u8, id: u8) -> Result<Self> {
        let token_size = token_size as usize;

        for i in 0..token_entries as usize {
            let offset = i * token_size;
            if offset + token_size > data.len() {
                return Err(EINVAL);
            }

            let entry_id = data[offset];
            if entry_id == id {
                return (&data[offset..offset + token_size]).try_into();
            }
        }

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
    Nbsi(NbsiBiosImage<'a>),
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
    // FWSEC-specific fields can be added here in the future.
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
        //considered the last image
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
        
        // For non-NBSI images with NPDE, use the NPDE image size
        if !matches!(self, Self::Nbsi(_)) {
            if let Some(ref npde) = base.npde {
                return npde.image_size_bytes();
            }
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
            0x00 => {
                Ok(BiosImage::PciAt(base.try_into()?))
            },
            0x03 => Ok(BiosImage::Efi(EfiBiosImage { base })),
            0x70 => Ok(BiosImage::Nbsi(NbsiBiosImage { base })),
            0xE0 => Ok(BiosImage::FwSec(FwSecBiosImage { base })),
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
        let base = BiosImageBase::try_from(data)?;
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
        // TODO: Single npde is image specific, should this logic be moved to the
        // specific BiosImage type? And ditto for is_last and image_size_bytes.
        let npde = if pcir.code_type != 0x70 {
            NpdeStruct::find_in_data(data, pcir_offset, pcir.pci_data_struct_len)
        } else {
            None
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

impl<'a> TryFrom<BiosImageBase<'a>> for PciAtBiosImage<'a> {
    type Error = Error;

    fn try_from(base: BiosImageBase<'a>) -> Result<Self> {
        // Get the bit_header from the data
        let bit_header = BitHeader::try_from(&base.data[2..])?;

        Ok(PciAtBiosImage { base, bit_header: Some(bit_header) })
    }
}

/* TODO: Review
/// Extension method for VBios to parse BiosImage
impl<'a> Vbios<'a> {
    /// Parse the VBIOS data to get a BiosImage
    pub fn parse_bios_image(&self) -> Result<BiosImage<'_>> {
        // Create a BiosImage from a slice of the KVec
        BiosImage::from_slice(&self.data[..])
    }

    /// Parse the VBIOS data at a specific offset
    pub fn parse_bios_image_at_offset(&self, offset: usize) -> Result<BiosImage<'_>> {
        if offset >= self.data.len() {
            return Err(EINVAL);
        }
        BiosImage::from_slice(&self.data[offset..])
    }

    /// Find all BIOS images in the ROM
    pub fn find_all_bios_images(&self) -> Result<Vec<BiosImage<'_>, Global>> {
        let mut images = Vec::try_with_capacity(4, GFP_KERNEL)?;
        let mut offset = 0;

        // Parse the first image
        while offset < self.data.len() {
            match self.parse_bios_image_at_offset(offset) {
                Ok(image) => {
                    // Calculate next offset
                    let image_size = if image.size > 0 {
                        image.size
                    } else {
                        // If size is unknown, we can't reliably find the next image
                        images.try_push(image, GFP_KERNEL)?;
                        break;
                    };

                    // Add the image to our collection
                    images.try_push(image, GFP_KERNEL)?;

                    // If this was the last image, we're done
                    if images.last().unwrap().is_last() {
                        break;
                    }

                    // Move to the next image (aligned to 512 bytes)
                    offset += image_size;
                    offset = (offset + 511) & !511;
                },
                Err(_) => break,
            }
        }

        if images.is_empty() {
            Err(ENOENT)
        } else {
            Ok(images)
        }
    }
}*/
