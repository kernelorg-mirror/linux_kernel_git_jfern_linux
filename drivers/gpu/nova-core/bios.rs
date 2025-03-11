#![allow(dead_code)]

use kernel::{
    devres::Devres,
    prelude::*,
    kvec,
    revocable::RevocableGuard,
};

use crate::driver::Bar0;

const PROM_OFFSET: usize = 0x300000;

#[repr(C)]
#[derive(Default)]
struct BitEntry {
    id: u8,
    version: u8,
    length: u16,
    offset: u16,
}

#[allow(dead_code)]
#[derive(Default)]
struct BiosPcirT {
    vendor_id: u16,
    device_id: u16,
    class_code: [u8; 3],
    image_size: u32,
    image_rev: u16,
    image_type: u8,
    last: bool,
}

#[derive(Default)]
struct BiosNpdeT {
    image_size: u32,
    last: bool,
}

#[derive(Default)]
struct BiosPmuE {
    pmutype: u8,
    data: u32,
}

#[derive(Default)]
struct BiosImage {
    base: usize,
    size: usize,
    itype: u8,
    last: bool,
}

pub(crate) struct Bios {
    image0_size: isize,
    imaged_addr: usize,

    bmp_offset: usize,
    bit_offset: usize,

    pub bios_vec: KVec<u8>,
}


impl Bios {
    pub(crate) fn new() -> Self
    {
        Self {
            image0_size : 0,
            imaged_addr : 0,
            bmp_offset : 0,
            bit_offset : 0,
            bios_vec: Default::default(),
        }
    }

    
    fn rd32(&self, offset: isize) -> u32 {
        let mut addr = offset;
        if addr >= self.image0_size && self.imaged_addr != 0 {
            addr -= self.image0_size;
            addr += self.imaged_addr as isize;
        }
        let ptr: *const u32 = (self.bios_vec.as_ptr() as usize + addr as usize) as *const u32;
        unsafe { core::ptr::read_unaligned(ptr) }
    }

    fn rd16(&self, offset: isize) -> u16 {
        let mut addr = offset;
        if addr >= self.image0_size && self.imaged_addr != 0 {
            addr -= self.image0_size;
            addr += self.imaged_addr as isize;
        }
        let ptr: *const u16 = (self.bios_vec.as_ptr() as usize + addr as usize) as *const u16;
        unsafe { core::ptr::read_unaligned(ptr) }
    }

    fn rd08(&self, offset: isize) -> u8 {
        let mut addr = offset;
        if addr >= self.image0_size && self.imaged_addr != 0 {
            addr -= self.image0_size;
            addr += self.imaged_addr as isize;
        }
        let ptr: *const u8 = self.bios_vec.as_ptr() as *const u8;
        unsafe { *(ptr.offset(addr)) }
    }

    pub(crate) fn ptr(&self, offset: isize) -> *const u8 {
        let mut addr = offset;
        if addr >= self.image0_size && self.imaged_addr != 0 {
            addr -= self.image0_size;
            addr += self.imaged_addr as isize;
        }
        (self.bios_vec.as_ptr() as usize + addr as usize) as *const u8
    }

    fn findbytes(&self, needle: &KVec<u8>) -> usize
    {
        for i in 0..self.bios_vec.len()-needle.len() {
            let mut found = false;
            for j in 0..needle.len() {
                if self.bios_vec[i + j] != needle[j] {
                    break;
                }
                if j == needle.len() - 1 {
                    found = true;
                }
            };
            if found {
                return i;
            }
        }
        0
    }

    fn bit_entry(bios: &Bios, id: u8, bit_entry: &mut BitEntry) -> Result<()>
    {
        let mut entries = bios.rd08((bios.bit_offset + 0x0a) as isize);
        let rec_size = bios.rd08((bios.bit_offset + 0x09) as isize);
        let mut entry = (bios.bit_offset + 0x0c) as isize;

        while { let tmp = entries; entries -= 1; tmp != 0 } {
            let idx = bios.rd08(entry);
            if idx == id {
                bit_entry.id = bios.rd08(entry);
                bit_entry.version = bios.rd08(entry + 1);
                bit_entry.length = bios.rd16(entry + 2);
                bit_entry.offset = bios.rd16(entry + 4);
                return Ok(());
            }
            entry += (rec_size as usize) as isize;
        }
        Err(EINVAL)
    }

    fn pmu_te(bios: &Bios, ver: &mut u8, hdr: &mut u8, cnt: &mut u8, len: &mut u8) -> Result<u32>
    {
        let mut bit_p: BitEntry = Default::default();

        let mut data = 0;

        let _ = Self::bit_entry(bios, 112_u8, &mut bit_p)?;
        if bit_p.id != 112_u8 {
            return Ok(0);
        }

        if bit_p.version == 2 && bit_p.length >= 4 {
            data = bios.rd32(bit_p.offset as isize);
        }
        if data != 0 {
            *ver = bios.rd08(data as isize);
            *hdr = bios.rd08((data + 0x01) as isize);
            *len = bios.rd08((data + 0x02) as isize);
            *cnt = bios.rd08((data + 0x03) as isize);
        }
        Ok(data)
    }

