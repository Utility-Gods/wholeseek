//! Renders a `Response` as compact text or as the `data`/`error` JSON the
//! Tech Lead's spec requires, independent of any real I/O so it is fully
//! unit-testable. `execute` does the work and returns what to print;
//! `run` (in `mod.rs`) does the actual `println!`/`eprintln!`.

use super::parse::{self, Command, Kind, Parsed};
use serde_json::Value;
use wholeseek_core::control::{
    BrowseData, Caller, ErrorCode, Failure, Mode, Request, Response, SearchData, SearchGroup,
    StatusData, TransferState, TransfersData,
};

/// What the caller of `run` should do: print at most one line/block to each
/// stream, then exit with `code`.
pub struct Outcome {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub code: i32,
}

impl Outcome {
    fn ok_stdout(text: String) -> Self {
        Self {
            stdout: Some(text),
            stderr: None,
            code: 0,
        }
    }

    fn usage(message: String, json: bool) -> Self {
        let code = ErrorCode::BadRequest.exit_code().into();
        if json {
            Self {
                stdout: Some(failure_json(&Failure::new(ErrorCode::BadRequest, &message))),
                stderr: None,
                code,
            }
        } else {
            Self {
                stdout: None,
                stderr: Some(message),
                code,
            }
        }
    }

    fn failed(failure: &Failure, json: bool) -> Self {
        let code = failure.code.exit_code().into();
        if json {
            Self {
                stdout: Some(failure_json(failure)),
                stderr: None,
                code,
            }
        } else {
            Self {
                stdout: None,
                stderr: Some(format!("wholeseek: {}", failure.message)),
                code,
            }
        }
    }
}

/// Parses `args`, calls `caller` for a `Run` outcome, and renders the
/// result. Never panics on a malformed response: a payload that doesn't
/// match its expected shape falls back to printing the raw JSON.
pub fn execute(args: &[String], caller: &mut dyn Caller) -> Outcome {
    match parse::parse(args) {
        Parsed::Help(text) => Outcome::ok_stdout(text),
        Parsed::UsageError { message, json } => Outcome::usage(message, json),
        Parsed::Run { command, json } => run_command(command, json, caller),
    }
}

fn run_command(command: Command, json: bool, caller: &mut dyn Caller) -> Outcome {
    let kind = command.kind();
    let request = Request::new(command.into_operation());
    match caller.call(&request) {
        Ok(response) => render_response(&response, kind, json),
        Err(e) => {
            // An I/O failure before any application-level response (the local
            // session could not be reached or started at all): report it as
            // an internal error, not a specific protocol failure code.
            let failure = Failure::new(ErrorCode::Internal, &e.to_string());
            Outcome::failed(&failure, json)
        }
    }
}

fn render_response(response: &Response, kind: Kind, json: bool) -> Outcome {
    if !response.ok {
        let failure = response
            .error
            .clone()
            .unwrap_or_else(|| Failure::new(ErrorCode::Internal, "missing error detail"));
        return Outcome::failed(&failure, json);
    }
    let data = response.data.clone().unwrap_or(Value::Null);
    if json {
        return Outcome::ok_stdout(serde_json::to_string(&data).unwrap_or_else(|_| "null".into()));
    }
    Outcome::ok_stdout(render_text(kind, &data))
}

fn failure_json(failure: &Failure) -> String {
    let value = serde_json::json!({ "error": failure });
    serde_json::to_string(&value).unwrap_or_else(|_| {
        r#"{"error":{"code":"internal","message":"could not encode error"}}"#.into()
    })
}

fn render_text(kind: Kind, data: &Value) -> String {
    match kind {
        Kind::Search => render_search(data),
        Kind::Browse => render_browse(data),
        Kind::Transfers => render_transfers(data),
        Kind::Status => render_status(data),
    }
}

