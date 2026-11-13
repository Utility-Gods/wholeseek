//! Update check: wholeseek isn't in an app store, so at start and then daily it asks GitHub
//! which release is latest and, if it's newer, the strip shows a small notice (`view::update`).
//!
//! The check reads the `Location` of `releases/latest` (a redirect to `…/releases/tag/vX.Y.Z`)
//! without following it: no API, no token, no rate limit. Only a `wholeseek/<version>`
//! User-Agent is sent. It runs on its own thread with a 5 s timeout; any failure logs one line
//! and shows nothing. Off with `Settings::check_updates`; a dismissed version
//! (`Settings::dismissed_update`) stays hidden until a newer one.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use crate::app::App;

/// The release redirect the check reads.
pub const LATEST_URL: &str = "https://github.com/Utility-Gods/wholeseek/releases/latest";
/// Where the notice leads.
pub const SITE: &str = "https://wholeseek.utilitygods.com";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Checks happen this often while the app runs (seconds).
pub const INTERVAL: f64 = 24.0 * 60.0 * 60.0;
/// The request's whole budget.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// While a check is out, frames come this often to pick up its answer.
const POLL: f64 = 0.25;

/// A release version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `x.y.z` and whether it carries a pre-release (`-rc.1`); a leading `v` and build metadata
/// (`+…`) are ignored. Anything else is `None`.
fn parse_parts(s: &str) -> Option<(Version, bool)> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, true),
        Some(_) => return None,
        None => (s, false),
    };
    let mut n = core.split('.').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let v = Version(n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some((v, pre))
}

/// A release tag's version (`v0.1.4`, `0.1.4`). Pre-releases and malformed tags are `None`:
/// they are never offered.
pub fn release_version(tag: &str) -> Option<Version> {
    parse_parts(tag).and_then(|(v, pre)| (!pre).then_some(v))
}

/// The release in `tag` if it is newer than `current` (a pre-release build is older than its
/// release).
pub fn newer(tag: &str, current: &str) -> Option<Version> {
    let latest = release_version(tag)?;
    let (cur, cur_pre) = parse_parts(current)?;
    (latest > cur || (latest == cur && cur_pre)).then_some(latest)
}

/// The tag in a `releases/latest` redirect target (`…/releases/tag/v0.1.4`, absolute or not).
pub fn tag_from_location(location: &str) -> Option<&str> {
    let (_, rest) = location.split_once("/releases/tag/")?;
    let tag = rest.split(['?', '#']).next()?.trim_end_matches('/');
    (!tag.is_empty() && !tag.contains('/')).then_some(tag)
}

/// Fetches the `Location` of [`LATEST_URL`]; injected so tests need no network.
pub type Fetch = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// The real request: one GET, redirects not followed, 5 s, `wholeseek/<version>`.
pub fn fetch_location() -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .user_agent(format!("wholeseek/{CURRENT}"))
        .build()
        .into();
    let resp = agent.get(LATEST_URL).call().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_redirection() {
        return Err(format!("HTTP {status}, not a redirect"));
    }
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| "redirect without a Location".to_string())
}

/// The check's state: when it next runs, what it found.
pub struct Updates {
    /// `None`: never checks (the demo).
    fetch: Option<Fetch>,
    pending: Option<Receiver<Result<String, String>>>,
    /// Clock time of the next check (0: as soon as the settings are loaded).
    next_at: f64,
    /// The newest release seen, when newer than this build.
    pub latest: Option<Version>,
}

impl Updates {
    /// Real mode checks the network; the demo never does.
    pub fn new(demo: bool) -> Self {
        Self::with_fetch((!demo).then(|| Arc::new(fetch_location) as Fetch))
    }

    pub fn with_fetch(fetch: Option<Fetch>) -> Self {
        Self {
            fetch,
            pending: None,
            next_at: 0.0,
            latest: None,
        }
    }

