use kernel::prelude::*;
use core::convert::TryFrom;
use crate::driver::Bar0;
use kernel::error::Result;
use kernel::devres::Devres;
use crate::regs::RomShadow;

/// Helper function to create u16 from two u8 values (little-endian)
pub fn u16_from_u8s(high: u8, low: u8) -> u16 {
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
    pub fn read_more(&mut self, bytes: u32) -> Result {
        with_bar!(self.bar0, |bar0| {
            // Get current length
            let current_len = self.data.len();

            // Read ROM data bytes push directly to vector
            for i in 0..bytes as usize {
                // Read a byte from the VBIOS ROM and push it to the data vector
                let rom_addr = ROM_OFFSET + current_len + i;
                let byte = bar0.readb(rom_addr);
                self.data.push(byte, GFP_KERNEL)?;
            }

            Ok(())
        })?
    }

    /// Read bytes at a specific offset, filling any gap
    pub fn read_more_at_offset(&mut self, offset: u32, bytes: u32) -> Result {
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

        Ok(vbios)
    }
}

/// PCI Data Structure as defined in PCI Firmware Specification
#[derive(Debug)]
pub struct PcirStruct {
    /// PCI Data Structure signature ("PCIR")
    pub signature: [u8; 4],
    /// PCI Vendor ID (e.g., 0x10DE for NVIDIA)
    pub vendor_id: u16,
    /// PCI Device ID
    pub device_id: u16,
    /// Size of this image in 512-byte blocks
    pub image_size: u16,
    /// Size of PCI Data Structure
    pub structure_len: u16,
    /// ROM image type (0x00 = PC-AT compatible, 0x03 = EFI, 0x70 = NBSI)
    pub code_type: u8,
    /// Last image indicator (0x00 = Not last image, 0x80 = Last image)
    pub last_image: u8,
    /// Class code (3 bytes, 0x03 for display controller)
    pub class_code: [u8; 3],
}

impl TryFrom<&[u8]> for PcirStruct {
    type Error = Error;

    fn try_from(data: &[u8]) -> Result<Self> {
        if data.len() < 18 {
            return Err(EINVAL);
        }

        let mut signature = [0u8; 4];
        signature.copy_from_slice(&data[0..4]);

        // Signature should be "PCIR"
        if &signature != b"PCIR" {
            return Err(EINVAL);
        }

        let mut class_code = [0u8; 3];
        class_code.copy_from_slice(&data[15..18]);

        Ok(PcirStruct {
            signature,
            vendor_id: u16_from_u8s(data[5], data[4]),
            device_id: u16_from_u8s(data[7], data[6]),
            image_size: u16_from_u8s(data[11], data[10]),
            structure_len: u16_from_u8s(data[13], data[12]),
            code_type: data[14],
            last_image: data[15],
            class_code,
        })
    }
}

impl PcirStruct {
    /// Check if this is the last image in the ROM
    pub fn is_last(&self) -> bool {
        self.last_image & 0x80 != 0
    }
}

/// BIOS Information Table (BIT) Header
#[derive(Debug, Clone, Copy)]
pub struct BitHeader {
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
    pub fn from_id(data: &[u8], token_size: u8, token_entries: u8, id: u8) -> Result<Self> {
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