fn render_search(data: &Value) -> String {
    let Ok(parsed) = serde_json::from_value::<SearchData>(data.clone()) else {
        return data.to_string();
    };
    if parsed.groups.is_empty() {
        return "no results".into();
    }
    let mut lines = vec![format!(
        "{} result{}:",
        parsed.groups.len(),
        plural(parsed.groups.len())
    )];
    for (i, g) in parsed.groups.iter().enumerate() {
        lines.push(format!(
            "  {}. [{}] {}  {}  {}  {} file{}  {}  {}  {}",
            i + 1,
            g.id,
            g.user,
            crumbs(&g.folder),
            group_quality(g),
            g.file_count,
            plural(g.file_count),
            fmt_size(g.size_bytes),
            slot_text(g),
            fmt_speed(f64::from(g.speed_bytes_per_sec)),
        ));
    }
    lines.join("\n")
}

fn render_browse(data: &Value) -> String {
    let Ok(parsed) = serde_json::from_value::<BrowseData>(data.clone()) else {
        return data.to_string();
    };
    if parsed.folders.is_empty() {
        return format!("{}: no folders", parsed.user);
    }
    let mut lines = vec![format!("{}:", parsed.user)];
    for folder in &parsed.folders {
        lines.push(format!("  {}/", crumbs(&folder.path)));
        for f in &folder.files {
            lines.push(format!(
                "    {}  {}  {}",
                file_name(&f.path),
                file_quality(&f.format, f.bit_depth, f.sample_rate_hz, f.bitrate_kbps),
                fmt_size(f.size_bytes),
            ));
        }
    }
    lines.join("\n")
}

