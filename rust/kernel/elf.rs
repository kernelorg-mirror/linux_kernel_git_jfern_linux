// SPDX-License-Identifier: GPL-2.0

//! Simple ELF parser, useful for e.g. extracting relevant sections from loaded firmware.
//!
//! C headers: [`include/uapi/linux/elf.h`](srctree/uapi/include/linux/elf.h)

use core::ops::Deref;

use crate::bindings;
use crate::prelude::*;
use crate::transmute::FromBytes;

#[derive(Debug, Eq, PartialEq)]
#[repr(u32)]
enum ElfClass {
    Class32 = bindings::ELFCLASS32,
    Class64 = bindings::ELFCLASS64,
}

impl TryFrom<u8> for ElfClass {
    type Error = Error;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        let res = match value as u32 {
            bindings::ELFCLASS32 => Self::Class32,
            bindings::ELFCLASS64 => Self::Class64,
            _ => return Err(EINVAL),
        };

        Ok(res)
    }
}

const ELF_MAGIC: [u8; 4] = [
    bindings::ELFMAG0 as u8,
    bindings::ELFMAG1,
    bindings::ELFMAG2,
    bindings::ELFMAG3,
];

/// Wraps the passed `inner` type into an `outer` structure that implements [`FromBytes`] and
/// derefs into the inner type.
///
/// This is intended for local use with ELF structures for which any byte stream is valid.
macro_rules! frombytes_wrapper {
    ($(struct $outer:ident{$inner:ty};)*) => {
    $(
        #[repr(transparent)]
        #[derive(Clone, Copy)]
        struct $outer($inner);
        /// SAFETY: any set of values is a valid representation for this type.
        unsafe impl FromBytes for $outer {}
        impl Deref for $outer {
            type Target = $inner;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    )*
    };
}

// Make sure we can
frombytes_wrapper!(
    struct Elf32Ehdr { bindings::Elf32_Ehdr };
    struct Elf64Ehdr { bindings::Elf64_Ehdr };
    struct Elf32Shdr { bindings::Elf32_Shdr };
    struct Elf64Shdr { bindings::Elf64_Shdr };
);

/// Reinterprets a byte slice as a structure `T` and return a copy.
fn read_from_bytes<T: Copy + FromBytes>(data: &[u8]) -> Result<T> {
    if core::mem::size_of::<T>() > data.len() {
        Err(EINVAL)
    } else {
        // SAFETY: `data` is valid for reads and long enough to contain an instance of `T`.
        Ok(unsafe { core::ptr::read_unaligned(data.as_ptr() as *const T) })
    }
}

/// Converts a 32-bit section header to its 64-bit equivalent.
impl From<Elf32Shdr> for Elf64Shdr {
    fn from(shdr32: Elf32Shdr) -> Self {
        Elf64Shdr(bindings::Elf64_Shdr {
            sh_name: shdr32.sh_name,
            sh_type: shdr32.sh_type,
            sh_flags: shdr32.sh_flags as _,
            sh_addr: shdr32.sh_addr as _,
            sh_offset: shdr32.sh_offset as _,
            sh_size: shdr32.sh_size as _,
            sh_link: shdr32.sh_link,
            sh_info: shdr32.sh_info,
            sh_addralign: shdr32.sh_addralign as _,
            sh_entsize: shdr32.sh_entsize as _,
        })
    }
}

/// Safely obtain a sub-slice from `data`.
fn get_slice(data: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    offset
        .checked_add(size)
        .and_then(|end| data.get(offset..end))
        .ok_or(EOVERFLOW)
}

/// Parses an ELF binary from a byte slice.
#[derive(Debug)]
pub struct Parser<'a> {
    data: &'a [u8],
    class: ElfClass,
    shoff: u64,
    shentsize: u16,
    shnum: u16,
    shstrndx: u16,
}

impl<'a> Parser<'a> {
    /// Creates a new parser from a bytes array containing an ELF file.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        let ident = data.get(0..bindings::EI_NIDENT as usize).ok_or(EINVAL)?;

        // Validate ELF magic number.
        if ident[bindings::EI_MAG0 as usize..=bindings::EI_MAG3 as usize] != ELF_MAGIC {
            return Err(EINVAL);
        }

        let class = ElfClass::try_from(ident[bindings::EI_CLASS as usize])?;

        // Read the appropriate ELF header (32 or 64 bit).
        let (shoff, shentsize, shnum, shstrndx) = match class {
            ElfClass::Class64 => {
                if data.len() < core::mem::size_of::<Elf64Ehdr>() {
                    return Err(EINVAL);
                }
                let header: Elf64Ehdr = read_from_bytes(data)?;
                if header.e_shentsize as usize != core::mem::size_of::<Elf64Ehdr>() {
                    return Err(EINVAL);
                }
                (
                    header.e_shoff,
                    header.e_shentsize,
                    header.e_shnum,
                    header.e_shstrndx,
                )
            }
            ElfClass::Class32 => {
                if data.len() < core::mem::size_of::<Elf32Ehdr>() {
                    return Err(EINVAL);
                }
                let header: Elf32Ehdr = read_from_bytes(data)?;
                if header.e_shentsize as usize != core::mem::size_of::<Elf32Ehdr>() {
                    return Err(EINVAL);
                }
                (
                    header.e_shoff as u64,
                    header.e_shentsize,
                    header.e_shnum,
                    header.e_shstrndx,
                )
            }
        };

