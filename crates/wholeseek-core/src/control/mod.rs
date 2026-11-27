//! Versioned, local-only agent control protocol and session state primitives.

mod client;
mod data;
mod protocol;
mod state;

pub use client::{Caller, exit_code};
pub use data::*;
pub use protocol::*;
pub use state::*;