fn render_transfers(data: &Value) -> String {
    let Ok(parsed) = serde_json::from_value::<TransfersData>(data.clone()) else {
        return data.to_string();
    };
    if parsed.transfers.is_empty() {
        return "no transfers".into();
    }
    parsed
        .transfers
        .iter()
        .map(|t| {
            let progress = if t.total_bytes == 0 {
                fmt_size(t.bytes)
            } else {
                format!(
                    "{}/{} ({}%)",
                    fmt_size(t.bytes),
                    fmt_size(t.total_bytes),
                    (t.bytes.saturating_mul(100) / t.total_bytes.max(1)).min(100)
                )
            };
            let mut line = format!(
                "{}  {}  {}  {}  {}  {}",
                t.id,
                state_text(t.state),
                progress,
                fmt_speed(t.speed_bytes_per_sec),
                t.user,
                t.path,
            );
            if let Some(error) = &t.error {
                line.push_str(&format!("  ({error})"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_status(data: &Value) -> String {
    let Ok(parsed) = serde_json::from_value::<StatusData>(data.clone()) else {
        return data.to_string();
    };
    let mut lines = vec![
        format!("connected: {}", if parsed.connected { "yes" } else { "no" }),
        format!(
            "mode: {}",
            match parsed.mode {
                Mode::App => "app",
                Mode::Daemon => "daemon",
            }
        ),
    ];
    if let Some(secs) = parsed.idle_timeout_secs {
        lines.push(format!("idle timeout: {secs}s"));
    }
    lines.join("\n")
}

fn slot_text(g: &SearchGroup) -> String {
    if g.free_slot {
        "free slot".into()
    } else {
        match g.queue_position {
            Some(p) => format!("queued #{p}"),
            None => "no free slot".into(),
        }
    }
}

fn state_text(state: TransferState) -> &'static str {
    match state {
        TransferState::Queued => "queued",
        TransferState::Downloading => "downloading",
        TransferState::Done => "done",
        TransferState::Failed => "failed",
        TransferState::Cancelled => "cancelled",
    }
}

fn group_quality(g: &SearchGroup) -> String {
    file_quality(&g.format, g.bit_depth, g.sample_rate_hz, g.bitrate_kbps)
}

fn file_quality(
    format: &str,
    bit_depth: Option<u32>,
    sample_rate_hz: Option<u32>,
    bitrate_kbps: Option<u32>,
) -> String {
    let ext = format.to_ascii_uppercase();
    match (bit_depth, sample_rate_hz, bitrate_kbps) {
        (Some(d), Some(r), _) => format!("{ext} {d}/{}", khz(r)),
        (_, _, Some(br)) => format!("{ext} {br}"),
        _ => ext,
    }
}

fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    if k.fract() == 0.0 {
        format!("{k:.0}")
    } else {
        format!("{k:.1}")
    }
}

/// `Music\\Album (2019)` → `Music/Album (2019)`.
fn crumbs(path: &str) -> String {
    path.split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn fmt_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1e6 {
        format!("{:.1} MB/s", bytes_per_sec / 1e6)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1e3)
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use wholeseek_core::control::{
        BrowseFolder, FileData, Operation, TransferData, TransferOrigin,
    };

    struct FakeCaller {
        response: io::Result<Response>,
        last_request: Option<Request>,
    }

    impl FakeCaller {
        fn ok(response: Response) -> Self {
            Self {
                response: Ok(response),
                last_request: None,
            }
        }

        fn err(message: &str) -> Self {
            Self {
                response: Err(io::Error::other(message.to_string())),
                last_request: None,
            }
        }
    }

    impl Caller for FakeCaller {
        fn call(&mut self, req: &Request) -> io::Result<Response> {
            self.last_request = Some(req.clone());
            match &self.response {
                Ok(r) => Ok(r.clone()),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    fn file() -> FileData {
        FileData {
            path: r"Music\Album\01 Song.flac".into(),
            size_bytes: 28_000_000,
            format: "flac".into(),
            bitrate_kbps: None,
            bit_depth: Some(16),
            sample_rate_hz: Some(44_100),
            duration_secs: Some(245),
            vbr: None,
        }
    }

    fn group() -> SearchGroup {
        SearchGroup {
            id: "12.1".into(),
            user: "alice".into(),
            folder: r"Music\Album".into(),
            files: vec![file()],
            format: "flac".into(),
            bitrate_kbps: None,
            bit_depth: Some(16),
            sample_rate_hz: Some(44_100),
            size_bytes: 28_000_000,
            file_count: 1,
            free_slot: true,
            queue_position: None,
            speed_bytes_per_sec: 412_000,
        }
    }

    #[test]
    fn search_text_lists_results_with_ids() {
        let data = serde_json::to_value(SearchData {
            groups: vec![group()],
        })
        .unwrap();
        let mut caller = FakeCaller::ok(Response::success(data));
        let outcome = execute(&args(&["search", "flac album"]), &mut caller);
        assert_eq!(outcome.code, 0);
        let text = outcome.stdout.unwrap();
        assert!(text.contains("1 result:"));
        assert!(text.contains("[12.1]"));
        assert!(text.contains("alice"));
        assert!(text.contains("free slot"));
        assert_eq!(
            caller.last_request.unwrap().command,
            Operation::Search(wholeseek_core::control::SearchArgs {
                query: "flac album".into(),
                formats: vec![],
                min_bitrate: None,
                limit: None,
                wait_secs: None,
            })
        );
    }

    #[test]
    fn search_json_prints_the_data_payload_only() {
        let data = serde_json::to_value(SearchData {
            groups: vec![group()],
        })
        .unwrap();
        let mut caller = FakeCaller::ok(Response::success(data.clone()));
        let outcome = execute(&args(&["search", "flac album", "--json"]), &mut caller);
        assert_eq!(outcome.code, 0);
        let printed: Value = serde_json::from_str(&outcome.stdout.unwrap()).unwrap();
        assert_eq!(printed, data);
    }

    #[test]
    fn search_with_no_results_renders_a_plain_line() {
        let data = serde_json::to_value(SearchData { groups: vec![] }).unwrap();
        let mut caller = FakeCaller::ok(Response::success(data));
        let outcome = execute(&args(&["search", "nothing"]), &mut caller);
        assert_eq!(outcome.stdout.unwrap(), "no results");
    }

    #[test]
    fn browse_text_lists_folders_and_files() {
        let data = serde_json::to_value(BrowseData {
            user: "alice".into(),
            folders: vec![BrowseFolder {
                path: r"Music\Album".into(),
                files: vec![file()],
            }],
        })
        .unwrap();
        let mut caller = FakeCaller::ok(Response::success(data));
        let outcome = execute(&args(&["browse", "alice"]), &mut caller);
        assert_eq!(outcome.code, 0);
        let text = outcome.stdout.unwrap();
        assert!(text.contains("alice:"));
        assert!(text.contains("Music/Album/"));
        assert!(text.contains("01 Song.flac"));
    }

    #[test]
    fn transfers_text_shows_progress_and_error() {
        let data = serde_json::to_value(TransfersData {
            transfers: vec![TransferData {
                id: "45".into(),
                state: TransferState::Failed,
                bytes: 10,
                total_bytes: 100,
                speed_bytes_per_sec: 0.0,
                queue_position: None,
                user: "alice".into(),
                path: file().path,
                error: Some("peer disconnected".into()),
                origin: TransferOrigin::Cli,
            }],
        })
        .unwrap();
        let mut caller = FakeCaller::ok(Response::success(data));
        let outcome = execute(&args(&["transfers"]), &mut caller);
        let text = outcome.stdout.unwrap();
        assert!(text.contains("45"));
        assert!(text.contains("failed"));
        assert!(text.contains("10%"));
        assert!(text.contains("(peer disconnected)"));
    }

    #[test]
    fn status_text_shows_mode_and_idle_timeout() {
        let data = serde_json::to_value(StatusData {
            connected: true,
            mode: Mode::Daemon,
            idle_timeout_secs: Some(600),
        })
        .unwrap();
        let mut caller = FakeCaller::ok(Response::success(data));
        let outcome = execute(&args(&["status"]), &mut caller);
        let text = outcome.stdout.unwrap();
        assert!(text.contains("connected: yes"));
        assert!(text.contains("mode: daemon"));
        assert!(text.contains("idle timeout: 600s"));
    }

    #[test]
    fn application_failure_renders_text_and_exits_with_mapped_code() {
        let failure = Failure::new(ErrorCode::NotFound, "unknown transfer ID");
        let mut caller = FakeCaller::ok(Response::failure(failure));
        let outcome = execute(&args(&["cancel", "99"]), &mut caller);
        assert_eq!(outcome.code, 4);
        assert!(outcome.stdout.is_none());
        assert!(outcome.stderr.unwrap().contains("unknown transfer ID"));
    }

    #[test]
    fn application_failure_renders_json_with_retry_after() {
        let failure = Failure {
            code: ErrorCode::RateLimited,
            message: "search rate limit reached".into(),
            retry_after_secs: Some(12),
        };
        let mut caller = FakeCaller::ok(Response::failure(failure));
        let outcome = execute(&args(&["search", "q", "--json"]), &mut caller);
        assert_eq!(outcome.code, 5);
        let printed: Value = serde_json::from_str(&outcome.stdout.unwrap()).unwrap();
        assert_eq!(printed["error"]["code"], "rate_limited");
        assert_eq!(printed["error"]["retry_after_secs"], 12);
    }

    #[test]
    fn io_failure_is_reported_as_an_internal_error() {
        let mut caller = FakeCaller::err("daemon did not start");
        let outcome = execute(&args(&["status"]), &mut caller);
        assert_eq!(outcome.code, 1);
        assert!(outcome.stderr.unwrap().contains("daemon did not start"));
    }

    #[test]
    fn usage_error_never_calls_the_caller() {
        let mut caller = FakeCaller::err("must not be called");
        let outcome = execute(&args(&["search"]), &mut caller);
        assert_eq!(outcome.code, 2);
        assert!(caller.last_request.is_none());
        assert!(outcome.stderr.is_some());
    }

    #[test]
    fn usage_error_as_json_still_goes_to_stdout() {
        let mut caller = FakeCaller::err("must not be called");
        let outcome = execute(&args(&["bogus", "--json"]), &mut caller);
        assert_eq!(outcome.code, 2);
        let printed: Value = serde_json::from_str(&outcome.stdout.unwrap()).unwrap();
        assert_eq!(printed["error"]["code"], "bad_request");
    }

    #[test]
    fn help_never_calls_the_caller() {
        let mut caller = FakeCaller::err("must not be called");
        let outcome = execute(&args(&["--help"]), &mut caller);
        assert_eq!(outcome.code, 0);
        assert!(caller.last_request.is_none());
        assert!(outcome.stdout.is_some());
    }
}