    fn pmu_ee(bios: &Bios, idx: u8, ver: &mut u8, hdr: &mut u8) -> Result<u32>
    {
        let mut cnt: u8 = 0;
        let mut len: u8 = 0;
        let mut data = Self::pmu_te(bios, ver, hdr, &mut cnt, &mut len)?;
        if data != 0 && idx < cnt {
            data = data + (*hdr as u32) + ((idx as u32 * len as u32) as u32);
            *hdr = len;
            return Ok(data);
        }
        Ok(0)
    }

    fn pmu_ep(bios: &Bios, idx: u8, ver: &mut u8, hdr: &mut u8, info: &mut BiosPmuE) -> Result<u32>
    {
        let data = Self::pmu_ee(bios, idx, ver, hdr)?;
        if data != 0 {
            info.pmutype = bios.rd08(data as isize);
            info.data = bios.rd32((data + 0x02) as isize);
        }
        Ok(data as u32)
    }

    fn pcir_te(bios: &Bios, offset: isize, ver: &mut u8, hdr: &mut u16) -> Result<u32>
    {
        let mut data = bios.rd16(offset + 0x18) as u32;
        if data != 0 {
            data += offset as u32;
            match bios.rd32(data as isize) {
                0x52494350 | 0x53494752 | 0x5344504e => {
                    *hdr = bios.rd16((data + 0x0a) as isize);
                    *ver = bios.rd08((data + 0x0c) as isize);
                }
                _ => {
                    pr_info!("{:#x} Unknown PCIR signature {:#x}\n",
                              data, bios.rd32(data as isize));
                    *hdr = 0;
                    *ver = 0;
                    return Err(EINVAL);
                }
            }
        }
        Ok(data as u32)
    }

    fn pcir_tp(bios: &Bios, offset: isize, ver: &mut u8, hdr: &mut u16, info: &mut BiosPcirT) -> Result<u32>
    {
        let data = Self::pcir_te(bios, offset, ver, hdr)?;
        if data != 0 {
            info.vendor_id = bios.rd16((data + 0x04) as isize);
            info.device_id = bios.rd16((data + 0x06) as isize);
            info.class_code[0] = bios.rd08((data + 0x0d) as isize);
            info.class_code[1] = bios.rd08((data + 0x0e) as isize);
            info.class_code[2] = bios.rd08((data + 0x0f) as isize);
            info.image_size = (bios.rd16((data + 0x10) as isize) as u32) * 512;
            info.image_type = bios.rd08((data + 0x14) as isize);
            info.last = bios.rd08((data + 0x15) as isize) != 0;
        }
        Ok(data as u32)
    }

    fn npde_te(bios: &Bios, offset: isize) -> Result<u32>
    {
        let mut pcir: BiosPcirT = Default::default();
        let mut ver: u8 = 0;
        let mut hdr: u16 = 0;
        let mut data = Self::pcir_tp(bios, offset, &mut ver, &mut hdr, &mut pcir)?;
        data = (data + (hdr as u32) + 0x0f) & !0x0f;
        if data != 0 {
            match bios.rd32(data as isize) {
                0x4544504e => {}
                _ => {
                    pr_info!("{:#x} Unknown NPDE signature {:#x}\n",
                             data, bios.rd32(data as isize));
                    data = 0;
                }
            }
        }
        Ok(data)
    }

    fn npde_tp(bios: &Bios, offset: isize, info: &mut BiosNpdeT) -> Result<u32>
    {
        let data = Self::npde_te(bios, offset)?;
        if data != 0 {
            info.image_size = (bios.rd16((data + 0x08) as isize) as u32) * 512;
            info.last = (bios.rd08((data + 0x0a) as isize) & 0x80) != 0;
        }
        Ok(data)
    }

    fn imagen(bios: &Bios, image: &mut BiosImage) -> Result<bool>
    {
        let data = bios.rd16(image.base as isize);

        match data {
            0xaa55 | 0xbb77 | 0x4e56 => {}
            x => { pr_info!("{:#x}: ROM signature unknown {:#x}", image.base, x); return Ok(false); }
        };

        let mut pcir: BiosPcirT = Default::default();
        let mut ver: u8 = 0;
        let mut hdr: u16 = 0;

        let data = Self::pcir_tp(bios, image.base as isize, &mut ver, &mut hdr, &mut pcir)?;
        if data == 0 {
            return Ok(false);
        }
        image.size = pcir.image_size as usize;
        image.itype = pcir.image_type;
        image.last = pcir.last;

        if pcir.image_type != 0x70 {
            let mut npde: BiosNpdeT = Default::default();
            let data = Self::npde_tp(bios, image.base as isize, &mut npde)?;
            if data != 0 {
                image.size = npde.image_size as usize;
                image.last = npde.last;
            }
        } else {
            image.last = true;
        }
        Ok(true)
    }

