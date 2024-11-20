
use kernel::prelude::*;
use kernel::devres::Devres;
use kernel::delay::sleep;
use core::time::Duration;
use crate::driver::Bar0;

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
