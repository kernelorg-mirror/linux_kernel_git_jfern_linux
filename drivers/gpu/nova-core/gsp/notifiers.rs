pub(crate) use kernel::macros::versions;

use kernel::prelude::*;
use crate::falcon::Falcon;
use kernel::sync::Arc;
use kernel::delay::sleep;
use core::time::Duration; 
use crate::timer_msec;
use crate::timer_usec;
use crate::timer_nsec;
use crate::timer::TimerWait;
use crate::gsp::gsp_falcon;

use crate::gsp::*;

#[versions(GSP)]
pub(crate) struct Notifiers {
}

#[versions(GSP)]
impl Notifiers::ver {


pub(crate) fn run_cpu_sequencer(gsp_falcon: &gsp_falcon::GspFalcon,
                                sec2_falcon: &Arc<Falcon>, msg: &mut KVec<u8>) -> Result<()> {

    pr_info!("running cpu seq");

    let bar = gsp_falcon.falcon.base.bar.try_access().ok_or(ENXIO)?;
    let mut run_cpu = fw::ver::gen::s_rpc_run_cpu_sequencer_v17_00::new(unsafe { msg.as_mut_ptr().byte_offset(RpcMsg::ver::get_gsp_rpc_hdr_size() as isize)} );
    let cmd_index: usize;
    let mut reg_save_area: [u32; 8] = Default::default();
    cmd_index = run_cpu.get_cmdIndex() as usize;

    let mut ptr: usize = 0;

    while ptr < cmd_index {
        let opcode: u32;

        let base_ptr = unsafe { msg.as_mut_ptr().byte_offset(RpcMsg::ver::get_gsp_rpc_hdr_size() as isize + fw::ver::gen::s_rpc_run_cpu_sequencer_v17_00::str_size() as isize + (ptr * 4) as isize) };
        let mut cmd = fw::ver::gen::s_GSP_SEQUENCER_BUFFER_CMD::new(base_ptr);

        opcode = cmd.get_opCode();

        ptr += 1;

        match opcode {
            0 => { // GSP REG WRITE
                let regwrite = cmd.new_S_payload_regWrite();
                let addr = regwrite.get_addr() as usize;
                let val = regwrite.get_val();
                pr_info!("GSP reg write {:#x} {:#x}", addr, val);
                bar.try_writel(val, addr)?;
                ptr += fw::ver::gen::s_GSP_SEQ_BUF_PAYLOAD_REG_WRITE::str_size() / 4;
            },
            1 => { // GSP REG MODIFY
                let regmod = cmd.new_S_payload_regModify();

                let val = regmod.get_val();
                let mask = regmod.get_mask();
                let addr = regmod.get_addr() as usize;

                let temp = bar.try_readl(addr)?;
                bar.try_writel((temp & !mask) | val, addr)?;

                pr_info!("GSP reg modify {:#x} {:#x} {:#x}",
                         addr, mask, val);
                ptr += fw::ver::gen::s_GSP_SEQ_BUF_PAYLOAD_REG_MODIFY::str_size() / 4;
            },
            2 => { // GSP REG POLL
                let regpoll = cmd.new_S_payload_regPoll();

                let addr = regpoll.get_addr() as usize;
                let mask = regpoll.get_mask();
                let val = regpoll.get_val();
                let mut timeout = regpoll.get_timeout() as u64;
                let error = regpoll.get_error();

                pr_info!("GSP reg poll {:#x} {:#x} {:#x} {} {}",
                         addr, mask, val, timeout, error);

                if timeout == 0 {
                    timeout = 4000000;
                }
                bar.try_readl(addr)?;

                timer_usec!({
                    if (bar.try_readl(addr)? & mask) == val {
                        break;
                    }
                }, timeout, &gsp_falcon.falcon.base.timer);
                ptr += fw::ver::gen::s_GSP_SEQ_BUF_PAYLOAD_REG_POLL::str_size() / 4;
            }
            3 => { // GSP DELAY US
                let delay = cmd.new_S_payload_delayUs();
                let delay_val : u32 = delay.get_val();

                pr_info!("GSP delay us {}", delay_val);

                sleep(Duration::from_micros(delay_val as u64));
                ptr += fw::ver::gen::s_GSP_SEQ_BUF_PAYLOAD_DELAY_US::str_size() / 4;
            }
            4 => { // GSP RegStore
                let regstore = cmd.new_S_payload_regStore();

                reg_save_area[regstore.get_index() as usize] = bar.try_readl(regstore.get_addr() as usize)?;
                run_cpu.set_regSaveArea(reg_save_area);
                ptr += fw::ver::gen::s_GSP_SEQ_BUF_PAYLOAD_REG_STORE::str_size() / 4;
            },
            5 => { // GSP Core Reset
                pr_info!("GSP CORE RESET");
                let _ = gsp_falcon.falcon.reset();
                gsp_falcon.falcon.mask(0x624, 0x80, 0x80)?;
                gsp_falcon.falcon.wr32(0x10c, 0)?;
            },
            6 => { // GSP Core Start
                pr_info!("GSP CORE START");
                if (gsp_falcon.falcon.rd32(0x100)? & 0x00000040) != 0 {
                    gsp_falcon.falcon.wr32(0x130, 0x2)?;
                } else {
                    gsp_falcon.falcon.wr32(0x100, 0x2)?;
                }
            },
            7 => { // GSP Core Wait for Halt
                pr_info!("GSP CORE WAIT FOR HALT");

                let _ = gsp_falcon.falcon.wait_for_reg_bits_set(0x100, 0x10, 2000000);
            },
            8 => { // GSP Core Resume
                pr_info!("GSP CORE RESUME");

                let _ = gsp_falcon.reset();

                gsp_falcon.write_libos_addr()?;

                // start sec2 falcon
                sec2_falcon.v1_start()?;

                if timer_msec!({
                    if (bar.readl(0x1180f8) & 0x04000000) != 0 {
                        break;
                    }
                }, 2000, &gsp_falcon.falcon.base.timer) < 0 {
                    return Err(ETIME);
                }

                //read sec2 falcon mbox
                let mbox0 = sec2_falcon.rd32(0x40)?;

                if mbox0 != 0 {
                    pr_info!("SEC2 FALCON MBOX {}", mbox0);
                    return Err(ETIME);
                }

                    gsp_falcon.write_app_version()?;
                if !gsp_falcon.falcon.riscv_active()? {
                    pr_info!("GSP FALCON LOAD FAILED - RISCV NOT ACTIVE PART 2");
                    return Err(ETIME);
                }

                // okay this is funky to reboot
            },
            _ => {
                pr_info!("UNKNOWN ERROR ERROR");
            }
        }
    }
    Ok(())
}

pub(crate) fn os_error_log(msg: &mut KVec<u8>) {
    let mut os_error_log = fw::ver::gen::s_rpc_os_error_log_v17_00::new(unsafe { msg.as_mut_ptr().byte_offset(RpcMsg::ver::get_gsp_rpc_hdr_size() as isize)} );

    pr_info!("ERROR: type: {} runlist {} chid {}", os_error_log.get_exceptType(),
             os_error_log.get_runlistId(), os_error_log.get_chid());
    pr_info!("STR: {:?}", core::str::from_utf8(&os_error_log.get_errString()));
}
}
