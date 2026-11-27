//! Hand-rolled, strict argument parsing for the seven allowed commands.
//!
//! No flag abbreviation, no implicit coercion: an unknown command or flag,
//! a missing or extra positional, or a flag with a bad value is always a
//! usage error (exit code 2 via `ErrorCode::BadRequest`), never a guess.

use std::collections::BTreeMap;
use std::path::PathBuf;
use wholeseek_core::control::{
    BrowseArgs, DownloadArgs, Format, IdArgs, Operation, SearchArgs, Target, TransferArgs,
};

/// One of the seven allowed operations, with its validated arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Search(SearchArgs),
    Browse(BrowseArgs),
    Download(DownloadArgs),
    Transfers(TransferArgs),
    Retry(IdArgs),
    Status,
    Cancel(IdArgs),
}

/// Which response payload a command expects, for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Search,
    Browse,
    Transfers,
    Status,
}

impl Command {
    pub fn kind(&self) -> Kind {
        match self {
            Command::Search(_) => Kind::Search,
            Command::Browse(_) => Kind::Browse,
            Command::Download(_)
            | Command::Transfers(_)
            | Command::Retry(_)
            | Command::Cancel(_) => Kind::Transfers,
            Command::Status => Kind::Status,
        }
    }

    pub fn into_operation(self) -> Operation {
        match self {
            Command::Search(a) => Operation::Search(a),
            Command::Browse(a) => Operation::Browse(a),
            Command::Download(a) => Operation::Download(a),
            Command::Transfers(a) => Operation::Transfers(a),
            Command::Retry(a) => Operation::RetryOtherSource(a),
            Command::Status => Operation::Status,
            Command::Cancel(a) => Operation::Cancel(a),
        }
    }
}

/// The outcome of parsing argv (excluding the program name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Print `text` to stdout and exit 0 (`--help`, global or per-command).
    Help(String),
    /// Print a usage error (text, or a JSON error object if `json`) and exit
    /// 2, without contacting the local session.
    UsageError { message: String, json: bool },
    /// A fully validated command, ready to send.
    Run { command: Command, json: bool },
}

const COMMANDS: &[&str] = &[
    "search",
    "browse",
    "download",
    "transfers",
    "retry",
    "status",
    "cancel",
];

pub fn parse(args: &[String]) -> Parsed {
    let Some(head) = args.first().map(String::as_str) else {
        return Parsed::Help(super::help::global());
    };
    match head {
        "--help" | "-h" => Parsed::Help(super::help::global()),
        "search" => parse_search(&args[1..]),
        "browse" => parse_browse(&args[1..]),
        "download" => parse_download(&args[1..]),
        "transfers" => parse_transfers(&args[1..]),
        "retry" => parse_retry(&args[1..]),
        "status" => parse_status(&args[1..]),
        "cancel" => parse_cancel(&args[1..]),
        other => usage_error_unknown(other, &args[1..]),
    }
}

fn usage_error_unknown(given: &str, rest: &[String]) -> Parsed {
    Parsed::UsageError {
        message: format!(
            "wholeseek: unknown command '{given}'\nusage: wholeseek {{{}}} [--json] ...\n       wholeseek --help",
            COMMANDS.join("|")
        ),
        json: rest.iter().any(|a| a == "--json"),
    }
}

#[derive(Debug, Clone, Copy)]
struct FlagSpec {
    name: &'static str,
    takes_value: bool,
}

const fn flag(name: &'static str) -> FlagSpec {
    FlagSpec {
        name,
        takes_value: false,
    }
}

const fn flag_value(name: &'static str) -> FlagSpec {
    FlagSpec {
        name,
        takes_value: true,
    }
}

enum Scan<'a> {
    Help,
    Err(String),
    Ok {
        flags: BTreeMap<&'static str, Option<String>>,
        positionals: Vec<&'a str>,
    },
}

/// Flags may appear anywhere, in any order; everything else is positional.
fn scan<'a>(args: &'a [String], specs: &[FlagSpec]) -> Scan<'a> {
    let mut flags: BTreeMap<&'static str, Option<String>> = BTreeMap::new();
    let mut positionals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--help" || a == "-h" {
            return Scan::Help;
        }
        if let Some(name) = a.strip_prefix("--") {
            let Some(spec) = specs.iter().find(|s| s.name == name) else {
                return Scan::Err(format!("unknown flag --{name}"));
            };
            if flags.contains_key(spec.name) {
                return Scan::Err(format!("--{name} given more than once"));
            }
            if spec.takes_value {
                i += 1;
                let Some(value) = args.get(i) else {
                    return Scan::Err(format!("--{name} requires a value"));
                };
                flags.insert(spec.name, Some(value.clone()));
            } else {
                flags.insert(spec.name, None);
            }
        } else if a.starts_with('-') && a != "-" {
            return Scan::Err(format!("unknown flag {a}"));
        } else {
            positionals.push(a);
        }
        i += 1;
    }
    Scan::Ok { flags, positionals }
}

