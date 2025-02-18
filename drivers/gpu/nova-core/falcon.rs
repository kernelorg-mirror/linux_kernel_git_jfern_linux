
#![allow(dead_code)]

use crate::firmware::NvkmFirmware;
use kernel::prelude::*;
use kernel::sync::Arc;
use kernel::delay::sleep;
use kernel::bindings::dma_addr_t;
use core::time::Duration;
use crate::gpu::GpuBase;
use crate::gpu::Chipset;
use crate::timer::TimerWait;
use crate::chipsets_after;
use crate::chipsets_before;
use crate::{timer_nsec, timer_usec};
use crate::firmware::FlcnBlDmemDesc_v2;
use core::slice;

#[derive(Default,Debug)]
pub(crate) struct FalconFwSign {
    pub sig_base_prd: usize,
    pub sig_base_dbg: usize,
    sig_base_img: usize,
    sig_size: usize,
    pub sig_nr_prd: usize,
    sigs: KVec<u8>,
}

impl FalconFwSign {
    pub(crate) fn new(sig_base_img: usize,
		      sig_size: usize,
		      sig_nr_prd: usize,
		      sig_base_prd: usize,
		      sig_container_vec: &[u8]) -> Result<Self>
    {
	let mut sigs: KVec<u8> = Default::default();

	sigs.extend_from_slice(&sig_container_vec[sig_base_prd..sig_base_prd + (sig_nr_prd * sig_size)], GFP_KERNEL)?;
	Ok(Self {
	    sig_base_prd,
	    sig_base_dbg: 0,
	    sig_base_img,
	    sig_size,
	    sig_nr_prd,
	    sigs,
	})
    }
}

#[derive(Default,Debug)]
pub(crate) struct FalconFwInfo {
    pub fuse_ver: u32,
    pub engine_id: u32,
    pub ucode_id: u32,

    pub nmem_base_img: u32,
    pub nmem_base: u32,
    pub nmem_size: u32,

    pub imem_base_img: u32,
    pub imem_base: u32,
    pub imem_size: u32,

    pub dmem_base_img: u32,
    pub dmem_base: u32,
    pub dmem_size: u32,
    pub dmem_sign: u32,

    pub boot: KVec<u8>,
    pub boot_size: u32,
    pub boot_addr: u32,
}

pub(crate) struct FalconFw {
    pub fw: NvkmFirmware,
    pub falcon: Arc<Falcon>,
    pub sigs: FalconFwSign,
    pub info: FalconFwInfo,    
}

impl FalconFw {
    pub(crate) fn new_from_info(fw: NvkmFirmware,
                                falcon: &Arc<Falcon>,
				sigs: FalconFwSign,
				info: FalconFwInfo) -> Self {
	Self {
	    fw,
	    falcon: falcon.clone(),
	    sigs,
	    info,
	}
    }

    pub(crate) fn patch(&mut self, idx: u32, sig_base_src: u32) -> Result<()> {
	pr_info!("sigs {:?}", self.sigs);
	pr_info!("patching sigs {} size {}", self.sigs.sig_nr_prd, self.sigs.sig_size);

	let mut src : usize = (idx as u32 * self.sigs.sig_size as u32) as usize;
	let mut dst = self.sigs.sig_base_img;
	let len = self.sigs.sig_size / 4;
	pr_info!("patch idx:{} src:{:#x} dst {:#x}", idx, sig_base_src + src as u32, dst);
	for _i in 0..len {
	    let sig : u32 = u32::from_le_bytes(self.sigs.sigs[src..src+4].try_into().unwrap());

	    self.fw.dma.wr32(sig, dst)?;

	    src += 4;
	    dst += 4;
	}
	Ok(())
    }
    pub(crate) fn tu102_fw_signature(&self, sig_base_src: &mut u32) -> Result<u32> {
	let addr = self.falcon.debug;

	if addr == 0 {
	    Ok(0)
	} else {
	    self.falcon.enable()?;

	    if (self.falcon.rd32(0x408)? & 0x00100000) != 0 {
		*sig_base_src = self.sigs.sig_base_dbg as u32;
		Ok(1)
	    } else {
		Ok(0)
	    }
	}
    }

    pub(crate) fn boot(&self, mbox0: u32, mbox1: Option<u32>) -> Result<()> {

	pr_info!("attempting to boot falcon {} sigs: {}", self.fw.name, self.sigs.sig_nr_prd);

	self.falcon.reset()?;
	// reset

	// setup

	// load

	if chipsets_after!(&self.falcon.base.spec.chipset, GA102) {
	    self.dma_load()?;
	} else {
	    self.pio_load()?;
	}

	// boot
	self.boot_falcon(mbox0, mbox1, 0, 0)
    }

