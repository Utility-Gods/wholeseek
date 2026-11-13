//! System media keys and "now playing", so Play/Pause, Next and Previous keys and headphone
//! buttons (and the GNOME/KDE/macOS media widgets) control wholeseek when it is not focused.
//!
//! - Linux: MPRIS (`org.mpris.MediaPlayer2.wholeseek` on the session bus) via zbus, served from
//!   its own thread. No session bus: one log line, the app works as before.
//! - macOS: `MPRemoteCommandCenter` handlers and `MPNowPlayingInfoCenter` (info + playback state,
//!   so the keys come to us and not to Music.app).
//!
//! Commands only ever travel over a channel (plus a window wake); [`App::media_tick`] applies
//! them on the UI thread with the existing `toggle_pause` / `next_track` / `previous_track`, then
//! publishes what plays. Nothing here blocks the UI thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use wholeseek_audio::PlayState;

use crate::app::App;

/// The D-Bus name suffix and the identity shown in media widgets.
pub const NAME: &str = "wholeseek";

/// What a media key or widget asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

/// The current track, as media widgets show it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    /// Stable per (user, remote path).
    pub id: u64,
    /// File name without extension (as the player strip shows it).
    pub title: String,
    /// The Soulseek user it streams from (the strip's "from <user>").
    pub artist: String,
    /// The remote folder name.
    pub album: String,
    pub length: Option<Duration>,
    /// The album cover in the cover cache, if one is cached.
    pub art: Option<PathBuf>,
}

/// Everything published to the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub track: Option<Track>,
    pub status: Status,
    pub position: Duration,
    pub volume_pct: u8,
}

impl State {
    pub const STOPPED: Self = Self {
        track: None,
        status: Status::Stopped,
        position: Duration::ZERO,
        volume_pct: 100,
    };

    /// Same track, status and volume: only the clock moved.
    fn same_but_position(&self, o: &Self) -> bool {
        self.track == o.track && self.status == o.status && self.volume_pct == o.volume_pct
    }
}

/// What the app does for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    Next,
    Previous,
    Nothing,
}

/// The command mapping. Stop pauses (it never cancels fetches or drops the queue); Play and
/// Pause only act when they change something; Next/Previous always go to the queue.
#[must_use]
pub fn action(cmd: Command, status: Status) -> Action {
    use Command as C;
    use Status as S;
    match (cmd, status) {
        (C::Next, _) => Action::Next,
        (C::Previous, _) => Action::Previous,
        (C::PlayPause, S::Playing | S::Paused)
        | (C::Play, S::Paused)
        | (C::Pause | C::Stop, S::Playing) => Action::TogglePause,
        _ => Action::Nothing,
    }
}

/// What plays now, from the player strip's state.
#[must_use]
pub fn state_of(s: &App) -> State {
    let volume_pct = (s.volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    let Some(p) = &s.playing else {
        return State {
            volume_pct,
            ..State::STOPPED
        };
    };
    let snap = p.snapshot.as_ref();
    let status = match snap.map(|s| &s.state) {
        Some(PlayState::Playing | PlayState::Buffering) => Status::Playing,
        Some(PlayState::Paused | PlayState::Ended) => Status::Paused,
        Some(PlayState::Idle | PlayState::Error(_)) => Status::Stopped,
        // Fetching the first bytes: it is about to play.
        None if p.failed.is_none() => Status::Playing,
        None => Status::Stopped,
    };
    let secs = |x: f64| Duration::from_secs_f64(x.max(0.0));
    let folder = p.file.folder();
    // A Library file: its tags (never an empty "artist" from the pseudo user).
    let (title, artist, album) = if p.file.is_local() {
        s.library_names(&p.file)
    } else {
        (
            crate::fmt::title(&p.file),
            p.file.username.clone(),
            wholeseek_core::paths::remote_basename(folder).to_string(),
        )
    };
    let track = Track {
        id: track_id(&p.file.username, &p.file.path),
        title,
        artist,
        album,
        length: snap.and_then(|s| s.duration_secs).map(secs),
        art: s
            .backend
            .covers()
            .and_then(|c| c.cached(&p.file.username, folder)),
    };
    State {
        track: Some(track),
        status,
        position: snap.map_or(Duration::ZERO, |s| secs(s.position_secs)),
        volume_pct,
    }
}

fn track_id(user: &str, path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (user, path).hash(&mut h);
    h.finish()
}

/// One MPRIS metadata value, platform-neutral (so the mapping is testable anywhere).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Path(String),
    Str(String),
    Strs(Vec<String>),
    Micros(i64),
}

