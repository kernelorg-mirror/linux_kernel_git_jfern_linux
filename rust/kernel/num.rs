// SPDX-License-Identifier: GPL-2.0

//! Numerical and binary utilities for primitive types.

/// A trait providing alignment operations for `usize`.
pub trait UsizeAlign {
    /// Aligns `self` upwards to the nearest multiple of `align`.
    fn align_up(self, align: usize) -> usize;
}

impl UsizeAlign for usize {
    fn align_up(mut self, align: usize) -> usize {
        self = (self + align - 1) & !(align - 1);
        self
    }
}

/// Aligns `val` upwards to the nearest multiple of `align`.
pub const fn usize_align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}
