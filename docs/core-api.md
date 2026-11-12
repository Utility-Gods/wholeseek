# wholeseek-core — library survey & core API (CP0, implemented in CP1)

Library: `soulseek-rs-lib` @ `434dcc0155ed87203cafcd1cf7b1a1073f88a63d` (lib crate `soulseek_rs`).
File references are relative to `soulseek-rs-lib/src/` in that revision unless prefixed
(`soulseek-rs/…` = the reference CLI/TUI binary crate in the same repo).
Everything below was read from source; **nothing was exercised against the live network** (no
credentials authorised at CP0).

---

## 1. Survey

### 1.1 Public surface and threading model

- Public modules: `client`, `error`, `message`, `shares`, `types`, `utils`; the actor system, peer
  plumbing and download store are `pub(crate)` (`lib.rs:9-17`). Re-exports: `Client`,
  `ClientSettings`, `CancelHandle`, `SearchResult`, `File`, `DownloadStatus`, `UploadInfo`,
  `UploadStatus`, `SessionLoss`, `SharedDirectory`, `SharedFileEntry`, … (`lib.rs:34-44`).
- Zero dependencies apart from `libc` on unix (`soulseek-rs-lib/Cargo.toml`). **No async**: plain
  `std::thread` + `std::sync::mpsc`. Every actor (server connection, each peer) owns an OS thread
  with a 256 KiB stack (`actor/mod.rs:60-73`); the listener runs on its own thread
  (`client/connection.rs:137-147`); `connect()` starts an operations-dispatch thread
  (`client/connection.rs:150-154`).
- `Client` (`client/mod.rs:656-675`) is not `Clone`. `connect`/`disconnect` take `&mut self`
  (`client/connection.rs:50,170`); everything else takes `&self` and goes through an
  `Arc<RwLock<ClientContext>>`. `Client: Send` (asserted by our test
  `core::tests::library_client_is_send`), so it can live on our worker thread.
- **Event delivery is mostly polling.** Only per-download status is pushed (an
  `mpsc::Receiver<DownloadStatus>` per download, `client/downloads.rs:112-182`). Search results,
  browse listings, folder contents, uploads, session loss and share counts are read by calling
  accessors (`get_search_results`, `take_browse_result`, `take_folder_contents`, `uploads`,
  `take_upload_events`, `session_loss`, `shared_counts`). wholeseek-core therefore polls on a tick.
- Logging: a global logger to stderr (ANSI colours), configured by env `LOG_LEVEL`/`RUST_LOG`
  and `LOG_FILE`; `Client::with_settings` calls `logger::init()` (`client/mod.rs:693`,
  `utils/logger.rs:41-95`). Default level Warn. Login logs the username only
  (`client/connection.rs:211`); no password logging found.

### 1.2 Login / session

| Need | API | Ref |
|---|---|---|
| Configure | `ClientSettings { username, password, server_address, enable_listen, listen_port, shared_directories, accept_children, version }` | `client/mod.rs:231-282` |
| Connect | `Client::connect(&mut self) -> Result<()>`: scans shares **synchronously**, binds the listener, spawns server actor | `client/connection.rs:50-157` |
| Log in | `Client::login(&self) -> Result<bool>`; blocks ≤45 s; `Ok(true)` or `Err(AuthenticationFailed)` / `Err(Timeout)` | `client/connection.rs:210-240`, `actor/server_actor/handlers.rs:481-508` |
| Session lost? | `Client::session_loss() -> Option<SessionLoss>` (`Displaced` / `Disconnected`) — polled | `client/mod.rs:747-749`, `types.rs:554-560` |
| Disconnect | `Client::disconnect(&mut self)`, also on `Drop` | `client/connection.rs:170-201`, `client/mod.rs:677-681` |
| Reconnect | **none** — "Nothing reconnects it, so the session is over" | `actor/server_actor.rs:605-623` |
| Client version | `ClientVersion { major: 176, minor }`; minor 1 = unidentified; projects should reserve a minor | `types.rs:13-56` |

### 1.3 Network search (+ streaming)

- `Client::search(query, timeout) -> Result<Vec<SearchResult>>` blocks for the whole window
  (`client/search.rs:249-255`, `287-296`, `collect_for` `382-396`).
- Results accumulate **keyed by the query string** in `ctx.searches` (`client/search.rs:350-363`),
  so a non-blocking caller can stream them by polling `get_search_results_count(query)` /
  `get_search_results(query)` / `search_file_counts()` (`client/search.rs:398-478`). There is no
  public non-blocking "start search" for normal searches: `send_search` is private; the public
  non-blocking variants are wishlist / user / room searches (`client/search.rs:270,325,339`). A
  worker can run `search_with_cancel` on a helper thread while polling results from another — the
  `Client` methods are `&self` + lock-based, so that is safe (`Client` must be in an `Arc`).
- Results keep arriving after the window until `forget_search(query)` (`client/search.rs:437-441`).
- Caps: 500 responses / 10 000 files per search (`types.rs:81-109`).
- `SearchResult { token, files: Vec<File>, slots: u8, speed: u32, username }`,
  `File { username, name (full virtual path), size, attribs: HashMap<u32,u32> }` (`types.rs:5-66`).
  Queue length is not parsed (`types.rs:152-153` reads slots and speed only).
- Attribute codes: 0 bitrate, 1 duration, 2 VBR, 4 sample rate, 5 bit depth
  (`message/peer/shared_file_list.rs:19-28`).

### 1.4 Browse user / folder contents

| Need | API | Ref |
|---|---|---|
| Browse | `browse_user(username) -> Result<()>`; then poll `take_browse_result(username) -> Option<Vec<SharedDirectory>>` | `client/rooms.rs:237-242`, `287-295` |
| One folder | `request_folder_contents(username, folder)`; poll `take_folder_contents(username, folder)` | `client/social.rs:207-231` |
| Shape | `SharedDirectory { name (full virtual path), files: Vec<SharedFileEntry { name (basename), size, attributes: Vec<(u32,u32)> }> }` | `message/peer/shared_file_list.rs:10-36` |

