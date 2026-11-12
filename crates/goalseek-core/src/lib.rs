//! goalseek-core: Soulseek session, search, browse, transfers, shares and config.
//!
//! The UI talks to [`Core`] (commands, non-blocking) and drains
//! [`CoreEvent`]s from one channel. See `docs/core-api.md` for the library
//! survey and the design.

pub mod cache;
pub mod config;
mod core;
mod event;
mod model;
mod net;
pub mod paths;
mod port_mapping;
pub mod rank;
pub mod status;
mod worker;

pub use crate::core::{Command, Core, NOT_IMPLEMENTED, POLL_TICK};
pub use crate::worker::PLAY_STALLED;
pub use event::{CoreEvent, DisconnectReason, PortMapOutcome};
pub use model::{
    AudioAttrs, BrowseDir, Credentials, PlayTicket, RemoteFile, SearchHit, SearchId, Settings,
    TransferId, TransferKind, UploadRow, UploadState, UserProfile,
};
pub use paths::{DOWNLOAD_SKIP, is_download_junk};
pub use status::{
    CacheUsage, CoreStatus, ListenStatus, ReconnectStatus, SearchProgress, ShareStatus,
};

/// Client version goalseek reports on login. Major 176 is reserved for
/// soulseek-rs-based clients; minor 1 is "unidentified" until we reserve our
/// own minor upstream (gap G9).
pub const CLIENT_VERSION: soulseek_rs::ClientVersion = soulseek_rs::ClientVersion {
    major: soulseek_rs::ClientVersion::MAJOR,
    minor: 1,
};
