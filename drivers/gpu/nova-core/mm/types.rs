// SPDX-License-Identifier: GPL-2.0

//! Page table data management for NVIDIA GPUs.
//!
//! This module provides data structures for GPU page table management, including
//! address types, page table entries (PTEs), page directory entries (PDEs), and
//! the page table hierarchy levels.
//!
//! # Examples
//!
//! ## Creating and writing a PDE
//!
//! ```no_run
//! let new_pde = Pde::default()
//!     .set_valid(true)
//!     .set_aperture(AperturePde::VideoMemory)
//!     .set_table_frame_number(new_table.frame_number());
//! // Call a function to write PDE to VRAM address
//! write_pde(pde_addr, new_pde)?;
//! ```
//!
//! ## Given a PTE, Get or allocate a PFN (page frame number).
//!
//! ```no_run
//! fn get_frame_number(pte_addr: VramAddress) -> Result<Pfn> {
//!     // Call a function to read 64-bit PTE value from VRAM address
//!     let pte = Pte(read_u64_from_vram(pte_addr)?);
//!     if pte.valid() {
//!         // Return physical frame number from existing mapping
//!         Ok(Pfn::new(pte.frame_number()))
//!     } else {
//!         // Create new PTE mapping
//!         // Call a function to allocate a physical page, returning a Pfn
//!         let phys_pfn = allocate_page()?;
//!         let new_pte = Pte::default()
//!             .set_valid(true)
//!             .set_frame_number(phys_pfn.raw())
//!             .set_aperture(AperturePte::VideoMemory)
//!             .set_privilege(false)   // User-accessible
//!             .set_read_only(false);  // Writable
//!
//!         // Call a function to write 64-bit PTE value to VRAM address
//!         write_u64_to_vram(pte_addr, new_pte.raw())?;
//!         Ok(phys_pfn)
//!     }
//! }
//! ```

#![expect(dead_code)]

/// Memory size constants
pub(crate) const KB: usize = 1024;
pub(crate) const MB: usize = KB * 1024;

/// Page size: 4 KiB
pub(crate) const PAGE_SIZE: usize = 4 * KB;

/// Page Table Level hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageTableLevel {
    Pdb, // Level 0 - Page Directory Base
    L1,  // Level 1
    L2,  // Level 2
    L3,  // Level 3 - Dual PDE (128-bit entries)
    L4,  // Level 4 - PTEs
}

impl PageTableLevel {
    /// Get the entry size for this level.
    pub(crate) fn entry_size(&self) -> usize {
        match self {
            Self::L3 => 16, // 128-bit dual PDE
            _ => 8,         // 64-bit PDE/PTE
        }
    }

    /// PDE levels constant array for iteration.
    const PDE_LEVELS: [PageTableLevel; 4] = [
        PageTableLevel::Pdb,
        PageTableLevel::L1,
        PageTableLevel::L2,
        PageTableLevel::L3,
    ];

    /// Get iterator over PDE levels.
    pub(crate) fn pde_levels() -> impl Iterator<Item = PageTableLevel> {
        Self::PDE_LEVELS.into_iter()
    }
}

/// Memory aperture for Page Directory Entries (PDEs)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AperturePde {
    #[default]
    Invalid = 0,
    VideoMemory = 1,
    SystemCoherent = 2,
    SystemNonCoherent = 3,
}

impl From<u8> for AperturePde {
    fn from(val: u8) -> Self {
        match val {
            1 => Self::VideoMemory,
            2 => Self::SystemCoherent,
            3 => Self::SystemNonCoherent,
            _ => Self::Invalid,
        }
    }
}

impl From<AperturePde> for u8 {
    fn from(val: AperturePde) -> Self {
        val as u8
    }
}

/// Memory aperture for Page Table Entries (PTEs)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AperturePte {
    #[default]
    VideoMemory = 0,
    PeerVideoMemory = 1,
    SystemCoherent = 2,
    SystemNonCoherent = 3,
}

impl From<u8> for AperturePte {
    fn from(val: u8) -> Self {
        match val {
            0 => Self::VideoMemory,
            1 => Self::PeerVideoMemory,
            2 => Self::SystemCoherent,
            3 => Self::SystemNonCoherent,
            _ => Self::VideoMemory,
        }
    }
}