Requests to unconnected peers are queued and the peer's address looked up
(`client/rooms.rs:249-282`); queued peer messages expire after 1 min (`client/mod.rs:65`). There is
no failure/timeout signal for a browse that never answers.

### 1.5 Download, progress, `.part`, cancel

- `download(filename, username, size, download_directory) -> Result<(Download, Receiver<DownloadStatus>)>`
  returns as soon as it is queued (`client/downloads.rs:96-110`, `112-182`). **`download_directory`
  is per call**, so play→cache and download→downloads is supported per transfer.
- Speed limit (downloads only, global): `set_download_speed_limit(bps)` (`client/downloads.rs:17-21`).
- `DownloadStatus = Queued | InProgress{bytes_downloaded,total_bytes,speed_bytes_per_sec} |
  Paused{..} | Completed | Cancelled | Failed(Option<String>) | TimedOut` (`types.rs:226-257`).
- Paths: `final = expand_tilde(dir) / basename(remote)`; `part = final + ".part"`
  (`peer/download_peer.rs:74-125`, `utils/path.rs:7-17`). `part_path_of` exists but is in a
  `pub(crate)` module (`peer/download_peer.rs:85`, `lib.rs:15`) — we reproduce the rule in
  `wholeseek_core::paths` (unit-tested).
- Write path: `PartFile::open` creates parent dirs, **resumes an existing smaller `.part`**
  (appends, requests offset), restarts one ≥ size (`peer/download_peer.rs:137-165`); writes are
  unbuffered `File::write_all` (`:169-178`), so bytes are visible to another reader immediately.
- Progress: one `InProgress` right after the `.part` is opened, before payload
  (`start_transfer`, `peer/download_peer.rs:306-318`), then every 128 KiB
  (`PROGRESS_INTERVAL_BYTES`, `:24`, loop `:403-423`); reads are 64 KiB (`:18`). Stall deadline 30 s
  (`:20`, `:429-436`).
- Completion: `.part` renamed to final under the store lock, then `Completed` sent — **with no
  path** (`peer/download_peer.rs:487-530`, `191-202`). Failure → `Failed(Some(reason))`, `.part`
  kept for resume (`:189-202`, `:600-609`).
- Cancel: `cancel_download(username, filename) -> bool` marks it cancelled and **deletes the
  `.part`** (`client/downloads.rs:45-61`); keyed by (user, filename), not token. Also
  `pause_download`/`resume_download`/`remove_download`/`remove_queued_download`
  (`client/downloads.rs:23-94`). Snapshot: `get_all_downloads()` (`:9-15`).
- Remote queue place: `request_place_in_queue` → `Download.queue_position` (`client/social.rs:243-250`).

**Play-while-downloading verdict: feasible.** The `.part` path is deterministic before the call,
exists by the first `InProgress` event, grows unbuffered in ≥128 KiB steps, and on Linux an open fd
survives the rename to the final name.

### 1.6 Shares

| Need | API | Ref |
|---|---|---|
| Set before connect | `ClientSettings.shared_directories` (scanned in `connect`) | `client/mod.rs:240`, `client/connection.rs:70-93` |
| Set / rescan at runtime | `set_shared_directories(dirs) -> Result<()>`: rescans **synchronously**, swaps index, announces counts; `Err(NotConnected)` if no server (index still swapped) | `client/mod.rs:848-878` |
| Counts | `shared_counts() -> (folders, files)` | `client/mod.rs:778-782` |
| Listing as peers see it | `shared_listing()` | `client/mod.rs:769-774` |
| Index | `Shares::scan_many` (skips unreadable roots, de-dups root names, reads audio attrs) | `shares/mod.rs:61-91`, `shares/audio.rs:24` |

No separate rescan call: rescan = `set_shared_directories(same dirs)`.

### 1.7 Uploads

| Need | API | Ref |
|---|---|---|
| Queue | handled by the library (privileged-first, then FIFO) | `client/upload_queue.rs:57-125` |
| Slots | `set_upload_slots(n)` (min 1, default 10) | `client/uploads.rs:60-64`, `client/mod.rs:57` |
| Snapshot | `uploads() -> Vec<UploadInfo>`; `take_upload_events()` for transitions between polls | `client/mod.rs:788-819`, `client/uploads.rs:51-56` |
| Cancel | `cancel_upload(username, filename) -> bool` | `client/mod.rs:824-840` |
| Speed limit | **none** | — |

`UploadInfo { username, filename, size, bytes_sent, status, speed_bytes_per_sec }`,
`UploadStatus = Queued(place) | InProgress | Completed | Cancelled | Failed(String)` (`types.rs:523-545`).

### 1.8 Listen port / UPnP

- `ClientSettings.listen_port` (default 2234), `enable_listen` (default true)
  (`client/mod.rs:37,236-237`). Bound in `connect`; if taken, **silently falls back to an
  ephemeral port** (`peer/listen.rs:419-430`); `listen_port()` reports the real one
  (`client/mod.rs:738-740`). Changing the port needs disconnect + new `Client`.
- **UPnP / NAT-PMP is not in the library.** It lives in the binary crate
  (`soulseek-rs/src/port_mapping/mod.rs:1-80`: `PortMapper::spawn(port)`, UPnP via `igd-next`,
  hand-rolled std-only NAT-PMP in `soulseek-rs/src/port_mapping/nat_pmp.rs`), with `igd-next`
  as a dependency of the binary only (`soulseek-rs/Cargo.toml:38`).

---

## 2. Gaps

