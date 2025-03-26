// SPDX-License-Identifier: GPL-2.0

use crate::falcon::{Falcon, FalconEngine};

pub(crate) struct Gsp;
impl FalconEngine for Gsp {
    const BASE: usize = 0x00110000;
}

pub(crate) type GspFalcon = Falcon<Gsp>;
