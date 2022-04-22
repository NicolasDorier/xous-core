pub const SERVER_NAME_USBTEST: &'static str = "_USB test and development server_";

#[derive(num_derive::FromPrimitive, num_derive::ToPrimitive, Debug)]
pub enum Opcode {
    /// A test opcode
    Test1,
    /// Suspend/resume callback
    SuspendResume,
    /// Exits the server
    Quit,
}
