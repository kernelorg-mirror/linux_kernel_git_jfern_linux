#![allow(dead_code)]

use kernel::prelude::*;
use kernel::sync::{Arc, ArcBorrow, Mutex, new_mutex, UniqueArc};
use kernel::devres::Devres;
use kernel::irq;
use kernel::pci;
use crate::gpu::msi_rearm;
use crate::driver::Bar0;

const VFN_BASE: u32 = 0xb80000;
const VFN_NUM_MASKS: usize = 8;

pub(crate) trait VfnHandler {
    fn handle_vfn(&self) -> Result<u32>;
}

struct Inth {
    leaf: u32,
    mask: u32,
    handler: Arc<dyn VfnHandler>,
}

struct VfnInner {
    irq: Option<irq::Registration<Vfn>>,
}

#[pin_data]
pub(crate) struct Vfn {
    bar: Arc<Devres<Bar0>>,
    addr: u32,

    #[pin]
    inner: Mutex<VfnInner>,
    #[pin]
    handlers: Mutex<KVec<Inth>>,
}

impl irq::Handler for Vfn {
    type Data = Arc<Vfn>;
    fn handle_irq(vfn: ArcBorrow<'_, Vfn>) -> irq::Return {
        let _ = vfn.disarm();

        irq::Return::WakeThread
    }

    fn handle_thread_irq(vfn: ArcBorrow<'_, Vfn>) -> irq::Return {
        let _ = msi_rearm(&vfn.bar);

        let pending = vfn.pending();

        pr_info!("thread irq: pending {:?}\n", pending);

        let pending = match pending {
            Err(_) => { let _ = vfn.rearm(); return irq::Return::None; }
            Ok(x) => { x }
        };
        match pending {
            Some(x) => {
                let mut res = 0;
                let handlers = vfn.handlers.lock();
                for inth in handlers.iter() {
                    if x[inth.leaf as usize] & inth.mask != 0 {
                        let _ = vfn.reset(inth.leaf, inth.mask);
                        res = match inth.handler.handle_vfn() {
                            Err(_) => { let _ = vfn.rearm(); return irq::Return::None; }
                            Ok(x) => { x }
                        };
                    }
                }
                if res == 0 {
                    for i in 0..VFN_NUM_MASKS {
                        if x[i as usize] != 0 {
                            let _ = vfn.block(i as u32, x[i]);
                        }
                    }
                }
            },
            None => { let _ = vfn.rearm(); return irq::Return::None; }
        }

        let _ = vfn.rearm();
        irq::Return::Handled
    }
}

impl Vfn {
    pub(crate) fn new(bar: Arc<Devres<Bar0>>) -> Result<Arc<Self>> {

        let vfn = UniqueArc::pin_init(pin_init!(Self {
            bar,
            addr: VFN_BASE,
            inner <- new_mutex!(VfnInner { irq: None }),
            handlers <- new_mutex!(KVec::<Inth>::new()),
        }), GFP_KERNEL)?;

        Ok(vfn.into())
    }

    pub(crate) fn install_irq(self: &Arc<Self>, pdev: &pci::Device) -> Result<()> {
        let irq = pdev.request_irq::<Self>(0, self.clone(), format_args!("nova-core"))?;

        self.inner.lock().irq.replace(irq);
        Ok(())
    }

    pub(crate) fn unregister_irq(&self) {
        let registration = self.inner.lock().irq.take();
        drop(registration);
    }

    pub(crate) fn add_handler(&self, intr: u32, handler: Arc<dyn VfnHandler>) -> Result<()> {
        let (leaf, mask) = self.xlat(intr)?;
        self.handlers.lock().push(Inth {
            leaf, mask, handler
        }, GFP_KERNEL)?;
        Ok(())
    }

    fn rd32(&self, offset: u32) -> Result<u32> {
        let access = self.bar.try_access();
        let bar = access.ok_or(ENXIO)?;
        bar.try_readl((self.addr + offset) as usize)
    }

    fn wr32(&self, offset: u32, val: u32) -> Result<()> {
        let bar = self.bar.try_access().ok_or(ENXIO)?;
        bar.try_writel(val, (self.addr + offset) as usize)
    }

    fn xlat(&self, intr: u32) -> Result<(u32, u32)> {
        if intr >= (VFN_NUM_MASKS * core::mem::size_of::<u32>() * 8) as u32 {
            return Err(EINVAL);
        }
        Ok((intr / 32, (1 << (intr % 32))))
    }

    pub(crate) fn intr_allow(&self, intr: u32) -> Result <()> {
        let (leaf, mask) = self.xlat(intr)?;

        self.reset(leaf, mask)?;
        self.allow(leaf, mask)?;
        Ok(())
    }

    fn pending(&self) -> Result<Option<[u32; VFN_NUM_MASKS]>> {
        let intr_top = self.rd32(0x1600)?;
        let mut stat : [u32; 8] = Default::default();
        let mut pending: bool = false;
        for leaf in 0..VFN_NUM_MASKS {
            if intr_top & (1 << (leaf / 2)) != 0 {
                stat[leaf] = self.rd32((0x1000 + (leaf * 4)) as u32)?;
                if stat[leaf] != 0 {
                    pending = true;
                }
            }
        }
        if pending {
            Ok(Some(stat))
        } else {
            Ok(None)
        }
    }

    fn reset(&self, leaf: u32, mask: u32) -> Result<()> {
        self.wr32(0x1000 + (leaf * 4), mask)
    }

    fn allow(&self, leaf: u32, mask: u32) -> Result<()> {
        self.wr32(0x1200 + (leaf * 4), mask)
    }

    fn block(&self, leaf: u32, mask: u32) -> Result<()> {
        self.wr32(0x1400 + (leaf * 4), mask)
    }

    pub(crate) fn rearm(&self) -> Result<()> {
        self.wr32(0x1608, 0x0000000f)
    }

    pub(crate) fn disarm(&self) -> Result<()> {
        self.wr32(0x1610, 0x0000000f)
    }
}