    pub(crate) fn boot_falcon(&self, mbox0: u32, mbox1: Option<u32>, mbox0_ok: u32, irqsclr: u32) -> Result<()> {

	if chipsets_after!(&self.falcon.base.spec.chipset, GA102) {
	    self.falcon.wr32(self.falcon.addr2 + 0x210, self.info.dmem_sign)?;
	    self.falcon.wr32(self.falcon.addr2 + 0x19c, self.info.engine_id)?;
	    self.falcon.wr32(self.falcon.addr2 + 0x198, self.info.ucode_id)?;
	    self.falcon.wr32(self.falcon.addr2 + 0x180, 0x1)?;
	}

	// GM200+
	self.falcon.wr32(0x40, mbox0)?;

	if !mbox1.is_none() {
	    self.falcon.wr32(0x44, mbox1.unwrap())?;
	}

	self.falcon.wr32(0x104, self.info.boot_addr)?;
	self.falcon.wr32(0x100, 0x2)?;

	self.falcon.wait_for_reg_bits_set(0x100, 0x10, 2000000)?;

	let mbox0 = self.falcon.rd32(0x40)?;
	let mbox1 = self.falcon.rd32(0x44)?;

	if mbox0 != mbox0_ok {
	    pr_info!("mbox {:x} {:x}", mbox0, mbox1);
	    return Err(EINVAL);
	}

	if irqsclr != 0 {
	    self.falcon.mask(0x4, 0xffffffff, irqsclr)?;
	}
	Ok(())
    }

    pub(crate) fn dma_load(&self) -> Result<()> {
	self.falcon.mask(0x624, 0x80, 0x80)?;
	self.falcon.wr32(0x10c, 0x0)?;
	self.falcon.mask(0x600, 0x00010007, (0 << 16) | (1 << 2) | 1)?;

	self.falcon.dma_wr(self.fw.dma.dma.dma_handle(), self.info.imem_base_img,
			   FalconMem::IMEM, self.info.imem_base, self.info.imem_size, true)?;

	self.falcon.dma_wr(self.fw.dma.dma.dma_handle(), self.info.dmem_base_img,
			   FalconMem::DMEM, self.info.dmem_base, self.info.dmem_size, false)?;
	Ok(())
    }

    //FALCON_DMAIDX_PHYS_SYS_NCOH     = 4,
    fn pio_load_bld(&self) -> Result<()> {
	let desc : FlcnBlDmemDesc_v2 = FlcnBlDmemDesc_v2 {
	    reserved: [0, 0, 0, 0],
	    signature: [0, 0, 0, 0],
	    ctx_dma: 4,
	    code_dma_base: self.fw.dma.dma.dma_handle(),
	    non_sec_code_off: self.info.nmem_base,
	    non_sec_code_size: self.info.nmem_size,
	    sec_code_off: self.info.imem_base,
	    sec_code_size: self.info.imem_size,
	    code_entry_point: 0,
	    data_dma_base: self.fw.dma.dma.dma_handle() + self.info.dmem_base_img as u64,
	    data_size: self.info.dmem_size,
	    argc: 0,
	    argv: 0,
	};

	let ptr = &desc as *const FlcnBlDmemDesc_v2 as *const u8;

	let desc_bytes: &[u8] = unsafe {
            slice::from_raw_parts(ptr, core::mem::size_of::<FlcnBlDmemDesc_v2>())
	};
	self.falcon.mask(0x600 + desc.ctx_dma * 4, 0x7, 0x5)?;

	self.falcon.pio_wr(desc_bytes,
			   FalconMem::DMEM, 0,
			   0, false)
    }