    /// Take a finished check's answer: true if what the notice shows changed.
    fn settle(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        let answer = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("check thread ended".into()),
        };
        self.pending = None;
        let found = match answer {
            Ok(loc) => match tag_from_location(&loc) {
                Some(tag) => newer(tag, CURRENT),
                None => {
                    eprintln!("wholeseek: update check: unexpected redirect {loc:?}");
                    None
                }
            },
            Err(e) => {
                eprintln!("wholeseek: update check failed ({e})");
                return false;
            }
        };
        if let Some(v) = found
            && self.latest != Some(v)
        {
            eprintln!("wholeseek: wholeseek {v} is available (this is {CURRENT})");
            self.latest = Some(v);
            return true;
        }
        false
    }

    /// Start a check now on its own thread.
    fn start(&mut self, now: f64) {
        let Some(fetch) = self.fetch.clone() else {
            return;
        };
        self.next_at = now + INTERVAL;
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("wholeseek-update".into())
            .spawn(move || {
                let _ = tx.send(fetch());
            });
        match spawned {
            Ok(_) => self.pending = Some(rx),
            Err(e) => eprintln!("wholeseek: update check not started ({e})"),
        }
    }
}

impl App {
    /// Each frame: start a due check (settings loaded and checks on), take its answer.
    /// Returns true if the notice changed; asks for frames while a check is out and for one
    /// when the next is due.
    pub fn tick_updates(&mut self, wake: &mut Option<f64>) -> bool {
        let changed = self.updates.settle();
        let on = self.boot.is_none() && self.backend.settings().check_updates;
        let u = &mut self.updates;
        if on && u.fetch.is_some() && u.pending.is_none() {
            if self.now >= u.next_at {
                u.start(self.now);
            } else {
                let d = u.next_at - self.now;
                *wake = Some(wake.map_or(d, |w| w.min(d)));
            }
        }
        if u.pending.is_some() {
            *wake = Some(wake.map_or(POLL, |w| w.min(POLL)));
        }
        changed
    }

    /// The version the strip offers, if any: newer, checks on, not dismissed.
    pub fn update_offer(&self) -> Option<Version> {
        let v = self.updates.latest?;
        let cfg = self.backend.settings();
        let dismissed = cfg.dismissed_update.as_deref().and_then(release_version);
        (cfg.check_updates && dismissed.is_none_or(|d| v > d)).then_some(v)
    }

    /// The notice clicked: the download page in the browser.
    pub fn open_update_site(&mut self) {
        self.desktop.open(Path::new(SITE));
    }

    /// × on the notice: hide this version (saved), a newer one shows again.
    pub fn dismiss_update(&mut self) {
        if let Some(v) = self.update_offer() {
            self.commit_settings(|c| c.dismissed_update = Some(v.to_string()));
        }
    }

    /// The Settings switch.
    pub fn toggle_update_checks(&mut self) {
        self.commit_settings(|c| c.check_updates = !c.check_updates);
    }

    /// Demo scenes: pretend the next patch release is out.
    pub fn demo_update_offer(&mut self) {
        let (Version(a, b, c), _) = parse_parts(CURRENT).unwrap_or((Version(0, 0, 0), false));
        self.updates.latest = Some(Version(a, b, c + 1));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rune::{Env, Runtime};

    use super::*;
    use crate::desktop::DesktopCall;
    use crate::{poll, view};

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(newer("v0.1.10", "0.1.9"), Some(Version(0, 1, 10)));
        assert_eq!(newer("0.2.0", "0.1.99"), Some(Version(0, 2, 0)));
        assert_eq!(newer("v1.0.0", "0.9.9"), Some(Version(1, 0, 0)));
        assert_eq!(newer("v0.1.3", "0.1.3"), None, "same version");
        assert_eq!(newer("v0.1.2", "0.1.3"), None, "older");
        assert_eq!(newer("v0.1.4+build.7", "0.1.3"), Some(Version(0, 1, 4)));
    }

    #[test]
    fn a_pre_release_is_never_offered() {
        assert_eq!(newer("v0.1.4-beta.1", "0.1.3"), None);
        assert_eq!(newer("v0.2.0-rc1", "0.1.3"), None);
        // A pre-release build is older than its release.
        assert_eq!(newer("v0.1.4", "0.1.4-rc.1"), Some(Version(0, 1, 4)));
    }

