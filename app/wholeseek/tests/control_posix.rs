#![cfg(unix)]

// The binary's shared dispatch is integrated separately. Compile and exercise
// the exact same host module in this target without starting the desktop.
#[allow(dead_code)]
#[path = "../src/control/mod.rs"]
mod control;
