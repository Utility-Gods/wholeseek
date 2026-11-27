//! Executes the closed agent surface against the same Core as the GUI.

use super::ownership::{HANDOVER_MAX_AGE, read_state, write_atomic};
use super::*;
use crate::{
    BrowseDir, Core, CoreEvent, DisconnectReason, RemoteFile, SearchHit, SearchId, TransferId,
    TransferKind, paths, rank,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DAEMON_IDLE: Duration = Duration::from_secs(600);
const MAX_HITS: usize = 5_000;
const MAX_GROUPS: usize = 100;
const MAX_FOLDER_FILES: usize = 1_000;
const MAX_PENDING: usize = 32;
const MAX_ROWS: usize = 5_000;
const ALTERNATE_BATCHES: usize = 4;
const MAX_ALTERNATE_HITS: usize = 1_000;
const BROWSE_TIMEOUT: Duration = Duration::from_secs(65);
const DOWNLOAD_WAIT: Duration = Duration::from_secs(55 * 60);
type Reply = Sender<Response>;

#[derive(Default, Serialize, Deserialize)]
struct State {
    results: ResultCache<GroupKey>,
    rate: SearchRate,
    hits: VecDeque<(u64, Vec<SearchHit>)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupKey {
    user: String,
    folder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentTransfer {
    id: String,
    file: RemoteFile,
    dir: PathBuf,
    tried: BTreeSet<String>,
    origin: TransferOrigin,
}

struct Row {
    data: TransferData,
    agent: Option<AgentTransfer>,
    acknowledged: bool,
}

struct Search {
    args: SearchArgs,
    hits: Vec<SearchHit>,
    deadline: Instant,
    reply: Reply,
}

enum BrowseWork {
    List(Option<String>),
    Download {
        path: String,
        folder_only: bool,
        to: Option<PathBuf>,
        wait: bool,
    },
}

struct Browse {
    work: BrowseWork,
    deadline: Instant,
    reply: Reply,
}

struct Waiting {
    ids: Vec<TransferId>,
    all_done: bool,
    allow_cancelled: bool,
    deadline: Instant,
    reply: Reply,
}

pub struct Engine {
    state: State,
    state_path: PathBuf,
    mode: Mode,
    connected: bool,
    connecting: bool,
    displaced: bool,
    prefix: String,
    searches: HashMap<SearchId, Search>,
    browses: HashMap<String, Vec<Browse>>,
    rows: BTreeMap<TransferId, Row>,
    waiting: Vec<Waiting>,
    waiting_for_login: Vec<(Request, Reply, Instant)>,
    last_request: Instant,
    restoring: HashSet<TransferId>,
    handover_path: PathBuf,
}

impl Engine {
    pub fn open(dir: &Path, mode: Mode, connecting: bool) -> io::Result<Self> {
        let state_path = dir.join("state-v1.json");
        let handover_path = dir.join("handover-v1.json");
        let state = read_state(&state_path, None)?.unwrap_or_default();
        let prefix = format!("c-{:x}-{:x}", std::process::id(), since_epoch().as_nanos());
        Ok(Self {
            state,
            state_path,
            mode,
            connected: false,
            connecting,
            displaced: false,
            prefix,
            searches: HashMap::new(),
            browses: HashMap::new(),
            rows: BTreeMap::new(),
            waiting: Vec::new(),
            waiting_for_login: Vec::new(),
            last_request: Instant::now(),
            restoring: HashSet::new(),
            handover_path,
        })
    }

    pub fn displaced(&self) -> bool {
        self.displaced
    }

    pub fn idle(&self) -> bool {
        !self.has_pending()
            && !self.rows.values().any(|row| active(row.data.state))
            && self.last_request.elapsed() >= DAEMON_IDLE
    }

    pub fn has_pending(&self) -> bool {
        !self.searches.is_empty()
            || !self.browses.is_empty()
            || !self.waiting.is_empty()
            || !self.waiting_for_login.is_empty()
    }

    fn pending_count(&self) -> usize {
        self.searches.len()
            + self.browses.values().map(Vec::len).sum::<usize>()
            + self.waiting.len()
            + self.waiting_for_login.len()
    }

    pub fn request(&mut self, core: &Core, request: Request, reply: Reply) {
        self.last_request = Instant::now();
        match request.command {
            Operation::Status => {
                respond(
                    reply,
                    &StatusData {
                        connected: self.connected,
                        mode: self.mode,
                        idle_timeout_secs: (self.mode == Mode::Daemon)
                            .then_some(DAEMON_IDLE.as_secs()),
                    },
                );
                return;
            }
            Operation::Transfers(args) => {
                let transfers: Vec<_> = self
                    .rows
                    .values()
                    .filter(|row| args.id.as_ref().is_none_or(|id| id == &row.data.id))
                    .map(|row| row.data.clone())
                    .collect();
                if transfers.is_empty() && args.id.is_some() {
                    fail(reply, ErrorCode::NotFound, "unknown transfer ID");
                } else {
                    respond(reply, &TransfersData { transfers });
                }
                return;
            }
            _ => {}
        }

        if self.pending_count() >= MAX_PENDING {
            fail(reply, ErrorCode::Busy, "too many pending control requests");
            return;
        }
        if !self.connected {
            if self.connecting && self.mode == Mode::Daemon {
                self.waiting_for_login.push((
                    request,
                    reply,
                    Instant::now() + Duration::from_secs(50),
                ));
            } else {
                fail(reply, ErrorCode::NotConnected, "Soulseek is not connected");
            }
            return;
        }
        match request.command {
            Operation::Search(args) => self.search(core, args, reply),
            Operation::Browse(args) => {
                self.browse(core, args.user, BrowseWork::List(args.folder), reply)
            }
            Operation::Download(args) => self.download(core, args, reply),
            Operation::RetryOtherSource(args) => self.retry(core, &args.id, reply),
            Operation::Cancel(args) => self.cancel(core, &args.id, reply),
            Operation::Status | Operation::Transfers(_) => unreachable!(),
        }
    }

    fn search(&mut self, core: &Core, args: SearchArgs, reply: Reply) {
        let limit = args.limit.unwrap_or(20);
        let wait = args.wait_secs.unwrap_or(30);
        if !valid_text(&args.query)
            || !(1..=MAX_GROUPS).contains(&limit)
            || !(1..=60).contains(&wait)
        {
            fail(
                reply,
                ErrorCode::BadRequest,
                "invalid query, search limit, or wait time",
            );
            return;
        }
        let settings = core.settings();
        if let Err(seconds) = self.state.rate.reserve(
            since_epoch().as_secs(),
            settings.cli_search_min_interval_secs,
            settings.cli_search_per_hour,
        ) {
            let _ = reply.send(Response::failure(Failure {
                code: ErrorCode::RateLimited,
                message: "search rate limit reached".into(),
                retry_after_secs: Some(seconds),
            }));
            return;
        }
        if write_atomic(&self.state_path, &self.state).is_err() {
            fail(
                reply,
                ErrorCode::Internal,
                "could not persist search rate state",
            );
            return;
        }
        let id = core.search(&args.query);
        self.searches.insert(
            id,
            Search {
                args,
                hits: Vec::new(),
                deadline: Instant::now() + Duration::from_secs(wait),
                reply,
            },
        );
    }

    fn finish_search(&mut self, mut search: Search) {
        rank::rank(&mut search.hits, true);
        let groups = search_groups(&search.hits, &search.args);
        let now = since_epoch().as_secs();
        let ids = self.state.results.insert(
            now,
            groups
                .iter()
                .map(|group| GroupKey {
                    user: group.user.clone(),
                    folder: group.folder.clone(),
                })
                .collect(),
        );
        self.state.hits.push_back((
            now,
            search.hits.into_iter().take(MAX_ALTERNATE_HITS).collect(),
        ));
        while self.state.hits.len() > ALTERNATE_BATCHES {
            self.state.hits.pop_front();
        }
        if write_atomic(&self.state_path, &self.state).is_err() {
            fail(
                search.reply,
                ErrorCode::Internal,
                "could not persist search results",
            );
            return;
        }
        respond(
            search.reply,
            &SearchData {
                groups: groups
                    .into_iter()
                    .zip(ids)
                    .map(|(mut group, id)| {
                        group.id = id;
                        group
                    })
                    .collect(),
            },
        );
    }

    fn browse(&mut self, core: &Core, user: String, work: BrowseWork, reply: Reply) {
        if !valid_text(&user) {
            fail(
                reply,
                ErrorCode::BadRequest,
                "a nonempty remote username is required",
            );
            return;
        }
        let pending = self.browses.entry(user.clone()).or_default();
        if pending.is_empty() {
            core.browse(&user);
        }
        pending.push(Browse {
            work,
            reply,
            deadline: Instant::now() + BROWSE_TIMEOUT,
        });
    }

    fn download(&mut self, core: &Core, args: DownloadArgs, reply: Reply) {
        if args.to.as_ref().is_some_and(|dir| !dir.is_absolute()) {
            fail(
                reply,
                ErrorCode::BadRequest,
                "download destination must be an absolute path",
            );
            return;
        }
        let (user, path, folder_only) = match args.target {
            Target::Result { result_id } => {
                match self.state.results.get(since_epoch().as_secs(), &result_id) {
                    Some(group) => (group.user.clone(), group.folder.clone(), true),
                    None => {
                        fail(reply, ErrorCode::NotFound, "unknown or expired result ID");
                        return;
                    }
                }
            }
            Target::Path { user, path } => {
                if !valid_text(&user) || !valid_text(&path) {
                    fail(
                        reply,
                        ErrorCode::BadRequest,
                        "nonempty remote username and path required",
                    );
                    return;
                }
                (user, path, false)
            }
        };
        if !folder_only
            && let Some(file) = self
                .state
                .hits
                .iter()
                .rev()
                .filter(|(at, _)| fresh(*at))
                .flat_map(|(_, hits)| hits)
                .map(|hit| &hit.file)
                .find(|f| f.username == user && f.path == path)
                .cloned()
        {
            self.enqueue(core, vec![file], false, args.to, args.wait, reply);
            return;
        }
        self.browse(
            core,
            user,
            BrowseWork::Download {
                path,
                folder_only,
                to: args.to,
                wait: args.wait,
            },
            reply,
        );
    }

    fn finish_browse(&mut self, core: &Core, user: &str, dirs: &[BrowseDir], call: Browse) {
        match call.work {
            BrowseWork::List(folder) => {
                let folders: Vec<_> = dirs
                    .iter()
                    .filter(|d| folder.as_ref().is_none_or(|f| same_path(f, &d.path)))
                    .map(|dir| BrowseFolder {
                        path: dir.path.clone(),
                        files: dir.files.iter().map(file_data).collect(),
                    })
                    .collect();
                if folder.is_some() && folders.is_empty() {
                    fail(call.reply, ErrorCode::NotFound, "remote folder not found");
                } else {
                    respond(
                        call.reply,
                        &BrowseData {
                            user: user.into(),
                            folders,
                        },
                    );
                }
            }
            BrowseWork::Download {
                path,
                folder_only,
                to,
                wait,
            } => {
                let selected = dirs.iter().find(|d| same_path(&d.path, &path));
                let (mut files, folder) = match selected {
                    Some(dir) => (
                        dir.files
                            .iter()
                            .filter(|f| !paths::is_download_junk(&f.path))
                            .cloned()
                            .collect(),
                        true,
                    ),
                    None if folder_only => (Vec::new(), true),
                    None => (
                        dirs.iter()
                            .flat_map(|d| &d.files)
                            .find(|f| same_path(&f.path, &path))
                            .cloned()
                            .into_iter()
                            .collect(),
                        false,
                    ),
                };
                files.retain(|f| f.username == user && safe_remote_file(f));
                if files.is_empty() {
                    fail(
                        call.reply,
                        ErrorCode::NotFound,
                        "remote path has no downloadable files",
                    );
                } else {
                    self.enqueue(core, files, folder, to, wait, call.reply);
                }
            }
        }
    }

    fn enqueue(
        &mut self,
        core: &Core,
        files: Vec<RemoteFile>,
        folder: bool,
        to: Option<PathBuf>,
        wait: bool,
        reply: Reply,
    ) {
        if files.len() > MAX_FOLDER_FILES {
            fail(
                reply,
                ErrorCode::BadRequest,
                "remote folder exceeds the per-request file limit",
            );
            return;
        }
        if files.iter().any(|f| !safe_remote_file(f)) {
            fail(
                reply,
                ErrorCode::BadRequest,
                "remote filename is not safe to download",
            );
            return;
        }
        if !self.reserve_rows(files.len()) {
            fail(reply, ErrorCode::Busy, "too many active transfers");
            return;
        }
        let root = to.as_deref().unwrap_or(&core.settings().download_dir);
        let ids = files
            .into_iter()
            .map(|file| {
                let dir = if folder || core.settings().single_file_subfolder {
                    paths::download_dir_for(root, &file.path)
                } else {
                    root.to_path_buf()
                };
                self.start_transfer(
                    core,
                    AgentTransfer {
                        id: String::new(),
                        tried: BTreeSet::from([file.username.clone()]),
                        file,
                        dir,
                        origin: TransferOrigin::Cli,
                    },
                )
            })
            .collect();
        self.waiting.push(Waiting {
            ids,
            all_done: wait,
            allow_cancelled: false,
            deadline: Instant::now() + if wait { DOWNLOAD_WAIT } else { IO_ACK },
            reply,
        });
    }

    fn reserve_rows(&mut self, extra: usize) -> bool {
        let mut excess = self
            .rows
            .len()
            .saturating_add(extra)
            .saturating_sub(MAX_ROWS);
        if excess > 0 {
            self.rows.retain(|id, row| {
                if excess > 0
                    && !active(row.data.state)
                    && !self.waiting.iter().any(|w| w.ids.contains(id))
                {
                    excess -= 1;
                    false
                } else {
                    true
                }
            });
        }
        excess == 0
    }

    fn start_transfer(&mut self, core: &Core, mut agent: AgentTransfer) -> TransferId {
        let id = core.download_agent_file(&agent.file, &agent.dir);
        if agent.id.is_empty() {
            agent.id = format!("{}-{}", self.prefix, id.0);
        }
        let data = transfer_data(agent.id.clone(), &agent.file, agent.origin);
        self.rows.insert(
            id,
            Row {
                data,
                agent: Some(agent),
                acknowledged: false,
            },
        );
        id
    }

    fn find_row(&self, ext_id: &str) -> Option<(TransferId, &Row)> {
        self.rows
            .iter()
            .find_map(|(&id, row)| (row.data.id == ext_id).then_some((id, row)))
    }

    fn retry(&mut self, core: &Core, ext_id: &str, reply: Reply) {
        let Some((id, row)) = self.find_row(ext_id) else {
            fail(reply, ErrorCode::NotFound, "unknown transfer ID");
            return;
        };
        let Some(mut agent) = row.agent.clone() else {
            fail(
                reply,
                ErrorCode::Forbidden,
                "cannot switch an app-started transfer",
            );
            return;
        };
        if row.data.state == TransferState::Done {
            fail(reply, ErrorCode::BadRequest, "transfer is already complete");
            return;
        }
        let hits: Vec<_> = self
            .state
            .hits
            .iter()
            .filter(|(at, _)| fresh(*at))
            .flat_map(|(_, h)| h.iter().cloned())
            .collect();
        let Some(alternate) = other_source(&agent, &hits) else {
            fail(
                reply,
                ErrorCode::NotFound,
                "no other source in recent searches",
            );
            return;
        };
        let was_active = active(row.data.state);
        if !self.reserve_rows(1) {
            fail(reply, ErrorCode::Busy, "too many active transfers");
            return;
        }
        if was_active {
            core.cancel(id);
        }
        agent.tried.insert(alternate.username.clone());
        agent.file = alternate;
        agent.id.clear(); // a replacement has its own transfer ID
        let id = self.start_transfer(core, agent);
        self.waiting.push(Waiting {
            ids: vec![id],
            all_done: false,
            allow_cancelled: false,
            deadline: Instant::now() + IO_ACK,
            reply,
        });
    }

    fn cancel(&mut self, core: &Core, external: &str, reply: Reply) {
        let Some((id, row)) = self.find_row(external) else {
            fail(reply, ErrorCode::NotFound, "unknown transfer ID");
            return;
        };
        if row.data.origin != TransferOrigin::Cli {
            fail(
                reply,
                ErrorCode::Forbidden,
                "cannot cancel an app-started transfer",
            );
            return;
        }
        if active(row.data.state) {
            core.cancel(id);
            self.waiting.push(Waiting {
                ids: vec![id],
                all_done: true,
                allow_cancelled: true,
                deadline: Instant::now() + IO_ACK,
                reply,
            });
        } else {
            respond(
                reply,
                &TransfersData {
                    transfers: vec![row.data.clone()],
                },
            );
        }
    }

    pub fn event(&mut self, core: &Core, event: &CoreEvent) {
        match event {
            CoreEvent::Connected { .. } => {
                self.connected = true;
                self.connecting = false;
                self.last_request = Instant::now();
                self.restore(core);
                for (request, reply, _) in std::mem::take(&mut self.waiting_for_login) {
                    self.request(core, request, reply);
                }
            }
            CoreEvent::Disconnected { reason, will_retry } => {
                self.connected = false;
                self.connecting = *will_retry;
                self.displaced = *reason == DisconnectReason::Displaced;
                if !will_retry {
                    for (_, reply, _) in self.waiting_for_login.drain(..) {
                        fail(reply, ErrorCode::NotConnected, "Soulseek did not connect");
                    }
                }
            }
            CoreEvent::SearchResults { search, hits } => {
                if let Some(pending) = self.searches.get_mut(search) {
                    pending.hits.extend(
                        hits.iter()
                            .take(MAX_HITS.saturating_sub(pending.hits.len()))
                            .filter(|hit| {
                                hit.file.path.len() <= 4096 && hit.file.username.len() <= 512
                            })
                            .cloned(),
                    );
                }
            }
            CoreEvent::SearchFinished { search, .. } => {
                if let Some(pending) = self.searches.remove(search) {
                    self.finish_search(pending);
                }
            }
            CoreEvent::BrowseResult { username, dirs } => {
                for pending in self.browses.remove(username).unwrap_or_default() {
                    self.finish_browse(core, username, dirs, pending);
                }
            }
            CoreEvent::BrowseFailed { username, .. } => {
                for pending in self.browses.remove(username).unwrap_or_default() {
                    fail(
                        pending.reply,
                        ErrorCode::NotFound,
                        "could not browse remote shares",
                    );
                }
            }
            CoreEvent::TransferQueued {
                id,
                file,
                kind: TransferKind::Download,
                ..
            } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.acknowledged = true;
                    row.data.state = TransferState::Queued;
                    row.data.error = None;
                    row.data.queue_position = None;
                } else if self.reserve_rows(1) {
                    self.rows.insert(
                        *id,
                        Row {
                            data: transfer_data(format!("app-{}", id.0), file, TransferOrigin::App),
                            agent: None,
                            acknowledged: true,
                        },
                    );
                }
            }
            CoreEvent::TransferRemoteQueued { id, place } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.data.state = TransferState::Queued;
                    row.data.queue_position = *place;
                }
            }
            CoreEvent::TransferProgress {
                id,
                bytes,
                total,
                speed,
                ..
            } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.data.state = TransferState::Downloading;
                    row.data.bytes = *bytes;
                    row.data.total_bytes = *total;
                    row.data.speed_bytes_per_sec = if speed.is_finite() { *speed } else { 0.0 };
                    row.data.queue_position = None;
                }
            }
            CoreEvent::TransferDone { id, .. } => self.terminal(*id, TransferState::Done),
            CoreEvent::TransferFailed { id, .. } => self.terminal(*id, TransferState::Failed),
            CoreEvent::TransferCancelled { id } => self.terminal(*id, TransferState::Cancelled),
            _ => {}
        }
    }

    fn terminal(&mut self, id: TransferId, state: TransferState) {
        if let Some(row) = self.rows.get_mut(&id) {
            row.acknowledged = true;
            row.data.state = state;
            row.data.queue_position = None;
            row.data.speed_bytes_per_sec = 0.0;
            if state == TransferState::Done {
                row.data.bytes = row.data.total_bytes;
            } else if state == TransferState::Failed {
                // Core reasons can contain local paths and peer-controlled strings.
                row.data.error = Some("transfer failed".into());
            }
        }
    }

    pub fn tick(&mut self, core: &Core) {
        let now = Instant::now();
        for (id, search) in std::mem::take(&mut self.searches) {
            if search.deadline <= now {
                core.cancel_search(id);
                self.finish_search(search);
            } else {
                self.searches.insert(id, search);
            }
        }
        self.browses.retain(|_, pending| {
            pending.retain(|b| {
                if b.deadline <= now {
                    let _ = b.reply.send(Response::failure(Failure::new(
                        ErrorCode::NotFound,
                        "browse timed out",
                    )));
                    false
                } else {
                    true
                }
            });
            !pending.is_empty()
        });
        self.waiting_for_login.retain(|(_, reply, deadline)| {
            if *deadline <= now {
                let _ = reply.send(Response::failure(Failure::new(
                    ErrorCode::NotConnected,
                    "Soulseek connection timed out",
                )));
                false
            } else {
                true
            }
        });
        for waiting in std::mem::take(&mut self.waiting) {
            let rows = waiting.ids.iter().filter_map(|id| self.rows.get(id));
            let ready = rows
                .clone()
                .all(|row| row.acknowledged && (!waiting.all_done || !active(row.data.state)));
            if ready {
                let transfers: Vec<_> = rows.map(|row| row.data.clone()).collect();
                if let Some(row) = transfers.iter().find(|row| {
                    row.state == TransferState::Failed
                        || (!waiting.allow_cancelled && row.state == TransferState::Cancelled)
                }) {
                    fail(
                        waiting.reply,
                        ErrorCode::TransferFailed,
                        &format!("transfer {} did not complete", row.id),
                    );
                } else {
                    respond(waiting.reply, &TransfersData { transfers });
                }
            } else if waiting.deadline <= now {
                fail(
                    waiting.reply,
                    ErrorCode::Busy,
                    "wait timed out; transfer may remain queued or running",
                );
            } else {
                self.waiting.push(waiting);
            }
        }
        if !self.restoring.is_empty()
            && self
                .restoring
                .iter()
                .all(|id| self.rows.get(id).is_some_and(|row| row.acknowledged))
        {
            self.restoring.clear();
            let _ = std::fs::remove_file(&self.handover_path);
        }
    }

    /// Call only after closing admission and dropping Core. Any Done events
    /// emitted between the last poll and shutdown are excluded from the batch.
    pub fn save_handover(&self, completed_during_shutdown: &HashSet<TransferId>) -> io::Result<()> {
        let entries: Vec<_> = self
            .rows
            .iter()
            .filter_map(|(id, row)| {
                (active(row.data.state) && !completed_during_shutdown.contains(id))
                    .then_some(row.agent.as_ref())
                    .flatten()
            })
            .collect();
        write_atomic(&self.handover_path, &entries)
    }

    fn restore(&mut self, core: &Core) {
        if self.mode != Mode::App || !self.restoring.is_empty() {
            return;
        }
        let Ok(entries) =
            read_state::<Vec<AgentTransfer>>(&self.handover_path, Some(HANDOVER_MAX_AGE))
        else {
            return;
        };
        for agent in entries.unwrap_or_default().into_iter().take(MAX_ROWS) {
            if agent.origin == TransferOrigin::Cli && safe_remote_file(&agent.file) {
                let id = self.start_transfer(core, agent);
                self.restoring.insert(id);
            }
        }
        if self.restoring.is_empty() {
            let _ = std::fs::remove_file(&self.handover_path);
        }
    }
}

