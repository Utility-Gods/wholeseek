//! Agent control is enabled on Unix only. Windows keeps the desktop-only
//! session path; the experimental named-pipe implementation is not compiled.

#[cfg(unix)]
mod posix;
#[cfg(unix)]
pub use posix::{Host, SocketCaller, daemon, prepare_app, take_app_host};

#[cfg(windows)]
mod desktop_only {
    use std::sync::Arc;
    use wholeseek_core::{Core, CoreEvent};

    pub struct Host;

    impl Host {
        pub fn set_waker(&self, _: Arc<dyn Fn() + Send + Sync>) {}

        pub fn event(&mut self, _: &Core, _: &CoreEvent) {}

        pub fn step(&mut self, _: &Core) {}

        pub fn next_tick(&self) -> Option<f64> {
            None
        }
    }

    pub fn take_app_host() -> Host {
        Host
    }
}

#[cfg(windows)]
pub use desktop_only::{Host, take_app_host};