impl From<AperturePte> for u8 {
    fn from(val: AperturePte) -> Self {
        val as u8
    }
}

/// Common trait for address types
pub(crate) trait Address {
    /// Get raw u64 value.
    fn raw(&self) -> u64;

    /// Convert an Address to a frame number.
    fn frame_number(&self) -> u64 {
        self.raw() >> 12
    }

    /// Get the frame offset within an Address.
    fn frame_offset(&self) -> u16 {
        (self.raw() & 0xFFF) as u16
    }
}

bitfield! {
    pub(crate) struct VramAddress(u64), "Physical VRAM address representation." {
        11:0    offset          as u16;    // Offset within 4KB page
        63:12   frame_number    as u64;    // Frame number
    }
}

impl Address for VramAddress {
    fn raw(&self) -> u64 {
        self.0
    }
}

impl From<Pfn> for VramAddress {
    fn from(pfn: Pfn) -> VramAddress {
        VramAddress::default().set_frame_number(pfn.raw())
    }
}

bitfield! {
    pub(crate) struct VirtualAddress(u64), "Virtual address representation for GPU." {
        11:0    offset      as u16;    // Offset within 4KB page
        20:12   l4_index    as u16;    // Level 4 index (PTE)
        29:21   l3_index    as u16;    // Level 3 index
        38:30   l2_index    as u16;    // Level 2 index
        47:39   l1_index    as u16;    // Level 1 index
        56:48   l0_index    as u16;    // Level 0 index (PDB)

        63:12   frame_number as u64;   // Frame number (combination of levels).
    }
}

impl VirtualAddress {
    /// Get index for a specific page table level.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let va = VirtualAddress::default();
    /// let pte_idx = va.level_index(PageTableLevel::L4);
    /// ```
    pub(crate) fn level_index(&self, level: PageTableLevel) -> u16 {
        match level {
            PageTableLevel::Pdb => self.l0_index(),
            PageTableLevel::L1 => self.l1_index(),
            PageTableLevel::L2 => self.l2_index(),
            PageTableLevel::L3 => self.l3_index(),
            PageTableLevel::L4 => self.l4_index(),
        }
    }
}

impl Address for VirtualAddress {
    fn raw(&self) -> u64 {
        self.0
    }
}

impl From<Vfn> for VirtualAddress {
    fn from(vfn: Vfn) -> VirtualAddress {
        VirtualAddress::default().set_frame_number(vfn.raw())
    }
}

bitfield! {
    pub(crate) struct Pte(u64), "Page Table Entry (PTE) to map virtual pages to physical frames." {
        0:0     valid           as bool;    // (1 = valid for PTEs)
        1:1     privilege       as bool;    // P - Privileged/kernel-only access
        2:2     read_only       as bool;    // RO - Write protection
        3:3     atomic_disable  as bool;    // AD - Disable atomic ops
        4:4     encrypted       as bool;    // E - Encryption enabled
        39:8    frame_number    as u64;     // PA[39:8] - Physical frame number (32 bits)
        41:40   aperture        as u8 => AperturePte;   // Memory aperture type.
        42:42   volatile        as bool;    // VOL - Volatile flag
        50:43   kind            as u8;      // K[7:0] - Compression/tiling kind
        63:51   comptag_line    as u16;     // CTL[12:0] - Compression tag line
    }
}

impl Pte {
    /// Set the physical address mapped by this PTE.
    pub(crate) fn set_address(&mut self, addr: VramAddress) {
        self.set_frame_number(addr.frame_number());
    }

    /// Get the physical address mapped by this PTE.
    pub(crate) fn address(&self) -> VramAddress {
        VramAddress::default().set_frame_number(self.frame_number())
    }

    /// Get raw u64 value.
    pub(crate) fn raw(&self) -> u64 {
        self.0
    }
}

bitfield! {
    pub(crate) struct Pde(u64), "Page Directory Entry (PDE) pointing to next-level page tables." {
        0:0     valid_inverted       as bool;    // V - Valid bit (0=valid for PDEs)
        2:1     aperture             as u8 => AperturePde;      // Memory aperture type
        3:3     volatile             as bool;    // VOL - Volatile flag
        39:8    table_frame_number   as u64;     // PA[39:8] - Table frame number (32 bits)
    }
}