| # | Gap | Impact | Proposed handling |
|---|---|---|---|
| G1 | No upload speed limit (only `set_download_speed_limit`). | Settings "speed limit" (MVP) can't be honoured. | **Decided (TL): deferred, out of MVP.** Upload slot cap only; the library is not patched. `Settings` has no upload-limit field. |
| G2 | No UPnP/NAT-PMP in the lib; MVP doc credits the lib with it. | Firewalled users unreachable without manual forwarding. | **Decided (TL): wholeseek-core owns port mapping — CP1 item.** Port `soulseek-rs/src/port_mapping/{mod,nat_pmp}.rs` (MIT) into wholeseek-core, keeping the soulseek-rs copyright/MIT attribution notice in the ported file; UPnP via `igd-next`, NAT-PMP std-only. See §3.1. |
| G3 | Events are polled, not pushed (except per-download status). | Need a worker with a tick (~150 ms). | `Core` worker polls; emits deltas. |
| G4 | No public non-blocking network search; results keyed by query string (same query twice shares/replaces one entry). | Streaming needs a helper thread; duplicate queries collide. | Run `search_with_cancel` on a helper thread with `Arc<Client>`, poll `get_search_results` for deltas; one live search per query string in the core; `forget_search` on cancel/close. |
| G5 | `.part`/final path not reported; `Completed` carries no path. Same basename into the same dir collides (shared `.part`, resume across unrelated files, rename overwrites). | Must derive paths ourselves; must avoid collisions. | `paths::part_path/final_path` reproduce the rule; per-transfer cache dirs `cache/t<id>/`; downloads into `downloads/<folder>/` with a core-side collision check. |
| G6 | No auto-reconnect; `Displaced` vs `Disconnected` only by polling `session_loss()`. | MVP needs auto-reconnect. | Worker polls; on `Disconnected` drop the `Client` and rebuild with backoff; on `Displaced` stop. |
| G7 | Login rejection reason (bad password vs other) not surfaced: only `AuthenticationFailed`. | Generic error text in onboarding. | Accept; message "login rejected". |
| G8 | `DownloadStatus::TimedOut` is never emitted; a remote-queued download can sit in `Queued` forever; browse has no timeout/failure. | ▶ could hang. | Core timeouts: play fails if no `InProgress` within N s (UI may pick another source); browse fails after N s. |
| G9 | Default `ClientVersion` minor 1 = "unidentified". | Etiquette only. | Request a minor upstream (issue #12) before release. |
| G10 | `connect` and `set_shared_directories` scan shares synchronously on the caller's thread. | Large shares block. | Always called on the core worker, never the UI thread. |
| G11 | `cancel_download` deletes the `.part` and is keyed by (user, filename). | Cancelling a play and a download of the same file from the same user would hit both. | Core refuses a second concurrent transfer of the same (user, path); ↓ of an in-flight play promotes it (moves on completion) instead. |
| G12 | Listener silently falls back to an ephemeral port if the configured one is busy. | UPnP/forwarding mismatch. | Report the bound port in `Connected { listen_port }`; settings shows it. |
| G13 | Library logger writes ANSI to stderr by default. | Noise in a GUI. | App sets `LOG_FILE` (e.g. `~/.cache/wholeseek/slsk.log`) or `logger::set_log_level`. |
| G14 | Folder contents are keyed by the folder string the peer echoes. | Lookup could miss if a peer echoes a different spelling. | Fall back to the peer's full browse listing. |
| G15 | Peer UserInfo picture bytes are parsed past and dropped (`PeerInfo::picture_bytes` is only a length). | Profile header cannot show the peer's picture. | `UserProfile.picture` stays `None`; needs an upstream change to keep the bytes (with a cap). |

### 2.1 CP1 status per gap

| # | Status | How (where) |
|---|---|---|
| G1 | Out of MVP (TL) | Only `upload_slots` → `set_upload_slots`. |
| G2 | **Done** | `src/port_mapping/` (MIT-attributed port), `CoreEvent::PortMapping { port, outcome }`; started after each login with the bound port when `port_mapping` is on, stopped on logout/reconnect/drop. |
| G3 | **Done** | Worker tick `POLL_TICK` (150 ms) polls session loss, search counts, browse/folder answers, download status channels, uploads (1 s). |
| G4 | **Done** | Search window 30 s (§3.10) on a helper thread (`search_with_cancel`); deltas `results[seen..]` ranked per batch; a repeat of a live query ends the old `SearchId` first; `forget_search` on finish/cancel (deferred until the helper returns). Offline/empty query → `Error` + `SearchFinished { total_files: 0 }`. |
| G5 | **Done** | ▶ into `cache/t<id>/`. ↓ streams into `downloads/.wholeseek-partial/t<id>/` and is moved to `downloads/<folder>/` on completion; existing names get ` (1)`, ` (2)` … — never overwritten. `TransferQueued/Progress.part_path` is the real growing file. |
| G6 | **Done** | `Disconnected{Network("… — reconnecting in N s (attempt k)"), will_retry:true}` → reconnect after 2, 5, 15, 30, then 60 s (reset on success); countdown in `status().reconnect` (§3.8). The new session re-announces shares and the bound port at login. `LoginRejected` / `Displaced` / `User` → no retry. Login while connected restarts the session. |
| G7 | Accepted | `DisconnectReason::LoginRejected`. |
| G8 | **Done** | ▶ that has received no data within `Settings::play_start_timeout_secs` (default 8 s, counted from that ▶'s own start) → `TransferFailed`, reason starts with `PLAY_STALLED` ("no data from <user> within …"); library transfer cancelled unless a ↓ shares it (§3.5). Browse fails after 60 s, folder contents falls back after 30 s. After 5 s without bytes: `TransferRemoteQueued { place: None }`, then the polled place when known. |
| G9 | Open | Upstream request before release. |
| G10 | **Done** | `connect` (share scan) on a connect helper thread; `set_shares`/`rescan_shares` on a shares helper thread; offline, `ShareCounts` comes from a local scan. |
| G11 | **Done** | One library transfer per (user, path); further ▶/↓ subscribe to it. ↓ during ▶ = promotion (cache copy kept, ↓ gets a hard link/copy). ▶ during ↓ gets a hard link to the growing `.part` at its ticket path. Second ↓ of the same file → `TransferFailed("already being downloaded …")`. `cancel` only cancels the library transfer when the last subscriber leaves. |
| G12 | **Done** | `Connected { listen_port }` = bound port; port mapping uses it. If it differs from `Settings::listen_port`, an `Error { context: "listen_port" }` explains why and `status().listen` keeps `{requested, bound, note}` (§3.8). Reconnects wait (≤ 5 s) for the old session to release the port, so our own previous session no longer causes the fallback. |
| G13 | **Done** | `wholeseek_core::config::init_logging(&default_log_path())` (unsafe: call first in `main`) → `~/.cache/wholeseek/slsk.log`, Info level. |
| G14 | **Done** | Folder contents: exact (case/slash-insensitive) match, else the whole reply; no reply within 30 s → full browse of the user, then the folder is picked from it. |
| G15 | Open (upstream) | See §3.4; everything else in the profile is served. |

---

## 3. wholeseek-core interface (implemented)

Design: one `Core` handle, non-blocking commands, one ordered event channel. A single worker
thread owns the `soulseek_rs::Client` (in an `Arc` so a search helper thread can poll), receives
`Command`s, polls the library every `POLL_TICK` (150 ms), and emits `CoreEvent`s.

```rust
// src/core.rs
impl Core {
    pub fn start(settings: Settings) -> io::Result<(Core, Receiver<CoreEvent>)>;
    pub fn login(&self, Credentials);            // auto-reconnect until logout
    pub fn logout(&self);
    pub fn search(&self, query: &str) -> SearchId;   // sends query::normalize_query(query); may broaden once (§3.10)
    pub fn cancel_search(&self, SearchId);
    pub fn browse(&self, username: &str);
    pub fn play_fetch(&self, &RemoteFile) -> PlayTicket { id, part_path, final_path }  // cache only
    pub fn download(&self, &RemoteFile) -> TransferId;     // downloads/<folder>/; reuses cache
    pub fn download_folder(&self, username: &str, folder: &str);
    pub fn cancel(&self, TransferId);
    pub fn retry(&self, TransferId);
    pub fn set_shares(&mut self, Vec<PathBuf>);
    pub fn rescan_shares(&self);
    pub fn apply_settings(&mut self, Settings);  // port change → reconnect
    // additive (CP1)
    pub fn set_audio_only(&self, on: bool);      // search batch filter, default on
    pub fn cancel_upload(&self, username: &str, path: &str);  // then an Uploads snapshot
    pub fn user_profile(&self, username: &str);  // → UserProfile* events (§3.4)
    pub fn status(&self) -> CoreStatus;          // polled snapshot (§3.8)
    pub fn clear_cache(&self);                   // keeps plays in use (§3.8)
}

// src/event.rs
pub enum CoreEvent {
    Connected { username, listen_port: Option<u16> },
    Disconnected { reason: DisconnectReason /* LoginRejected|Displaced|Network(_)|User */, will_retry },
    SearchResults { search, hits: Vec<SearchHit> },   // deltas, filtered + FLAC-first ranked
    SearchFinished { search, total_files },
    BrowseResult { username, dirs: Vec<BrowseDir> },
    FolderContents { username, folder, dirs },
    BrowseFailed { username, reason },
    TransferQueued { id, kind: Play|Download, file, part_path },
    TransferRemoteQueued { id, place: Option<u32> },
    TransferProgress { id, bytes, total, speed, part_path },
    TransferDone { id, path },
    TransferFailed { id, reason },
    TransferCancelled { id },
    ShareCounts { folders, files },
    Uploads(Vec<UploadRow>),
    Error { context, message },
    // additive (CP1)
    PortMapping { port: u16, outcome: PortMapOutcome /* Mapped { method, external: Option<String> } | Unavailable(String) */ },
    UserProfile { username: String, profile: UserProfile },        // may repeat; each = all known so far
    UserProfileFailed { username: String, reason: String },        // only if nothing at all came back
}
```

`Command` gained `SetAudioOnly(bool)`, `CancelUpload { username, path }` `UserProfile(String)` and `ClearCache` (additive).

Data (`src/model.rs`): `RemoteFile { username, path, size, attrs: AudioAttrs }`,
`SearchHit { file, free_slot, speed }`, `BrowseDir { path, files }`, `UploadRow`, `Settings`
(download dir, cache dir + cap, share dirs, listen port, upload slots, port_mapping (G2);
no upload speed limit — deferred, G1), `Credentials` (Debug-redacted). No `soulseek_rs` types leak into the UI.

Helpers (real, tested): `paths` (`.part`/final/cache-dir/download-dir rules), `rank`
(audio filter, FLAC → MP3 → other audio, then free slot, speed; conversion from
`soulseek_rs::SearchResult`).

Play / download rules (as implemented):
- ▶ `play_fetch` → library download into `cache/t<id>/` (ticket paths known immediately; the
  player opens `part_path` after the first `TransferProgress`). Never writes to downloads. A
  complete cached copy of the same (user, path) — also from a previous run, via
  `cache/t<id>/.wholeseek-meta` — is linked into the new ticket dir: `TransferQueued`,
  one `TransferProgress { bytes = total }`, `TransferDone`, no network.
- ↓ `download` → complete in cache: hard link (copy across filesystems) into
  `downloads/<folder>/`, cache untouched. In flight (▶ or ↓): subscribe (G11). Else fetch
  via the staging dir (G5).
- `retry` reuses the same `t<id>` dir so the library resumes the `.part`.
- `cancel` of a failed transfer just emits `TransferCancelled` (lets the UI clear the row).
- Cache cap: at start, on each completed fetch and on cap change, delete the oldest
  `cache/t*/` dirs (by mtime) until under `cache_cap_bytes`, never the current/active play.
- Ids start above the largest `t<id>` found in the cache and staging dirs.
- `Core::start` does no network and persists nothing.

### 3.1 Port mapping (G2, done)

- New module `wholeseek-core/src/port_mapping/` ported from `soulseek-rs/src/port_mapping/mod.rs`
  and `nat_pmp.rs` @ 434dcc0. Each ported file keeps a header with the original copyright
  (Michel de Graaf, soulseek-rs, MIT) and the source path/revision.
- Shape kept from upstream, plus a report callback: `PortMapper::spawn(port, report) -> PortMapper` (background thread, renews at
  half the lease, removes the mapping on `Drop`, never blocks shutdown >1 s). UPnP-IGD first via
  `igd-next`, then std-only NAT-PMP. Replace upstream's use of `soulseek_rs::utils::logger` levels
  with our own logging as needed.
- The worker starts it after `connect()` with the **bound** port (`Client::listen_port()`, G12)
  when `Settings.port_mapping` is true, and restarts it on reconnect/port change. Outcome surfaces
  as `CoreEvent::PortMapping { port, outcome }` (method `"upnp"`/`"natpmp"`, `external` = `"ip:port"` when UPnP reports it).
- `igd-next = "0.17"` is a direct dependency in `crates/wholeseek-core/Cargo.toml` (not in the
  root workspace table; TL may hoist it).

### 3.2 Persistence — `wholeseek_core::config` (CP1)

Called by the app before `Core::start`; the worker never touches config or keyring.

```rust
pub fn load() -> io::Result<Loaded>;            // Loaded { settings, credentials: Option<Credentials>, keyring_available }
pub fn save_settings(&Settings) -> io::Result<()>;          // keeps the remembered username
pub fn save_credentials(&Credentials) -> Result<(), KeyringUnavailable>;
pub fn forget_credentials() -> io::Result<()>;
// *_in(dir, [&Arc<dyn SecretStore>], …) variants for tests; SecretStore { get, set, delete }
pub fn config_dir() -> PathBuf;                 // $XDG_CONFIG_HOME/wholeseek
pub fn default_log_path() -> PathBuf;           // $XDG_CACHE_HOME/wholeseek/slsk.log
pub unsafe fn init_logging(&Path) -> io::Result<()>;  // G13; first thing in main
```

- `config.toml`: `username` + `[settings]` (serde, every field defaulted, so partial files load).
  Atomic write (temp + rename). Invalid TOML → `Err(InvalidData)`.
- Password only in the OS keyring (service `wholeseek`, account = username; Secret Service on Linux
  via the pure-Rust zbus backend). Never in the file, never logged. Each keyring call is bounded by
  `KEYRING_TIMEOUT` (5 s). Unreachable keyring: `load` → `keyring_available: false`,
  `credentials: None`; `save_credentials` → `Err(KeyringUnavailable(msg))` (username is still saved).
- `Settings::default()`: downloads `$XDG_DOWNLOAD_DIR/wholeseek`, cache `$XDG_CACHE_HOME/wholeseek`,
  cap 1 GiB, port 2234, 10 upload slots, port mapping on.

### 3.4 User profile (additive)

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserProfile {
    pub online: Option<bool>,          // server presence (away counts as online)
    pub avg_speed: Option<u32>,        // B/s, server stats
    pub shared_files: Option<u32>,
    pub shared_dirs: Option<u32>,
    pub description: Option<String>,   // peer UserInfo reply
    pub picture: Option<Vec<u8>>,      // raw image bytes from peer, if any
    pub upload_slots: Option<u32>,
    pub free_slot: Option<bool>,
    pub queue_size: Option<u32>,
}
```

- `Core::user_profile(u)` sends `Client::request_user_info` (server GetUserStatus + GetUserStats)
  and `Client::request_peer_info` (peer UserInfoRequest, code 15) — both exist at the pinned rev,
  no lib changes. Both drop the previous answer first, so polling sees only fresh replies.
- The worker polls `user_info(u)` / `peer_info(u)` each tick and emits `UserProfile` whenever the
  merged snapshot changes (non-empty). Finished when server presence + stats are in and either the
  peer answered or the server says the user is offline (no peer answer will come). Timeout 10 s:
  if nothing at all arrived → `UserProfileFailed { reason }`; if something did, no failure event
  (the last `UserProfile` stands, with `None` for missing parts). Offline → immediate
  `UserProfileFailed { reason: "not connected" }`; pending requests at session loss fail the same way.
- Field mapping: `online = status != Offline`; `avg_speed/shared_files/shared_dirs` from
  `UserStats`; `description`, `free_slot = slots_free`, `queue_size` from `PeerInfo`;
  `upload_slots` = `PeerInfo::total_uploads` (the wire field after the picture is the peer's upload
  slot count; Nicotine+ shows it as "upload slots" and soulseek-rs itself sends its slot count there).
- **Gap G15 — `picture` is always `None`** at the pinned rev: `UserInfoResponseHandler` skips the image
  bytes and keeps only `picture_bytes: Option<usize>`. Surfacing it needs an upstream change
  (keep the bytes, with a size cap); not forked here.
- Our side: the library already answers peers' UserInfoRequest itself (`ClientOperation::UserInfoRequested`
  → `build_user_info(upload_slots, upload queue length, has free slot)`, empty description, no
  picture). Nothing to do in wholeseek-core.

### 3.5 ▶ source fallback (additive)

Live data: many peers that advertise a free slot never send (3 of 4 in one run).

- `Settings::play_start_timeout_secs: u64` (default 8; `#[serde(default)]`, so older config files
  load). A ▶ that has received **no data** (a progress report with 0 bytes, as the library sends on
  opening the `.part`, does not count) that long after its own start is cancelled and reported as
  **`CoreEvent::TransferFailed { id, reason }`**, with `reason` starting with
  `wholeseek_core::PLAY_STALLED` (`"no data from"`). No new variant: the app matches `CoreEvent`
  exhaustively and is mid-restructure, and every ▶ failure before playback should trigger the same
  fallback anyway. `retry(id)` still works. ↓ is unaffected (keeps waiting in the remote queue); a
  ▶ attached to a ↓ fails on its own deadline but leaves the ↓'s library transfer running.
- `rank::alternates(file: &RemoteFile, hits: &[SearchHit]) -> Vec<RemoteFile>`: other users' copies
  of the same track, best first. Match = `rank::normalized_title` equal (basename without
  extension, lower-case, leading track/disc numbers and punctuation removed, letters+digits only;
  or one title ends with the other when the shorter is ≥ 6 chars, for "Artist - Title") **and**
  (size within 1 % **or** both durations known and equal). Excludes `file.username`; one entry per
  user. Order: free slot, then same extension as `file`, then speed.
- App wiring: on a ▶ `TransferFailed` before playback starts, `play_fetch` the next of
  `alternates(&file, &hits_of_that_search)`.

### 3.6 Rename prep — `config::migrate_legacy` (additive)

- `config::APP_NAME` (`"wholeseek"`) names the config dir, default download/cache dirs, log path and
  keyring service (`KEYRING_SERVICE = APP_NAME`); `config::LEGACY_APP_NAME` (`"wholeseek"`) is the
  previous name. The Wholeseek rename flips `APP_NAME = "wholeseek"`, `LEGACY_APP_NAME = "wholeseek"`.
- `config::migrate_legacy() -> io::Result<Migration>`: call once at startup, before `load()`.
  Idempotent — does nothing if the names are equal (today), the new config dir exists, or there is
  no old `config.toml`. Otherwise copies `config.toml` to the new dir, replacing `download_dir` /
  `cache_dir` only when they equal the old defaults (`<XDG downloads>/<old>`, `<XDG cache>/<old>`),
  and copies the saved username's password from keyring service `<old>` to `<new>` (each call bounded
  by `KEYRING_TIMEOUT`). Old files/entries are left in place. Result:
  `Migration::{NotNeeded, Migrated { password: PasswordMigration::{Copied, NothingSaved, Failed(msg)} }}`;
  a keyring failure is logged and returned, never an error (the user signs in again).