    #[test]
    fn malformed_tags_are_ignored() {
        for tag in [
            "",
            "v",
            "latest",
            "v1",
            "v1.2",
            "v1.2.3.4",
            "1..3",
            "v1.2.x",
            "v1.2.3-",
            " - ",
            "nightly-2026",
            "v01.2.3a",
        ] {
            assert_eq!(newer(tag, "0.1.3"), None, "{tag:?}");
        }
    }

    #[test]
    fn the_tag_comes_from_the_redirect_target() {
        let base = "https://github.com/Utility-Gods/wholeseek/releases/tag/";
        assert_eq!(tag_from_location(&format!("{base}v0.1.4")), Some("v0.1.4"));
        assert_eq!(tag_from_location(&format!("{base}v0.1.4/")), Some("v0.1.4"));
        assert_eq!(
            tag_from_location(&format!("{base}v0.1.4?x=1")),
            Some("v0.1.4")
        );
        assert_eq!(
            tag_from_location("/Utility-Gods/wholeseek/releases/tag/v0.2.0"),
            Some("v0.2.0")
        );
        // No releases yet: GitHub sends you to the list.
        assert_eq!(
            tag_from_location("https://github.com/Utility-Gods/wholeseek/releases"),
            None
        );
        assert_eq!(tag_from_location(base), None);
        assert_eq!(tag_from_location(&format!("{base}a/b")), None);
    }

    /// A demo app whose check answers `location` and counts requests.
    fn app_with(location: &'static str) -> (Runtime<App>, Arc<AtomicUsize>) {
        app_with_checks(location, true)
    }