        // Basic validation of section header table parameters
        if shnum > 0 && shoff > 0 {
            let table_size = (shnum as u64)
                .checked_mul(shentsize as u64)
                .ok_or(EOVERFLOW)?;
            let table_end = shoff.checked_add(table_size).ok_or(EOVERFLOW)?;
            if table_end > data.len() as u64 {
                return Err(EINVAL);
            }
        } else if shnum > 0 && shoff == 0 {
            // Section headers declared but no offset provided.
            return Err(EINVAL);
        }

        Ok(Self {
            data,
            class,
            shoff,
            shentsize,
            shnum,
            shstrndx,
        })
    }

    /// Reads the section header at the given `index`, returning its representation as `Elf64_Shdr`.
    fn section_header(&self, index: u16) -> Result<Elf64Shdr> {
        if index >= self.shnum {
            return Err(EINVAL);
        }

        let offset = (index as u64)
            .checked_mul(self.shentsize as u64)
            .and_then(|r| r.checked_add(self.shoff))
            .ok_or(EOVERFLOW)? as usize;

        let header_slice = self.data.get(offset..).ok_or(EINVAL)?;

        match self.class {
            ElfClass::Class64 => read_from_bytes::<Elf64Shdr>(header_slice),
            ElfClass::Class32 => read_from_bytes::<Elf32Shdr>(header_slice).map(Into::into),
        }
    }

    /// Retrieves the raw byte data of the section header string table.
    fn string_table_data(&self) -> Result<&'a [u8]> {
        if self.shnum > 0 && (self.shstrndx == 0 || self.shstrndx >= self.shnum) {
            return Err(EINVAL);
        } else if self.shnum == 0 {
            return Ok(&self.data[0..0]);
        }
        let strtab_header = self.section_header(self.shstrndx)?;
        if strtab_header.sh_type != bindings::SHT_STRTAB {
            return Err(EINVAL);
        }
        let size = usize::try_from(strtab_header.sh_size).map_err(|_| EOVERFLOW)?;
        get_slice(self.data, strtab_header.sh_offset as usize, size)
    }

    /// Looks up a section name in the string table.
    fn section_name(&self, strtab: &'a [u8], name_offset: u32) -> Result<&'a str> {
        let name_bytes_with_suffix = strtab.get(name_offset as usize..).ok_or(EINVAL)?;
        let name_bytes = name_bytes_with_suffix
            .split(|&c| c == 0)
            .next()
            .unwrap_or(&[]);
        core::str::from_utf8(name_bytes).map_err(|_| EINVAL)
    }

    /// Returns an iterator over the sections.
    pub fn sections_iter(&'a self) -> Result<SectionsIterator<'a>> {
        let strtab_data = self.string_table_data()?;
        Ok(SectionsIterator {
            parser: self,
            strtab_data,
            current_index: 0,
        })
    }
}

/// Describes a single ELF section.
pub struct Section<'a> {
    /// Name of the section.
    pub name: &'a str,
    /// Type of the section (e.g., `SHT_PROGBITS`, `SHT_STRTAB`).
    pub type_: u32,
    /// Section flags (e.g., `SHF_ALLOC`, `SHF_EXECINSTR`).
    pub flags: u64,
    /// Virtual address of the section in memory.
    pub addr: u64,
    /// Byte slice containing the raw data of the section from the file.
    pub data: &'a [u8],
}

/// An iterator over the sections of an ELF file.
pub struct SectionsIterator<'a> {
    parser: &'a Parser<'a>,
    strtab_data: &'a [u8],
    current_index: u16,
}

impl<'a> Iterator for SectionsIterator<'a> {
    type Item = Result<Section<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_index >= self.parser.shnum {
            return None;
        }

        let index = self.current_index;
        self.current_index += 1;

        // Skip the NULL section header (index 0).
        if index == 0 {
            return self.next();
        }

        let header = match self.parser.section_header(index) {
            Ok(header) => header,
            Err(e) => return Some(Err(e)),
        };

        let section_name = match self.parser.section_name(self.strtab_data, header.sh_name) {
            Ok(name) => name,
            Err(e) => return Some(Err(e)),
        };

        let section_data = if header.sh_type == bindings::SHT_NOBITS {
            &self.parser.data[0..0]
        } else {
            match get_slice(
                self.parser.data,
                header.sh_offset as usize,
                header.sh_size as usize,
            ) {
                Ok(slice) => slice,
                Err(e) => return Some(Err(e)),
            }
        };

        Some(Ok(Section {
            name: section_name,
            type_: header.sh_type,
            flags: header.sh_flags,
            addr: header.sh_addr,
            data: section_data,
        }))
    }
}