/// `--json` may still be present on an otherwise-invalid invocation; a usage
/// error that mentions it should still render as JSON.
fn usage_error(message: &str, args: &[String], cmd: &str) -> Parsed {
    Parsed::UsageError {
        message: format!("wholeseek {cmd}: {message}"),
        json: args.iter().any(|a| a == "--json"),
    }
}

fn parse_opt_u32(
    flags: &BTreeMap<&'static str, Option<String>>,
    name: &str,
) -> Result<Option<u32>, String> {
    match flags.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_deref()
            .unwrap_or_default()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("--{name} must be a non-negative whole number")),
    }
}

fn parse_opt_usize(
    flags: &BTreeMap<&'static str, Option<String>>,
    name: &str,
) -> Result<Option<usize>, String> {
    match flags.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_deref()
            .unwrap_or_default()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| format!("--{name} must be a non-negative whole number")),
    }
}

fn parse_opt_u64(
    flags: &BTreeMap<&'static str, Option<String>>,
    name: &str,
) -> Result<Option<u64>, String> {
    match flags.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_deref()
            .unwrap_or_default()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("--{name} must be a non-negative whole number")),
    }
}

fn parse_search(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[
        flag("flac"),
        flag("mp3"),
        flag("wav"),
        flag("lossless"),
        flag_value("min-bitrate"),
        flag_value("limit"),
        flag_value("wait"),
        flag("json"),
    ];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::search()),
        Scan::Err(message) => return usage_error(&message, args, "search"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if positionals.len() != 1 {
        return Parsed::UsageError {
            message: super::help::search(),
            json,
        };
    }
    let mut formats = Vec::new();
    for (name, format) in [
        ("flac", Format::Flac),
        ("mp3", Format::Mp3),
        ("wav", Format::Wav),
        ("lossless", Format::Lossless),
    ] {
        if flags.contains_key(name) {
            formats.push(format);
        }
    }
    if formats.len() > 1 {
        return Parsed::UsageError {
            message: "wholeseek search: choose at most one of --flac, --mp3, --wav, --lossless"
                .into(),
            json,
        };
    }
    let min_bitrate = match parse_opt_u32(&flags, "min-bitrate") {
        Ok(v) => v,
        Err(message) => return Parsed::UsageError { message, json },
    };
    let limit = match parse_opt_usize(&flags, "limit") {
        Ok(v) => v,
        Err(message) => return Parsed::UsageError { message, json },
    };
    let wait_secs = match parse_opt_u64(&flags, "wait") {
        Ok(v) => v,
        Err(message) => return Parsed::UsageError { message, json },
    };
    Parsed::Run {
        command: Command::Search(SearchArgs {
            query: positionals[0].to_string(),
            formats,
            min_bitrate,
            limit,
            wait_secs,
        }),
        json,
    }
}

fn parse_browse(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::browse()),
        Scan::Err(message) => return usage_error(&message, args, "browse"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if positionals.is_empty() || positionals.len() > 2 {
        return Parsed::UsageError {
            message: super::help::browse(),
            json,
        };
    }
    Parsed::Run {
        command: Command::Browse(BrowseArgs {
            user: positionals[0].to_string(),
            folder: positionals.get(1).map(|s| s.to_string()),
        }),
        json,
    }
}

fn parse_download(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag_value("to"), flag("wait"), flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::download()),
        Scan::Err(message) => return usage_error(&message, args, "download"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    let target = match positionals.as_slice() {
        [result_id] => Target::Result {
            result_id: result_id.to_string(),
        },
        [user, path] => Target::Path {
            user: user.to_string(),
            path: path.to_string(),
        },
        _ => {
            return Parsed::UsageError {
                message: super::help::download(),
                json,
            };
        }
    };
    let to = flags.get("to").and_then(Option::clone).map(PathBuf::from);
    let wait = flags.contains_key("wait");
    Parsed::Run {
        command: Command::Download(DownloadArgs { target, to, wait }),
        json,
    }
}

fn parse_transfers(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::transfers()),
        Scan::Err(message) => return usage_error(&message, args, "transfers"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if positionals.len() > 1 {
        return Parsed::UsageError {
            message: super::help::transfers(),
            json,
        };
    }
    Parsed::Run {
        command: Command::Transfers(TransferArgs {
            id: positionals.first().map(|s| s.to_string()),
        }),
        json,
    }
}

