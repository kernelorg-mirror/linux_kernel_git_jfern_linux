// SPDX-License-Identifier: GPL-2.0

use kernel::{
    bindings, c_str,
    device_id::IdArray,
    pci,
    prelude::*,
    sync::Arc,
};

use crate::{gpu::Gpu};

pub(crate) struct NovaCoreDriver(Arc<NovaCoreData>);

unsafe impl Sync for NovaCoreDriver {}
unsafe impl Send for NovaCoreDriver {}

#[allow(dead_code)]
#[pin_data]
#[repr(C)]
pub(crate) struct NovaCoreData {
    #[pin]
    pub(crate) gpu: Gpu,
    pub(crate) pdev: pci::Device,
}

const BAR0_SIZE: usize = 8 * 1024 * 1024;
pub(crate) type Bar0 = pci::Bar<BAR0_SIZE>;

impl pci::Driver for NovaCoreDriver {
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
        pdev.enable_msi();

        let bar = Arc::new(pdev.iomap_region_sized::<BAR0_SIZE>(0, c_str!("nova"))?, GFP_KERNEL)?;
        let p = pdev.clone();

        let gpu = Gpu::new(&p, bar)?;

        let data = Arc::pin_init(try_pin_init!(NovaCoreData {
            gpu <- gpu,
            pdev: p,
        }), GFP_KERNEL)?;

        Ok(KBox::new(Self(data), GFP_KERNEL)?.into())
    }

    fn sriov_configure(
        _pdev: &mut pci::Device,
        _num_vfs: i32) -> Result<i32> {
        Err(EINVAL)
    }
}

impl Drop for NovaCoreDriver {
    fn drop(&mut self) {
        self.0.gpu.release();
        dev_dbg!(self.0.pdev.as_ref(), "Remove Nova GPU driver.\n");
    }
}
