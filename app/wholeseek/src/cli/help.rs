//! Usage text for `--help` and usage errors. Full agent-facing reference:
//! `docs/CLI.md`.

pub fn global() -> String {
    [
        "usage: wholeseek <command> [--json] [args...]",
        "commands:",
        "  search '<query>' [--flac|--mp3|--wav|--lossless] [--min-bitrate N] [--limit N] [--wait SECS]",
        "  browse <user> [<folder>]",
        "  download <result-id> | <user> <path> [--to DIR] [--wait]",
        "  transfers [<id>]",
        "  retry <id> --other-source",
        "  status",
        "  cancel <id>",
        "run 'wholeseek <command> --help' for details; see docs/CLI.md for the full reference.",
    ]
    .join("\n")
}

pub fn search() -> String {
    "usage: wholeseek search '<query>' [--flac|--mp3|--wav|--lossless] [--min-bitrate N] [--limit N] [--wait SECS] [--json]".into()
}

pub fn browse() -> String {
    "usage: wholeseek browse <user> [<folder>] [--json]".into()
}

pub fn download() -> String {
    [
        "usage: wholeseek download <result-id> [--to DIR] [--wait] [--json]",
        "       wholeseek download <user> <path> [--to DIR] [--wait] [--json]",
    ]
    .join("\n")
}

pub fn transfers() -> String {
    "usage: wholeseek transfers [<id>] [--json]".into()
}

pub fn retry() -> String {
    "usage: wholeseek retry <id> --other-source [--json]".into()
}

pub fn status() -> String {
    "usage: wholeseek status [--json]".into()
}

pub fn cancel() -> String {
    "usage: wholeseek cancel <id> [--json]".into()
}
