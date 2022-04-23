use rkyv::{Archive, Deserialize, Serialize};
pub const SERVER_NAME_USBTEST: &'static str = "_USB test and development server_";

#[derive(num_derive::FromPrimitive, num_derive::ToPrimitive, Debug)]
pub enum Opcode {
    /// A test opcode
    Test1,
    /// a test case
    SubscribeWifiStats,
    /// Suspend/resume callback
    SuspendResume,
    /// Exits the server
    Quit,
}

#[derive(Debug, Archive, Serialize, Deserialize, Copy, Clone)]
pub struct WifiStateSubscription {
    pub sid: [u32; 4],
    pub opcode: u32,
}