    pub(crate) fn pio_load(&self) -> Result<()> {
	self.falcon.mask(0x624, 0x80, 0x80)?;
	self.falcon.wr32(0x10c, 0x0)?;

	if self.info.boot.len() != 0 {

	    self.falcon.pio_wr(self.info.boot.as_slice(),
			       FalconMem::IMEM, self.falcon.code_limit - self.info.boot_size,
			       (self.info.boot_addr >> 8) as u16, false)?;
	    self.pio_load_bld()?;
	    return Ok(());
	}

	self.falcon.pio_wr(&self.fw.dma.get_slice(self.info.nmem_base_img as usize, self.info.nmem_size as usize),
			   FalconMem::IMEM, self.info.nmem_base, (self.info.nmem_base >> 8) as u16, false)?;

	self.falcon.pio_wr(&self.fw.dma.get_slice(self.info.imem_base_img as usize, self.info.imem_size as usize),
			   FalconMem::IMEM, self.info.imem_base, (self.info.imem_base >> 8) as u16, true)?;

	self.falcon.pio_wr(&self.fw.dma.get_slice(self.info.dmem_base_img as usize, self.info.dmem_size as usize),
			   FalconMem::DMEM, self.info.dmem_base, 0, false)?;
	Ok(())
    }

}

#[allow(unused)]
pub(crate) struct Falcon {
    pub addr: u32,
    pub addr2: u32,
    pub debug: u32,
    pub version: u32,
    pub secret: u32,
    pub code_limit: u32,
    pub code_ports: u32,
    pub data_limit: u32,
    pub data_ports: u32,
    pub riscv_irqmask: u32,
    pub reset_pmc: bool,
    pub base: Arc<GpuBase>,
}

#[derive(PartialEq)]
pub(crate) enum FalconMem {
    IMEM,
    DMEM,
//    EMEM
}

impl Falcon {
    pub(crate) fn new(base: &Arc<GpuBase>, addr: u32, addr2: u32, debug: u32, riscv_irqmask: u32, reset_pmc: bool) -> Result<Self> {
	let bar = base.bar.try_access().ok_or(ENXIO)?;
	let reg = bar.try_readl((addr + 0x12c) as usize)?;
	let version = reg & 0x0000000f;
	let secret = (reg >> 4) & 0x3;
	let code_ports = (reg >> 8) & 0xf;
	let data_ports = (reg >> 12) & 0xf;

	let reg = bar.try_readl((addr + 0x108) as usize)?;
	let code_limit = (reg & 0x00001ff) << 8;
	let data_limit = (reg & 0x0003fe00) >> 1;
	Ok(Self {
	    addr,
	    addr2,
	    debug,
	    version,
	    secret,
	    code_limit,
	    code_ports,
	    data_limit,
	    data_ports,
	    riscv_irqmask,
	    reset_pmc,
	    base: base.clone(),
	})
    }
    pub(crate) fn wait_for_reg_bits_set(&self, offset: u32, mask: u32, timeout_us: u64) -> Result<()> {
	let bar = self.base.bar.try_access().ok_or(ENXIO)?;
	let _init_val = bar.try_readl((self.addr + offset) as usize)?;
	if (timer_usec!({
	    if bar.try_readl((self.addr + offset) as usize)? & mask != 0 {
		break;
	    }
	}, timeout_us, &self.base.timer) < 0) {
	    pr_info!("timed out waiting for {}", offset);
	    return Err(ETIME);
	}
	Ok(())
    }

    pub(crate) fn wait_for_reg_bits_clear(&self, offset: u32, mask: u32, timeout_us: u64) -> Result<()> {
	let bar = self.base.bar.try_access().ok_or(ENXIO)?;
	let _init_val = bar.try_readl((self.addr + offset) as usize)?;
	if (timer_usec!({
	    if bar.try_readl((self.addr + offset) as usize)? & mask == 0 {
		break;
	    }
	}, timeout_us, &self.base.timer) < 0) {
	    pr_info!("timed out waiting for {}", offset);
	    return Err(ETIME);
	}
	Ok(())
    }
    
    pub(crate) fn rd32(&self, offset: u32) -> Result<u32> {
	let bar = self.base.bar.try_access().ok_or(ENXIO)?;
	bar.try_readl((self.addr + offset) as usize)
    }

    pub(crate) fn wr32(&self, offset: u32, val: u32) -> Result<()> {
	let bar = self.base.bar.try_access().ok_or(ENXIO)?;	
	bar.try_writel(val, (self.addr + offset) as usize)?;
	Ok(())
    }

    pub(crate) fn mask(&self, offset: u32, mask: u32, val: u32) -> Result<u32> {
	let bar = self.base.bar.try_access().ok_or(ENXIO)?;
	let temp = bar.try_readl((self.addr + offset) as usize)?;
	bar.try_writel((temp & !mask) | val, (self.addr + offset) as usize)?;
	Ok(temp)
    }
    
