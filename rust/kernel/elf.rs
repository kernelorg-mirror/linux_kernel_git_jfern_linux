// SPDX-License-Identifier: GPL-2.0

//! ELF
//!
//! C header: [`include/linux/elf.h`](srctree/include/linux/elf.h)
use crate::{
    bindings,
    error::{Result, code::*},
    str::CStr,
};
use core::{ffi::c_char};

/// Serves as a generic interface for either a 32 or 64-bit ELF header
trait ElfHeader {
    /// The initial bytes mark the file as an object file.
    /// These bytes provide machine-independent data with which to decode and interpret
    /// the file's contents.
    fn ident(&self) -> &[u8];

    /// The number of entries in the section header table.
    fn shnum(&self) -> usize;

    /// The section header table index of the entry that is associated with the
    /// section name string table.
    fn shstrndx(&self) -> usize;

    /// The section header table's file offset in bytes.
    fn shoff(&self) -> usize;

    /// A section header's size in bytes.
    fn shentsize(&self) -> usize;
}

#[repr(transparent)]
struct E64(bindings::elf64_hdr);
#[repr(transparent)]
struct E32(bindings::elf32_hdr);

impl ElfHeader for E64 {
    fn ident(&self) -> &[u8] {
        &self.0.e_ident
    }

    fn shnum(&self) -> usize {
        self.0.e_shnum as usize
    }

    fn shstrndx(&self) -> usize {
        self.0.e_shstrndx as usize
    }

    fn shoff(&self) -> usize {
        self.0.e_shoff as usize
    }
    fn shentsize(&self) -> usize {
        self.0.e_shentsize as usize
    }
}

impl ElfHeader for E32 {
    fn ident(&self) -> &[u8] {
        &self.0.e_ident
    }

    fn shnum(&self) -> usize {
        self.0.e_shnum as usize
    }

    fn shstrndx(&self) -> usize {
        self.0.e_shstrndx as usize
    }

    fn shoff(&self) -> usize {
        self.0.e_shoff as usize
    }
    fn shentsize(&self) -> usize {
        self.0.e_shentsize as usize
    }
}

/// Serves as a generic interface for either a 32 or 64-bit ELF section header
trait SectionHeader {
    /// The byte offset from the beginning of the file to the first byte in the section
    fn sh_offset(&self) -> usize;

    /// The section's size in bytes.
    fn sh_size(&self) -> usize;

    /// An index into the section header string table section giving the location of
    /// a null-terminated string
    fn sh_name(&self) -> usize;
}

#[repr(transparent)]
struct E64sh(bindings::elf64_shdr);
#[repr(transparent)]
struct E32sh(bindings::elf32_shdr);

impl SectionHeader for E64sh {
    fn sh_offset(&self) -> usize {
        self.0.sh_offset as usize
    }
    fn sh_size(&self) -> usize {
        self.0.sh_size as usize
    }
    fn sh_name(&self) -> usize {
        self.0.sh_name as usize
    }
}
impl SectionHeader for E32sh {
    fn sh_offset(&self) -> usize {
        self.0.sh_offset as usize
    }
    fn sh_size(&self) -> usize {
        self.0.sh_size as usize
    }
    fn sh_name(&self) -> usize {
        self.0.sh_name as usize
    }
}

/// Corresponds to e_ident[] Identification Indexes
enum Eident
{
    EIMAG0 =  0,
    EIMAG1 =  1,
    EIMAG2 =  2,
    EIMAG3 =  3,
    EICLASS = 4,
}

/// Corresponds to e_ident[EI_CLASS], identifies the file's class, or capacity.
enum ElfClass
{
    Invalid = 0,
    Elf32 = 1,
    Elf64 = 2,
}

/// Represents an ELF-formatted binary data.
pub struct Elf<'a> {
    raw_buffer: &'a [u8],
}

