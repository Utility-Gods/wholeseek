// The binary's dispatch wires this module into the real `main`; this target
// compiles and exercises the exact same CLI module standalone.
#[allow(dead_code)]
#[path = "../src/cli/mod.rs"]
mod cli;
