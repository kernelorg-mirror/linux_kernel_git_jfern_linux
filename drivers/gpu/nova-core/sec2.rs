
#![allow(dead_code)]

use crate::falcon::{FalconFw, FalconFwInfo, FalconFwSign, Falcon};
use crate::firmware::NvkmFirmware;
use crate::firmware::fw_rd32;
use crate::gpu::GpuBase;
use crate::gpu::Chipset;
use crate::chipsets_after;
use kernel::error::code::*;
use kernel::prelude::*;
use kernel::firmware::Firmware;
use kernel::device;
use kernel::sync::Arc;
use kernel::kvec;
use crate::dma::DmaObject;

use crate::firmware::{BinHdr, HsHeader_v2, HsLoadHeader_v2};

pub(crate) struct Sec2 {
    pub falcon: Arc<Falcon>,
}

impl Sec2 {
    pub(crate) fn new(gpu_base: &Arc<GpuBase>) -> Result<Self> {
	Ok(Self {
	    falcon: Arc::new(Falcon::new(gpu_base,
					 gpu_base.spec.gpu_consts.sec2_addr,
					 0x1000, 0x408, 0, true)?, GFP_KERNEL)?,
	})
    }
}

pub(crate) struct Sec2Fw {
    pub fw: FalconFw,
}

impl Sec2Fw {
    fn fill_falcon_fw_info_tu102(fwinfo: &mut FalconFwInfo, load_hdr_v2: &HsLoadHeader_v2, loc: u32) {
	fwinfo.nmem_base = load_hdr_v2.os_code_offset;
	fwinfo.nmem_size = load_hdr_v2.os_code_size;
	fwinfo.imem_base_img = load_hdr_v2.os_code_size;
	fwinfo.imem_base = load_hdr_v2.apps[0].0;
	fwinfo.imem_size = load_hdr_v2.apps[0].1;
	fwinfo.dmem_base_img = load_hdr_v2.os_data_offset;
	fwinfo.dmem_size = load_hdr_v2.os_data_size;
	fwinfo.dmem_sign = loc - load_hdr_v2.os_data_offset;
	fwinfo.boot_addr = load_hdr_v2.os_code_offset;
    }

