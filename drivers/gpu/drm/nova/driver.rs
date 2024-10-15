// SPDX-License-Identifier: GPL-2.0

use kernel::{
    bindings, c_str,
    device_id::IdArray,
    drm,
    drm::{drv, ioctl},
    pci,
    prelude::*,
    sync::Arc,
    types::ARef,
};

use crate::{file::File, gpu::Gpu};

pub(crate) struct NovaDriver(ARef<NovaDevice>);

/// Convienence type alias for the DRM device type for this driver
pub(crate) type NovaDevice = drm::device::Device<NovaDriver>;

#[allow(dead_code)]
#[pin_data]
pub(crate) struct NovaData {
    #[pin]
    pub(crate) gpu: Gpu,
    pub(crate) pdev: pci::Device,
}

const BAR0_SIZE: usize = 8;
pub(crate) type Bar0 = pci::Bar<BAR0_SIZE>;

const INFO: drm::drv::DriverInfo = drm::drv::DriverInfo {
    major: 0,
    minor: 0,
    patchlevel: 0,
    name: c_str!("nova"),
    desc: c_str!("Nvidia Graphics"),
    date: c_str!("20240227"),
};

impl pci::Driver for NovaDriver {
    type IdInfo = ();

    const ID_TABLE: pci::IdTable<Self::IdInfo> = &IdArray::new([(
        pci::DeviceId::new(bindings::PCI_VENDOR_ID_NVIDIA, bindings::PCI_ANY_ID as u32),
        (),
    )]);

    fn probe(
        pdev: &mut pci::Device,
        _id: &pci::DeviceId,
        _info: &Self::IdInfo,
    ) -> Result<Pin<KBox<Self>>> {
        dev_dbg!(pdev.as_ref(), "Probe Nova GPU driver.\n");

        pdev.enable_device_mem()?;
        pdev.set_master();

        let bar = pdev.iomap_region_sized::<BAR0_SIZE>(0, c_str!("nova"))?;

        let p = pdev.clone();
        let data = Arc::pin_init(
            try_pin_init!(NovaData {
                gpu <- Gpu::new(&p, bar)?,
                pdev: p,
            }),
            GFP_KERNEL,
        )?;

        let drm = drm::device::Device::<NovaDriver>::new(pdev.as_ref(), data.clone())?;
        drm::drv::Registration::new_foreign_owned(drm.clone(), 0)?;

        Ok(KBox::new(Self(drm), GFP_KERNEL)?.into())
    }
}

impl Drop for NovaDriver {
    fn drop(&mut self) {
        let data = self.0.data();

        dev_dbg!(data.pdev.as_ref(), "Remove Nova GPU driver.\n");
    }
}

#[vtable]
impl drm::drv::Driver for NovaDriver {
    type Data = Arc<NovaData>;
    type File = File;
    type Object = crate::gem::Object;

    const INFO: drm::drv::DriverInfo = INFO;
    const FEATURES: u32 = drv::FEAT_GEM;

    kernel::declare_drm_ioctls! {
        (NOVA_GETPARAM, drm_nova_getparam, ioctl::RENDER_ALLOW, File::get_param),
        (NOVA_GEM_CREATE, drm_nova_gem_create, ioctl::AUTH | ioctl::RENDER_ALLOW, File::gem_create),
        (NOVA_GEM_INFO, drm_nova_gem_info, ioctl::AUTH | ioctl::RENDER_ALLOW, File::gem_info),
    }
}
