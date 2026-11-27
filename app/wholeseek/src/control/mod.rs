//! Desktop and daemon entry points for the single local session.

#[cfg(windows)]
mod windows;

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wholeseek_core::control::engine::Engine;
use wholeseek_core::control::ownership::{Lease, Ownership};
use wholeseek_core::control::{self, Caller, ErrorCode, Failure, Mode, Request, Response};
use wholeseek_core::{Core, CoreEvent, config};

#[cfg(unix)]
use control::unix::{Endpoint, Listener, Stream};
#[cfg(windows)]
use windows::{Endpoint, Listener, Stream};

const POLL: Duration = Duration::from_millis(50);
const START_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_CLIENTS: usize = 32;

type Wake = Arc<dyn Fn() + Send + Sync>;
type RequestEnvelope = (Request, mpsc::Sender<Response>);

struct Local {
    endpoint: Endpoint,
    owner: Ownership,
}

impl Local {
    fn system() -> io::Result<Self> {
        let owner = Ownership::new(config::config_dir().join("agent-control"))?;
        #[cfg(unix)]
        let endpoint = Endpoint::new(
            &dirs::runtime_dir()
                .unwrap_or_else(|| config::BaseDirs::system().cache)
                .join("wholeseek-control"),
            "control",
        )?;
        #[cfg(windows)]
        let endpoint = Endpoint::new("control")?;
        Ok(Self { endpoint, owner })
    }

    fn bind_owned(&self, _lease: &Lease) -> io::Result<Server> {
        #[cfg(unix)]
        self.endpoint.remove_stale()?;
        Server::start(self.endpoint.bind()?)
    }
}

/// Constructor is cheap and does not read the keyring or start a process.
/// Startup only happens after an initial socket connection fails.
pub struct SocketCaller;

impl Caller for SocketCaller {
    fn call(&mut self, request: &Request) -> io::Result<Response> {
        let local = Local::system()?;
        let mut stream = connect_or_start(
            || local.endpoint.connect(),
            || Ok(local.owner.session()?.is_some()),
            spawn_daemon,
            START_TIMEOUT,
        )?;
        // Never replay after a write: downloads may already have been queued.
        control::transport::exchange(&mut stream, request)
    }
}

fn connect_or_start<T>(
    mut connect: impl FnMut() -> io::Result<T>,
    may_spawn: impl FnOnce() -> io::Result<bool>,
    spawn: impl FnOnce() -> io::Result<()>,
    timeout: Duration,
) -> io::Result<T> {
    let initial = match connect() {
        Ok(stream) => return Ok(stream),
        Err(e) if waiting_for_endpoint(&e) => e,
        Err(e) => return Err(e),
    };
    if initial.kind() != io::ErrorKind::WouldBlock && may_spawn()? {
        spawn()?;
    }
    let deadline = Instant::now() + timeout;
    loop {
        match connect() {
            Ok(stream) => return Ok(stream),
            Err(e) if waiting_for_endpoint(&e) && Instant::now() < deadline => {
                thread::sleep(POLL);
            }
            Err(e) => return Err(e),
        }
    }
}

fn waiting_for_endpoint(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    #[cfg(windows)]
    if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) {
        return true;
    }
    false
}

fn spawn_daemon() -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    command.spawn().map(|_| ())
}

struct Server {
    inbox: Receiver<RequestEnvelope>,
    stopped: Arc<AtomicBool>,
    wake: Arc<Mutex<Option<Wake>>>,
    acceptor: Option<JoinHandle<()>>,
}

impl Server {
    fn start(mut listener: Listener) -> io::Result<Self> {
        let (send, inbox) = mpsc::sync_channel(MAX_CLIENTS);
        let stopped = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Mutex::new(None));
        let acceptor = {
            let stopped = stopped.clone();
            let wake = wake.clone();
            thread::Builder::new()
                .name("wholeseek-control".into())
                .spawn(move || {
                    let clients = Arc::new(AtomicUsize::new(0));
                    while !stopped.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok(Some(stream)) if clients.load(Ordering::Relaxed) < MAX_CLIENTS => {
                                serve_client(stream, send.clone(), wake.clone(), clients.clone());
                            }
                            Ok(Some(_)) => {}
                            Ok(None) | Err(_) => thread::sleep(POLL),
                        }
                    }
                })?
        };
        Ok(Self {
            inbox,
            stopped,
            wake,
            acceptor: Some(acceptor),
        })
    }

    fn set_waker(&self, wake: Wake) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

struct ClientSlot(Arc<AtomicUsize>);

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn serve_client(
    mut stream: Stream,
    send: SyncSender<RequestEnvelope>,
    wake: Arc<Mutex<Option<Wake>>>,
    clients: Arc<AtomicUsize>,
) {
    clients.fetch_add(1, Ordering::Relaxed);
    let slot = ClientSlot(clients);
    // Capturing the RAII slot before spawning also releases it on spawn error.
    let _ = thread::Builder::new()
        .name("wholeseek-control-client".into())
        .spawn(move || {
            let _slot = slot;
            let _ = control::transport::serve(&mut stream, |req| {
                let (reply, response) = mpsc::channel();
                match send.try_send((req, reply)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        return failure(ErrorCode::Busy, "control server is busy");
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return failure(ErrorCode::NotConnected, "session is shutting down");
                    }
                }
                let wake = wake.lock().ok().and_then(|slot| slot.clone());
                if let Some(wake) = wake {
                    wake();
                }
                response
                    .recv_timeout(Duration::from_secs(59 * 60))
                    .unwrap_or_else(|_| {
                        failure(ErrorCode::NotConnected, "session stopped before replying")
                    })
            });
        });
}

