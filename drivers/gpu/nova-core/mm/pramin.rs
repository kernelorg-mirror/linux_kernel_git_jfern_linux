// SPDX-License-Identifier: GPL-2.0

//! Direct VRAM access through PRAMIN window before page tables are set up.
//! PRAMIN can also write to system memory, however for simplicty we only
//! support VRAM access.
//!
//! # Examples
//!
//! ## Writing u32 data to VRAM
//!
//! ```no_run
//! use crate::driver::Bar0;
//! use crate::mm::pramin::PraminVram;
//!
//! fn write_data_to_vram(bar: &Bar0) -> Result {
//!     let pramin = PraminVram::new(bar);
//!     // Write 4 32-bit words to VRAM at offset 0x10000
//!     let data: [u32; 4] = [0xDEADBEEF, 0xCAFEBABE, 0x12345678, 0x87654321];
//!     pramin.write::<u32>(0x10000, &data)?;
//!     Ok(())
//! }
//! ```
//!
//! ## Reading bytes from VRAM
//!
//! ```no_run
//! use crate::driver::Bar0;
//! use crate::mm::pramin::PraminVram;
//!
//! fn read_data_from_vram(bar: &Bar0, buffer: &mut KVec<u8>) -> Result {
//!     let pramin = PraminVram::new(bar);
//!     // Read a u8 from VRAM starting at offset 0x20000
//!     pramin.read::<u8>(0x20000, buffer)?;
//!     Ok(())
//! }
//! ```

#![expect(dead_code)]

use crate::driver::Bar0;
use crate::regs;
use core::mem;
use kernel::prelude::*;

/// PRAMIN is a window into the VRAM (not a hardware block) that is used to access
/// the VRAM directly. These addresses are consistent across all GPUs.
const PRAMIN_BASE: usize = 0x700000; // PRAMIN is always at BAR0 + 0x700000
const PRAMIN_SIZE: usize = 0x100000; // 1MB aperture - max access per window position

/// Trait for types that can be read/written through PRAMIN.
pub(crate) trait PraminNum: Copy + Default + Sized {
    fn read_from_bar(bar: &Bar0, offset: usize) -> Result<Self>;

    fn write_to_bar(self, bar: &Bar0, offset: usize) -> Result;

    fn size_bytes() -> usize {
        mem::size_of::<Self>()
    }

    fn alignment() -> usize {
        Self::size_bytes()
    }
}

/// Macro to implement PraminNum trait for unsigned integer types.
macro_rules! impl_pramin_unsigned_num {
    ($bits:literal) => {
        ::kernel::macros::paste! {
            impl PraminNum for [<u $bits>] {
                fn read_from_bar(bar: &Bar0, offset: usize) -> Result<Self> {
                    bar.[<try_read $bits>](offset)
                }

                fn write_to_bar(self, bar: &Bar0, offset: usize) -> Result {
                    bar.[<try_write $bits>](self, offset)
                }
            }
        }
    };
}

impl_pramin_unsigned_num!(8);
impl_pramin_unsigned_num!(16);
impl_pramin_unsigned_num!(32);
impl_pramin_unsigned_num!(64);

/// Direct VRAM access through PRAMIN window before page tables are set up.
pub(crate) struct PraminVram<'a> {
    bar: &'a Bar0,
    saved_window_addr: usize,
}

impl<'a> PraminVram<'a> {
    /// Create a new PRAMIN VRAM accessor, saving current window state,
    /// the state is restored when the accessor is dropped.
    ///
    /// The BAR0 window base must be 64KB aligned but provides 1MB of VRAM access.
    /// Window is repositioned automatically when accessing data beyond 1MB boundaries.
    pub(crate) fn new(bar: &'a Bar0) -> Self {
        let saved_window_addr = Self::get_window_addr(bar);
        Self {
            bar,
            saved_window_addr,
        }
    }

    /// Set BAR0 window to point to specific FB region.
    ///
    /// # Arguments
    ///
    /// * `fb_offset` - VRAM byte offset where the window should be positioned.
    ///                 Must be 64KB aligned (lower 16 bits zero).
    fn set_window_addr(&self, fb_offset: usize) -> Result {
        // FB offset must be 64KB aligned (hardware requirement for window_base field)
        // Once positioned, the window provides access to 1MB of VRAM through PRAMIN aperture
        if fb_offset & 0xFFFF != 0 {
            return Err(EINVAL);
        }

        let window_reg = regs::NV_PBUS_BAR0_WINDOW::default().set_window_addr(fb_offset);
        window_reg.write(self.bar);

        // Read back to ensure it took effect
        let readback = regs::NV_PBUS_BAR0_WINDOW::read(self.bar);
        if readback.window_base() != window_reg.window_base() {
            return Err(EIO);
        }

        Ok(())
    }

