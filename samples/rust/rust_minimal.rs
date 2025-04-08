// SPDX-License-Identifier: GPL-2.0

//! Rust minimal sample.

use kernel::prelude::*;
use kernel::c_str;
use kernel::debugfs;

module! {
    type: RustMinimal,
    name: "rust_minimal",
    authors: ["Rust for Linux Contributors"],
    description: "Rust minimal sample",
    license: "GPL",
}

struct MyData {
    x: i32,
}

impl AsRef<[u8]> for MyData {
    fn as_ref(&self) -> &[u8] {
        // Convert `MyData` into a byte slice
        unsafe {
            core::slice::from_raw_parts(
                self as *const MyData as *const u8,
                core::mem::size_of::<MyData>(),
            )
        }
    }
}

struct RustMinimal {
    numbers: KVec<i32>,
    parent: debugfs::DebugfsEntry,
    entry: debugfs::Envelope<'static, debugfs::DebugfsBlobEntry>,
    data: KBox::<MyData>,
}

impl kernel::Module for RustMinimal {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("Rust minimal sample (init)\n");
        pr_info!("Am I built-in? {}\n", !cfg!(MODULE));

        let mut numbers = KVec::new();
        numbers.push(72, GFP_KERNEL)?;
        numbers.push(108, GFP_KERNEL)?;
        numbers.push(200, GFP_KERNEL)?;

        let parent = debugfs::DebugfsEntry::debugfs_create_dir(c_str!("my_debug_dir"), None)?;

        let mut data = KBox::<MyData>::new( MyData {x:3}, GFP_KERNEL)?;

        if let Ok(entry) =
            debugfs::DebugfsBlobEntry::new(c_str!("blob"), 0x664, Some(&parent), &mut data) {
            pr_info!("created debugfs entry\n");
            Ok(Self { numbers, parent, entry, data })
        } else {
            pr_info!("failed to create debugfs entry\n");
            return Err(EINVAL)
        }
    }
}

impl Drop for RustMinimal {
    fn drop(&mut self) {
        pr_info!("My numbers are {:?}\n", self.numbers);
        pr_info!("Rust minimal sample (exit)\n");
    }
}