/// MPRIS `Metadata` for `track` (spec: a `mpris:trackid` is always present).
#[must_use]
pub fn metadata(track: Option<&Track>) -> Vec<(&'static str, Field)> {
    let Some(t) = track else {
        return vec![(
            "mpris:trackid",
            Field::Path("/org/mpris/MediaPlayer2/TrackList/NoTrack".into()),
        )];
    };
    let mut m = vec![
        (
            "mpris:trackid",
            Field::Path(format!("/org/wholeseek/track/t{:016x}", t.id)),
        ),
        ("xesam:title", Field::Str(t.title.clone())),
        ("xesam:artist", Field::Strs(vec![t.artist.clone()])),
        ("xesam:album", Field::Str(t.album.clone())),
    ];
    if let Some(l) = t.length {
        m.push((
            "mpris:length",
            Field::Micros(i64::try_from(l.as_micros()).unwrap_or(i64::MAX)),
        ));
    }
    if let Some(a) = &t.art {
        m.push(("mpris:artUrl", Field::Str(file_url(a))));
    }
    m
}

/// `file://` URL with everything but unreserved characters and `/` percent-encoded.
#[must_use]
pub fn file_url(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A state change for the platform side; `at` anchors `state.position` for clients that
/// read the position later (it advances while playing).
#[derive(Debug, Clone)]
pub enum Update {
    State(State, Instant),
    /// The position jumped (a seek or a track restart).
    Seeked(State, Instant),
}

/// The running integration: commands in, state out.
pub struct Media {
    commands: Receiver<Command>,
    platform: platform::Handle,
    last: Option<(State, Instant)>,
}

impl Media {
    /// Start listening for media keys; `None` where unsupported. Call on the UI (main) thread.
    /// Never blocks: the Linux bus connection is made on its own thread.
    #[must_use]
    pub fn start(wake: rune::Waker) -> Option<Self> {
        let (tx, commands) = mpsc::channel();
        let platform = platform::start(tx, wake)?;
        Some(Self {
            commands,
            platform,
            last: None,
        })
    }

    /// Publish `state` if it changed; position-only changes are sent only when the position
    /// jumped (clients extrapolate while playing).
    fn publish(&mut self, state: State, now: Instant) {
        let update = match &self.last {
            Some((old, _)) if *old == state => return,
            Some((old, at)) if state.same_but_position(old) => {
                let expected = if old.status == Status::Playing {
                    old.position + now.saturating_duration_since(*at)
                } else {
                    old.position
                };
                let drift = state.position.abs_diff(expected);
                if drift < Duration::from_millis(1500) {
                    return;
                }
                Update::Seeked(state.clone(), now)
            }
            _ => Update::State(state.clone(), now),
        };
        self.platform.send(update);
        self.last = Some((state, now));
    }
}

impl App {
    /// Apply media-key commands and publish what plays. Returns true if a command changed
    /// something. A no-op until [`Media::start`] ran (tests, headless).
    pub fn media_tick(&mut self) -> bool {
        let Some(m) = &self.media else { return false };
        let cmds: Vec<Command> = m.commands.try_iter().collect();
        let mut changed = false;
        for c in cmds {
            let status = state_of(self).status;
            let a = action(c, status);
            eprintln!("wholeseek: media key {c:?} → {a:?}");
            match a {
                Action::TogglePause => self.toggle_pause(),
                Action::Next => self.next_track(),
                Action::Previous => self.previous_track(),
                Action::Nothing => continue,
            }
            changed = true;
        }
        let state = state_of(self);
        if let Some(m) = &mut self.media {
            m.publish(state, Instant::now());
        }
        changed
    }
}

#[cfg(target_os = "linux")]
mod platform {
    pub use super::mpris::Handle;
    use super::{Command, mpris};
    use std::sync::mpsc::Sender;

    pub fn start(tx: Sender<Command>, wake: rune::Waker) -> Option<Handle> {
        Some(mpris::spawn(None, tx, wake))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub use super::mac::Handle;
    use super::{Command, mac};
    use std::sync::mpsc::Sender;

    pub fn start(tx: Sender<Command>, wake: rune::Waker) -> Option<Handle> {
        Some(mac::start(tx, wake))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{Command, Update};
    use std::sync::mpsc::Sender;

    pub struct Handle;
    impl Handle {
        pub fn send(&self, _u: Update) {}
    }
    pub fn start(_tx: Sender<Command>, _wake: rune::Waker) -> Option<Handle> {
        None
    }
}

// ------------------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
pub(crate) mod mpris {
    use super::{Command, Field, NAME, State, Status, Update, metadata};
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};
    use zbus::{blocking, interface};

    const PATH: &str = "/org/mpris/MediaPlayer2";

    pub struct Handle(pub(super) Sender<Update>);

    impl Handle {
        pub fn send(&self, u: Update) {
            // The service thread is gone (no session bus): nothing to tell.
            let _ = self.0.send(u);
        }
    }

    /// Serve MPRIS on its own thread: on `address` (a private bus, tests) or the session bus.
    /// Any failure is logged once and ends the thread; the app keeps working.
    pub fn spawn(address: Option<String>, tx: Sender<Command>, wake: rune::Waker) -> Handle {
        let (utx, urx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("wholeseek-mpris".into())
            .spawn(move || match serve(address, tx, wake, &urx) {
                Ok(name) => eprintln!("wholeseek: media keys: MPRIS {name} closed"),
                Err(e) => eprintln!("wholeseek: media keys unavailable (MPRIS: {e})"),
            });
        if let Err(e) = spawned {
            eprintln!("wholeseek: media keys unavailable ({e})");
        }
        Handle(utx)
    }

    fn serve(
        address: Option<String>,
        tx: Sender<Command>,
        wake: rune::Waker,
        updates: &Receiver<Update>,
    ) -> zbus::Result<String> {
        let builder = match address {
            Some(a) => blocking::connection::Builder::address(a.as_str())?,
            None => blocking::connection::Builder::session()?,
        };
        let conn = builder
            .serve_at(PATH, Root)?
            .serve_at(
                PATH,
                Player {
                    tx,
                    wake,
                    state: State::STOPPED,
                    at: Instant::now(),
                },
            )?
            .build()?;
        // A second wholeseek gets `….wholeseek.instance<pid>` (MPRIS convention).
        let name = format!("org.mpris.MediaPlayer2.{NAME}");
        let flags = RequestNameFlags::DoNotQueue.into();
        let name = match conn.request_name_with_flags(name.as_str(), flags)? {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => name,
            _ => {
                let alt = format!("{name}.instance{}", std::process::id());
                conn.request_name_with_flags(alt.as_str(), flags)?;
                alt
            }
        };
        eprintln!("wholeseek: media keys: MPRIS as {name}");
        let iface = conn.object_server().interface::<_, Player>(PATH)?;
        for u in updates {
            let mut p = iface.get_mut();
            let e = iface.signal_emitter();
            match u {
                Update::State(s, at) => {
                    let old = std::mem::replace(&mut p.state, s);
                    p.at = at;
                    if old.status != p.state.status {
                        zbus::block_on(p.playback_status_changed(e))?;
                    }
                    if old.track != p.state.track {
                        zbus::block_on(p.metadata_changed(e))?;
                        zbus::block_on(p.can_play_changed(e))?;
                        zbus::block_on(p.can_pause_changed(e))?;
                    }
                    if old.volume_pct != p.state.volume_pct {
                        zbus::block_on(p.volume_changed(e))?;
                    }
                }
                Update::Seeked(s, at) => {
                    p.state = s;
                    p.at = at;
                    let us = micros(p.state.position);
                    zbus::block_on(Player::seeked(e, us))?;
                }
            }
        }
        Ok(name)
    }

    fn micros(d: Duration) -> i64 {
        i64::try_from(d.as_micros()).unwrap_or(i64::MAX)
    }

    struct Root;

    #[interface(name = "org.mpris.MediaPlayer2")]
    impl Root {
        fn raise(&self) {}
        fn quit(&self) {}
        #[zbus(property)]
        fn can_quit(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn can_raise(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn has_track_list(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn identity(&self) -> String {
            NAME.into()
        }
        #[zbus(property)]
        fn desktop_entry(&self) -> String {
            NAME.into()
        }
        #[zbus(property)]
        fn supported_uri_schemes(&self) -> Vec<String> {
            Vec::new()
        }
        #[zbus(property)]
        fn supported_mime_types(&self) -> Vec<String> {
            Vec::new()
        }
    }

    struct Player {
        tx: Sender<Command>,
        wake: rune::Waker,
        state: State,
        at: Instant,
    }

    impl Player {
        fn post(&self, c: Command) {
            if self.tx.send(c).is_ok() {
                (self.wake)();
            }
        }
    }

    fn value(f: Field) -> Option<OwnedValue> {
        let v = match f {
            Field::Path(p) => Value::from(ObjectPath::try_from(p).ok()?),
            Field::Str(s) => Value::from(s),
            Field::Strs(v) => Value::from(v),
            Field::Micros(n) => Value::from(n),
        };
        v.try_to_owned().ok()
    }

    #[interface(name = "org.mpris.MediaPlayer2.Player")]
    impl Player {
        fn next(&self) {
            self.post(Command::Next);
        }
        fn previous(&self) {
            self.post(Command::Previous);
        }
        fn pause(&self) {
            self.post(Command::Pause);
        }
        fn play_pause(&self) {
            self.post(Command::PlayPause);
        }
        fn stop(&self) {
            self.post(Command::Stop);
        }
        fn play(&self) {
            self.post(Command::Play);
        }
        /// CanSeek is false: seeking stays in the window (it depends on what is downloaded).
        fn seek(&self, _offset: i64) {}
        fn set_position(&self, _track: ObjectPath<'_>, _position: i64) {}
        fn open_uri(&self, _uri: &str) {}

        #[zbus(signal)]
        async fn seeked(e: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

        #[zbus(property)]
        fn playback_status(&self) -> String {
            match self.state.status {
                Status::Playing => "Playing",
                Status::Paused => "Paused",
                Status::Stopped => "Stopped",
            }
            .into()
        }
        #[zbus(property)]
        fn rate(&self) -> f64 {
            1.0
        }
        #[zbus(property)]
        fn minimum_rate(&self) -> f64 {
            1.0
        }
        #[zbus(property)]
        fn maximum_rate(&self) -> f64 {
            1.0
        }
        #[zbus(property)]
        fn metadata(&self) -> HashMap<String, OwnedValue> {
            metadata(self.state.track.as_ref())
                .into_iter()
                .filter_map(|(k, f)| Some((k.to_string(), value(f)?)))
                .collect()
        }
        #[zbus(property)]
        fn volume(&self) -> f64 {
            f64::from(self.state.volume_pct) / 100.0
        }
        /// Not signalled (MPRIS): clients read it, and extrapolate while playing.
        #[zbus(property(emits_changed_signal = "false"))]
        fn position(&self) -> i64 {
            let mut p = self.state.position;
            if self.state.status == Status::Playing {
                p += self.at.elapsed();
            }
            if let Some(len) = self.state.track.as_ref().and_then(|t| t.length) {
                p = p.min(len);
            }
            micros(p)
        }
        #[zbus(property(emits_changed_signal = "const"))]
        fn can_go_next(&self) -> bool {
            true
        }
        #[zbus(property(emits_changed_signal = "const"))]
        fn can_go_previous(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_play(&self) -> bool {
            self.state.track.is_some()
        }
        #[zbus(property)]
        fn can_pause(&self) -> bool {
            self.state.track.is_some()
        }
        #[zbus(property(emits_changed_signal = "const"))]
        fn can_seek(&self) -> bool {
            false
        }
        #[zbus(property(emits_changed_signal = "const"))]
        fn can_control(&self) -> bool {
            true
        }
    }
}

// ------------------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
mod mac {
    //! Handlers run on the main run loop (winit's NSApp): they only post to the channel and
    //! wake the window, and always report success.

    use super::{Command, State, Status, Update};
    use block2::RcBlock;
    use objc2::Message;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    use objc2_media_player::{
        MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
        MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
        MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate,
        MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent,
        MPRemoteCommandHandlerStatus,
    };
    use std::ptr::NonNull;
    use std::sync::mpsc::Sender;

    pub struct Handle {
        /// The commands and their handler tokens, kept for the app's lifetime.
        _targets: Vec<(Retained<MPRemoteCommand>, Retained<AnyObject>)>,
    }

    pub fn start(tx: Sender<Command>, wake: rune::Waker) -> Handle {
        // SAFETY: plain Objective-C calls on the main thread (`Media::start` runs in setup); the
        // handler blocks capture only Send data and return the required status.
        unsafe {
            let center = MPRemoteCommandCenter::sharedCommandCenter();
            let commands = [
                (center.playCommand(), Command::Play),
                (center.pauseCommand(), Command::Pause),
                (center.togglePlayPauseCommand(), Command::PlayPause),
                (center.nextTrackCommand(), Command::Next),
                (center.previousTrackCommand(), Command::Previous),
                (center.stopCommand(), Command::Stop),
            ];
            let mut targets = Vec::new();
            for (command, cmd) in commands {
                let (tx, wake) = (tx.clone(), wake.clone());
                let handler = RcBlock::new(move |_e: NonNull<MPRemoteCommandEvent>| {
                    if tx.send(cmd).is_ok() {
                        wake();
                    }
                    MPRemoteCommandHandlerStatus::Success
                });
                command.setEnabled(true);
                let token = command.addTargetWithHandler(&handler);
                targets.push((command, token));
            }
            eprintln!("wholeseek: media keys: MPRemoteCommandCenter");
            Handle { _targets: targets }
        }
    }

    fn any<T: Message>(o: Retained<T>) -> Retained<AnyObject> {
        // SAFETY: every Objective-C object is an AnyObject.
        unsafe { Retained::cast_unchecked(o) }
    }

    impl Handle {
        pub fn send(&self, u: Update) {
            let (Update::State(s, _) | Update::Seeked(s, _)) = u;
            publish(&s);
        }
    }

    fn publish(s: &State) {
        // SAFETY: called on the main thread (from `App::media_tick`); the keys are Apple's
        // constant NSStrings and the values NSString/NSNumber, as documented.
        unsafe {
            let center = MPNowPlayingInfoCenter::defaultCenter();
            match &s.track {
                None => center.setNowPlayingInfo(None),
                Some(t) => {
                    let rate = if s.status == Status::Playing {
                        1.0
                    } else {
                        0.0
                    };
                    let mut keys: Vec<&NSString> = vec![
                        MPMediaItemPropertyTitle,
                        MPMediaItemPropertyArtist,
                        MPMediaItemPropertyAlbumTitle,
                        MPNowPlayingInfoPropertyElapsedPlaybackTime,
                        MPNowPlayingInfoPropertyPlaybackRate,
                    ];
                    let mut values = vec![
                        any(NSString::from_str(&t.title)),
                        any(NSString::from_str(&t.artist)),
                        any(NSString::from_str(&t.album)),
                        any(NSNumber::new_f64(s.position.as_secs_f64())),
                        any(NSNumber::new_f64(rate)),
                    ];
                    if let Some(len) = t.length {
                        keys.push(MPMediaItemPropertyPlaybackDuration);
                        values.push(any(NSNumber::new_f64(len.as_secs_f64())));
                    }
                    let info = NSDictionary::from_retained_objects(&keys, &values);
                    center.setNowPlayingInfo(Some(&info));
                }
            }
            // Without this macOS sends the media keys to Music.app.
            center.setPlaybackState(match s.status {
                Status::Playing => MPNowPlayingPlaybackState::Playing,
                Status::Paused => MPNowPlayingPlaybackState::Paused,
                Status::Stopped => MPNowPlayingPlaybackState::Stopped,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_map_to_the_app_actions() {
        use Action as A;
        use Command as C;
        use Status as S;
        let cases = [
            (C::PlayPause, S::Playing, A::TogglePause),
            (C::PlayPause, S::Paused, A::TogglePause),
            (C::PlayPause, S::Stopped, A::Nothing),
            (C::Play, S::Paused, A::TogglePause),
            (C::Play, S::Playing, A::Nothing),
            (C::Pause, S::Playing, A::TogglePause),
            (C::Pause, S::Paused, A::Nothing),
            (C::Stop, S::Playing, A::TogglePause),
            (C::Stop, S::Paused, A::Nothing),
            (C::Next, S::Stopped, A::Next),
            (C::Next, S::Playing, A::Next),
            (C::Previous, S::Paused, A::Previous),
        ];
        for (c, s, want) in cases {
            assert_eq!(action(c, s), want, "{c:?} while {s:?}");
        }
    }

    #[test]
    fn metadata_has_the_mpris_fields() {
        let t = Track {
            id: 0xab,
            title: "01 It Wasn't Me".into(),
            artist: "nadia".into(),
            album: "Hot Shot".into(),
            length: Some(Duration::from_millis(227_500)),
            art: Some(PathBuf::from("/tmp/covers/a b#.jpg")),
        };
        let m: std::collections::HashMap<_, _> = metadata(Some(&t)).into_iter().collect();
        assert_eq!(
            m["mpris:trackid"],
            Field::Path("/org/wholeseek/track/t00000000000000ab".into())
        );
        assert_eq!(m["xesam:title"], Field::Str("01 It Wasn't Me".into()));
        assert_eq!(m["xesam:artist"], Field::Strs(vec!["nadia".into()]));
        assert_eq!(m["xesam:album"], Field::Str("Hot Shot".into()));
        assert_eq!(m["mpris:length"], Field::Micros(227_500_000));
        assert_eq!(
            m["mpris:artUrl"],
            Field::Str("file:///tmp/covers/a%20b%23.jpg".into())
        );
        let none = metadata(None);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].0, "mpris:trackid");
        let bare = metadata(Some(&Track::default()));
        assert!(
            bare.iter()
                .all(|(k, _)| *k != "mpris:length" && *k != "mpris:artUrl")
        );
    }

    #[test]
    fn state_follows_the_player_strip() {
        let mut s = App::demo();
        assert_eq!(state_of(&s).status, Status::Stopped);
        assert!(state_of(&s).track.is_none());
        let f = crate::demo::catalog()[0].clone();
        s.play(&f);
        let st = state_of(&s);
        let t = st.track.expect("track");
        assert_eq!(t.title, crate::fmt::title(&f));
        assert_eq!(t.artist, f.username);
        assert_eq!(t.album, wholeseek_core::paths::remote_basename(f.folder()));
        assert_eq!(st.status, Status::Playing, "fetching counts as playing");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publish_sends_changes_and_seeks_but_not_the_clock() {
        let (tx, rx) = mpsc::channel();
        let mut m = Media {
            commands: mpsc::channel().1,
            platform: mpris::Handle(tx),
            last: None,
        };
        let t0 = Instant::now();
        let mut s = State {
            track: Some(Track::default()),
            status: Status::Playing,
            position: Duration::from_secs(10),
            volume_pct: 80,
        };
        m.publish(s.clone(), t0);
        assert!(matches!(rx.try_recv(), Ok(Update::State(..))));
        s.position = Duration::from_secs(12);
        m.publish(s.clone(), t0 + Duration::from_secs(2));
        assert!(rx.try_recv().is_err(), "the clock moving is not news");
        s.position = Duration::from_secs(60);
        m.publish(s.clone(), t0 + Duration::from_secs(3));
        assert!(matches!(rx.try_recv(), Ok(Update::Seeked(..))));
        s.status = Status::Paused;
        m.publish(s.clone(), t0 + Duration::from_secs(4));
        assert!(matches!(rx.try_recv(), Ok(Update::State(..))));
    }

    /// A private `dbus-daemon` (never the desktop's session bus), stopped on drop.
    #[cfg(target_os = "linux")]
    struct PrivateBus(std::process::Child, String);

    #[cfg(target_os = "linux")]
    impl PrivateBus {
        fn start() -> Option<Self> {
            use std::io::BufRead;
            let mut child = std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()?;
            let mut line = String::new();
            let out = child.stdout.take()?;
            std::io::BufReader::new(out).read_line(&mut line).ok()?;
            let addr = line.trim().to_string();
            (!addr.is_empty()).then_some(Self(child, addr))
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Drives our MPRIS object like `playerctl` does, on a private bus. Skips (passes) where
    /// `dbus-daemon` is not installed.
    #[cfg(target_os = "linux")]
    #[test]
    fn mpris_object_answers_on_a_private_bus() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use zbus::blocking::{Proxy, connection, proxy::Builder};
        use zbus::proxy::CacheProperties;
        use zbus::zvariant::OwnedValue;

        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let (tx, commands) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let wake: rune::Waker = Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        let handle = mpris::spawn(Some(bus.1.clone()), tx, wake);
        let client = connection::Builder::address(bus.1.as_str())
            .unwrap()
            .build()
            .unwrap();
        // Uncached, as playerctl reads them (Position is never signalled).
        let proxy = |iface: &'static str| -> Proxy<'static> {
            Builder::new(&client)
                .destination("org.mpris.MediaPlayer2.wholeseek")
                .unwrap()
                .path("/org/mpris/MediaPlayer2")
                .unwrap()
                .interface(iface)
                .unwrap()
                .cache_properties(CacheProperties::No)
                .build()
                .unwrap()
        };
        let player = proxy("org.mpris.MediaPlayer2.Player");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            match player.get_property::<String>("PlaybackStatus") {
                Ok(s) => break s,
                Err(e) if Instant::now() > deadline => panic!("no MPRIS object: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert_eq!(status, "Stopped");
        let root = proxy("org.mpris.MediaPlayer2");
        assert_eq!(
            root.get_property::<String>("Identity").unwrap(),
            "wholeseek"
        );

        // Methods become commands on the app's channel, each with a wake.
        for m in ["PlayPause", "Next", "Previous", "Play", "Pause", "Stop"] {
            player.call_method(m, &()).unwrap();
        }
        let got: Vec<Command> = (0..6)
            .map(|_| commands.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        use Command as C;
        assert_eq!(
            got,
            [
                C::PlayPause,
                C::Next,
                C::Previous,
                C::Play,
                C::Pause,
                C::Stop
            ]
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 6);

        // State in: status, metadata and an extrapolated position out.
        let track = Track {
            id: 7,
            title: "Windowlicker".into(),
            artist: "nadia".into(),
            album: "Windowlicker EP".into(),
            length: Some(Duration::from_secs(367)),
            art: None,
        };
        let state = State {
            track: Some(track),
            status: Status::Playing,
            position: Duration::from_secs(30),
            volume_pct: 80,
        };
        handle.send(Update::State(state, Instant::now()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.get_property::<String>("PlaybackStatus").unwrap() != "Playing" {
            assert!(Instant::now() < deadline, "status never changed");
            std::thread::sleep(Duration::from_millis(20));
        }
        let meta: std::collections::HashMap<String, OwnedValue> =
            player.get_property("Metadata").unwrap();
        let s = |k: &str| String::try_from(meta[k].try_clone().unwrap()).unwrap();
        assert_eq!(s("xesam:title"), "Windowlicker");
        assert_eq!(s("xesam:album"), "Windowlicker EP");
        assert_eq!(
            i64::try_from(meta["mpris:length"].try_clone().unwrap()).unwrap(),
            367_000_000
        );
        let pos: i64 = player.get_property("Position").unwrap();
        assert!((30_000_000..40_000_000).contains(&pos), "position {pos}");
        assert!((player.get_property::<f64>("Volume").unwrap() - 0.8).abs() < 1e-9);
        assert!(player.get_property::<bool>("CanGoNext").unwrap());
        drop(handle); // the service thread ends with the app
    }
}