fn parse_retry(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag("other-source"), flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::retry()),
        Scan::Err(message) => return usage_error(&message, args, "retry"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if positionals.len() != 1 {
        return Parsed::UsageError {
            message: super::help::retry(),
            json,
        };
    }
    if !flags.contains_key("other-source") {
        return Parsed::UsageError {
            message: "wholeseek retry: --other-source is required".into(),
            json,
        };
    }
    Parsed::Run {
        command: Command::Retry(IdArgs {
            id: positionals[0].to_string(),
        }),
        json,
    }
}

fn parse_status(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::status()),
        Scan::Err(message) => return usage_error(&message, args, "status"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if !positionals.is_empty() {
        return Parsed::UsageError {
            message: super::help::status(),
            json,
        };
    }
    Parsed::Run {
        command: Command::Status,
        json,
    }
}

fn parse_cancel(args: &[String]) -> Parsed {
    const SPECS: &[FlagSpec] = &[flag("json")];
    let (flags, positionals) = match scan(args, SPECS) {
        Scan::Help => return Parsed::Help(super::help::cancel()),
        Scan::Err(message) => return usage_error(&message, args, "cancel"),
        Scan::Ok { flags, positionals } => (flags, positionals),
    };
    let json = flags.contains_key("json");
    if positionals.len() != 1 {
        return Parsed::UsageError {
            message: super::help::cancel(),
            json,
        };
    }
    Parsed::Run {
        command: Command::Cancel(IdArgs {
            id: positionals[0].to_string(),
        }),
        json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wholeseek_core::control::ErrorCode;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    fn assert_usage(parsed: Parsed) -> (String, bool) {
        match parsed {
            Parsed::UsageError { message, json } => (message, json),
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    fn assert_run(parsed: Parsed) -> (Command, bool) {
        match parsed {
            Parsed::Run { command, json } => (command, json),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn a_forbidden_command_does_not_exist() {
        for cmd in [
            "message", "chat", "rooms", "profile", "settings", "shares", "upload", "library",
            "play", "delete", "daemon", "unknown",
        ] {
            let (_, _) = assert_usage(parse(&args(&[cmd])));
        }
    }

    #[test]
    fn unknown_command_is_a_usage_error_with_json_detection() {
        let (message, json) = assert_usage(parse(&args(&["bogus"])));
        assert!(message.contains("unknown command"));
        assert!(!json);
        let (_, json) = assert_usage(parse(&args(&["bogus", "--json"])));
        assert!(json);
    }

    #[test]
    fn empty_argv_shows_global_help() {
        assert_eq!(parse(&[]), Parsed::Help(super::super::help::global()));
        assert_eq!(
            parse(&args(&["--help"])),
            Parsed::Help(super::super::help::global())
        );
        assert_eq!(
            parse(&args(&["-h"])),
            Parsed::Help(super::super::help::global())
        );
    }

    #[test]
    fn per_command_help_short_circuits_other_errors() {
        // Even with an otherwise-invalid invocation, --help wins.
        assert_eq!(
            parse(&args(&["search", "--help"])),
            Parsed::Help(super::super::help::search())
        );
        assert_eq!(
            parse(&args(&["retry", "--help"])),
            Parsed::Help(super::super::help::retry())
        );
    }

    #[test]
    fn unknown_or_duplicate_flag_is_a_usage_error() {
        assert_usage(parse(&args(&["status", "--nope"])));
        assert_usage(parse(&args(&["cancel", "1", "--json", "--json"])));
        assert_usage(parse(&args(&["search", "q", "-x"])));
    }

    #[test]
    fn search_requires_exactly_one_query() {
        assert_usage(parse(&args(&["search"])));
        assert_usage(parse(&args(&["search", "a", "b"])));
        let (command, json) = assert_run(parse(&args(&["search", "music", "--json"])));
        assert!(json);
        assert_eq!(
            command,
            Command::Search(SearchArgs {
                query: "music".into(),
                formats: vec![],
                min_bitrate: None,
                limit: None,
                wait_secs: None,
            })
        );
    }

    #[test]
    fn search_accepts_one_format_and_numeric_flags() {
        let (command, _) = assert_run(parse(&args(&[
            "search",
            "music",
            "--flac",
            "--min-bitrate",
            "320",
            "--limit",
            "5",
            "--wait",
            "10",
        ])));
        assert_eq!(
            command,
            Command::Search(SearchArgs {
                query: "music".into(),
                formats: vec![Format::Flac],
                min_bitrate: Some(320),
                limit: Some(5),
                wait_secs: Some(10),
            })
        );
    }

    #[test]
    fn search_rejects_two_format_flags() {
        let (message, _) = assert_usage(parse(&args(&["search", "music", "--flac", "--mp3"])));
        assert!(message.contains("at most one"));
    }

    #[test]
    fn search_rejects_non_numeric_flag_values() {
        let (message, _) = assert_usage(parse(&args(&["search", "music", "--limit", "many"])));
        assert!(message.contains("--limit"));
    }

    #[test]
    fn browse_takes_user_and_optional_folder() {
        assert_usage(parse(&args(&["browse"])));
        let (command, _) = assert_run(parse(&args(&["browse", "alice"])));
        assert_eq!(
            command,
            Command::Browse(BrowseArgs {
                user: "alice".into(),
                folder: None,
            })
        );
        let (command, _) = assert_run(parse(&args(&["browse", "alice", r"MusicAlbum"])));
        assert_eq!(
            command,
            Command::Browse(BrowseArgs {
                user: "alice".into(),
                folder: Some(r"MusicAlbum".into()),
            })
        );
        assert_usage(parse(&args(&["browse", "alice", "folder", "extra"])));
    }

    #[test]
    fn download_accepts_result_id_or_user_and_path() {
        let (command, _) = assert_run(parse(&args(&["download", "12.1"])));
        assert_eq!(
            command,
            Command::Download(DownloadArgs {
                target: Target::Result {
                    result_id: "12.1".into()
                },
                to: None,
                wait: false,
            })
        );
        let (command, _) = assert_run(parse(&args(&[
            "download",
            "alice",
            r"MusicSong.flac",
            "--to",
            "/tmp/out",
            "--wait",
        ])));
        assert_eq!(
            command,
            Command::Download(DownloadArgs {
                target: Target::Path {
                    user: "alice".into(),
                    path: r"MusicSong.flac".into(),
                },
                to: Some(PathBuf::from("/tmp/out")),
                wait: true,
            })
        );
        assert_usage(parse(&args(&["download"])));
        assert_usage(parse(&args(&["download", "a", "b", "c"])));
    }

    #[test]
    fn transfers_id_is_optional() {
        let (command, _) = assert_run(parse(&args(&["transfers"])));
        assert_eq!(command, Command::Transfers(TransferArgs { id: None }));
        let (command, _) = assert_run(parse(&args(&["transfers", "45"])));
        assert_eq!(
            command,
            Command::Transfers(TransferArgs {
                id: Some("45".into())
            })
        );
        assert_usage(parse(&args(&["transfers", "45", "99"])));
    }

    #[test]
    fn retry_requires_id_and_other_source_flag() {
        assert_usage(parse(&args(&["retry", "45"])));
        assert_usage(parse(&args(&["retry", "--other-source"])));
        let (command, _) = assert_run(parse(&args(&["retry", "45", "--other-source"])));
        assert_eq!(command, Command::Retry(IdArgs { id: "45".into() }));
    }

    #[test]
    fn status_takes_no_positionals() {
        let (command, _) = assert_run(parse(&args(&["status"])));
        assert_eq!(command, Command::Status);
        assert_usage(parse(&args(&["status", "extra"])));
    }

    #[test]
    fn cancel_requires_exactly_one_id() {
        assert_usage(parse(&args(&["cancel"])));
        let (command, _) = assert_run(parse(&args(&["cancel", "45"])));
        assert_eq!(command, Command::Cancel(IdArgs { id: "45".into() }));
        assert_usage(parse(&args(&["cancel", "45", "99"])));
    }

    #[test]
    fn command_kind_matches_expected_payload() {
        assert_eq!(Command::Status.kind(), Kind::Status);
        assert_eq!(
            Command::Cancel(IdArgs { id: "1".into() }).kind(),
            Kind::Transfers
        );
        assert_eq!(
            Command::Browse(BrowseArgs {
                user: "a".into(),
                folder: None
            })
            .kind(),
            Kind::Browse
        );
    }

    /// The CLI relies entirely on `control::ErrorCode::exit_code` for exit
    /// codes; pin the mapping from `AGENTS.md`/the Tech Lead's spec here too.
    #[test]
    fn exit_code_mapping_matches_spec() {
        use ErrorCode::*;
        for (code, expected) in [
            (Internal, 1),
            (BadRequest, 2),
            (NotConnected, 3),
            (NotFound, 4),
            (RateLimited, 5),
            (TransferFailed, 6),
            (Forbidden, 7),
            (Busy, 1),
        ] {
            assert_eq!(code.exit_code(), expected, "{code:?}");
        }
    }
}