    fn fetch(vec: &mut KVec<u8>, bar: &RevocableGuard<'_, Bar0>, offset: usize, length: usize) -> Result<()> {
        vec.resize(offset + length, 0, GFP_KERNEL)?;
        for i in (offset..offset+length).step_by(4) {
            let ptr: *mut u32 = vec.as_mut_ptr() as *mut u32;
            unsafe { *(ptr.offset((i / 4) as isize)) = bar.try_readl(PROM_OFFSET + i)?; }
        }
	Ok(())
    }

    pub(crate) fn probe(&mut self, bar: &Devres<Bar0>) -> Result<()> {
        let bar = bar.try_access().ok_or(ENXIO)?;	
        /* just do PROM probe */
        /* hardcoded lots */
        let mut data = bar.readl(0x88000 + 0x50);
        data &= !0x00000001;
        bar.writel(data, 0x88000 + 0x50);

	let mut image: BiosImage = Default::default();
        let mut idx = 0;
        let mut first_e0_done = false;

	loop {
            Self::fetch(&mut self.bios_vec, &bar, image.base, image.base + 4096)?;
            let mut imaged_addr = self.imaged_addr;
            self.imaged_addr = 0;
            if Self::imagen(&self, &mut image)? == false {
                self.imaged_addr = imaged_addr;
                break;
            }
            self.imaged_addr = imaged_addr;
            if idx == 0 {
                self.image0_size = image.size as isize;
            }

            if image.size > 0x100000 {
                break;
            }

            if image.itype == 0xe0 && first_e0_done == false {
                self.imaged_addr = image.base;
                imaged_addr = self.imaged_addr;
                first_e0_done = true;
            }

            Self::fetch(&mut self.bios_vec, &bar, image.base, image.size)?;

            pr_info!("Bios image.size {:#x} {:#x} {} {} {:#x}", image.base, image.itype, image.size, first_e0_done, self.imaged_addr);

            if image.last {
                if !first_e0_done {
                    self.imaged_addr = imaged_addr;
                }
                break;
            }

            image.base += image.size;
            if image.base > 0x100000 {
                break;
            }
            idx += 1;
        }

        pr_info!("Imaged addr {:#x}", self.imaged_addr);

        let bmp_vec = kvec![0xff, 0x7f, 0x78, 0x86, 0x00]?;
        self.bmp_offset = self.findbytes(&bmp_vec);

        let bit_vec: KVec<u8> = kvec![0xff, 0xb8, b'B', b'I', b'T']?;
        self.bit_offset = self.findbytes(&bit_vec);

        let mut bit_entry:  BitEntry = Default::default();

        Bios::bit_entry(self, b'i', &mut bit_entry)?;

        if bit_entry.length >= 4 {
            let major = self.rd08((bit_entry.offset + 3) as isize);
            let chip = self.rd08((bit_entry.offset + 2) as isize);
            let minor = self.rd08((bit_entry.offset + 1) as isize);
            let micro = self.rd08((bit_entry.offset + 0) as isize);
            let patch = self.rd08((bit_entry.offset + 4) as isize);

            pr_info!("version {:x}:{:x}:{:x}:{:x}:{:x}\n",
                     major, chip, minor, micro, patch);
        }
        Ok(())
    }

    pub(crate) fn get_range(&self, range: core::ops::Range<usize>) -> Option<&[u8]>
    {
        if range.end <= self.bios_vec.len() {
            Some(&self.bios_vec[range])
        } else {
            None
        }
    }

    pub(crate) fn find_fwsec_offset(&self) -> Result<u32> {
        let mut idx = 0;
        let mut ver = 0;
        let mut hdr = 0;
        let mut pmue : BiosPmuE = Default::default();
        let mut found: i8 = -1;
        loop {
            let data = Self::pmu_ep(self, idx, &mut ver, &mut hdr, &mut pmue)?;
            if data == 0 {
                break;
            }

            pr_info!("pmue {:#x}", pmue.pmutype);

            if pmue.pmutype == 0x85 {
                found = idx as i8;
                break;
            }
            idx += 1;
        }

        match found {
            -1 => { return Err(EINVAL); }
            _ => {
                pr_info!("pmu found idx {}\n", found);
            }
        }

        let desc_hdr = self.rd32(pmue.data as isize);
        pr_info!("flcn {:#x} {:#x} {} {}\n", pmue.data, desc_hdr, (desc_hdr & 0xffff0000) >> 16,
                 (desc_hdr & 0xff00) >> 8);

        Ok(pmue.data)
    }
}
