//! Public v1 response payloads. Unadvertised metadata is represented by null.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchData {
    pub groups: Vec<SearchGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchGroup {
    pub id: String,
    pub user: String,
    pub folder: String,
    pub files: Vec<FileData>,
    /// Lowercase extension; "mixed" when the folder has multiple formats.
    pub format: String,
    pub bitrate_kbps: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub size_bytes: u64,
    pub file_count: usize,
    pub free_slot: bool,
    pub queue_position: Option<u32>,
    pub speed_bytes_per_sec: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileData {
    /// Full remote virtual path.
    pub path: String,
    pub size_bytes: u64,
    pub format: String,
    pub bitrate_kbps: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub duration_secs: Option<u32>,
    pub vbr: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseData {
    pub user: String,
    pub folders: Vec<BrowseFolder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseFolder {
    pub path: String,
    pub files: Vec<FileData>,
}

/// Used by transfers, download, retry and cancel. IDs are opaque strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransfersData {
    pub transfers: Vec<TransferData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferData {
    pub id: String,
    pub state: TransferState,
    pub bytes: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_sec: f64,
    pub queue_position: Option<u32>,
    pub user: String,
    pub path: String,
    pub error: Option<String>,
    pub origin: TransferOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Queued,
    Downloading,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrigin {
    Cli,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    App,
    Daemon,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusData {
    pub connected: bool,
    pub mode: Mode,
    /// Configured maximum idle duration, not a countdown; null in app mode.
    pub idle_timeout_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Response;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn response_round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(data: T) {
        let response = Response::success(serde_json::to_value(&data).unwrap());
        let wire = serde_json::to_string(&response).unwrap();
        let decoded: Response = serde_json::from_str(&wire).unwrap();
        assert!(decoded.ok);
        assert_eq!(
            serde_json::from_value::<T>(decoded.data.unwrap()).unwrap(),
            data
        );
    }

    fn file() -> FileData {
        FileData {
            path: r"Music\Album\01 Song.flac".into(),
            size_bytes: 123,
            format: "flac".into(),
            bitrate_kbps: None,
            bit_depth: Some(16),
            sample_rate_hz: Some(44100),
            duration_secs: Some(45),
            vbr: None,
        }
    }

    #[test]
    fn search_response_round_trips() {
        response_round_trip(SearchData {
            groups: vec![SearchGroup {
                id: "12.1".into(),
                user: "peer".into(),
                folder: r"Music\Album".into(),
                files: vec![file()],
                format: "flac".into(),
                bitrate_kbps: None,
                bit_depth: Some(16),
                sample_rate_hz: Some(44100),
                size_bytes: 123,
                file_count: 1,
                free_slot: true,
                queue_position: None,
                speed_bytes_per_sec: 4000,
            }],
        });
    }

    #[test]
    fn browse_response_round_trips() {
        response_round_trip(BrowseData {
            user: "peer".into(),
            folders: vec![BrowseFolder {
                path: r"Music\Album".into(),
                files: vec![file()],
            }],
        });
    }

    #[test]
    fn transfer_response_round_trips() {
        response_round_trip(TransfersData {
            transfers: vec![TransferData {
                id: "45".into(),
                state: TransferState::Downloading,
                bytes: 12,
                total_bytes: 123,
                speed_bytes_per_sec: 25.5,
                queue_position: None,
                user: "peer".into(),
                path: file().path,
                error: None,
                origin: TransferOrigin::Cli,
            }],
        });
    }

    #[test]
    fn status_response_round_trips() {
        for (mode, idle_timeout_secs) in [(Mode::App, None), (Mode::Daemon, Some(600))] {
            response_round_trip(StatusData {
                connected: true,
                mode,
                idle_timeout_secs,
            });
        }
    }
}