    pub(crate) fn dma_init(&self, dma_addr: dma_addr_t, xfer_len: u32, sec: bool,
		    mem_type: FalconMem, cmd: &mut u32) -> Result<()> {
	*cmd = (xfer_len.ilog2() - 2) << 8;
	if mem_type == FalconMem::IMEM {
	    *cmd |= 0x00000010;
	}
	if sec {
	    *cmd |= 0x00000004;
	}

	self.wr32(0x110, (dma_addr >> 8) as u32)?;
	self.wr32(0x128, 0)?;
	Ok(())
    }

    pub(crate) fn dma_xfer(&self, mem_base: u32, dma_base: u32, cmd: u32) -> Result<()> {
	self.wr32(0x114, mem_base)?;
	self.wr32(0x11c, dma_base)?;
	self.wr32(0x118, cmd)
    }

    pub(crate) fn dma_done(&self) -> Result<()> {
	self.wait_for_reg_bits_set(0x118, 0x2, 50000)
    }

    pub(crate) fn dma_wr(&self, dma_addr_arg: dma_addr_t, dma_base: u32,
		  mem_type: FalconMem, mem_base: u32, len: u32, sec: bool) -> Result<()> {
	let mut dma_start = 0;
	let mut dma_addr = dma_addr_arg;
	let mut cmd : u32 = 0;
	let dmalen : u32 = 256;

	if mem_type == FalconMem::DMEM {
	    dma_start = dma_base;
	    dma_addr += dma_base as u64;
	}

	self.dma_init(dma_addr, dmalen, sec, mem_type, &mut cmd)?;

	let mut dst = mem_base;
	let mut src = dma_base;
	let mut remain = len;

	while remain >= dmalen {
	    self.dma_xfer(dst, src - dma_start, cmd)?;

	    self.dma_done()?;

	    src += dmalen;
	    dst += dmalen;
	    remain -= dmalen;
	}
	Ok(())
    }

    pub(crate) fn pio_imem_wr_init(&self, port: u8, sec: bool, imem_base: u32) -> Result<()> {
	let sec_val = if sec { 1 << 28 } else { 0 };
	self.wr32(0x180 as u32 + (port as u32 * 0x10) as u32, sec_val | (1 << 24) | imem_base)
    }

    pub(crate) fn pio_imem_wr(&self, port: u8, img: &[u8], tag: u16) -> Result<()> {
	let mut remain = img.len();
	let mut offset = 0;
	pr_info!("imem_wr: remain {} {} {} {:#x}", port, img.len(), tag, img.as_ptr() as *const u8 as u64);

	self.wr32(0x188 + (port as u32 * 0x10) as u32, tag as u32)?;
	while remain >= 4 {
	    let this32 = u32::from_le_bytes(img[offset..offset+4].try_into().unwrap());
	    self.wr32((0x184 as u32 + (port as u32 * 0x10) as u32) as u32, this32)?;
	    remain -= 4;
	    offset += 4;
	}
	Ok(())
    }

    pub(crate) fn pio_dmem_wr_init(&self, port: u8, _sec: bool, dmem_base: u32) -> Result<()> {
	self.wr32(0x1c0 as u32 + (port as u32 * 8) as u32, (1 << 24) | dmem_base)
    }

    pub(crate) fn pio_dmem_wr(&self, port: u8, img: &[u8]) -> Result<()> {
	let mut remain = img.len();
	let mut offset = 0;
	pr_info!("dmem_wr: remain {} {} {:#x}", port, img.len(), img.as_ptr() as *const u8 as u64);
	while remain >= 4 {
	    let this32 = u32::from_le_bytes(img[offset..offset+4].try_into().unwrap());
	    self.wr32(0x1c4 as u32 + (port as u32 * 8) as u32, this32)?;
	    remain -= 4;
	    offset += 4;
	}
	Ok(())
    }


    pub(crate) fn pio_wr(&self, img: &[u8],
		  mem_type: FalconMem, mem_base: u32, tag: u16, sec: bool) -> Result<()> {
	let port = 0;
	match mem_type {
	    FalconMem::IMEM => {
		self.pio_imem_wr_init(port, sec, mem_base)?;
	    }
	    FalconMem::DMEM => {
		self.pio_dmem_wr_init(port, sec, mem_base)?;
	    }
	}

	let mut tag = tag;
	let mut len : usize = img.len() as usize;
	let mut offset : usize = 0;
	let pio_max = 0x100;
	pr_info!("pio_wr: remain {:#x} {} {} {} {}", mem_base, port, len, tag, sec);
	loop {
	    let xfer_len : usize = core::cmp::min::<usize>(len, pio_max);
	    match mem_type {
		FalconMem::IMEM => {
		    self.pio_imem_wr(port, &img[offset..offset+xfer_len], tag)?;
		}
		FalconMem::DMEM => {
		    self.pio_dmem_wr(port, &img[offset..offset+xfer_len])?;
		}
	    }

	    tag += 1;

	    offset += xfer_len;
	    len -= xfer_len;

	    if len == 0 {
		break;
	    }
	}
	Ok(())
    }

