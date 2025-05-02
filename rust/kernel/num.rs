// SPDX-License-Identifier: GPL-2.0

//! Numerical and binary utilities for primitive types.

/// A trait providing alignment operations for `usize`.
pub trait Align {
    /// Aligns `self` upwards to the nearest multiple of `align`.
    fn align_up(self, align: Self) -> Self;
}

impl Align for usize {
    fn align_up(mut self, align: Self) -> Self {
        self = (self + align - 1) & !(align - 1);
        self
    }
}

impl Align for u64 {
    fn align_up(mut self, align: Self) -> Self {
        self = (self + align - 1) & !(align - 1);
        self
    }
}

/// Aligns `val` upwards to the nearest multiple of `align`.
pub const fn usize_align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}
