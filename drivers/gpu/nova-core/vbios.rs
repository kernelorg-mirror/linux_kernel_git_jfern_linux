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
    /// PCI Vendor ID (e.g., 0x10DE for NVIDIA)
    pub vendor_id: u16,
    /// PCI Device ID
    pub device_id: u16,
    /// PCIR length
    pub pcir_length: u16,
    /// PCIR version
    pub pcir_version: u8,
    /// ROM image type (0x00 = PC-AT compatible, 0x03 = EFI, 0x70 = NBSI)
    pub image_type: u8,
    /// Last image indicator (0x00 = Not last image, 0x80 = Last image)
    pub last_image: u8,
    /// Size of this image in 512-byte blocks
    pub image_size: u16,
    /// Class code (3 bytes, 0x03 for display controller)
    pub class_code: [u8; 3],
    /// PCI Data Structure signature ("PCIR")
    pub signature_offset: u16, // at offset 16 from the start of the PCIR
    pub signature: [u8; 4],
}

impl TryFrom<&[u8]> for PcirStruct {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 22 {
            pr_info!("Not enough data for PcirStruct\n");
            return Err(EINVAL);
        }

        let mut class_code = [0u8; 3];
        class_code.copy_from_slice(&data[11..14]);

        let mut signature_offset = u16_from_u8s(data[17], data[16]);
        // account for the ROM signature
        signature_offset -= 2;

        let mut signature = [0u8; 4];

        // BROKEN: causes out of bounds runtime panic
        // signature.copy_from_slice(&data[signature_offset as usize..signature_offset as usize + 4]);

        // Signature should be "PCIR" (0x52494350) or "RGIS" (0x53494752) or "NPDS" (0x5344504e)
        if &signature != b"PCIR" && &signature != b"RGIS" && &signature != b"NPDS" {
            pr_info!("Invalid signature for PcirStruct, bytes: {:02x} {:02x} {:02x} {:02x}\n", data[16], data[17], data[18], data[19]);
            return Err(EINVAL);
        }


        Ok(PcirStruct {
            vendor_id: u16_from_u8s(data[2], data[3]),
            device_id: u16_from_u8s(data[4], data[5]),
            pcir_length: u16_from_u8s(data[9], data[8]),
            pcir_version: data[10],
            image_type: data[12],
            last_image: data[13],
            image_size: u16_from_u8s(data[15], data[14]),
            class_code,
            signature_offset,
            signature,
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
        if self.image_size > 0 {
            // Image size is in 512-byte blocks
            Ok(self.image_size as usize * 512)
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
    u16       pciDataStrucPtr;    //  18h: Pointer to PCI Data Structure
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
    /// Signature (0xAA55)
    pub signature: u16,
}

impl TryFrom<&[u8]> for PciRomHeader {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 4 {
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

        Ok(PciRomHeader {
            signature,
        })
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
        self.base().pcir.is_last()
    }
    
    /// Get the image size in bytes
    pub(crate) fn image_size_bytes(&self) -> Result<usize> {
        self.base().pcir.image_size_bytes()
    }
}

// Convert from BiosImageBase to BiosImage
impl<'a> TryFrom<BiosImageBase<'a>> for BiosImage<'a> {
    type Error = Error;

    fn try_from(base: BiosImageBase<'a>) -> Result<Self> {
        pr_info!("BiosImageBase called with: {:?}\n", base);
        match base.pcir.image_type {
            0x00 => {
                Ok(BiosImage::PciAt(base.try_into()?))
            },
            0x03 => Ok(BiosImage::Efi(EfiBiosImage { base })),
            0x70 => Ok(BiosImage::Nbsi(NbsiBiosImage { base })),
            0xE0 => Ok(BiosImage::FwSec(FwSecBiosImage { base })),
            _ => {
                pr_info!("Unknown BIOS image type {:#x}\n", base.pcir.image_type);
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
        pr_info!("BiosImageBase try_from called with: {:?}\n", data);
        // Ensure we have enough data for the ROM header
        if data.len() < 4 {
            pr_info!("Not enough data for ROM header\n");
            return Err(EINVAL);
        }

        // Parse the ROM header
        // let rom_header = PciRomHeader::try_from(&data[0..4])?;
        let rom_header = match PciRomHeader::try_from(&data[0..4]) {
            Ok(rom_header) => rom_header,
            Err(e) => {
                pr_info!("Failed to create PciRomHeader: {:?}\n", e);
                return Err(e);
            }
        };

        let pcir_data = &data[2..];
        // let pcir = PcirStruct::try_from(pcir_data)?;
        let pcir = match PcirStruct::try_from(pcir_data) {
            Ok(pcir) => pcir,
            Err(e) => {
                pr_info!("Failed to create PcirStruct: {:?}\n", e);
                return Err(e);
            }
        };
        Ok(BiosImageBase {
            rom_header,
            pcir,
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
