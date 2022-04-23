#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod api;
pub use api::*;
use xous::{Message, CID, SID, msg_blocking_scalar_unpack, send_message};
use xous_ipc::Buffer;
use std::thread;

use num_traits::*;

#[cfg(any(target_os = "none", target_os = "xous"))]
mod implementation {
    use utralib::generated::*;
    // use crate::api::*;
    use susres::{RegManager, RegOrField, SuspendResume};

    pub struct UsbTest {
        //csr: utralib::CSR<u32>,
        //fifo: xous::MemoryRange,
        // susres_manager: RegManager::<{utra::audio::AUDIO_NUMREGS}>,
    }

    impl UsbTest {
        pub fn new() -> UsbTest {
            /*
            let csr = xous::syscall::map_memory(
                xous::MemoryAddress::new(utra::audio::HW_AUDIO_BASE),
                None,
                4096,
                xous::MemoryFlags::R | xous::MemoryFlags::W,
            )
            .expect("couldn't map Audio CSR range");
            let fifo = xous::syscall::map_memory(
                xous::MemoryAddress::new(utralib::HW_AUDIO_MEM),
                None,
                utralib::HW_AUDIO_MEM_LEN,
                xous::MemoryFlags::R | xous::MemoryFlags::W,
            )
            .expect("couldn't map Audio CSR range");
            */
            let mut usbtest = UsbTest {
                // csr: CSR::new(csr.as_mut_ptr() as *mut u32),
                // susres_manager: RegManager::new(csr.as_mut_ptr() as *mut u32),
                // fifo,
            };

            usbtest
        }

        pub fn suspend(&mut self) {
            // self.susres_manager.suspend();
        }
        pub fn resume(&mut self) {
            // self.susres_manager.resume();
        }
    }
}

// a stub to try to avoid breaking hosted mode for as long as possible.
#[cfg(not(any(target_os = "none", target_os = "xous")))]
mod implementation {
    pub struct UsbTest {
    }

    impl UsbTest {
        pub fn new() -> UsbTest {
            UsbTest {
            }
        }
        pub fn suspend(&self) {
        }
        pub fn resume(&self) {
        }
    }
}

#[derive(num_derive::FromPrimitive, num_derive::ToPrimitive, Debug)]
pub(crate) enum ConnectionManagerOpcode {
    SubscribeWifiStats,
    Quit,
}

pub(crate) fn connection_manager(sid: xous::SID) {
    loop {
        let msg = xous::receive_message(sid).unwrap();
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(ConnectionManagerOpcode::SubscribeWifiStats) => {
                log::info!("incoming second hand message");
                let buffer = unsafe {
                    Buffer::from_memory_message(msg.body.memory_message().unwrap())
                };
                let sub = buffer.to_original::<WifiStateSubscription, _>().unwrap();
                log::info!("got {:?}, {}", sub.sid, sub.opcode);
            },
            Some(ConnectionManagerOpcode::Quit) => msg_blocking_scalar_unpack!(msg, _, _, _, _, {
                xous::return_scalar(msg.sender, 0).unwrap();
                log::warn!("exiting connection manager");
                break;
            }),
            None => {
                log::error!("couldn't convert opcode: {:?}", msg);
            }
        }
    }
    xous::destroy_server(sid).unwrap();
}


#[xous::xous_main]
fn xmain() -> ! {
    use crate::implementation::UsbTest;

    log_server::init_wait().unwrap();
    log::set_max_level(log::LevelFilter::Info);
    log::info!("my PID is {}", xous::process::id());

    let xns = xous_names::XousNames::new().unwrap();
    let usbtest_sid = xns.register_name(api::SERVER_NAME_USBTEST, None).expect("can't register server");
    log::trace!("registered with NS -- {:?}", usbtest_sid);

    let mut usbtest = UsbTest::new();

    log::trace!("ready to accept requests");

    let cm_sid = xous::create_server().expect("couldn't create connection manager server");
    let cm_cid = xous::connect(cm_sid).unwrap();
    thread::spawn({
        move || {
            connection_manager(cm_sid);
        }
    });

    std::thread::spawn({
        move || {
            let tt = ticktimer_server::Ticktimer::new().unwrap();
            let mut keepalive = 0;
            loop {
                tt.sleep_ms(2500).unwrap();
                log::info!("keepalive {}", keepalive);
                keepalive += 1;
            }
        }
    });

    // register a suspend/resume listener
    let sr_cid = xous::connect(usbtest_sid).expect("couldn't create suspend callback connection");
    let mut susres = susres::Susres::new(None, &xns, api::Opcode::SuspendResume as u32, sr_cid).expect("couldn't create suspend/resume object");

    loop {
        let mut msg = xous::receive_message(usbtest_sid).unwrap();
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(Opcode::SuspendResume) => xous::msg_scalar_unpack!(msg, token, _, _, _, {
                usbtest.suspend();
                susres.suspend_until_resume(token).expect("couldn't execute suspend/resume");
                usbtest.resume();
            }),
            Some(Opcode::Test1) => {
                let body = msg.body.memory_message_mut().expect("incorrect message type received");
                let buf = body.buf.as_slice_mut::<u8>();
                for i in buf.iter_mut() {
                    *i = *i + 1;
                }
            },
            Some(Opcode::SubscribeWifiStats) => {
                log::info!("first level rx");
                let buffer =
                    unsafe { Buffer::from_memory_message(msg.body.memory_message().unwrap()) };
                // have to transform it through the local memory space because you can't re-lend pages
                let sub = buffer.to_original::<WifiStateSubscription, _>().unwrap();
                let buf = Buffer::into_buf(sub).expect("couldn't convert to memory message");
                log::info!("regift!");
                buf.send(
                    cm_cid,
                    ConnectionManagerOpcode::SubscribeWifiStats
                        .to_u32()
                        .unwrap(),
                )
                .expect("couldn't forward subscription request");
            }
            Some(Opcode::Quit) => {
                log::warn!("Quit received, goodbye world!");
                break;
            },
            None => {
                log::error!("couldn't convert opcode: {:?}", msg);
            }
        }
    }
    // clean up our program
    log::trace!("main loop exit, destroying servers");
    xns.unregister_server(usbtest_sid).unwrap();
    xous::destroy_server(usbtest_sid).unwrap();
    log::trace!("quitting");
    xous::terminate_process(0)
}
