use std::collections::HashMap;
use std::convert::TryInto;
use std::io::{Read, Write};
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};

use crate::{Result, SysCall, SysCallResult, PID, TID};

mod mem;
pub use mem::*;

mod threading;
pub use threading::*;

lazy_static::lazy_static! {
    static ref NETWORK_CONNECT_ADDRESS: SocketAddr = {
        std::env::var("XOUS_SERVER")
        .map(|s| {
            s.to_socket_addrs()
                .expect("invalid server address")
                .next()
                .expect("unable to resolve server address")
        })
        .unwrap_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
    };
    static ref PROCESS_KEY: ProcessKey = {
        std::env::var("XOUS_PROCESS_KEY")
        .map(|s| {
            let mut base = ProcessKey([0u8; 16]);
            hex::decode_to_slice(s, &mut base.0).unwrap();
            base
        })
        .unwrap_or(ProcessKey([0u8; 16]))
    };

    static ref SERVER_CONNECTION: ServerConnection = {
        // By the time we get our process' key, it should not be zero
        assert_ne!(&PROCESS_KEY.0, &[0u8; 16]);

        // Note: &* is required due to how `lazy_static` works behind the scenes:
        // https://github.com/rust-lang-nursery/lazy-static.rs/issues/119#issuecomment-419595818
        let mut conn = TcpStream::connect(&*NETWORK_CONNECT_ADDRESS).expect("unable to connect to Xous kernel");

        // Disable Nagel's algorithm, since we're running locally and managing buffers ourselves
        conn.set_nodelay(true).unwrap();

        // Send key to authenticate us as a known process
        conn.write_all(&PROCESS_KEY.0).unwrap();
        conn.flush().unwrap();

        // Read the 8-bit process ID and verify it matches what we were told.
        let mut pid = [0u8];
        conn.read_exact(&mut pid).unwrap();
        assert_eq!(pid[0], PROCESS_ID.get(), "process ID mismatch");

        let call_mem_tracker = Arc::new(Mutex::new(HashMap::new()));
        // let call_tracker = Arc::new(Mutex::new(HashMap::new()));
        // let response_tracker = Arc::new(Mutex::new(HashMap::new()));
        let mailbox = Arc::new((Mutex::new(HashMap::new()), Condvar::new()));

        let mut reader_conn = conn.try_clone().unwrap();
        let reader_call_mem_tracker = call_mem_tracker.clone();
        // let reader_call_tracker = call_tracker.clone();
        // let reader_response_tracker = response_tracker.clone();
        let reader_mailbox = mailbox.clone();
        let _network_watcher = std::thread::spawn(move || {
            loop {
                let (tid, response) = read_next_syscall_result(&mut reader_conn, &reader_call_mem_tracker);
                // assert!(reader_call_tracker.lock().unwrap().remove(&tid).is_some());
                // assert!(reader_response_tracker.lock().unwrap().insert(tid, ()).is_none());
                let (lock, cv) = &*reader_mailbox;
                let existing = lock.lock().unwrap().insert(tid, response);
                assert!(existing.is_none(), "got two responses for the same thread");
                cv.notify_all();
            }
        });

        ServerConnection {
            send: Arc::new(Mutex::new(conn)),
            mailbox,
            call_mem_tracker,
            // call_tracker,
            // response_tracker,
        }
    };

    /// The ID of the current process
    static ref PROCESS_ID: PID = {
        use std::str::FromStr;
        PID::from_str(&std::env::var("XOUS_PID")
            .expect("missing environment variable XOUS_PID"))
            .expect("XOUS_PID environment variable was not valid")
    };

    /// The network address to connect to when making a kernel call
    static ref CHILD_PROCESS_ADDRESS: Arc<Mutex<SocketAddr>> = Arc::new(Mutex::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)));
}

pub fn set_xous_address(new_address: SocketAddr) {
    *CHILD_PROCESS_ADDRESS.lock().unwrap() = new_address;
}

pub fn set_thread_id(new_tid: TID) {
    THREAD_ID.with(|tid| *tid.borrow_mut() = Some(new_tid));
}

/// A 16-byte random nonce that identifies this process to the kernel. This
/// is usually provided through the environment variable `XOUS_PROCESS_KEY`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProcessKey([u8; 16]);
impl ProcessKey {
    pub fn new(key: [u8; 16]) -> ProcessKey {
        ProcessKey(key)
    }
}

