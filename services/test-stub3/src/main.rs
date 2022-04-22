#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use num_traits::*;

#[derive(num_derive::FromPrimitive, num_derive::ToPrimitive, Debug)]
pub(crate) enum Opcode {
    DoTest,
}

#[derive(Debug)]
#[repr(C, align(4096))]
pub struct RawData {
    pub raw: [u8; 4096],
}

#[xous::xous_main]
fn main() -> ! {
    log_server::init_wait().unwrap();
    log::set_max_level(log::LevelFilter::Info);
    log::info!("my PID is {}", xous::process::id());

    let xns = xous_names::XousNames::new().unwrap();
    let sid = xns.register_name("test stub 3 server", None).expect("can't register server");
    let tt = ticktimer_server::Ticktimer::new().unwrap();
    let cid = xous::connect(sid).unwrap();

    const BUFLEN: usize = 2048;
    let mut bigbuf = [0u8; BUFLEN];
    for i in 0..BUFLEN {
        bigbuf[i] = (i * 13 % 11) as u8;
    }
    let mut bigbuf2 = [0u8; BUFLEN];
    for i in 16..BUFLEN {
        bigbuf2[i] = (i * 11 % 7) as u8;
    }

    std::thread::spawn({
        let cid = cid.clone();
        move || {
            let tt = ticktimer_server::Ticktimer::new().unwrap();
            loop {
                tt.sleep_ms(4000).unwrap();
                log::info!("test-stub3 pinger");
                xous::send_message(cid, xous::Message::new_scalar(
                    Opcode::DoTest.to_usize().unwrap(),
                    0, 0, 0, 0)).unwrap();
            }
        }
    });

    // yah, this is hard coded for a test...
    let test_conn = xns.request_connection_blocking(usb_test::SERVER_NAME_USBTEST).unwrap();
    loop {
        let msg = xous::receive_message(sid).unwrap();
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(Opcode::DoTest) => xous::msg_scalar_unpack!(msg, _, _, _, _, {

                let mut request = RawData { raw: [0u8; 4096] };
                for (&s, d) in bigbuf.iter().zip(request.raw.iter_mut()) {
                    *d = s;
                }
                log::info!("buf: {:?}", &request.raw[..]);
                let buf = unsafe {
                    xous::MemoryRange::new(
                        &mut request as *mut RawData as usize,
                        core::mem::size_of::<RawData>(),
                    )
                    .unwrap()
                };
                log::info!("small message 1");
                let response = xous::send_message(
                    test_conn,
                    xous::Message::new_lend_mut(
                        usb_test::Opcode::Test1.to_usize().unwrap(),
                        buf,
                        None,
                        None,
                    ),
                );
                match response {
                    Ok(xous::Result::MemoryReturned(_offset, _valid)) => {
                        // contrived example just copies whatever comes back from the server
                        let response = buf.as_slice::<u8>();
                        log::info!("buf_ret: {:?}", &response[..]);
                    }
                    _ => log::warn!("unexpected response"),
                }
                log::info!("buf2: {:?}", &bigbuf2[..]);
                log::info!("small message 3");
            }),
            None => {
                log::error!("couldn't convert opcode: {:?}", msg);
            }
        }
    }
}