fn failure(code: ErrorCode, message: &str) -> Response {
    Response::failure(Failure::new(code, message))
}

pub struct Host {
    server: Server,
    engine: Engine,
}

impl Host {
    fn new(local: &Local, lease: &Lease, mode: Mode, connecting: bool) -> io::Result<Self> {
        let engine = Engine::open(local.owner.directory(), mode, connecting)?;
        let server = local.bind_owned(lease)?;
        Ok(Self { server, engine })
    }

    pub fn set_waker(&self, wake: Wake) {
        self.server.set_waker(wake);
    }

    pub fn event(&mut self, core: &Core, event: &CoreEvent) {
        self.engine.event(core, event);
    }

    pub fn step(&mut self, core: &Core) {
        for _ in 0..MAX_CLIENTS {
            let Ok((request, reply)) = self.server.inbox.try_recv() else {
                break;
            };
            self.engine.request(core, request, reply);
        }
        self.engine.tick(core);
    }

    pub fn next_tick(&self) -> Option<f64> {
        self.engine.has_pending().then_some(POLL.as_secs_f64())
    }
}

// Never release before process exit: Core's helper threads can outlive its
// main worker briefly during teardown. The OS closes the lease on exit/crash.
static LEASE: OnceLock<Lease> = OnceLock::new();
static BOOTSTRAP: Mutex<Option<Host>> = Mutex::new(None);

/// Called before the desktop starts any path that could issue Core::login.
pub fn prepare_app() -> io::Result<()> {
    let local = Local::system()?;
    let intent = local
        .owner
        .app_intent()?
        .ok_or_else(|| io::Error::other("another app is starting"))?;
    let deadline = Instant::now() + START_TIMEOUT;
    let lease = loop {
        if let Some(lease) = local.owner.session()? {
            break lease;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "another wholeseek session is running",
            ));
        }
        thread::sleep(POLL);
    };
    let host = Host::new(&local, &lease, Mode::App, false)?;
    LEASE
        .set(lease)
        .map_err(|_| io::Error::other("session already initialized"))?;
    *BOOTSTRAP
        .lock()
        .map_err(|_| io::Error::other("control startup failed"))? = Some(host);
    drop(intent);
    Ok(())
}

pub fn take_app_host() -> io::Result<Host> {
    BOOTSTRAP
        .lock()
        .ok()
        .and_then(|mut host| host.take())
        .ok_or_else(|| io::Error::other("app has no session lease"))
}

/// Hidden process entry point. No CLI client code can log in directly.
pub fn daemon() -> io::Result<()> {
    let local = Local::system()?;
    let Some(lease) = local.owner.session()? else {
        return Ok(());
    };
    if local.owner.app_is_waiting()? {
        return Ok(());
    }

    // SAFETY: this standalone daemon path runs before spawning any threads.
    let log = config::default_log_path();
    unsafe { config::init_logging(&log) }?;
    daemon_log(&log, "control session starting");
    LEASE
        .set(lease)
        .map_err(|_| io::Error::other("session already initialized"))?;

    let _ = config::migrate_legacy();
    let loaded = config::load()?;
    let mut host = Host::new(
        &local,
        LEASE.get().expect("set above"),
        Mode::Daemon,
        loaded.credentials.is_some(),
    )?;
    let (core, events) = Core::start(loaded.settings)?;
    if let Some(credentials) = loaded.credentials {
        core.login(credentials);
    }
    let handover = loop {
        for event in events.try_iter() {
            host.event(&core, &event);
        }
        if host.engine.displaced() {
            daemon_log(
                &log,
                "displaced by another login; exiting without reconnecting",
            );
            break false;
        }
        if local.owner.app_is_waiting()? {
            break true;
        }
        host.step(&core);
        if host.engine.idle() {
            daemon_log(&log, "idle timeout; exiting");
            break false;
        }
        thread::sleep(POLL);
    };

    let Host { server, engine } = host;
    drop(server);
    drop(core);
    if handover {
        let done: HashSet<_> = events
            .try_iter()
            .filter_map(|event| match event {
                CoreEvent::TransferDone { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        engine.save_handover(&done)?;
        daemon_log(&log, "queued work handed to app; exiting");
    }
    Ok(())
}

fn daemon_log(path: &std::path::Path, text: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "daemon: {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_app_connection_never_starts_a_daemon_or_probes_the_login_lock() {
        let socket = connect_or_start(
            || Ok("existing app socket"),
            || panic!("must never probe ownership when app is reachable"),
            || panic!("must never start a second login"),
            START_TIMEOUT,
        )
        .unwrap();
        assert_eq!(socket, "existing app socket");
    }
}