/// Describes all parameters that are required to start a new process
/// on this platform.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProcessInit {
    pub key: ProcessKey,
}

pub struct ProcessArgs {
    command: String,
    name: String,
}

impl ProcessArgs {
    pub fn new(name: &str, command: String) -> ProcessArgs {
        ProcessArgs {
            command,
            name: name.to_owned(),
        }
    }
}

#[derive(Debug)]
pub struct ProcessHandle(std::process::Child);

/// If no connection exists, create a new connection to the server. This means
/// our parent PID will be PID1. Otherwise, reuse the same connection.
pub fn create_process_pre(_args: &ProcessArgs) -> core::result::Result<ProcessInit, crate::Error> {
    unimplemented!()
}

/// Launch a new process with the current PID as the parent.
pub fn create_process_post(
    args: ProcessArgs,
    init: ProcessInit,
    pid: PID,
) -> core::result::Result<ProcessHandle, crate::Error> {
    use std::process::Command;
    let server_env = format!("{}", CHILD_PROCESS_ADDRESS.lock().unwrap());
    let pid_env = format!("{}", pid);
    let process_name_env = args.name.to_string();
    let process_key_env = hex::encode(&init.key.0);
    let (shell, args) = if cfg!(windows) {
        ("cmd", ["/C", &args.command])
    } else if cfg!(unix) {
        ("sh", ["-c", &args.command])
    } else {
        panic!("unrecognized platform -- don't know how to shell out");
    };

    // println!("Launching process...");
    Command::new(shell)
        .args(&args)
        .env("XOUS_SERVER", server_env)
        .env("XOUS_PID", pid_env)
        .env("XOUS_PROCESS_NAME", process_name_env)
        .env("XOUS_PROCESS_KEY", process_key_env)
        .spawn()
        .map(ProcessHandle)
        .map_err(|_| {
            // eprintln!("couldn't start command: {}", e);
            crate::Error::InternalError
        })
}

pub fn wait_process(mut joiner: ProcessHandle) -> crate::SysCallResult {
    joiner
        .0
        .wait()
        .or(Err(crate::Error::InternalError))
        .and_then(|e| {
            if e.success() {
                Ok(crate::Result::Ok)
            } else {
                Err(crate::Error::UnknownError)
            }
        })
}

#[derive(PartialEq)]
enum CallMemoryKind {
    Borrow,
    MutableBorrow,
    Move,
    ReturnMemory,
}

#[derive(Clone)]
struct ServerConnection {
    send: Arc<Mutex<TcpStream>>,
    mailbox: Arc<(Mutex<HashMap<TID, Result>>, Condvar)>,
    call_mem_tracker: Arc<Mutex<HashMap<TID, (crate::MemoryRange, CallMemoryKind)>>>,
    // call_tracker: Arc<Mutex<HashMap<TID, ()>>>,
    // response_tracker: Arc<Mutex<HashMap<TID, ()>>>,
}

pub fn process_to_args(call: usize, init: &ProcessInit) -> [usize; 8] {
    [
        call,
        u32::from_le_bytes(init.key.0[0..4].try_into().unwrap()) as _,
        u32::from_le_bytes(init.key.0[4..8].try_into().unwrap()) as _,
        u32::from_le_bytes(init.key.0[8..12].try_into().unwrap()) as _,
        u32::from_le_bytes(init.key.0[12..16].try_into().unwrap()) as _,
        0,
        0,
        0,
    ]
}

pub fn args_to_process(
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    _a5: usize,
    _a6: usize,
    _a7: usize,
) -> core::result::Result<ProcessInit, crate::Error> {
    let mut v = vec![];
    v.extend_from_slice(&(a1 as u32).to_le_bytes());
    v.extend_from_slice(&(a2 as u32).to_le_bytes());
    v.extend_from_slice(&(a3 as u32).to_le_bytes());
    v.extend_from_slice(&(a4 as u32).to_le_bytes());
    let mut key = [0u8; 16];
    key.copy_from_slice(&v);
    Ok(ProcessInit {
        key: ProcessKey(key),
    })
}

