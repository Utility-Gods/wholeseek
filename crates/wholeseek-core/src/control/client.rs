use super::{Failure, Request, Response};
use std::io;

/// A single request made to the current local session owner. An I/O error is
/// distinct from a successfully received application-level failure.
pub trait Caller {
    fn call(&mut self, req: &Request) -> io::Result<Response>;
}

pub const fn exit_code(failure: &Failure) -> u8 {
    failure.code.exit_code()
}