    fn app_with_checks(location: &'static str, on: bool) -> (Runtime<App>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let mut app = App::demo();
        app.updates = Updates::with_fetch(Some(Arc::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(location.to_string())
        })));
        if !on {
            app.toggle_update_checks();
        }
        let mut rt = Runtime::new(
            app,
            view::view,
            (1600.0, 900.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.poll = Some(poll);
        rt.frame(0.0);
        (rt, calls)
    }

    fn run(rt: &mut Runtime<App>, secs: f64) {
        for _ in 0..(secs * 60.0).round() as usize {
            rt.frame(1.0 / 60.0);
        }
    }

    /// Wait (in real time; the answer comes from a thread) until `f` holds.
    fn until(rt: &mut Runtime<App>, what: &str, f: impl Fn(&App) -> bool) {
        let t0 = std::time::Instant::now();
        while !f(&rt.state) {
            assert!(t0.elapsed() < Duration::from_secs(2), "{what}");
            std::thread::sleep(Duration::from_millis(5));
            rt.frame(1.0 / 60.0);
        }
        rt.frame(1.0 / 60.0);
    }

    /// What the strip's pill (`view::update::pill`) is drawn from.
    fn shown(rt: &Runtime<App>) -> bool {
        rt.state.update_offer().is_some()
    }

    const NEXT: &str = "https://github.com/Utility-Gods/wholeseek/releases/tag/v99.0.0";

    #[test]
    fn a_newer_release_shows_the_notice_and_it_opens_the_site() {
        let (mut rt, calls) = app_with(NEXT);
        rt.state.demo_login();
        until(&mut rt, "check answered", |s| s.updates.latest.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one request at start");
        assert_eq!(rt.state.update_offer(), Some(Version(99, 0, 0)));
        assert!(shown(&rt), "the strip shows it");

        rt.update(|s, _| s.open_update_site());
        assert!(
            rt.state
                .desktop_log
                .borrow()
                .contains(&DesktopCall::Open(SITE.into())),
            "opens the site"
        );
        run(&mut rt, 1.0);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no second request soon");
    }

    #[test]
    fn nothing_newer_shows_nothing() {
        let current = format!("https://github.com/x/y/releases/tag/v{CURRENT}");
        let current: &'static str = Box::leak(current.into_boxed_str());
        let (mut rt, calls) = app_with(current);
        rt.state.demo_login();
        until(&mut rt, "check ran", |_| calls.load(Ordering::SeqCst) == 1);
        run(&mut rt, 0.2);
        assert!(rt.state.updates.latest.is_none());
        assert!(!shown(&rt));
    }

    #[test]
    fn a_failed_check_is_silent() {
        let mut app = App::demo();
        app.updates = Updates::with_fetch(Some(Arc::new(|| Err("offline".into()))));
        app.updates.start(0.0);
        let rx_done = |u: &mut Updates| {
            for _ in 0..400 {
                if u.pending.is_some() {
                    u.settle();
                }
                if u.pending.is_none() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("no answer");
        };
        rx_done(&mut app.updates);
        assert!(app.updates.latest.is_none());
        assert!(app.update_offer().is_none());
        assert!(app.error.is_none(), "no error line either");
    }

    #[test]
    fn dismissing_hides_that_version_until_a_newer_one() {
        let (mut rt, _) = app_with(NEXT);
        rt.state.demo_login();
        until(&mut rt, "check answered", |s| s.updates.latest.is_some());
        rt.update(|s, _| s.dismiss_update());
        rt.frame(1.0 / 60.0);
        assert!(!shown(&rt), "dismissed");
        assert_eq!(
            rt.state.backend.settings().dismissed_update.as_deref(),
            Some("99.0.0"),
            "remembered in the settings"
        );
        assert!(
            rt.state.store.calls().iter().any(|c| matches!(
                c,
                crate::store::StoreCall::SaveSettings(s)
                    if s.dismissed_update.as_deref() == Some("99.0.0")
            )),
            "and saved"
        );
        // A later check finds a newer release: it shows again.
        rt.state.updates.latest = Some(Version(99, 0, 1));
        rt.frame(1.0 / 60.0);
        assert!(shown(&rt));
    }

    #[test]
    fn checks_off_means_no_request_and_no_notice() {
        let (mut rt, calls) = app_with_checks(NEXT, false);
        assert!(!rt.state.backend.settings().check_updates);
        rt.state.demo_login();
        run(&mut rt, 1.0);
        std::thread::sleep(Duration::from_millis(50));
        run(&mut rt, 0.2);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no request");
        // Even a version found earlier stays hidden while checks are off.
        rt.state.updates.latest = Some(Version(99, 0, 0));
        rt.frame(1.0 / 60.0);
        assert!(!shown(&rt));
        // On again: the check runs.
        rt.update(|s, _| s.toggle_update_checks());
        until(&mut rt, "check ran", |_| calls.load(Ordering::SeqCst) == 1);
    }

    #[test]
    fn checks_again_after_a_day() {
        let (mut rt, calls) = app_with(NEXT);
        until(&mut rt, "first check", |s| s.updates.latest.is_some());
        rt.frame(INTERVAL - 1.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        rt.frame(2.0);
        until(&mut rt, "daily check", |_| {
            calls.load(Ordering::SeqCst) == 2
        });
    }

    #[test]
    fn settings_show_the_version_and_the_switch() {
        let (mut rt, _) = app_with(NEXT);
        rt.state.demo_login();
        run(&mut rt, 1.0);
        rt.update(|s, _| s.toggle_panel(crate::app::Panel::Settings));
        run(&mut rt, 0.5);
        assert!(rt.find("app-version").is_some());
        assert!(rt.find("set-check-updates").is_some());
    }

    /// Live: the real redirect names a release tag. `cargo test -p wholeseek -- --ignored
    /// live_latest_release`.
    #[test]
    #[ignore = "live network"]
    fn live_latest_release_redirects_to_a_tag() {
        let loc = fetch_location().expect("redirect");
        println!("[update] Location: {loc}");
        let tag = tag_from_location(&loc).expect("a release tag");
        let v = release_version(tag).expect("x.y.z");
        println!(
            "[update] latest release {v}; this build {CURRENT}; newer: {:?}",
            newer(tag, CURRENT)
        );
    }
}