/// Perform a synchronous syscall to the kernel.
pub fn syscall(call: SysCall) -> SysCallResult {
    let tid = thread_id();

    // If this call has memory attached to it, save that memory information
    // in a call tracker. That way we'll know how much data to read when
    // the kernel response comes back.
    if let Some(range) = call.memory() {
        let kind = if call.is_borrow() {
            CallMemoryKind::Borrow
        } else if call.is_mutableborrow() {
            CallMemoryKind::MutableBorrow
        } else if call.is_move() {
            CallMemoryKind::Move
        } else if call.is_return_memory() {
            CallMemoryKind::ReturnMemory
        } else {
            panic!("call had memory, but was unrecognized")
        };

        SERVER_CONNECTION
            .call_mem_tracker
            .lock()
            .unwrap()
            .insert(tid, (range, kind));
    }

    // let start_time = std::time::Instant::now();
    loop {
        // assert!(SERVER_CONNECTION
        //     .call_tracker
        //     .lock()
        //     .unwrap()
        //     .insert(tid, ())
        //     .is_none());
        send_syscall(&call);

        let result = match read_syscall_result(tid) {
            Result::Error(e) => Some(Err(e)),
            Result::RetryCall => None,
            other => Some(Ok(other)),
        };
        // assert!(SERVER_CONNECTION
        //     .response_tracker
        //     .lock()
        //     .unwrap()
        //     .remove(&tid)
        //     .is_some());
        if let Some(val) = result {
            // println!("[{:2}:{:2}] Syscall took {:7} usec: {:x?}", PROCESS_ID.get(), tid, start_time.elapsed().as_micros(), call);
            return val;
        }

        // If the syscall would block, give it 5ms and retry the call.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn read_next_syscall_result(
    stream: &mut TcpStream,
    call_mem_tracker: &Arc<Mutex<HashMap<TID, (crate::MemoryRange, CallMemoryKind)>>>,
) -> (TID, Result) {
    loop {
        // This thread_id doesn't exist in the mailbox, so read additional data.
        let mut pkt = [0usize; 8];
        let mut raw_bytes = [0u8; size_of::<usize>() * 9];
        if let Err(e) = stream.read_exact(&mut raw_bytes) {
            eprintln!("Server shut down: {}", e);
            std::process::exit(0);
        }

        let mut raw_bytes_chunks = raw_bytes.chunks(size_of::<usize>());

        // Read the Thread ID, which comes across first, followed by the 8 words of
        // the message data.
        let msg_thread_id =
            TID::from_le_bytes(raw_bytes_chunks.next().unwrap().try_into().unwrap());
        for (pkt_word, word) in pkt.iter_mut().zip(raw_bytes_chunks) {
            *pkt_word = usize::from_le_bytes(word.try_into().unwrap());
        }

        // Reconstitute the response from the args.
        let mut response = Result::from_args(pkt);

        // This indicates that the request was successful, but there is no response
        // yet. The client should request the response again without sending a packet.
        if response == Result::BlockedProcess {
            // println!("   Waiting again");
            continue;
        }

        // The syscall MAY contain memory, however the call was unsuccessful and should
        // be retried. Examples of this are SendMessage when there is no server available
        // to handle the message.
        if response == Result::RetryCall {
            return (msg_thread_id, response);
        }

        // If control flow got here, then we have a valid syscall.

        // If the client is passing us memory, read bytes from the data, then
        // remap the addresses to function in our address space.
        if let Result::Message(msg) = &mut response {
            match &mut msg.body {
                crate::Message::Move(ref mut memory_message)
                | crate::Message::Borrow(ref mut memory_message)
                | crate::Message::MutableBorrow(ref mut memory_message) => {
                    let data = vec![0u8; memory_message.buf.len()];
                    let mut data = std::mem::ManuallyDrop::new(data);
                    if let Err(e) = stream.read_exact(&mut data) {
                        eprintln!("Server shut down: {}", e);
                        std::process::exit(0);
                    }
                    data.shrink_to_fit();
                    assert_eq!(data.len(), data.capacity());
                    let len = data.len();
                    let addr = data.as_mut_ptr();
                    memory_message.buf =
                        unsafe { crate::MemoryRange::new(addr as _, len).unwrap() };
                }
                _ => (),
            }
        }

        // If the original call contained memory, then the server will send a copy of the
        // buffer back to us. Ensure the memory we get back is correct.
        if let Some((mem, kind)) = call_mem_tracker.lock().unwrap().remove(&msg_thread_id) {
            if response == Result::RetryCall {
            } else if kind == CallMemoryKind::Borrow || kind == CallMemoryKind::MutableBorrow {
                // Read the buffer back from the remote host.
                use core::slice;
                let mut data = unsafe { slice::from_raw_parts_mut(mem.as_mut_ptr(), mem.len()) };

                // If it's a Borrow, verify the contents haven't changed by saving the previous
                // buffer in a Vec called `previous_data`.
                let previous_data = match kind {
                    CallMemoryKind::Borrow => Some(data.to_vec()),
                    _ => None,
                };

                // Read the incoming data from the network.
                if let Err(e) = stream.read_exact(&mut data) {
                    eprintln!("Server shut down: {}", e);
                    std::process::exit(0);
                }

                // If it is an immutable borrow, verify the contents haven't changed somehow
                if let Some(previous_data) = previous_data {
                    if data != previous_data.as_slice() {
                        println!("Data: {:x?}", data);
                        println!("Previous data: {:x?}", previous_data);
                        ::debug_here::debug_here!();
                        panic!("Data was not equal!");
                    }
                    // assert_eq!(
                    //     data,
                    //     previous_data.as_slice(),
                    //     "Data changed. Was: {:x?}, Now: {:x?}",
                    //     data,
                    //     previous_data
                    // );
                }
            }

            if kind == CallMemoryKind::Move {
                // In a hosted environment, the message contents are leaked when
                // it gets converted into a MemoryMessage. Now that the call is
                // complete, free the memory.
                mem::unmap_memory_post(mem).unwrap();
            }

            // If we're returning memory to the Server, then reconstitute the buffer we just passed,
            // and Drop it so it can be freed.
            if kind == CallMemoryKind::ReturnMemory {
                let rebuilt =
                    unsafe { Vec::from_raw_parts(mem.as_mut_ptr(), mem.len(), mem.len()) };
                drop(rebuilt);
            }
        }
        return (msg_thread_id, response);
    }
}

/// Read a response from the kernel
fn read_syscall_result(thread_id: TID) -> Result {
    // Check to see if this thread id has an entry in the mailbox already.
    // This will block until the hashmap is free.
    let (lock, cv) = &*SERVER_CONNECTION.mailbox;
    let mut mailbox = lock.lock().unwrap();
    loop {
        if let Some(entry) = mailbox.remove(&thread_id) {
            return entry;
        }

        mailbox = cv.wait(mailbox).unwrap();
    }
}

fn send_syscall(call: &crate::SysCall) {
    // println!("Making Syscall: {:?}", call);
    let tid = thread_id();

    send_syscall_from_tid(call, tid)
}

fn send_syscall_from_tid(call: &crate::SysCall, tid: TID) {
    let args = call.as_args();

    // Send the packet to the server
    let mut capacity = args.len() * core::mem::size_of::<usize>() + core::mem::size_of_val(&tid);

    // If there's memory attached to this packet, add that on to the
    // number of bytes.
    if let Some(mem) = call.memory() {
        capacity += mem.len();
    }

    let mut pkt = Vec::with_capacity(capacity);

    // 1. Add in the Thread ID
    pkt.extend_from_slice(&tid.to_le_bytes());

    // 2. Add in each of the args
    for word in &args {
        pkt.extend_from_slice(&word.to_le_bytes());
    }

    // 3. (Optional) add in memory data, if present
    if let Some(memory) = call.memory() {
        use core::slice;
        // Unsafety: As long as `memory` is a valid pointer, this is safe.
        // The call to `MemoryRange::new()` is unsafe, and requires that
        // the memory pointed to is always valid.
        let data: &[u8] = unsafe { slice::from_raw_parts(memory.as_ptr(), memory.len()) };
        pkt.extend_from_slice(data);
    }

    let mut stream = SERVER_CONNECTION.send.lock().unwrap();
    if let Err(e) = stream.write_all(&pkt) {
        eprintln!("Server shut down: {}", e);
        std::process::exit(0);
    }
}
