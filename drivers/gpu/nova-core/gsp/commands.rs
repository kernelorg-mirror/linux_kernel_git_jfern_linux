// SPDX-License-Identifier: GPL-2.0

use kernel::transmute::{AsBytes, FromBytes};

use crate::nvfw::r570_144 as fw;

unsafe impl FromBytes for fw::GspFwWprMeta {}
unsafe impl AsBytes for fw::GspFwWprMeta {}
unsafe impl FromBytes for fw::GspSystemInfo {}
unsafe impl AsBytes for fw::GspSystemInfo {}