- Testable form: `migrate_legacy_in(&BaseDirs { config, downloads, cache }, old, new, &old_store, &new_store)`.
  Also new: `config::BaseDirs::system()`, `config::NamedKeyringStore { service }`.

### 3.8 Status snapshot — `Core::status()` (additive)

The app matches `CoreEvent` and `DisconnectReason` exhaustively, so nothing below adds a variant.
Instead `Core::status() -> CoreStatus` returns a snapshot refreshed every tick (150 ms) and after
every command/internal message, i.e. current by the time the UI handles the matching event.

```rust
pub struct CoreStatus {
    pub reconnect: Option<ReconnectStatus>, // { attempt: u32 (1-based), delay, at: Instant }; .in_secs()
    pub listen: ListenStatus,               // { requested: u16, bound: Option<u16>, note: Option<String> }
    pub searches: Vec<SearchProgress>,      // open windows: { id, query, files, users, ends_at }; .remaining()
    pub cache: CacheUsage,                  // { used_bytes, cap_bytes, in_use_bytes }
    pub shares: Vec<ShareStatus>,           // per Settings::share_dirs entry: { dir, folders, files, scanning }
}
pub fn clear_cache(&self);                  // Command::ClearCache
```

**(a) Session loss.** "Another login took this username" is `DisconnectReason::Displaced`
(`will_retry: false`, credentials dropped, never retried) — already distinct, so no
`KickedByOtherLogin` variant. A network drop or failed login attempt (not a rejection) sends
`Disconnected { Network("the connection to the server dropped — reconnecting in 5 s (attempt 2)"),
will_retry: true }` and sets `status().reconnect` until the attempt starts; delays 2, 5, 15, 30,
60 s (cap), reset after a successful login. That message + snapshot replace the requested
`Reconnecting { attempt, in_secs }` event. Each new session is built with the current
`share_dirs` and the library sends SharedFoldersFiles + the bound port right after login, then
`ShareCounts` is emitted again.

