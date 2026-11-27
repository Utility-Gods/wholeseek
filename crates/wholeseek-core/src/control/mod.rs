//! Versioned, local-only agent control protocol and session state primitives.

mod client;
mod data;
mod protocol;
mod state;
pub mod transport;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod unix;

pub use client::{Caller, exit_code};
pub use data::*;
pub use protocol::*;
pub use state::*;