impl Pde {
    /// Check if PDE is valid.
    pub(crate) fn is_valid(&self) -> bool {
        !self.valid_inverted() && self.aperture() != AperturePde::Invalid
    }

    /// The valid bit is inverted so add an accessor to flip it.
    pub(crate) fn set_valid(&self, value: bool) -> Pde {
        self.set_valid_inverted(!value)
    }

    /// Set the physical table address mapped by this PDE.
    pub(crate) fn set_table_address(&mut self, addr: VramAddress) {
        self.set_table_frame_number(addr.frame_number());
    }

    /// Get the physical table address mapped by this PDE.
    pub(crate) fn table_address(&self) -> VramAddress {
        VramAddress::default().set_frame_number(self.table_frame_number())
    }

    /// Get raw u64 value.
    pub(crate) fn raw(&self) -> u64 {
        self.0
    }
}

/// Dual PDE at Level 3 - 128-bit entry containing both LPT and SPT pointers.
/// Lower 64 bits = big/large page, upper 64 bits = small page.
///
/// # Example
///
/// ## Set the SPT (small page table) address in a Dual PDE
///
/// ```no_run
/// // Call a function to read dual PDE from VRAM address
/// let mut dual_pde: DualPde = read_dual_pde(dpde_addr)?;
/// // Call a function to allocate a page table and return its VRAM address
/// let spt_addr = allocate_page_table()?;
/// dual_pde.set_spt(Pfn::from(spt_addr), AperturePde::VideoMemory);
/// // Call a function to write dual PDE to VRAM address
/// write_dual_pde(dpde_addr, dual_pde)?;
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct DualPde {
    pub lpt: Pde, // Large/Big Page Table pointer (2MB pages) - bits 63:0 (lower)
    pub spt: Pde, // Small Page Table pointer (4KB pages) - bits 127:64 (upper)
}

impl DualPde {
    /// Create a new empty dual PDE.
    pub(crate) fn new() -> Self {
        Self {
            spt: Pde::default(),
            lpt: Pde::default(),
        }
    }

    /// Set the Small Page Table address with aperture.
    pub(crate) fn set_small_pt_address(&mut self, addr: VramAddress, aperture: AperturePde) {
        self.spt = Pde::default()
            .set_valid(true)
            .set_table_frame_number(addr.frame_number())
            .set_aperture(aperture);
    }

    /// Set the Large Page Table address with aperture.
    pub(crate) fn set_large_pt_address(&mut self, addr: VramAddress, aperture: AperturePde) {
        self.lpt = Pde::default()
            .set_valid(true)
            .set_table_frame_number(addr.frame_number())
            .set_aperture(aperture);
    }

    /// Check if has valid Small Page Table.
    pub(crate) fn has_small_pt_address(&self) -> bool {
        self.spt.is_valid()
    }

    /// Check if has valid Large Page Table.
    pub(crate) fn has_large_pt_address(&self) -> bool {
        self.lpt.is_valid()
    }

    /// Set SPT (Small Page Table) using Pfn.
    pub(crate) fn set_spt(&mut self, pfn: Pfn, aperture: AperturePde) {
        self.spt = Pde::default()
            .set_valid(true)
            .set_aperture(aperture)
            .set_table_frame_number(pfn.raw());
    }
}

/// Virtual Frame Number - virtual address divided by 4KB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Vfn(u64);

impl Vfn {
    /// Create a new VFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get raw frame number.
    pub(crate) const fn raw(&self) -> u64 {
        self.0
    }
}

impl From<VirtualAddress> for Vfn {
    fn from(vaddr: VirtualAddress) -> Self {
        Self(vaddr.frame_number())
    }
}

/// Physical Frame Number - physical address divided by 4KB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pfn(u64);

impl Pfn {
    /// Create a new PFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get raw frame number.
    pub(crate) const fn raw(&self) -> u64 {
        self.0
    }
}

impl From<VramAddress> for Pfn {
    fn from(addr: VramAddress) -> Self {
        Self(addr.frame_number())
    }
}
