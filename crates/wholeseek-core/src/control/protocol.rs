use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

pub const VERSION: u16 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u16,
    #[serde(flatten)]
    pub command: Operation,
}

impl Request {
    pub const fn new(command: Operation) -> Self {
        Self {
            v: VERSION,
            command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "cmd",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Operation {
    Search(SearchArgs),
    Browse(BrowseArgs),
    Download(DownloadArgs),
    Transfers(TransferArgs),
    RetryOtherSource(IdArgs),
    Status,
    Cancel(IdArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Flac,
    Mp3,
    Wav,
    Lossless,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default)]
    pub formats: Vec<Format>,
    pub min_bitrate: Option<u32>,
    pub limit: Option<usize>,
    pub wait_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowseArgs {
    pub user: String,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Result { result_id: String },
    Path { user: String, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadArgs {
    pub target: Target,
    pub to: Option<PathBuf>,
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferArgs {
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    BadRequest,
    NotConnected,
    NotFound,
    RateLimited,
    TransferFailed,
    Forbidden,
    Busy,
}

impl ErrorCode {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Internal | Self::Busy => 1,
            Self::BadRequest => 2,
            Self::NotConnected => 3,
            Self::NotFound => 4,
            Self::RateLimited => 5,
            Self::TransferFailed => 6,
            Self::Forbidden => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl Failure {
    pub fn new(code: ErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub v: u16,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

impl Response {
    pub fn success(data: Value) -> Self {
        Self {
            v: VERSION,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub const fn failure(error: Failure) -> Self {
        Self {
            v: VERSION,
            ok: false,
            data: None,
            error: Some(error),
        }
    }
}

/// Bounds memory use before JSON deserialization, even if a peer never sends LF.
pub fn read_frame(reader: &mut impl BufRead, max: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader
        .take((max as u64).saturating_add(1))
        .read_until(b'\n', &mut line)?;
    if line.len() > max || line.pop() != Some(b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid NDJSON frame",
        ));
    }
    Ok(line)
}

pub fn write_frame(writer: &mut impl Write, value: &impl Serialize, max: usize) -> io::Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() >= max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized NDJSON frame",
        ));
    }
    writer.write_all(&data)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub fn read_request(reader: &mut impl BufRead) -> Result<Request, Failure> {
    let bad = || Failure::new(ErrorCode::BadRequest, "invalid request frame or arguments");
    let line = read_frame(reader, MAX_REQUEST_BYTES).map_err(|_| bad())?;
    // Classify unknown commands without echoing any untrusted input. Decode the
    // original bytes into the strict type so duplicate fields still fail.
    let raw: Value = serde_json::from_slice(&line).map_err(|_| bad())?;
    let cmd = raw.get("cmd").and_then(Value::as_str).ok_or_else(bad)?;
    if !matches!(
        cmd,
        "search" | "browse" | "download" | "transfers" | "retry_other_source" | "status" | "cancel"
    ) {
        return Err(Failure::new(ErrorCode::Forbidden, "command is not allowed"));
    }
    let request: Request = serde_json::from_slice(&line).map_err(|_| bad())?;
    if request.v != VERSION {
        return Err(Failure::new(
            ErrorCode::BadRequest,
            "unsupported protocol version",
        ));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_whitelist_is_accepted() {
        for cmd in [
            "message", "chat", "rooms", "profile", "settings", "shares", "upload", "library",
            "play", "delete", "unknown",
        ] {
            let line = format!("{{\"v\":1,\"cmd\":\"{cmd}\"}}\n");
            assert_eq!(
                read_request(&mut line.as_bytes()).unwrap_err().code,
                ErrorCode::Forbidden
            );
        }
        let request = Request::new(Operation::Search(SearchArgs {
            query: "music".into(),
            formats: vec![Format::Flac],
            min_bitrate: None,
            limit: Some(5),
            wait_secs: None,
        }));
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request, MAX_REQUEST_BYTES).unwrap();
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), request);
    }

    #[test]
    fn rejects_extra_duplicate_or_unknown_fields() {
        for line in [
            r#"{"v":1,"cmd":"status","extra":true}"#,
            r#"{"v":1,"cmd":"status","args":{"extra":true}}"#,
            r#"{"v":1,"v":1,"cmd":"status"}"#,
            r#"{"v":1,"cmd":"status","cmd":"status"}"#,
            r#"{"v":1,"cmd":"search","args":{"query":"test","extra":true}}"#,
            r#"{"v":1,"cmd":"search","args":{"query":"test","formats":["unknown"]}}"#,
            r#"{"v":1,"cmd":"download","args":{"target":{"kind":"path","user":"x","path":"y","extra":true}}}"#,
        ] {
            let line = format!("{line}\n");
            assert_eq!(
                read_request(&mut line.as_bytes()).unwrap_err().code,
                ErrorCode::BadRequest,
                "{line}"
            );
        }
    }

    #[test]
    fn rejects_oversized_or_unterminated_line() {
        let bytes = vec![b'x'; MAX_REQUEST_BYTES * 2];
        let mut input = bytes.as_slice();
        assert_eq!(
            read_request(&mut input).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(input.len(), MAX_REQUEST_BYTES - 1);
        assert!(read_request(&mut br#"{"v":1,"cmd":"status"}"#.as_slice()).is_err());
    }
}