**(b) Search progress.** Results stream as `SearchResults` deltas for the whole window (30 s), not
only the first batch; `status().searches` has the running file and distinct-user counts and
`ends_at` for "128 results from 34 users… still searching (9 s left)". The definite end is
`SearchFinished { total_files }`, sent when the window closes (or on cancel / a repeat of the same
query / session loss); the search then leaves `status().searches`.

**(c) Listen port.** soulseek-rs binds `Settings::listen_port` and, **only if it is in use**
(`AddrInUse`), silently falls back to an OS-chosen port and advertises that one. Core reports the
real port in `Connected { listen_port }` and `status().listen.bound`; when they differ,
`status().listen.note` and an `Error { context: "listen_port", message }` say "port 2234 is in use
(another Soulseek client or another copy of the app is listening on it), so the system picked port
45543; …". A likely cause of the user's "2234 vs 45543" is fixed: a reconnect could start while
the previous session was still held by a search helper thread (still bound to 2234); reconnects
now wait up to 5 s for it to be dropped. Another running instance (or any program on 2234) still
causes the fallback, and is now explained.

**Cache.** `status().cache.used_bytes` is the size of `cache/t*` (rescanned every 2 s and after
completions/eviction/clear); `in_use_bytes` is the part eviction and `clear_cache()` keep — the
current play and plays still being fetched. `clear_cache()` deletes the rest and forgets them, so a
later ▶ of a cleared file fetches it again.

