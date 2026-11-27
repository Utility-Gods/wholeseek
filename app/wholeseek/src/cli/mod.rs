//! The wholeseek command-line surface: strict argument parsing, text/JSON
//! rendering and exit codes, talking to the local session owner through a
//! `Caller`. See `docs/CLI.md` for the full reference.

mod help;
mod parse;

#[allow(unused_imports)] // wired up by the rendering slice
pub use parse::{Command, Kind, Parsed};

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

/// Parses `args` (starting at the subcommand or `--help`) and returns the
/// outcome. Wired up to a `Caller` and stdout by `run` once rendering lands.
pub fn parse(args: &[String]) -> Parsed {
    parse::parse(args)
}