    fn tu102_boot_fw_setup(dev: &device::Device, falcon: &Arc<Falcon>, fw: &Firmware, name: &'static str) -> Result<Self>
    {
	let load_hdr = BinHdr::from_fw(fw);
	let hs_hdr_v2 = HsHeader_v2::from_fw(fw, load_hdr.header_offset as usize);
	let loc = fw_rd32(fw, hs_hdr_v2.patch_loc as usize);
	let sig = fw_rd32(fw, hs_hdr_v2.patch_sig as usize);
	let cnt = fw_rd32(fw, hs_hdr_v2.num_sig as usize);

	let hs_load_hdr_v2 = HsLoadHeader_v2::from_fw(fw, hs_hdr_v2.header_offset as usize)?;
	let mut fw_info : FalconFwInfo = Default::default();

	Self::fill_falcon_fw_info_tu102(&mut fw_info, &hs_load_hdr_v2, loc);

	let dma : DmaObject = DmaObject::new_from_data(dev,
						       &fw.data()[(load_hdr.data_offset as usize)..(load_hdr.data_offset as usize+load_hdr.data_size as usize)], "boot fw")?;
	let nvfw = NvkmFirmware::new(name, dma);

	let sigs = FalconFwSign::new(loc as usize, (hs_hdr_v2.sig_prod_size / cnt) as usize,
				     cnt as usize,
				     (hs_hdr_v2.sig_prod_offset + sig) as usize,
				     &fw.data())?;

	let mut fw = FalconFw::new_from_info(nvfw,
					     falcon,
					     sigs,
					     fw_info);
	
	let mut sig_base_src : u32 = fw.sigs.sig_base_prd as u32;
	let idx = fw.tu102_fw_signature(&mut sig_base_src)?;

	if fw.sigs.sig_nr_prd != 0 {
	    fw.patch(idx, sig_base_src)?;
	}	
	Ok(Sec2Fw {
	    fw
	})
    }

    fn fill_falcon_fw_info_ga102(info: &mut FalconFwInfo, load_hdr_v2: &HsLoadHeader_v2, loc: u32,
    				     meta: KVec<u32>) {
	info.imem_base_img = load_hdr_v2.apps[0].0;
	info.imem_size = load_hdr_v2.apps[0].1;
	info.dmem_base_img = load_hdr_v2.os_data_offset;
	info.dmem_size = load_hdr_v2.os_data_size;
	info.dmem_sign = loc - load_hdr_v2.os_data_offset;
	info.boot_addr = load_hdr_v2.apps[0].0;
	info.fuse_ver = meta[0];
	info.engine_id = meta[1];
	info.ucode_id = meta[2];
    }

    fn ga102_boot_fw_setup(dev: &device::Device, falcon: &Arc<Falcon>, fw: &Firmware, name: &'static str) -> Result<Self> {
	let load_hdr = BinHdr::from_fw(fw);
	let hs_hdr_v2 = HsHeader_v2::from_fw(fw, load_hdr.header_offset as usize);
        pr_info!("{} load_hdr {:?}", name, load_hdr);
	pr_info!("{} hs_hdr_v2 {:?}", name, hs_hdr_v2);

	let loc = fw_rd32(fw, hs_hdr_v2.patch_loc as usize);
	let sig = fw_rd32(fw, hs_hdr_v2.patch_sig as usize);
	let cnt = fw_rd32(fw, hs_hdr_v2.num_sig as usize);

	let meta = kvec!(fw_rd32(fw, hs_hdr_v2.meta_data_offset as usize),
			 fw_rd32(fw, (hs_hdr_v2.meta_data_offset + 4) as usize),
			 fw_rd32(fw, (hs_hdr_v2.meta_data_offset + 8) as usize))?;
	
	let hs_load_hdr_v2 = HsLoadHeader_v2::from_fw(fw, hs_hdr_v2.header_offset as usize)?;
	pr_info!("{} hs_load_hdr_v2 {:?}", name, hs_load_hdr_v2);

	let mut fw_info : FalconFwInfo = Default::default();

	Self::fill_falcon_fw_info_ga102(&mut fw_info, &hs_load_hdr_v2, loc,
					meta);

	let sigs = FalconFwSign::new(loc as usize, (hs_hdr_v2.sig_prod_size / cnt) as usize,
				     cnt as usize,
				     (hs_hdr_v2.sig_prod_offset + sig) as usize,
				     fw.data())?;

	let dma : DmaObject = DmaObject::new_from_data(dev,
						       &fw.data()[load_hdr.data_offset as usize..(load_hdr.data_offset + load_hdr.data_size) as usize], name)?;

	let nvfw = NvkmFirmware::new(name, dma);

	let mut fw = FalconFw::new_from_info(nvfw, falcon, sigs, fw_info);
	let sig_base_src : u32 = fw.sigs.sig_base_prd as u32;
	let idx = Sec2Fw::ga102_fw_signature(&mut fw)?;

	if fw.sigs.sig_nr_prd != 0 {
	    fw.patch(idx, sig_base_src)?;
	}
	
	Ok(Self {
	    fw
	})
    }

    fn ga102_fw_signature(fw: &FalconFw) -> Result<u32> {
	let bar = fw.falcon.base.bar.try_access().ok_or(ENXIO)?;
	let mut reg_fuse_version;
	let idx : u32;
	if fw.info.engine_id & 0x1 != 0 {
	    reg_fuse_version = bar.try_readl((0x824140 + (fw.info.ucode_id - 1) * 4) as usize)?;
	} else if fw.info.engine_id & 0x4 != 0 {
	    reg_fuse_version = bar.try_readl((0x824100 + (fw.info.ucode_id - 1) * 4) as usize)?;
	} else if fw.info.engine_id & 0x400 != 0 {
	    reg_fuse_version = bar.try_readl((0x8241c0 + (fw.info.ucode_id - 1) * 4) as usize)?;
	} else {
	    return Err(EINVAL);
	}

	if reg_fuse_version != 0 {
	    reg_fuse_version = 32 - reg_fuse_version.leading_zeros();
	    if fw.info.fuse_ver < reg_fuse_version {
		return Err(EINVAL);
	    }

	    idx = fw.info.fuse_ver - reg_fuse_version;
	} else {
	    idx = (fw.sigs.sig_nr_prd - 1) as u32;
	}
	Ok(idx)
    }
    pub(crate) fn boot(&self, mbox0: u32, mbox1: Option<u32>) -> Result<()> {
	self.fw.boot(mbox0, mbox1)
    }

    pub(crate) fn new(dev: &device::Device, falcon: &Arc<Falcon>, fw: &Firmware, name: &'static str) -> Result<Self> {
	if chipsets_after!(&falcon.base.spec.chipset, GA102) {
	    Self::ga102_boot_fw_setup(dev, falcon, fw, name)
	} else {
	    Self::tu102_boot_fw_setup(dev, falcon, fw, name)
	}		
    }
}
