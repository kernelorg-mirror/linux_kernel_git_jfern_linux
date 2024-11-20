
use kernel::prelude::*;
use kernel::devres::Devres;
use kernel::delay::sleep;
use core::time::Duration;
use crate::driver::Bar0;
use crate::gpu::GpuBase;
use crate::gpu::Chipset;
use crate::chipsets_after;

pub(crate) fn wait(bar: &Devres<Bar0>) -> Result<()> {
    let bar = bar.try_access().ok_or(ENXIO)?;
    let mut timeout = 50 + 2000;

    loop {
        if bar.try_readl(0x118128)? & 0x1 != 0 &&
           (bar.try_readl(0x118234)? & 0xff) == 0xff {
            return Ok(());
        }

        sleep(Duration::from_micros(2000));

        timeout -= 1;
        if timeout == 0 {
            break;
        }
    }
    Err(ETIME)
}

fn ga100_devinit_check_display_disable(bar: &Devres<Bar0>) -> Result<bool>
{
    let bar = bar.try_access().ok_or(ENXIO)?;
    let reg = bar.try_readl(0x820c04)?;

    Ok(reg & 0x1 != 0)
}

fn tu102_devinit_check_display_disable(bar: &Devres<Bar0>) -> Result<bool>
{
    let bar = bar.try_access().ok_or(ENXIO)?;
    let reg = bar.try_readl(0x021c04)?;

    Ok(reg & 0x1 != 0)
}

pub(crate) fn check_display_disable(base: &GpuBase) -> Result<bool> {
    if chipsets_after!(&base.spec.chipset, GA100) {
        ga100_devinit_check_display_disable(&base.bar)
    } else {
        tu102_devinit_check_display_disable(&base.bar)
    }
}

pub(crate) fn vidmem_size(gpu_base: &GpuBase) -> Result<u64> {
    let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;
    if chipsets_after!(&gpu_base.spec.chipset, GA102) {
        Ok((bar.try_readl(0x1183a4)? as u64) << 20)
    } else {
        let data = bar.try_readl(0x100ce0)?;
        let lmag = (data & 0x3f0) >> 4;
        let lsca = data & 0x0000000f;
        let size: u64 = (lmag as u64) << (lsca + 20);

        if (data & 0x40000000) != 0 {
            Ok(size / 16 * 15)
        } else {
            Ok(size)
        }
    }
}

pub(crate) fn vga_workspace_addr(gpu_base: &GpuBase, fb_size: u64, display_disabled: bool) -> Result<u64> {
    let bar = gpu_base.bar.try_access().ok_or(ENXIO)?;
    let base = fb_size - 0x100000;
    let mut addr: u64 = 0;

    if !display_disabled {
        addr = bar.try_readl(0x625f04)? as u64;
    }

    if (addr & 0x00000008) == 0 {
        return Ok(base);
    }

    addr = (addr & 0xffffff00) << 8;
    if addr < base {
        Ok(fb_size - 0x20000)
    } else {
        Ok(addr)
    }
}