**Per-share counts.** `status().shares` has one `ShareStatus { dir, folders, files, scanning }` per
`Settings::share_dirs` entry, in that order. Filled at `Core::start` (local scan, no network) when
shares are configured, and after every `set_shares` / `rescan_shares` / share change in
`apply_settings`: entries are marked `scanning: true` (keeping the previous counts) as soon as the
command is handled, and updated when the scan ends. The library reports only totals, so the share
helper thread also counts each directory on its own (an extra read-only walk); a missing or
unreadable directory reads 0/0. The `ShareCounts` event (totals) is unchanged.

### 3.9 Folder download skips OS junk (additive)

`wholeseek_core::DOWNLOAD_SKIP = ["thumbs.db", "desktop.ini", ".ds_store", "*.url", "._*"]`, matched
case-insensitively against the basename (`*x` = ends with, `x*` = starts with, else exact), and
`wholeseek_core::is_download_junk(remote_path)`. `Core::download_folder` leaves those files out;
`.cue`, `.log`, `.jpg`, `.png`, `.nfo` and everything else are downloaded. The skip is **silent**:
no existing event can carry a count without a new variant. `FolderContents` / `BrowseResult`
still list every file, so the UI can show "N system files skipped" with `is_download_junk`. A
folder that holds only junk gives `Error { context: "download_folder", "… has only system files
(Thumbs.db, …)" }`. Single-file `Core::download` is not filtered.

