// SPDX-License-Identifier: GPL-2.0

//! Memory management subsystems for nova-core.

#![expect(dead_code)]

pub(crate) mod pramin;

use kernel::sizes::SZ_4K;

use crate::num::u64_as_usize;

/// Page size in bytes (4 KiB).
pub(crate) const PAGE_SIZE: usize = SZ_4K;

bitfield! {
    pub(crate) struct VramAddress(u64), "Physical VRAM address in GPU video memory" {
        11:0    offset          as u64, "Offset within 4KB page";
        63:12   frame_number    as u64 => Pfn, "Physical frame number";
    }
}

impl VramAddress {
    /// Create a new VRAM address from a raw value.
    pub(crate) const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Get the raw address value as `usize` (useful for MMIO offsets).
    pub(crate) const fn raw(&self) -> usize {
        u64_as_usize(self.0)
    }

    /// Get the raw address value as `u64`.
    pub(crate) const fn raw_u64(&self) -> u64 {
        self.0
    }
}

impl PartialEq for VramAddress {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for VramAddress {}

impl PartialOrd for VramAddress {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VramAddress {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl From<Pfn> for VramAddress {
    fn from(pfn: Pfn) -> Self {
        Self::default().set_frame_number(pfn)
    }
}

bitfield! {
    pub(crate) struct VirtualAddress(u64), "Virtual address in GPU address space" {
        11:0    offset          as u64, "Offset within 4KB page";
        63:12   frame_number    as u64 => Vfn, "Virtual frame number";
    }
}

impl VirtualAddress {
    /// Create a new virtual address from a raw value.
    #[expect(dead_code)]
    pub(crate) const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Get the raw address value as `u64`.
    pub(crate) const fn raw_u64(&self) -> u64 {
        self.0
    }
}

impl From<Vfn> for VirtualAddress {
    fn from(vfn: Vfn) -> Self {
        Self::default().set_frame_number(vfn)
    }
}

/// Physical Frame Number.
///
/// Represents a physical page in VRAM.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pfn(u64);

impl Pfn {
    /// Create a new PFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VramAddress> for Pfn {
    fn from(addr: VramAddress) -> Self {
        addr.frame_number()
    }
}

impl From<u64> for Pfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Pfn> for u64 {
    fn from(pfn: Pfn) -> Self {
        pfn.0
    }
}

/// Virtual Frame Number.
///
/// Represents a virtual page in GPU address space.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Vfn(u64);

impl Vfn {
    /// Create a new VFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VirtualAddress> for Vfn {
    fn from(addr: VirtualAddress) -> Self {
        addr.frame_number()
    }
}

impl From<u64> for Vfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Vfn> for u64 {
    fn from(vfn: Vfn) -> Self {
        vfn.0
    }
}
