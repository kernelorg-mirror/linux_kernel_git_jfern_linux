// SPDX-License-Identifier: GPL-2.0

//! DebugFS abstraction
//!
//! C header: [`include/linux/debugfs.h`](srctree/include/linux/debugfs.h)

use kernel::prelude::*;
use kernel::{bindings, error::from_err_ptr};
use core::marker::PhantomData;

/// A DebugFS entry
pub struct DebugfsEntry {
    pub dentry: *mut crate::bindings::dentry,
}

// SAFETY: The pointer is only accessed in kernel context...etc, etc:
unsafe impl Send for DebugfsEntry {}
unsafe impl Sync for DebugfsEntry {}

impl DebugfsEntry {
    /// Create a new DebugFS directory
    pub fn debugfs_create_dir(name: &CStr, parent: Option<DebugfsEntry>) -> Result<Self> {
        let _parent: *mut bindings::dentry = match parent {
            // To do: probably should test for parent == PTR_ERR here
            Some(parent) => parent.dentry,
            None => core::ptr::null_mut(), // Root directory
        };

        pr_info!("creating debugfs entry\n");

        // The documentation states that driver should generally ignore errors from
        // the "create" functions, so as to transparently support systems where
        // debugfs is not enabled.  I don't know how to handle this in a Rust
        // constructor.  Right now, the constructor just returns an Err result on
        // failure.  Perhaps it should return an Option instead, and all other
        // constructors should ignore Options that are passed in?
        let dir = from_err_ptr(unsafe {
            crate::bindings::debugfs_create_dir(name.as_ptr(), _parent)
        })?;

        pr_info!("created debugfs directory\n");

        return Ok(Self { dentry: dir })
    }
}

impl Drop for DebugfsEntry {
    fn drop(&mut self) {
        pr_info!("deleting debugfs entry {:?}\n", self.dentry);
        unsafe {
            crate::bindings::debugfs_remove(self.dentry);
        }
    }
}

/// A debugfs binary blob
pub struct DebugfsBlobEntry {
    dentry: *mut crate::bindings::dentry,
    blob: bindings::debugfs_blob_wrapper, // Add the blob wrapper
}

pub struct Envelope<'a, T> {
    entry: KBox::<T>,
    _phantom: PhantomData<&'a ()>,
}

// SAFETY: The pointer is only accessed in kernel context...etc, etc:
unsafe impl Send for DebugfsBlobEntry {}
unsafe impl Sync for DebugfsBlobEntry {}

impl DebugfsBlobEntry {
    pub fn new<'a, T: AsRef<[u8]>>(
        name: &CStr,
        mode: bindings::umode_t,
        parent: Option<&DebugfsEntry>,
        data: &'a mut T,
    ) -> Result<Envelope<'a, Self>> {
        let _parent: *mut bindings::dentry = match parent {
            // To do: probably should test for parent == PTR_ERR here
            Some(parent) => parent.dentry,
            None => core::ptr::null_mut(), // Root directory
        };

        let mut entry = KBox::<DebugfsBlobEntry>::new(DebugfsBlobEntry {
            dentry: core::ptr::null_mut(), // Assign it to NULL for now
            blob: { bindings::debugfs_blob_wrapper {
                data: data as *mut T as *mut core::ffi::c_void, // Pointer to the data
                size: core::mem::size_of_val(data),
            }},
        }, GFP_KERNEL)?;

        entry.dentry = from_err_ptr(unsafe {
            // FIXME: Should this be using .into_raw() and consume entry.blob?
            // We need to preserve the memory for entry.blob because the kernel won't,
            // but we don't care about its contents any more.
            crate::bindings::debugfs_create_blob(name.as_ptr(), mode, _parent, &raw mut entry.blob)
        })?;

        Ok(Envelope {entry, _phantom: PhantomData::default()})
    }
}

impl Drop for DebugfsBlobEntry {
    fn drop(&mut self) {
        pr_info!("deleting debugfs entry {:?}\n", self.dentry);
        unsafe {
            crate::bindings::debugfs_remove(self.dentry);
        }
    }
}