### 3.10 Search quality — normalization, broadening, wishlist, window (additive)

**What goes on the wire.** `Core::search(typed)` sends `query::normalize_query(typed)`: punctuation
inside ordinary words becomes a space (`wasn't` → `wasn t`, `ac/dc` → `ac dc`, `p!nk` → `p nk`,
typographic quotes/dashes too), whitespace collapsed, case and accents kept. `-word` (exclude) and
`*part` (partial) are sent unchanged; `"phrase"` quotes are dropped. If nothing is left (`?!`) the
trimmed original is sent, never an empty query. The tab title stays what was typed
(`status().searches[].query`, `SearchInfo::typed`). This is exactly what Nicotine+ 3.3.11 sends
(`pynicotine/search.py` `sanitize_search_term`, "SoulseekQt doesn't seem to send search results if
such characters are included"), and it matches how clients index shares (file names split on
punctuation, `TRANSLATE_PUNCTUATION`).

Live measurement (throwaway account, all variants of a query searched at the same time, users/files,
two rounds; `tests/live.rs::live_query_variants`, `proof/search/`):

| query | as typed | apostrophe removed | punct → space (chosen) | + drop 1-char words | + accents folded |
|---|---|---|---|---|---|
| wasn't me shaggy | 273/625 · 9/26 | 94/197 · 2/2 | **456/1053 · 18/46** | 457/1062 · 17/45 | = punct |
| don't stop me now | 477/1408 · 66/185 | 74/168 · 2/2 | 463/1409 · 68/187 | 464/1429 · 68/191 | = punct |
| guns n' roses | 175/8073 · 174/8470 | 171/8011 · 185/8316 | 171/8011 · 185/8316 | 173/7964 · 184/8333 | = punct |
| ac/dc back in black | 23/212 · 21/240 | = typed | **128/1480 · 118/1468** | = punct | = punct |
| p!nk | 33/1349 · 21/1376 | = typed | 34/1488 · 19/1081 | 36/1329 · 22/1398 | = punct |
| beyoncé halo | 0/0 · 0/0 | = | 0/0 · 0/0 | = | 0/0 · 0/0 |
| sigur rós | 59/2337 · 7/756 | = | = typed | = | 51/2274 · 6/743 |

Removing apostrophes (`wasnt`) is much worse; punctuation → space wins clearly where punctuation is
inside a word and is neutral elsewhere; dropping 1-letter words and folding accents are neutral to
slightly worse, so they are only used as the zero-result fallback. Round 2 counts are lower across
the board (the server seems to throttle repeated searches), so compare within a round.
`beyoncé halo` returns nothing in any form (likely a server-excluded phrase).

**Broaden once on zero.** When the window closes with **no audio** results (non-audio hits do not
count), the same `SearchId` is re-run once with `query::broaden_query(sent)` (accents folded,
1-character words dropped: `wasn t me` → `wasn me`, `sigur rós` → `sigur ros`), if that differs.
`SearchResults` keep coming under the same id; exactly one `SearchFinished` ends both. Never
for queries with operators (`-x`, `*x`, `"…"`), never twice, never for wishlist searches.
`Core::search_info(id) -> Option<SearchInfo { id, typed, sent, broadened_to, wishlist, files,
users, finished }>` tells the UI "No results for X — showing results for Y"; kept for the newest
256 searches, after they finish.

**Wishlist.** `Settings::wishlist: Vec<String>` (serde default; persisted by
`config::save_settings`), `Core::wishlist() -> &[String]`, `Core::wishlist_add(&mut self, q) ->
bool` (false if empty or already there, case-insensitive), `Core::wishlist_remove(&mut self, q) ->
bool`. While logged in, one wish is searched per interval, round-robin (as Nicotine+ does), first
30 s after login: sent as a **wishlist search, server code 103** (soulseek-rs
`Client::start_wishlist_search`, already in our pin), interval = the server's code 104 value
(`Client::wishlist_interval`, library default 12 min until announced), never less than 10 min.
Results arrive as ordinary `SearchResults` / `SearchFinished` under a fresh `SearchId`;
`search_info(id).wishlist == true` and `.typed` is the wish. No new events.

**Window and caps.** A search collects for **30 s** (was 15 s). The library keeps at most
**500 responses / 10 000 files** per search (`MAX_SEARCH_RESPONSES` / `MAX_SEARCH_FILES`,
whole responses), and each peer reply carries at most 300 files. Measured, 5 queries at once over
60 s (`live_search_arrival_times`):

| query | responses | files | % responses after 5/10/15/20/30 s | % files after | last |
|---|---|---|---|---|---|
| aphex twin | 135 | 10 397 | 100/0/0/0/0 | 100/0/0/0/0 | 8.8 s (file cap) |
| daft punk discovery | 500 | 8 052 | 100/73/48/27/0 | 100/72/48/25/0 | 28.2 s (response cap) |
| radiohead ok computer | 458 | 10 023 | 100/70/41/12/0 | 100/72/47/10/0 | 23.2 s (file cap) |
| miles davis kind of blue | 500 | 6 229 | 100/86/73/57/37 | 100/90/65/50/32 | 45.2 s |
| shaggy wasn t me | 403 | 945 | 100/88/77/67/50 | 100/91/81/70/50 | 60.1 s |
| all | 1 996 | 35 646 | 100/74/55/37/19 | 100/55/37/19/7 | |

A 15 s window dropped 55 % of responses (37 % of files); 30 s keeps all but 19 % (7 % of files) —
mostly for rarer queries, where late answers matter most. Results stream throughout, so the UI is
not slower; only "still searching" and the broaden fallback wait longer.