    /// Get current BAR0 window offset.
    ///
    /// # Returns
    ///
    /// The byte offset in VRAM where the PRAMIN window is currently positioned.
    /// This offset is always 64KB aligned.
    fn get_window_addr(bar: &Bar0) -> usize {
        let window_reg = regs::NV_PBUS_BAR0_WINDOW::read(bar);
        window_reg.get_window_addr()
    }

    /// Common logic for accessing VRAM data through PRAMIN with windowing.
    ///
    /// # Arguments
    ///
    /// * `fb_offset` - Starting byte offset in VRAM (framebuffer) where access begins.
    ///                 Must be aligned to `T::alignment()`.
    /// * `num_items` - Number of items of type `T` to process.
    /// * `operation` - Closure called for each item to perform the actual read/write.
    ///                 Takes two parameters:
    ///                 - `data_idx`: Index of the item in the data array (0..num_items)
    ///                 - `pramin_offset`: BAR0 offset in the PRAMIN aperture to access
    ///
    /// The function automatically handles PRAMIN window repositioning when accessing
    /// data that spans multiple 1MB windows.
    fn access_vram<T: PraminNum, F>(
        &self,
        fb_offset: usize,
        num_items: usize,
        mut operation: F,
    ) -> Result
    where
        F: FnMut(usize, usize) -> Result,
    {
        // FB offset must be aligned to the size of T
        if fb_offset & (T::alignment() - 1) != 0 {
            return Err(EINVAL);
        }

        let mut offset_bytes = fb_offset;
        let mut remaining_items = num_items;
        let mut data_index = 0;

        while remaining_items > 0 {
            // Align the window to 64KB boundary
            let target_window = offset_bytes & !0xFFFF;
            let window_offset = offset_bytes - target_window;

            // Set window if needed
            if target_window != Self::get_window_addr(self.bar) {
                self.set_window_addr(target_window)?;
            }

            // Calculate how many items we can access from this window position
            // We can access up to 1MB total, minus the offset within the window
            let remaining_in_window = PRAMIN_SIZE - window_offset;
            let max_items_in_window = remaining_in_window / T::size_bytes();
            let items_to_write = core::cmp::min(remaining_items, max_items_in_window);

            // Process data through PRAMIN
            for i in 0..items_to_write {
                // Calculate the byte offset in the PRAMIN window to write to.
                let pramin_offset_bytes = PRAMIN_BASE + window_offset + (i * T::size_bytes());
                operation(data_index + i, pramin_offset_bytes)?;
            }

            // Move to next chunk.
            data_index += items_to_write;
            offset_bytes += items_to_write * T::size_bytes();
            remaining_items -= items_to_write;
        }

        Ok(())
    }

    /// Generic write for data to VRAM through PRAMIN.
    ///
    /// # Arguments
    ///
    /// * `fb_offset` - Starting byte offset in VRAM where data will be written.
    ///                 Must be aligned to `T::alignment()`.
    /// * `data` - Slice of items to write to VRAM. All items will be written sequentially
    ///            starting at `fb_offset`.
    pub(crate) fn write<T: PraminNum>(&self, fb_offset: usize, data: &[T]) -> Result {
        self.access_vram::<T, _>(fb_offset, data.len(), |data_idx, pramin_offset| {
            data[data_idx].write_to_bar(self.bar, pramin_offset)
        })
    }

    /// Generic read data from VRAM through PRAMIN.
    ///
    /// # Arguments
    ///
    /// * `fb_offset` - Starting byte offset in VRAM where data will be read from.
    ///                 Must be aligned to `T::alignment()`.
    /// * `data` - Mutable slice that will be filled with data read from VRAM.
    ///            The number of items read equals `data.len()`.
    pub(crate) fn read<T: PraminNum>(&self, fb_offset: usize, data: &mut [T]) -> Result {
        self.access_vram::<T, _>(fb_offset, data.len(), |data_idx, pramin_offset| {
            data[data_idx] = T::read_from_bar(self.bar, pramin_offset)?;
            Ok(())
        })
    }
}

impl<'a> Drop for PraminVram<'a> {
    fn drop(&mut self) {
        let _ = self.set_window_addr(self.saved_window_addr); // Restore original window.
    }
}
