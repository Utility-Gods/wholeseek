//! The wholeseek command-line surface: strict argument parsing, text/JSON
//! rendering and exit codes, talking to the local session owner through a
//! `Caller`. See `docs/CLI.md` for the full reference.

mod help;
mod parse;
mod render;

use wholeseek_core::control::Caller;

/// The exact set of tokens that put `argv[1]` on the CLI path instead of the
/// desktop app. Keep in sync with `docs/CLI.md` and `parse::parse`.
pub fn is_command(arg: &str) -> bool {
    matches!(
        arg,
        "search"
            | "browse"
            | "download"
            | "transfers"
            | "retry"
            | "status"
            | "cancel"
            | "--help"
            | "-h"
    )
}

/// Parses `args` (starting at the subcommand or `--help`), calls `caller` if
/// needed, prints exactly one block to stdout or stderr, and returns the
/// process exit code. `args` excludes the program name (pass `argv[1..]`).
///
/// Output has no colour: this surface never emits ANSI, on a TTY or not, so
/// agents and humans alike get the same bytes.
pub fn run(args: &[String], caller: &mut dyn Caller) -> i32 {
    let outcome = render::execute(args, caller);
    if let Some(text) = outcome.stdout {
        println!("{text}");
    }
    if let Some(text) = outcome.stderr {
        eprintln!("{text}");
    }
    outcome.code
}