### 3.11 Download to another folder (additive)

`Core::download_to(&mut self, file, dir: &Path) -> TransferId` and
`Core::download_folder_to(&mut self, username, folder, dir: &Path)`: like `download` /
`download_folder`, into `<dir>/<remote folder name>/` (`paths::download_dir_for(dir, path)`).
Staging, `.part` and collisions are unchanged: the file streams in
`<download_dir>/.wholeseek-partial/t<id>/` and is moved (rename, or copy + delete across
filesystems) on completion, never overwriting (unique name). `TransferDone { path }` is the final
file ("Show in folder"). Both put `dir` first in `Settings::recent_download_dirs` (MRU, max
`Settings::RECENT_DOWNLOAD_DIRS` = 5, serde default; `Settings::remember_download_dir`); the app
persists with `config::save_settings(core.settings())`. `&mut self` because they update settings.

### 3.12 Single files flat by default (additive)

`Settings::single_file_subfolder: bool` (serde default **false**, as in Nicotine+). Single-file
downloads — `Core::download`, `Core::download_to`, and keeping a ▶ from the cache (which goes
through `download`) — go flat into the downloads folder / chosen dir when false, into
`<remote folder name>/` when true. Folder downloads (`download_folder`, `download_folder_to`)
always keep the folder name; an album the app fetches file by file ("play album → keep") uses
`Core::download_album_file(&mut self, file, root: Option<&Path>) -> TransferId`, which always
keeps it (`None` = downloads folder; `Some` updates `recent_download_dirs`). Collisions still get a
unique name (`01 (2).flac`), never an overwrite. §3.11's layout description applies with this rule.

### 3.13 Download history (additive)

Every ↓ that ends — done, failed or cancelled — is recorded in
`<Settings::data_dir>/history.jsonl` (`data_dir` defaults to `$XDG_DATA_HOME/wholeseek`, i.e.
`~/.local/share/wholeseek`; read once at start). ▶ plays are not history; keeping one (a ↓ served
from the cache) is. A refused duplicate ↓ is not recorded; a retried transfer replaces its earlier
row. The row is written before the `Transfer*` event, so the UI sees it when handling the event.

- `HistoryEntry { id, finished_at (unix s), username, remote_path, local_path: Option (done only),
  size, format (extension), bitrate_kbps, sample_rate_hz, bit_depth, duration_secs (as advertised),
  album (remote folder name), outcome: Outcome::{Done, Failed, Cancelled}, reason: Option }`;
  `entry.exists_on_disk()` stats the file on each call (for "moved/deleted").
- `Core::history(&self, &HistoryQuery { text, limit, offset }) -> Vec<HistoryEntry>`: newest first;
  `text` = case-insensitive substring of remote path, user or local path; `limit 0` = all.
- `Core::history_remove(&self, id) -> bool`, `Core::history_clear(&self) -> io::Result<()>`
  (files on disk untouched).
- `Core::was_downloaded(&self, user, remote_path) -> bool` and
  `Core::downloaded_keys(&self) -> DownloadedKeys` (`Arc<HashSet<(user, remote_path)>>`, completed
  only; a new set after each change — fetch once per frame, look up per row for the ✓ badge).

Storage: append-only JSON lines (`add` / `remove` / `clear`), one write per line. Load skips lines
it cannot parse (a crash mid-write), keeps the newest `HISTORY_CAP` = 50 000 entries and rewrites the
file (temp + rename) if it held anything else; it is also compacted after 50 000 appends. If it
cannot be loaded the history is kept in memory and one `Error { context: "history" }` is emitted.
Only user names and paths are stored, never credentials. Nothing old is migrated.

### 3.7 Tests

Offline: `cargo test -p wholeseek-core` — paths, ranking + `alternates` matching, cache
scan/eviction plan, config round trip + legacy migration with a temp dir and in-memory keyring,
backoff, and the worker against a fake session/connector (login/reject/backoff/displaced, search
deltas, browse timeout, folder fallback, cache-only play, promotion, cache reuse across restart,
▶ start deadline + retry, eviction, shares, uploads, user profile, displaced vs. network-drop
reconnect with backoff status and share re-announce, port release before reconnect, bound-port note,
search progress over several batches, cache usage + `clear_cache`, folder junk skip, per-share
status at startup and across rescans, query normalization + broaden-once + operators, wishlist
scheduling and interval, download-to layout/staging/MRU, old config loading).

Live (`#[ignore]`, throwaway accounts only, never the user's own; `--nocapture`):
- `live_core_end_to_end` (`WHOLESEEK_USER`/`WHOLESEEK_PASS`, `WHOLESEEK_LISTEN_PORT`): login, search
  (`WHOLESEEK_QUERY`, default "aphex twin"), top FLAC hits, browse + profile of the top user,
  `play_fetch` with time to first `TransferProgress`, cancel, downloads dir stays empty.
  Proof: `proof/cp1/live/profile-live.log`.
- `live_share_upload_between_two_accounts` (+ `WHOLESEEK_USER2`/`WHOLESEEK_PASS2`,
  `WHOLESEEK_PORT_A`=2241, `WHOLESEEK_PORT_B`=2242, ffmpeg): A shares a generated FLAC, B browses A
  and downloads it, bytes must match, A must emit an `Uploads` row for B. Proof:
  `proof/cp1/live/shares-live.log`.

## 4. Smoke example

`WHOLESEEK_USER=… WHOLESEEK_PASS=… cargo run -p wholeseek-core --example smoke -- "<query>"`
(`crates/wholeseek-core/examples/smoke.rs`) uses `soulseek_rs::Client` directly: connect, login,
`search` (window `WHOLESEEK_SEARCH_SECS`, default 12), `rank::hits_from_results` + `rank::rank`
(audio-only, FLAC first), prints the top 10, `forget_search`, disconnect. Compiled, **not run**.
Live test: `crates/wholeseek-core/tests/live.rs`, `#[ignore]`d.