    pub(crate) fn intr_retrigger(&self) -> Result<()> {
	if chipsets_after!(&self.base.spec.chipset, GA100) {
	    self.wr32(0x3e8, 0x1)?;
	}
	Ok(())
    }

    pub(crate) fn tu102_riscv_active(&self) -> Result<bool> {
	Ok((self.rd32(self.addr2 + 0x240)? & 0x00000001) != 0)
    }

    pub(crate) fn ga102_riscv_active(&self) -> Result<bool> {
	Ok((self.rd32(self.addr2 + 0x388)? & 0x00000080) != 0)
    }

    pub(crate) fn riscv_active(&self) -> Result<bool> {
	if chipsets_after!(&self.base.spec.chipset, GA102) {
	    self.ga102_riscv_active()
	} else {
	    self.tu102_riscv_active()
	}
    }

    pub(crate) fn reset_prep(&self) -> Result<()> {

	if chipsets_before!(&self.base.spec.chipset, GA100) {
	    return Ok(());
	}

	self.rd32(0xf4)?;

	// this timesout according to nvidia driver
	let _ = self.wait_for_reg_bits_set(0xf4, 0x80000000, 150);
	Ok(())
    }

    pub(crate) fn reset_eng(&self) -> Result<()> {
	self.reset_prep()?;

	self.mask(0x3c0, 0x1, 0x1)?;

	sleep(Duration::from_micros(10));

	self.mask(0x3c0, 0x1, 0x0)?;

	self.reset_wait_mem_scrubbing()
    }

    pub(crate) fn tu102_reset_wait_mem_scrubbing(&self) -> Result<()> {
	self.mask(0x40, 0x0, 0x0)?;

	self.wait_for_reg_bits_clear(0x10c, 0x6, 10000)
    }

    pub(crate) fn ga102_reset_wait_mem_scrubbing(&self) -> Result<()> {
	self.mask(0x40, 0x0, 0x0)?;

	self.wait_for_reg_bits_clear(0xf4, 0x1000, 20000)
    }

    pub(crate) fn reset_wait_mem_scrubbing(&self) -> Result<()> {
	if chipsets_after!(&self.base.spec.chipset, GA102) {
	    self.ga102_reset_wait_mem_scrubbing()
	} else {
	    self.tu102_reset_wait_mem_scrubbing()
	}
    }

    pub(crate) fn select(&self) -> Result<()> {

	if chipsets_before!(&self.base.spec.chipset, GA100) {
	    return Ok(());
	}

	if self.rd32(self.addr2 + 0x668)? & 0x10 != 0x0 {
	    self.wr32(self.addr2 + 0x668, 0x0)?;

	    self.wait_for_reg_bits_set(self.addr2 + 0x668, 0x1, 10000)?;
	}
	Ok(())
    }

    pub(crate) fn disable(&self) -> Result<()> {
	self.select()?;

	self.mask(0x0048, 0x00000003, 0x00000000)?;
	self.wr32(0x0014, 0xffffffff)?;

	// reset_pmc
	if self.reset_pmc {
	    self.reset_prep()?;

	    // mc disable?
	}

	self.reset_eng()
    }

    pub(crate) fn enable(&self) -> Result<()> {
	self.reset_eng()?;

	self.select()?;

	if self.reset_pmc {
	    // mc enable?
	}

	// reset wait mem scrubbing
	self.reset_wait_mem_scrubbing()?;

	let bar = self.base.bar.try_access().ok_or(ENXIO)?;
	self.wr32(0x84, bar.readl(0))?;
	Ok(())
    }

    pub(crate) fn reset(&self) -> Result<()> {
	self.disable()?;
	self.enable()
    }


    pub(crate) fn v1_start(&self) -> Result<()> {
	let reg = self.rd32(0x100)?;
	if reg & 0x40 != 0 {
	    self.wr32(0x130, 0x2)
	} else {
	    self.wr32(0x100, 0x2)
	}
    }
}
    