impl<'a> Elf<'a> {
    /// Construct an Elf object from a binary image containing ELF-formatted data.
    ///
    /// # Invariants
    /// bytes must last at least `'a`.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        if bytes.is_empty() || Self::header_size(bytes) == 0 {
            return Err(EINVAL)
        }

        if Self::header_size(bytes) > bytes.len() {
            return Err(EOVERFLOW)
        }
        let e = Self::elf_header(bytes)?;
        // Check if first 4 bytes hold the ELF magic number (0x7fELF).
        if e.ident()[Eident::EIMAG0 as usize]  != 0x7f ||
            e.ident()[Eident::EIMAG1 as usize] != 'E' as u8 ||
            e.ident()[Eident::EIMAG2 as usize] != 'L' as u8 ||
            e.ident()[Eident::EIMAG3 as usize] != 'F' as u8 {
                return Err(EINVAL)
            }
        Ok(Self {raw_buffer: bytes,})
    }

    /// Returns the name of a section of this image located in index.
    pub  fn section_name(&self, index: usize) -> Result<&CStr>
    {
        let sec_hdr = self.section_hdr(index)?;
        let name = self.section_names()?[sec_hdr.sh_name()..].as_ptr() as *const c_char;
        // SAFETY: name is a valid pointer and is a view of the data wrapped by this
        // this object, hence has the same lifetime as this object.
        Ok(unsafe { CStr::from_char_ptr(name) })
    }

    /// Returns the data of a section of this image located in index.
    pub fn section_bytes(&self, index: usize) -> Result<&[u8]> {
        let sec_hdr = self.section_hdr(index)?;
        let offset = sec_hdr.sh_offset();
        let size = sec_hdr.sh_size();
        if (offset + size) > self.raw_buffer.len() {
            return Err(EOVERFLOW);
        }
        Ok(&self.raw_buffer[offset..offset + size])
    }

    /// Returns the total number of sections of this image
    pub fn section_header_count(&self) -> usize {
        match Self::elf_header(self.raw_buffer) {
            Ok(e) => return e.shnum(),
            Err(_) => return 0,
        };
    }

    fn section_names(&self) -> Result<&[u8]>
    {
        let e = Self::elf_header(self.raw_buffer)?;

        // Retrieve section header table entry containing the section name string table
        let sec_name_hdr = self.section_hdr(e.shstrndx())?;
        let name_offset = sec_name_hdr.sh_offset();
        let size = sec_name_hdr.sh_size();
        if (name_offset + size) > self.raw_buffer.len() {
            return Err(EOVERFLOW);
        }
        Ok(&self.raw_buffer[name_offset..name_offset + size])
    }

    fn section_hdr(&self, index: usize) -> Result<&dyn SectionHeader>
    {
        let e = Self::elf_header(self.raw_buffer)?;
        let size = self.section_header_count() * e.shentsize();
        if (e.shoff() + size) > self.raw_buffer.len() {
            return Err(EOVERFLOW);
        }
        let section_hdr = &self.raw_buffer[e.shoff()..e.shoff() + size];
        let h = Self::header_type(self.raw_buffer);
        match h {
            ElfClass::Elf64 => {
                return self.header_get::<E64sh>(section_hdr, index);
            },
            ElfClass::Elf32 => {
                return self.header_get::<E32sh>(section_hdr, index);
            },
            _ => Err(EINVAL),
        }
    }

    fn header_get<'b, U: SectionHeader + 'b>(
        &self,
        header: &[u8],
        index: usize) -> Result<&'b dyn SectionHeader>
    {
        if core::mem::size_of::<U>() > header.len() ||
            index >= self.section_header_count() {
            return Err(EOVERFLOW)
        }

        let section_hdr_base_ptr = header.as_ptr() as *const U;
        let sec_hdr = section_hdr_base_ptr.wrapping_add(index);
        // SAFETY: we just made sure the section header pointer is valid.
        return Ok(unsafe { &*(sec_hdr as *const U)});
    }

    fn elf_header(bytes: &[u8]) -> Result<&dyn ElfHeader>
    {
        let h = Self::header_type(bytes);
        match h {
            ElfClass::Elf64 => {
                let elf_hdr_ptr = bytes.as_ptr() as *const E64;
                // SAFETY: we've checked if elf_hdr_ptr is valid and properly initialized
                Ok(unsafe { &*elf_hdr_ptr })
            },
            ElfClass::Elf32 => {
                let elf_hdr_ptr = bytes.as_ptr() as *const E32;
                // SAFETY: we've checked if elf_hdr_ptr is valid and properly initialized
                Ok(unsafe { &*elf_hdr_ptr })
            },
            _ => Err(EINVAL),
        }
    }

    fn header_type(bytes: &[u8]) -> ElfClass {
        // At least should be sized ELF32 header
        if core::mem::size_of::<bindings::elf32_hdr>() > bytes.len() {
            return ElfClass::Invalid;
        }

        // Check e_ident[EI_CLASS], identifies the file's class, or capacity.
        let ei_class = bytes[Eident::EICLASS as usize];
        match ei_class {
            2 => ElfClass::Elf64,
            1 => ElfClass::Elf32,
            _ => ElfClass::Invalid
        }
    }

    fn header_size(bytes: &[u8]) -> usize {
        let h = Self::header_type(bytes);
        match h { ElfClass::Elf64 => core::mem::size_of::<bindings::elf64_hdr>(),
                  ElfClass::Elf32 => core::mem::size_of::<bindings::elf32_hdr>(),
                  _ => 0
        }
    }
}