const IO_ACK: Duration = Duration::from_secs(5);

fn other_source(agent: &AgentTransfer, hits: &[SearchHit]) -> Option<RemoteFile> {
    rank::alternates(&agent.file, hits)
        .into_iter()
        .find(|file| !agent.tried.contains(&file.username) && safe_remote_file(file))
}

fn since_epoch() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn fresh(at: u64) -> bool {
    since_epoch().as_secs().saturating_sub(at) < RESULT_TTL_SECS
}

fn active(state: TransferState) -> bool {
    matches!(state, TransferState::Queued | TransferState::Downloading)
}

fn transfer_data(id: String, file: &RemoteFile, origin: TransferOrigin) -> TransferData {
    TransferData {
        id,
        state: TransferState::Queued,
        bytes: 0,
        total_bytes: file.size,
        speed_bytes_per_sec: 0.0,
        queue_position: None,
        user: file.username.clone(),
        path: file.path.clone(),
        error: None,
        origin,
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
}

fn safe_remote_file(file: &RemoteFile) -> bool {
    let base = file.basename();
    valid_text(&file.username)
        && valid_text(base)
        && !base.contains(':')
        && matches!(
            Path::new(base).components().next(),
            Some(Component::Normal(_))
        )
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches(['/', '\\']).replace('/', "\\");
    norm(a).eq_ignore_ascii_case(&norm(b))
}

fn file_data(file: &RemoteFile) -> FileData {
    let a = file.attrs;
    FileData {
        path: file.path.clone(),
        size_bytes: file.size,
        format: file.extension(),
        bitrate_kbps: a.bitrate_kbps,
        bit_depth: a.bit_depth,
        sample_rate_hz: a.sample_rate_hz,
        duration_secs: a.duration_secs,
        vbr: a.vbr,
    }
}

fn format_matches(ext: &str, f: &Format) -> bool {
    match f {
        Format::Flac => ext == "flac",
        Format::Mp3 => ext == "mp3",
        Format::Wav => ext == "wav",
        Format::Lossless => matches!(ext, "flac" | "wav" | "aif" | "aiff" | "alac" | "ape" | "wv"),
    }
}

fn search_groups(hits: &[SearchHit], args: &SearchArgs) -> Vec<SearchGroup> {
    let mut groups: Vec<SearchGroup> = Vec::new();
    let mut indices = HashMap::new();
    let mut seen = HashSet::new();
    let limit = args.limit.unwrap_or(20);
    for hit in hits {
        let f = &hit.file;
        let ext = f.extension();
        if (!args.formats.is_empty()
            && !args
                .formats
                .iter()
                .any(|format| format_matches(&ext, format)))
            || args
                .min_bitrate
                .is_some_and(|min| f.attrs.bitrate_kbps.is_none_or(|rate| rate < min))
            || !safe_remote_file(f)
            || !seen.insert((&f.username, &f.path))
        {
            continue;
        }
        let key = (f.username.as_str(), f.folder());
        let idx = match indices.get(&key) {
            Some(&idx) => idx,
            None if groups.len() == limit => continue,
            None => {
                let idx = groups.len();
                indices.insert(key, idx);
                groups.push(SearchGroup {
                    id: String::new(),
                    user: f.username.clone(),
                    folder: f.folder().into(),
                    files: Vec::new(),
                    format: ext.clone(),
                    bitrate_kbps: f.attrs.bitrate_kbps,
                    bit_depth: f.attrs.bit_depth,
                    sample_rate_hz: f.attrs.sample_rate_hz,
                    size_bytes: 0,
                    file_count: 0,
                    free_slot: hit.free_slot,
                    queue_position: None,
                    speed_bytes_per_sec: hit.speed,
                });
                idx
            }
        };
        let group = &mut groups[idx];
        if group.format != ext {
            group.format = "mixed".into();
        }
        if group.bitrate_kbps != f.attrs.bitrate_kbps {
            group.bitrate_kbps = None;
        }
        if group.bit_depth != f.attrs.bit_depth {
            group.bit_depth = None;
        }
        if group.sample_rate_hz != f.attrs.sample_rate_hz {
            group.sample_rate_hz = None;
        }
        group.size_bytes = group.size_bytes.saturating_add(f.size);
        group.file_count += 1;
        group.files.push(file_data(f));
    }
    groups
}

fn respond(reply: Reply, value: &impl Serialize) {
    let response = serde_json::to_value(value)
        .map(Response::success)
        .unwrap_or_else(|_| {
            Response::failure(Failure::new(
                ErrorCode::Internal,
                "could not encode control response",
            ))
        });
    let _ = reply.send(response);
}

fn fail(reply: Reply, code: ErrorCode, message: &str) {
    let _ = reply.send(Response::failure(Failure::new(code, message)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioAttrs, Settings};
    use std::sync::mpsc;

    fn hit(user: &str, path: &str, free_slot: bool, speed: u32) -> SearchHit {
        SearchHit {
            file: RemoteFile {
                username: user.into(),
                path: path.into(),
                size: 44,
                attrs: AudioAttrs::default(),
            },
            free_slot,
            speed,
        }
    }

    #[test]
    fn retry_uses_ranked_alternate_and_keeps_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let (core, _events) = Core::start(Settings {
            cache_dir: tmp.path().join("cache"),
            data_dir: tmp.path().join("data"),
            download_dir: tmp.path().join("downloads"),
            ..Settings::default()
        })
        .unwrap();
        let source = hit("original", r"Music\01 Song.flac", true, 300);
        let earlier = hit("already-tried", r"Music\Song.flac", true, 999);
        let alternate = hit("other", r"Albums\02 Song.flac", true, 500);
        let busy = hit("busy", r"Albums\Song.flac", false, 9999);
        let dir = tmp.path().join("target");
        let mut engine = Engine::open(tmp.path(), Mode::App, false).unwrap();
        engine.connected = true;
        engine.state.hits.push_back((
            since_epoch().as_secs(),
            vec![source.clone(), earlier, busy, alternate.clone()],
        ));
        let original = engine.start_transfer(
            &core,
            AgentTransfer {
                id: "original-id".into(),
                file: source.file,
                dir: dir.clone(),
                tried: BTreeSet::from(["original".into(), "already-tried".into()]),
                origin: TransferOrigin::Cli,
            },
        );
        let (tx, _rx) = mpsc::channel();
        engine.request(
            &core,
            Request::new(Operation::RetryOtherSource(IdArgs {
                id: "original-id".into(),
            })),
            tx,
        );
        let (&id, row) = engine.rows.last_key_value().unwrap();
        assert_ne!(id, original);
        let row = row.agent.as_ref().unwrap();
        assert_eq!(row.file, alternate.file);
        assert_eq!(row.dir, dir);
        assert_eq!(row.origin, TransferOrigin::Cli);
        assert!(row.tried.contains("other"));
    }
}
