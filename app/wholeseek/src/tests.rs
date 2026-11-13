//! Headless UI flows in demo mode: the real view and handlers on Rune's runtime, the fake core
//! on the virtual clock. Clicks go through hit testing, like a pointer would.

use rune::{Env, Runtime};
use wholeseek_audio::PlayState;
use wholeseek_core::{RemoteFile, TransferKind};

use crate::app::{App, Conn, Panel, Screen, TState};
use crate::demo::Call;
use crate::{H, W, poll, view};

fn rt() -> Runtime<App> {
    let mut rt = Runtime::new(
        folders_view(App::demo()),
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt
}

/// The layout most tests were written against: folders by user, in availability order.
/// (The album view and relevance order have tests of their own.)
fn folders_view(mut s: App) -> App {
    use crate::tabs::{ResultsLayout, SortBy};
    s.last_filters.layout = ResultsLayout::Folders;
    s.last_filters.sort = SortBy::Availability;
    for t in &mut s.tabs {
        if let Some(st) = t.search_mut() {
            st.filters.layout = ResultsLayout::Folders;
            st.filters.sort = SortBy::Availability;
        }
    }
    s
}

fn run(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs * 60.0).round() as usize {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    run(rt, 0.05);
}

/// Click at a fraction of an element's width.
fn click_at(rt: &mut Runtime<App>, key: &str, fx: f32) {
    let r = rt
        .presented(key)
        .unwrap_or_else(|| panic!("no element {key}"))
        .0;
    let (x, y) = (r[0] + r[2] * fx, r[1] + r[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    run(rt, 0.05);
}

fn calls(rt: &Runtime<App>) -> Vec<Call> {
    rt.state.backend.calls().to_vec()
}

fn signed_in() -> Runtime<App> {
    let mut rt = rt();
    rt.state.demo_login();
    run(&mut rt, 1.0);
    assert_eq!(rt.state.screen, Screen::Main);
    rt
}

fn searched() -> Runtime<App> {
    let mut rt = signed_in();
    rt.state.search_for("hollow lights");
    run(&mut rt, 1.6);
    rt
}

/// The shown tab's search.
fn sv(rt: &Runtime<App>) -> &crate::tabs::SearchView {
    rt.state.active_search().expect("a search is shown")
}

fn row_of(rt: &Runtime<App>, user: &str, name: &str) -> (usize, RemoteFile) {
    let hits = &sv(rt).hits;
    let i = hits
        .iter()
        .position(|h| h.file.username == user && h.file.basename() == name)
        .expect("hit");
    (i, hits[i].file.clone())
}

#[test]
fn onboarding_validates_before_connecting() {
    let mut rt = rt();
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert!(rt.find("connect").is_some());

    click(&mut rt, "connect");
    assert_eq!(
        rt.state.onboarding.error.as_deref(),
        Some("Enter your Soulseek username.")
    );
    assert!(rt.find("ob-error").is_some(), "the error is drawn");

    rt.state.onboarding.username.replace_text("nadia");
    click(&mut rt, "connect");
    assert_eq!(
        rt.state.onboarding.error.as_deref(),
        Some("Enter your password.")
    );

    rt.state.onboarding.password.replace_text("pw");
    rt.state
        .onboarding
        .share_input
        .replace_text("/definitely/not/a/folder");
    click(&mut rt, "connect");
    assert!(
        rt.state
            .onboarding
            .error
            .as_deref()
            .unwrap()
            .starts_with("Folder not found")
    );
    assert!(
        calls(&rt).is_empty(),
        "nothing reaches the core while the form is invalid"
    );

    let dir = std::env::temp_dir();
    rt.state
        .onboarding
        .share_input
        .replace_text(dir.display().to_string());
    click(&mut rt, "ob-add");
    assert_eq!(rt.state.onboarding.shares, vec![dir.clone()]);
    click(&mut rt, "connect");
    assert_eq!(rt.state.conn, Conn::Connecting);
    assert_eq!(
        calls(&rt),
        vec![Call::SetShares(vec![dir]), Call::Login("nadia".into())]
    );
    run(&mut rt, 1.0);
    assert_eq!(rt.state.screen, Screen::Main);
    assert!(matches!(&rt.state.conn, Conn::Connected { username, .. } if username == "nadia"));
    assert!(rt.find("search").is_some() && rt.find("status").is_some());
    assert_eq!(rt.state.share_counts, Some((42, 518)));
}

#[test]
fn rejected_login_stays_on_onboarding_with_a_reason() {
    let mut rt = rt();
    rt.state.onboarding.username.replace_text("nadia");
    rt.state.onboarding.password.replace_text("wrong");
    click(&mut rt, "connect");
    run(&mut rt, 1.0);
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert_eq!(rt.state.conn, Conn::LoginRejected);
    assert!(
        rt.state
            .onboarding
            .error
            .as_deref()
            .unwrap()
            .contains("Login rejected")
    );
    assert!(!rt.state.onboarding.connecting);
}

#[test]
fn search_results_stream_in_ranked_by_availability() {
    let mut rt = signed_in();
    rt.state.search_for("hollow lights");
    run(&mut rt, 0.35);
    let first = sv(&rt).hits.len();
    assert!(
        first > 0 && !sv(&rt).finished,
        "a first batch arrives before the search ends"
    );
    assert!(rt.find("searching").is_some());
    run(&mut rt, 1.3);
    let s = sv(&rt);
    assert!(
        s.finished && s.hits.len() > first,
        "later batches append: {first} → {}",
        s.hits.len()
    );
    // Free slot first, then speed; the format plays no part.
    let keys: Vec<(bool, u32)> = s.hits.iter().map(|h| (h.free_slot, h.speed)).collect();
    assert!(
        keys.windows(2).all(|w| w[0] >= w[1]),
        "availability order: {keys:?}"
    );
    assert!(rt.find("row-0/play").is_some() && rt.find("searching").is_none());
    assert_eq!(
        calls(&rt).last(),
        Some(&Call::Search("hollow lights".into()))
    );
}

#[test]
fn play_calls_play_fetch_and_the_strip_follows_the_player() {
    let mut rt = searched();
    assert!(rt.find("idle").is_some(), "nothing playing yet");
    let (i, file) = row_of(&rt, "alice", "02 Salt Roads.flac");
    click(&mut rt, &format!("row-{i}/play"));
    assert_eq!(calls(&rt).last(), Some(&Call::PlayFetch(file.clone())));
    let p = rt.state.playing.as_ref().unwrap();
    assert!(!p.opened, "the player waits for the first TransferProgress");
    assert!(
        rt.find("now-title").is_some() && rt.find("notice").is_some(),
        "strip shows the track and 'Connecting…'"
    );

    run(&mut rt, 2.0);
    let p = rt.state.playing.as_ref().unwrap();
    assert!(p.opened);
    let snap = p.snapshot.as_ref().unwrap();
    assert_eq!(snap.state, PlayState::Playing);
    assert!(
        snap.position_secs > 0.5 && snap.buffered_fraction > 0.0,
        "{snap:?}"
    );
    assert_eq!(snap.duration_secs, Some(224.0));
    let played = rt.presented("played").unwrap().0[2];
    assert!(played > 0.0, "the seek bar's played part grows");

    // ▶ on the playing row pauses; the strip's button resumes.
    click(&mut rt, &format!("row-{i}/play"));
    assert_eq!(
        rt.state
            .playing
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .state,
        PlayState::Paused
    );
    click(&mut rt, "toggle");
    assert_ne!(
        rt.state
            .playing
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .state,
        PlayState::Paused
    );
}

#[test]
fn playing_never_downloads() {
    let mut rt = searched();
    let (i, _) = row_of(&rt, "alice", "01 Northern Glass.flac");
    click(&mut rt, &format!("row-{i}/play"));
    run(&mut rt, 1.0);
    let cache = rt.state.backend.settings().cache_dir.clone();
    let downloads = rt.state.backend.settings().download_dir.clone();
    let p = rt.state.playing.as_ref().unwrap();
    assert!(p.ticket.part_path.starts_with(&cache) && !p.ticket.part_path.starts_with(&downloads));
    assert!(
        rt.state
            .transfers
            .iter()
            .all(|t| t.kind == TransferKind::Play)
    );
    assert!(
        !calls(&rt)
            .iter()
            .any(|c| matches!(c, Call::Download(_) | Call::DownloadFolder(..)))
    );
}

#[test]
fn seek_past_the_buffer_shows_not_downloaded_yet() {
    let mut rt = searched();
    let (i, _) = row_of(&rt, "alice", "05 A Lamp in the Dunes.flac");
    click(&mut rt, &format!("row-{i}/play"));
    run(&mut rt, 1.0);
    click_at(&mut rt, "seek", 0.95);
    let notice = rt
        .state
        .notice
        .as_ref()
        .map(|n| n.0.clone())
        .unwrap_or_default();
    assert!(notice.starts_with("Not downloaded yet"), "{notice:?}");
    assert!(rt.find("notice").is_some());
    let before = rt
        .state
        .playing
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .position_secs;
    assert!(before < 10.0, "position did not jump");
    run(&mut rt, 3.2);
    assert!(rt.state.notice.is_none(), "the notice expires");
    // Inside the buffered range a seek lands.
    click_at(&mut rt, "seek", 0.02);
    assert!(rt.state.notice.is_none());
}

#[test]
fn volume_drag_sets_the_player_volume() {
    let mut rt = searched();
    click(&mut rt, "row-0/play");
    run(&mut rt, 1.0);
    click_at(&mut rt, "volume-track", 0.25);
    let v = rt
        .state
        .playing
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .volume;
    assert!((v - 0.25).abs() < 0.05, "{v}");
}

#[test]
fn download_shows_progress_then_done() {
    let mut rt = searched();
    let (i, file) = row_of(&rt, "alice", "03 Low Tide Radio.flac");
    click(&mut rt, &format!("row-{i}/download"));
    assert_eq!(calls(&rt).last(), Some(&Call::Download(file.clone())));
    run(&mut rt, 1.5);
    let d = rt.state.download_of(&file).unwrap();
    assert_eq!(d.state, TState::Active);
    assert!(d.fraction() > 0.0 && d.fraction() < 1.0);
    assert!(
        rt.find(&format!("row-{i}/download/bar")).is_some(),
        "the row draws a progress bar"
    );
    assert!(
        d.part_path
            .starts_with(&rt.state.backend.settings().download_dir)
    );
    run(&mut rt, 12.0);
    assert!(matches!(
        rt.state.download_of(&file).unwrap().state,
        TState::Done(_)
    ));
}

#[test]
fn failed_download_can_be_retried_from_the_transfers_panel() {
    let mut rt = searched();
    let (_, file) = row_of(&rt, "marten_k", "Hollow Lights - Salt Roads.flac");
    rt.update(|s, _| s.download(&file));
    run(&mut rt, 20.0);
    let d = rt.state.download_of(&file).unwrap();
    assert!(matches!(d.state, TState::Failed(_)), "{:?}", d.state);
    let id = d.id;
    click(&mut rt, "transfers");
    assert_eq!(rt.state.panel, Panel::Transfers);
    click(&mut rt, "dl-0/retry");
    assert_eq!(calls(&rt).last(), Some(&Call::Retry(id)));
    run(&mut rt, 1.0);
    assert_eq!(rt.state.download_of(&file).unwrap().state, TState::Active);
    click(&mut rt, "dl-0/cancel");
    assert_eq!(calls(&rt).last(), Some(&Call::Cancel(id)));
    run(&mut rt, 0.1);
    assert_eq!(
        rt.state.download_of(&file).unwrap().state,
        TState::Cancelled
    );
}

#[test]
fn browse_failure_is_shown() {
    let mut rt = signed_in();
    rt.state.open_user("ghost");
    run(&mut rt, 0.6);
    assert!(rt.find("browse-error").is_some());
}

#[test]
fn switching_tracks_cancels_the_previous_fetch_unless_it_is_being_downloaded() {
    let mut rt = searched();
    let (a, fa) = row_of(&rt, "alice", "01 Northern Glass.flac");
    let (b, _) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let (c, _) = row_of(&rt, "alice", "03 Low Tide Radio.flac");
    click(&mut rt, &format!("row-{a}/play"));
    let first = rt.state.playing.as_ref().unwrap().ticket.id;
    run(&mut rt, 0.5);
    click(&mut rt, &format!("row-{b}/play"));
    assert!(calls(&rt).contains(&Call::Cancel(first)));
    // ↓ on the playing track keeps its fetch alive when switching away.
    let second = rt.state.playing.as_ref().unwrap().ticket.id;
    click(&mut rt, &format!("row-{b}/download"));
    click(&mut rt, &format!("row-{c}/play"));
    assert!(!calls(&rt).contains(&Call::Cancel(second)));
    assert!(rt.state.download_of(&fa).is_none());
}

// ---- persistence, uploads, audio-only, port mapping --------------------------------------

use crate::app::Startup;
use crate::store::StoreCall;
use wholeseek_core::Credentials;

fn rt_with(startup: Startup) -> Runtime<App> {
    let mut rt = Runtime::new(
        App::demo_with(startup),
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt
}

fn store_calls(rt: &Runtime<App>) -> Vec<StoreCall> {
    rt.state.store.calls().to_vec()
}

#[test]
fn credentials_are_saved_only_after_the_server_accepts_them() {
    let mut rt = rt();
    rt.state.onboarding.username.replace_text("nadia");
    rt.state.onboarding.password.replace_text("pw");
    click(&mut rt, "connect");
    assert!(
        store_calls(&rt).is_empty(),
        "nothing saved before Connected"
    );
    run(&mut rt, 1.0);
    let saved = store_calls(&rt);
    assert_eq!(saved[0], StoreCall::SaveCredentials("nadia".into()));
    assert!(matches!(saved[1], StoreCall::SaveSettings(_)));

    let mut rt = self::rt();
    rt.state.onboarding.username.replace_text("nadia");
    rt.state.onboarding.password.replace_text("wrong");
    click(&mut rt, "connect");
    run(&mut rt, 1.0);
    assert!(
        store_calls(&rt).is_empty(),
        "a rejected login saves nothing"
    );
}

#[test]
fn remembered_account_skips_onboarding() {
    let creds = Credentials {
        username: "nadia".into(),
        password: "pw".into(),
    };
    let mut rt = rt_with(Startup {
        credentials: Some(creds),
        keyring_available: true,
    });
    assert_eq!(rt.state.screen, Screen::Main);
    assert_eq!(rt.state.conn, Conn::Connecting);
    assert_eq!(calls(&rt), vec![Call::Login("nadia".into())]);
    assert!(rt.find("connect").is_none() && rt.find("search").is_some());
    run(&mut rt, 1.0);
    assert!(matches!(rt.state.conn, Conn::Connected { .. }));
    assert!(
        store_calls(&rt).is_empty(),
        "already remembered: no re-save"
    );
}

#[test]
fn without_a_keyring_a_quiet_note_shows_and_login_still_works() {
    let mut rt = rt_with(Startup {
        credentials: None,
        keyring_available: false,
    });
    assert!(rt.find("ob-keyring").is_some());
    rt.state.demo_login();
    run(&mut rt, 1.0);
    assert_eq!(rt.state.screen, Screen::Main);
    click(&mut rt, "settings");
    assert!(
        rt.find("set-account").is_some(),
        "no paragraph, the account line only"
    );
}

#[test]
fn keyring_failure_on_save_is_reported_quietly() {
    let mut rt = rt();
    // A store whose keyring refuses the password.
    rt.state.store = Box::new(crate::store::MemoryStore::new(false));
    rt.state.demo_login();
    run(&mut rt, 1.0);
    assert!(!rt.state.keyring_available);
    assert!(
        rt.state.error.is_none(),
        "not an error banner, just the settings note"
    );
}

#[test]
fn plain_log_out_keeps_the_saved_account() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    click(&mut rt, "logout");
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert!(!store_calls(&rt).contains(&StoreCall::ForgetCredentials));
    assert_eq!(rt.state.onboarding.username.text, "nadia");
}

#[test]
fn uploads_can_be_cancelled() {
    let mut rt = signed_in();
    click(&mut rt, "transfers");
    let u = rt.state.uploads[0].clone();
    click(&mut rt, "up-0/cancel");
    assert_eq!(
        calls(&rt).last(),
        Some(&Call::CancelUpload(u.username, u.path))
    );
    run(&mut rt, 0.2);
    assert_eq!(
        rt.state.uploads[0].state,
        wholeseek_core::UploadState::Cancelled
    );
    assert!(rt.find("up-0/cancel").is_none());
}

// ---- profile header ------------------------------------------------------------------------

fn browsing(user: &str) -> Runtime<App> {
    let mut rt = signed_in();
    rt.update(|s, _| s.open_user(user));
    run(&mut rt, 0.05);
    rt
}

fn shown(rt: &Runtime<App>, key: &str) -> bool {
    rt.find(key).is_some()
}

#[test]
fn profile_is_requested_with_browse_and_shows_loading_until_the_first_answer() {
    let mut rt = browsing("alice");
    let c = calls(&rt);
    assert!(
        c.contains(&Call::Browse("alice".into())) && c.contains(&Call::UserProfile("alice".into()))
    );
    assert!(shown(&rt, "profile-loading") && !shown(&rt, "profile"));
    run(&mut rt, 0.3);
    assert!(!shown(&rt, "profile-loading") && shown(&rt, "profile"));
}

#[test]
fn profile_header_renders_known_fields_and_grows_as_answers_arrive() {
    let mut rt = browsing("alice");
    run(&mut rt, 0.3);
    // First answer: server stats only.
    assert!(
        shown(&rt, "profile-name") && shown(&rt, "profile-online") && shown(&rt, "profile-stats")
    );
    assert!(!shown(&rt, "profile-desc") && !shown(&rt, "profile-picture"));
    let stats = crate::profile::stats_line(&rt.state.profiles.get("alice").unwrap().profile);
    assert_eq!(stats, "3.1 MB/s · 29,469 files · 3,028 folders");
    run(&mut rt, 0.5);
    // Second: the peer's part. The picture decodes off-thread.
    let e = rt.state.profiles.get("alice").unwrap();
    e.picture
        .as_ref()
        .unwrap()
        .wait()
        .expect("demo PNG decodes");
    run(&mut rt, 0.05);
    assert!(
        shown(&rt, "profile-desc") && shown(&rt, "profile-picture") && shown(&rt, "profile-more")
    );
    assert!(
        crate::profile::stats_line(&rt.state.profiles.get("alice").unwrap().profile)
            .ends_with(" · free slot")
    );
    // Clamped to 3 lines; "more" expands it.
    let clamped = rt.presented("profile-desc").unwrap().0[3];
    click(&mut rt, "profile-more");
    let open = rt.presented("profile-desc").unwrap().0[3];
    assert!(open > clamped + 1.0, "expanded {open} > clamped {clamped}");
}

#[test]
fn profile_header_hides_unknown_fields() {
    let mut rt = browsing("vinyl_rips");
    run(&mut rt, 1.0);
    assert!(shown(&rt, "profile-name") && shown(&rt, "profile-stats"));
    assert!(
        !shown(&rt, "profile-desc")
            && !shown(&rt, "profile-picture")
            && !shown(&rt, "profile-more")
    );
    // Queue without a free slot; an undecodable picture is skipped, the rest shows.
    let mut rt = browsing("marten_k");
    run(&mut rt, 1.0);
    assert!(
        crate::profile::stats_line(&rt.state.profiles.get("marten_k").unwrap().profile)
            .ends_with(" · queue 12")
    );
    assert!(shown(&rt, "profile-desc") && !shown(&rt, "profile-more"));
    let mut rt = browsing("dj_ivo");
    run(&mut rt, 0.8);
    let _ = rt
        .state
        .profiles
        .get("dj_ivo")
        .unwrap()
        .picture
        .as_ref()
        .unwrap()
        .wait();
    run(&mut rt, 0.05);
    assert!(shown(&rt, "profile-desc") && !shown(&rt, "profile-picture"));
}

#[test]
fn profile_events_for_another_user_are_ignored() {
    let mut rt = browsing("alice");
    let other = wholeseek_core::UserProfile {
        description: Some("not alice".into()),
        ..Default::default()
    };
    rt.update(|s, _| {
        s.apply(wholeseek_core::CoreEvent::UserProfile {
            username: "mallory".into(),
            profile: other.clone(),
        })
    });
    rt.update(|s, _| {
        s.apply(wholeseek_core::CoreEvent::UserProfileFailed {
            username: "mallory".into(),
            reason: "x".into(),
        })
    });
    run(&mut rt, 0.02);
    assert!(shown(&rt, "profile-loading"), "still waiting for alice");
    assert!(rt.state.profiles.get("mallory").is_none());
    // A late answer for a user we already left doesn't show up on the next one.
    click(&mut rt, "tab-1/close");
    rt.update(|s, _| s.open_user("vinyl_rips"));
    rt.update(|s, _| {
        s.apply(wholeseek_core::CoreEvent::UserProfile {
            username: "alice".into(),
            profile: other,
        })
    });
    run(&mut rt, 0.02);
    assert!(shown(&rt, "profile-loading") && rt.state.profiles.get("alice").is_none());
}

#[test]
fn profile_failure_hides_the_header_but_folders_still_work() {
    let mut rt = browsing("nightbus");
    assert!(shown(&rt, "profile-loading"));
    run(&mut rt, 1.0);
    assert!(matches!(
        rt.state.profiles.get("nightbus").unwrap().status,
        crate::profile::ProfileStatus::Failed(_)
    ));
    assert!(!shown(&rt, "profile") && !shown(&rt, "profile-loading"));
    assert!(
        shown(&rt, "dir-0") && shown(&rt, "file-0/play"),
        "folders still work"
    );
    // A failure after fields arrived keeps them.
    let mut rt = browsing("alice");
    run(&mut rt, 1.0);
    rt.update(|s, _| {
        s.apply(wholeseek_core::CoreEvent::UserProfileFailed {
            username: "alice".into(),
            reason: "timeout".into(),
        })
    });
    run(&mut rt, 0.02);
    assert!(shown(&rt, "profile-stats"));
}

// ---- CP2: tabs ------------------------------------------------------------------------------

use rune::{Key, KeyEvent, Mods, PointerButton};

use crate::desktop::DesktopCall;
use crate::menu::{MenuAction, MenuAnchor};

fn ctrl(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    }
}

fn press(rt: &mut Runtime<App>, ev: KeyEvent) {
    rt.key(ev);
    run(rt, 0.05);
}

fn count_calls(rt: &Runtime<App>, f: impl Fn(&Call) -> bool) -> usize {
    calls(rt).iter().filter(|c| f(c)).count()
}

fn text_of(rt: &Runtime<App>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
}

fn focused(rt: &Runtime<App>) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| e.focused)
        .and_then(|e| e.key)
}

#[test]
fn clicking_a_username_opens_its_tab_and_again_focuses_it() {
    let mut rt = searched();
    click(&mut rt, "group-0/user");
    assert_eq!(rt.state.tabs.len(), 2);
    assert_eq!(rt.state.active, 1);
    assert_eq!(rt.state.tab().username(), Some("alice"));
    assert!(calls(&rt).contains(&Call::Browse("alice".into())));
    assert!(calls(&rt).contains(&Call::UserProfile("alice".into())));
    run(&mut rt, 0.6);
    assert!(rt.find("dir-0").is_some() && rt.find("file-0/play").is_some());

    click(&mut rt, "tab-0");
    assert_eq!(rt.state.active, 0);
    click(&mut rt, "group-0/user");
    assert_eq!(rt.state.tabs.len(), 2, "no second alice tab");
    assert_eq!(rt.state.active, 1);
    assert_eq!(
        count_calls(&rt, |c| *c == Call::Browse("alice".into())),
        1,
        "focusing doesn't browse again"
    );
}

#[test]
fn closing_tabs_and_the_last_tab_leaves_an_empty_search() {
    let mut rt = searched();
    rt.update(|s, _| {
        s.open_user("alice");
    });
    run(&mut rt, 0.1);
    click(&mut rt, "tab-1/close");
    assert_eq!(rt.state.tabs.len(), 1);
    assert_eq!(rt.state.active, 0);
    // Middle-click closes too.
    rt.update(|s, _| {
        s.open_user("vinyl_rips");
    });
    run(&mut rt, 0.1);
    let (x, y) = rt.find("tab-1").unwrap();
    rt.button_down(x, y, PointerButton::Middle);
    rt.button_up(x, y, PointerButton::Middle);
    run(&mut rt, 0.05);
    assert_eq!(rt.state.tabs.len(), 1);
    // The last tab: an empty search tab takes its place (the running search is stopped).
    click(&mut rt, "tab-0/close");
    assert_eq!(rt.state.tabs.len(), 1);
    let st = rt.state.tab().search().expect("a search tab");
    assert!(st.search.is_none() && st.edit.text.is_empty());
    assert!(rt.find("empty").is_some());
}

#[test]
fn each_tab_keeps_its_query_filters_results_and_scroll() {
    let mut rt = searched();
    click(&mut rt, "free-slot"); // free slots only in tab 0
    let (x, y) = rt.find("row-0").unwrap();
    rt.wheel(x, y, -120.0);
    run(&mut rt, 0.3);
    let scrolled = rt.state.tab().scroll.get();
    assert!(scrolled > 0.0, "the list scrolled");

    press(&mut rt, ctrl(Key::Command('t')));
    assert_eq!(rt.state.active, 1);
    rt.update(|s, _| {
        s.search_for("salt roads");
    });
    run(&mut rt, 1.6);
    assert!(
        rt.state.tab().search().unwrap().filters.free_slot,
        "a new tab starts with the last-used selection"
    );
    click(&mut rt, "free-slot"); // everyone in tab 1

    press(&mut rt, ctrl(Key::Tab)); // wraps to tab 0
    assert_eq!(rt.state.active, 0);
    let st = rt.state.tab().search().unwrap();
    assert_eq!(st.edit.text, "hollow lights");
    assert!(st.filters.free_slot, "tab 0 kept its own filters");
    assert!(!rt.state.tabs[1].search().unwrap().filters.free_slot);
    assert_eq!(st.search.as_ref().unwrap().query, "hollow lights");
    assert_eq!(rt.state.tab().scroll.get(), scrolled, "scroll offset kept");
    assert_eq!(rt.state.tabs[1].search().unwrap().edit.text, "salt roads");
}

#[test]
fn shortcuts_open_close_cycle_and_focus_search() {
    let mut rt = searched();
    rt.update(|s, _| {
        s.open_user("alice");
    });
    run(&mut rt, 0.1);
    assert_eq!(rt.state.active, 1);
    press(&mut rt, ctrl(Key::Command('l')));
    assert_eq!(rt.state.active, 0, "Ctrl+L goes back to a search tab");
    assert_eq!(focused(&rt).as_deref(), Some("query"));

    press(&mut rt, ctrl(Key::Command('t')));
    assert_eq!(
        (rt.state.tabs.len(), rt.state.active),
        (3, 1),
        "opens next to the current tab"
    );
    assert_eq!(focused(&rt).as_deref(), Some("query"));
    let mut back = ctrl(Key::Tab);
    back.mods.shift = true;
    rt.update(|s, _| s.activate(0));
    press(&mut rt, back);
    assert_eq!(rt.state.active, 2, "Ctrl+Shift+Tab wraps backwards");
    press(&mut rt, ctrl(Key::Command('w')));
    assert_eq!(rt.state.tabs.len(), 2);
    press(&mut rt, ctrl(Key::Command('f')));
    assert!(rt.state.tab().search().is_some());
}

// ---- CP2: search tab ------------------------------------------------------------------------

fn visible(rt: &Runtime<App>) -> usize {
    let st = rt.state.tab().search().unwrap();
    st.search.as_ref().unwrap().visible(&st.filters)
}

fn shown_formats(rt: &Runtime<App>) -> std::collections::BTreeSet<&'static str> {
    let st = rt.state.tab().search().unwrap();
    let v = st.search.as_ref().unwrap();
    v.hits
        .iter()
        .filter(|h| st.filters.accepts(h))
        .filter_map(|h| crate::tabs::format_of(&h.file.path))
        .collect()
}

#[test]
fn format_chips_come_from_the_results_with_counts_most_common_first() {
    let rt = searched();
    let st = rt.state.tab().search().unwrap();
    let counts = st.search.as_ref().unwrap().format_counts(&st.filters);
    // Demo results: 13 FLAC, 3 MP3, 2 WAV, 1 OGG, 1 Opus — no hardcoded FLAC default.
    assert_eq!(
        counts,
        [
            ("FLAC", 13),
            ("MP3", 3),
            ("WAV", 2),
            ("OGG", 1),
            ("Opus", 1)
        ]
    );
    assert!(
        st.filters.formats.is_empty(),
        "none selected = every format"
    );
    assert_eq!(visible(&rt), sv(&rt).hits.len());
    let n = rt
        .elements()
        .into_iter()
        .find(|e| e.path.ends_with("fmt-flac/n"))
        .and_then(|e| e.text);
    assert_eq!(n.as_deref(), Some("13"), "the chip reads \"FLAC 13\"");
    assert!(
        rt.find("fmt-wav").is_some() && rt.find("fmt-aac").is_none(),
        "only formats present"
    );
    assert!(rt.find("fmt-more").is_none());
    assert!(rt.find("flac-only").is_none() && rt.find("bit-16").is_none());
}

#[test]
fn format_chips_multi_select_as_a_union_and_none_means_all() {
    let mut rt = searched();
    let searches = count_calls(&rt, |c| matches!(c, Call::Search(_)));
    let all = visible(&rt);
    click(&mut rt, "fmt-mp3");
    assert_eq!(visible(&rt), 3);
    assert_eq!(shown_formats(&rt), ["MP3"].into());
    click(&mut rt, "fmt-wav");
    assert_eq!(visible(&rt), 5, "MP3 ∪ WAV");
    assert_eq!(shown_formats(&rt), ["MP3", "WAV"].into());
    click(&mut rt, "fmt-mp3");
    click(&mut rt, "fmt-wav");
    assert_eq!(visible(&rt), all, "none selected again: everything");

    // Free slot narrows the counts too (tomasz's WAVs and marten_k's FLAC are queued).
    click(&mut rt, "free-slot");
    let st = rt.state.tab().search().unwrap();
    let counts = st.search.as_ref().unwrap().format_counts(&st.filters);
    assert!(
        counts.contains(&("WAV", 0)) && counts.contains(&("FLAC", 12)),
        "{counts:?}"
    );
    // A format left with nothing has no chip at all.
    assert!(rt.find("fmt-wav").is_none() && rt.find("fmt-flac").is_some());
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Search(_))),
        searches,
        "filters never search again"
    );
}

#[test]
fn format_groups_merge_aliases_and_overflow_into_a_menu() {
    use crate::tabs::format_of;
    assert_eq!(format_of(r"a\x.M4A"), Some("AAC/M4A"));
    assert_eq!(format_of(r"a\x.aac"), Some("AAC/M4A"));
    assert_eq!(format_of(r"a\x.aif"), Some("AIFF"));
    assert_eq!(format_of(r"a\x.AIFF"), Some("AIFF"));
    assert_eq!(format_of(r"a\x.wv"), Some("WavPack"));
    assert_eq!(format_of(r"a\cover.jpg"), None);
    for e in wholeseek_core::rank::AUDIO_EXTENSIONS {
        assert!(format_of(&format!("a\\x.{e}")).is_some(), "{e} has a chip");
    }
    // Ten formats: seven chips, the rest under "+3".
    let mut rt = signed_in();
    let exts = [
        "flac", "mp3", "wav", "m4a", "aiff", "ogg", "opus", "alac", "ape", "wv",
    ];
    let hits: Vec<wholeseek_core::SearchHit> = exts
        .iter()
        .enumerate()
        .flat_map(|(i, e)| {
            (0..(10 - i)).map(move |k| wholeseek_core::SearchHit {
                file: wholeseek_core::RemoteFile {
                    username: "u".into(),
                    path: format!("m\\{k}.{e}"),
                    size: 1,
                    attrs: Default::default(),
                },
                free_slot: true,
                speed: 1,
            })
        })
        .collect();
    rt.update(|s, _| {
        s.new_search_tab();
        let st = s.tab_mut().search_mut().unwrap();
        st.search = Some(crate::tabs::SearchView {
            id: wholeseek_core::SearchId(999),
            query: "x".into(),
            hits,
            finished: true,
            keys: Vec::new(),
            matched: Default::default(),
        });
    });
    run(&mut rt, 0.05);
    assert!(rt.find("fmt-opus").is_some() && rt.find("fmt-alac").is_none());
    click(&mut rt, "fmt-more");
    let m = rt.state.menu.as_ref().unwrap();
    assert_eq!(m.ids(), ["fmt-alac", "fmt-ape", "fmt-wv"]);
    assert_eq!(m.items[2].label, "WavPack 1");
    click(&mut rt, "menu-fmt-ape");
    assert!(rt.state.tab().search().unwrap().filters.has("APE"));
    assert_eq!(visible(&rt), 2);
    click(&mut rt, "fmt-more");
    let m = rt.state.menu.as_ref().unwrap();
    assert_eq!(m.items[1].label, "APE 2");
    assert_eq!(
        m.items[1].icon.map(|i| i.name),
        Some("lucide-check"),
        "on: a check icon"
    );
    assert!(m.items[0].icon.is_none());
}

#[test]
fn a_new_search_tab_starts_with_the_last_used_filters() {
    let mut rt = searched();
    click(&mut rt, "fmt-flac");
    click(&mut rt, "fmt-wav");
    click(&mut rt, "free-slot");
    click(&mut rt, "new-tab");
    let f = &rt.state.tab().search().unwrap().filters;
    assert!(f.has("FLAC") && f.has("WAV") && !f.has("MP3") && f.free_slot);
}

#[test]
fn results_are_grouped_by_user_then_folder() {
    let rt = searched();
    let st = rt.state.tab().search().unwrap();
    let groups = st.search.as_ref().unwrap().groups(&st.filters);
    assert_eq!(groups[0].username, "alice");
    assert_eq!(groups[0].folders.len(), 2, "Coastline and Night Ferry");
    assert!(rt.find("group-0/user").is_some() && rt.find("group-0/meta").is_some());
    assert!(rt.find("folder-0-0").is_some() && rt.find("folder-0-1").is_some());
    // Rows under a folder are that folder's files, in natural name order.
    let first = &groups[0].folders[0];
    for i in &first.1 {
        assert_eq!(sv(&rt).hits[*i].file.folder(), first.0);
        assert!(rt.find(&format!("row-{i}")).is_some());
    }
    assert!(rt.find("count").is_some());
}

// ---- CP2: startup, displaced, reconnecting ---------------------------------------------------

fn booting() -> (Runtime<App>, std::sync::mpsc::Sender<crate::app::Booted>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let world = std::rc::Rc::new(std::cell::RefCell::new(crate::demo::World::default()));
    let boot = crate::app::Boot {
        rx,
        make: |_| Box::new(crate::demo::DemoBackend::new(Default::default())),
    };
    let app = App::booting(
        Box::new(crate::demo::FakePlayer::new(world)),
        Box::new(crate::store::MemoryStore::new(true)),
        boot,
        true,
    );
    let mut rt = Runtime::new(
        app,
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    (rt, tx)
}

#[test]
fn the_window_shows_at_once_and_signs_in_when_the_saved_account_loads() {
    let (mut rt, tx) = booting();
    assert_eq!(rt.state.screen, Screen::Starting);
    assert!(
        rt.find("starting").is_some(),
        "\"Signing in…\" on the first frame"
    );
    run(&mut rt, 0.5);
    assert_eq!(
        rt.state.screen,
        Screen::Starting,
        "still waiting for the loader"
    );
    tx.send(crate::app::Booted {
        settings: wholeseek_core::Settings::default(),
        startup: Startup {
            credentials: Some(Credentials {
                username: "nadia".into(),
                password: "pw".into(),
            }),
            keyring_available: true,
        },
    })
    .unwrap();
    run(&mut rt, 0.1);
    assert_eq!(rt.state.screen, Screen::Main);
    assert!(calls(&rt).contains(&Call::Login("nadia".into())));
    run(&mut rt, 0.5);
    assert!(matches!(rt.state.conn, Conn::Connected { .. }));
}

#[test]
fn without_a_saved_account_startup_goes_to_onboarding() {
    let (mut rt, tx) = booting();
    tx.send(crate::app::Booted {
        settings: wholeseek_core::Settings::default(),
        startup: Startup {
            credentials: None,
            keyring_available: false,
        },
    })
    .unwrap();
    run(&mut rt, 0.1);
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert!(!rt.state.keyring_available);
}

/// Sign in as a scripted demo user ("twice", "flaky", …).
fn login_as(user: &str) -> Runtime<App> {
    let mut rt = rt();
    rt.update(|s, _| {
        s.onboarding.username.replace_text(user);
        s.onboarding.password.replace_text("pw");
        s.onboarding.shares = vec![std::env::temp_dir()];
        s.connect();
    });
    rt
}

#[test]
fn signed_in_elsewhere_shows_a_banner_and_never_reconnects_by_itself() {
    let mut rt = login_as("twice");
    run(&mut rt, 2.0);
    assert_eq!(rt.state.conn, Conn::Displaced);
    assert!(rt.find("displaced").is_some() && rt.find("reconnect").is_some());
    let logins = count_calls(&rt, |c| matches!(c, Call::Login(_)));
    run(&mut rt, 10.0);
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Login(_))),
        logins,
        "no loop"
    );
    assert!(rt.find("displaced").is_some());
    click(&mut rt, "reconnect");
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Login(_))),
        logins + 1
    );
    assert_eq!(calls(&rt).last(), Some(&Call::Login("twice".into())));
}

#[test]
fn a_dropped_connection_shows_reconnecting_from_the_core_status() {
    let mut rt = login_as("flaky");
    run(&mut rt, 1.5);
    assert!(rt.state.status.reconnect.is_some());
    assert!(rt.find("reconnecting").is_some());
    run(&mut rt, 5.0);
    assert!(rt.find("reconnecting").is_none());
    assert!(matches!(rt.state.conn, Conn::Connected { .. }));
}

// ---- CP2: transfers drawer ------------------------------------------------------------------

const COAST: &str = r"Music\Hollow Lights\Coastline (2019)";
const FERRY: &str = r"Music\Hollow Lights\Night Ferry EP (2021)";

#[test]
fn download_folder_goes_to_the_core_which_skips_system_files() {
    let mut rt = signed_in();
    rt.update(|s, _| {
        s.open_user("alice");
    });
    run(&mut rt, 0.6);
    rt.update(|s, _| {
        s.download_folder("alice", FERRY);
    });
    run(&mut rt, 0.1);
    assert_eq!(
        calls(&rt).last(),
        Some(&Call::DownloadFolder("alice".into(), FERRY.into()))
    );
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Download(_))),
        0,
        "no per-file requests"
    );
    // Thumbs.db, desktop.ini and the .url never become transfers.
    assert_eq!(rt.state.transfers.len(), 4);
    assert!(
        rt.state
            .transfers
            .iter()
            .all(|t| !wholeseek_core::is_download_junk(&t.file.path))
    );
}

#[test]
fn downloads_group_by_folder_with_an_aggregate_line() {
    let mut rt = searched();
    rt.update(|s, _| {
        s.open_user("alice");
    });
    run(&mut rt, 0.6);
    rt.update(|s, _| s.activate(0));
    let (_, single) = row_of(&rt, "vinyl_rips", "A1 Northern Glass.flac");
    rt.update(|s, _| s.download(&single));
    run(&mut rt, 0.2);
    rt.update(|s, _| s.download_folder("alice", FERRY));
    run(&mut rt, 0.2);
    // Newest group first.
    let groups = crate::transfers::groups(
        &rt.state.transfers,
        &rt.state.drawer.hidden,
        Default::default(),
    );
    assert_eq!(groups.len(), 2);
    assert_eq!(
        (groups[0].folder.as_str(), groups[0].rows.len()),
        (FERRY, 4)
    );
    assert_eq!(
        (groups[1].username.as_str(), groups[1].rows.len()),
        ("vinyl_rips", 1)
    );
    let line = |rt: &Runtime<App>| {
        let rows: Vec<&crate::app::Transfer> = groups[0]
            .rows
            .iter()
            .map(|&i| &rt.state.transfers[i])
            .collect();
        crate::transfers::summary(&rows)
    };
    assert!(line(&rt).starts_with("0 of 4 · "), "{}", line(&rt));

    click(&mut rt, "transfers");
    // The album has a header and folds; a lone download is just its row.
    assert!(rt.find("group-0/group-head").is_some() && rt.find("group-0/dl-3").is_some());
    assert!(rt.find("group-1/dl-0").is_some() && rt.find("group-1/group-head").is_none());
    click(&mut rt, "group-0/group-head");
    assert!(rt.find("group-0/dl-3").is_none(), "collapsed");
    assert!(
        rt.find("group-0/group-head/group-summary").is_some(),
        "the aggregate line stays"
    );
    run(&mut rt, 90.0);
    assert!(line(&rt).starts_with("4 of 4 · "), "{}", line(&rt));
    assert!(line(&rt).ends_with(" · done"), "{}", line(&rt));
    assert_eq!(
        crate::transfers::of_size(212_000_000, 512_000_000),
        "212.0 / 512.0 MB"
    );
    assert_eq!(
        crate::transfers::of_size(900_000, 2_000_000),
        "900 KB / 2.0 MB"
    );
}

#[test]
fn cancel_all_asks_first_above_five_and_cancels_every_live_download() {
    let mut rt = signed_in();
    rt.update(|s, _| {
        s.open_user("alice");
    });
    run(&mut rt, 0.6);
    rt.state.download_folder("alice", COAST); // 8 files
    run(&mut rt, 0.1);
    click(&mut rt, "transfers");
    let live = rt.state.live_count();
    assert!(live > 5, "{live}");
    click(&mut rt, "cancel-all");
    assert!(rt.find("confirm-cancel").is_some());
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Cancel(_))),
        0,
        "nothing yet"
    );
    click(&mut rt, "keep-transfers");
    assert!(rt.find("confirm-cancel").is_none());
    click(&mut rt, "cancel-all");
    click(&mut rt, "confirm-cancel-all");
    run(&mut rt, 0.1);
    assert_eq!(rt.state.live_count(), 0);
    // Every download that hadn't finished got exactly one cancel; finished ones none.
    for t in &rt.state.transfers {
        let n = count_calls(&rt, |c| *c == Call::Cancel(t.id));
        assert_eq!(
            n,
            usize::from(!matches!(t.state, TState::Done(_))),
            "{:?}",
            t.file.basename()
        );
    }

    // Five or fewer: no question.
    let mut rt = searched();
    let (_, f) = row_of(&rt, "alice", "01 Northern Glass.flac");
    rt.update(|s, _| {
        s.download(&f);
    });
    click(&mut rt, "transfers");
    click(&mut rt, "cancel-all");
    assert!(rt.find("confirm-cancel").is_none());
    assert_eq!(count_calls(&rt, |c| matches!(c, Call::Cancel(_))), 1);
}

#[test]
fn retry_failed_and_clear_finished_work_on_the_whole_list() {
    let mut rt = searched();
    let (_, bad) = row_of(&rt, "marten_k", "Hollow Lights - Salt Roads.flac");
    let (_, ok) = row_of(&rt, "vinyl_rips", "A2 Salt Roads.flac");
    let (_, stop) = row_of(&rt, "alice", "01 Northern Glass.flac");
    rt.update(|s, _| {
        s.download(&bad);
    });
    rt.update(|s, _| {
        s.download(&ok);
    });
    rt.update(|s, _| {
        s.download(&stop);
    });
    run(&mut rt, 0.2);
    let stop_id = rt.state.download_of(&stop).unwrap().id;
    rt.update(|s, _| {
        s.cancel(stop_id);
    });
    run(&mut rt, 60.0);
    let bad_id = rt.state.download_of(&bad).unwrap().id;
    assert!(matches!(
        rt.state.download_of(&bad).unwrap().state,
        TState::Failed(_)
    ));
    assert!(matches!(
        rt.state.download_of(&ok).unwrap().state,
        TState::Done(_)
    ));
    click(&mut rt, "transfers");
    click(&mut rt, "filter-failed");
    assert_eq!(rt.state.drawer.filter, crate::transfers::TFilter::Failed);
    click(&mut rt, "retry-all");
    assert!(
        calls(&rt).contains(&Call::Retry(bad_id)) && calls(&rt).contains(&Call::Retry(stop_id))
    );

    run(&mut rt, 60.0);
    click(&mut rt, "filter-all");
    click(&mut rt, "clear-finished");
    assert_eq!(
        crate::transfers::listed(&rt.state.transfers, &rt.state.drawer.hidden).count(),
        0
    );
    assert!(
        rt.state.local_path_of(&ok).is_some(),
        "cleared rows are still known as downloaded (files stay on disk)"
    );
    assert!(rt.find("no-downloads").is_some());
}

#[test]
fn finished_downloads_offer_play_show_open_and_copy() {
    let mut rt = searched();
    let board = rune::clipboard::memory();
    rt.use_clipboard(&board);
    let (_, f) = row_of(&rt, "alice", "01 Northern Glass.flac");
    rt.update(|s, _| {
        s.download(&f);
    });
    run(&mut rt, 30.0);
    let d = rt.state.download_of(&f).unwrap().clone();
    let TState::Done(path) = d.state.clone() else {
        panic!("not done")
    };
    let ids: Vec<_> = rt.state.transfer_items(d.id).iter().map(|i| i.id).collect();
    assert_eq!(ids, ["play", "show-in-folder", "open-file", "copy-path"]);

    // Right-click the row: the same four, dispatched through the desktop seam.
    click(&mut rt, "transfers");
    let (x, y) = rt.find("group-0/dl-0").unwrap();
    rt.button_down(x, y, PointerButton::Secondary);
    rt.button_up(x, y, PointerButton::Secondary);
    run(&mut rt, 0.05);
    assert_eq!(rt.state.menu.as_ref().unwrap().ids(), ids);
    click(&mut rt, "menu-show-in-folder");
    assert!(rt.state.menu.is_none());
    assert_eq!(
        rt.state.desktop_log.borrow().last(),
        Some(&DesktopCall::ShowItem(path.clone()))
    );
    rt.update(|s, _| {
        s.run_menu(MenuAction::OpenFile(path.clone()));
    });
    assert_eq!(
        rt.state.desktop_log.borrow().last(),
        Some(&DesktopCall::Open(path.clone()))
    );
    let items = rt.state.transfer_items(d.id);
    rt.update(|s, _| s.open_menu(MenuAnchor::Point(10.0, 10.0), items));
    run(&mut rt, 0.05);
    click(&mut rt, "menu-copy-path");
    assert_eq!(
        board.text().as_deref().map(|s| s.as_str()),
        Some(path.to_str().unwrap())
    );

    // ▶ from disk: no new fetch.
    let fetches = count_calls(&rt, |c| matches!(c, Call::PlayFetch(_)));
    rt.update(|s, _| {
        s.play_local(d.id);
    });
    run(&mut rt, 0.5);
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::PlayFetch(_))),
        fetches
    );
    assert!(rt.state.playing.as_ref().unwrap().opened);
    assert!(rt.find("now/source").is_some());

    // Drawer header: the downloads folder.
    click(&mut rt, "open-downloads");
    assert_eq!(
        rt.state.desktop_log.borrow().last(),
        Some(&DesktopCall::Open("/home/demo/Downloads/wholeseek".into()))
    );
}

#[test]
fn show_item_asks_the_file_manager_then_falls_back_to_the_folder() {
    use crate::desktop::Os;
    let cmds = crate::desktop::show_item_commands(
        Os::Freedesktop,
        std::path::Path::new("/m/A b/01 ü.flac"),
    );
    assert_eq!(cmds[0].0, "gdbus");
    assert!(
        cmds[0]
            .1
            .contains(&"org.freedesktop.FileManager1.ShowItems".to_string())
    );
    assert!(
        cmds[0]
            .1
            .contains(&"['file:///m/A%20b/01%20%C3%BC.flac']".to_string())
    );
    assert_eq!(cmds[1].0, "dbus-send");
    assert_eq!(
        cmds.last().unwrap(),
        &("xdg-open", vec!["/m/A b".to_string()])
    );
}

#[test]
fn desktop_commands_per_os() {
    use crate::desktop::{Os, open_commands, show_item_commands};
    let f = std::path::Path::new("/Users/n/Music/A b/01.flac");
    let p = f.display().to_string();
    assert_eq!(open_commands(Os::MacOs, f), [("open", vec![p.clone()])]);
    assert_eq!(
        show_item_commands(Os::MacOs, f),
        [("open", vec!["-R".to_string(), p.clone()])],
        "Finder reveals and selects the file"
    );
    assert_eq!(open_commands(Os::Freedesktop, f), [("xdg-open", vec![p])]);
    assert_eq!(show_item_commands(Os::Freedesktop, f)[0].0, "gdbus");
    assert_eq!(
        crate::desktop::OS,
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Freedesktop
        }
    );
}

#[test]
fn uploads_list_in_the_drawer_with_cancel() {
    let mut rt = signed_in();
    click(&mut rt, "transfers");
    assert!(rt.find("up-0").is_some() && rt.find("up-0/user").is_some());
}

// ---- CP2: menus, folder links, keyboard ------------------------------------------------------

#[test]
fn menus_offer_what_each_place_needs() {
    let rt = searched();
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let ids = |v: Vec<crate::menu::MenuItem>| v.iter().map(|i| i.id).collect::<Vec<_>>();
    assert_eq!(
        ids(crate::menu::file_items(&f, false, &[])),
        [
            "play",
            "download",
            "download-to",
            "download-folder",
            "open-folder",
            "browse-user",
            "open-folder-new-tab",
            "browse-user-new-tab",
            "copy-name",
            "copy-path"
        ]
    );
    // In a user tab the folder is open already and its header downloads it.
    assert_eq!(
        ids(crate::menu::file_items(&f, true, &[])),
        ["play", "download", "download-to", "copy-name", "copy-path"]
    );
    // Two parts: this row's actions, then (under a line) the common group.
    let v = crate::menu::file_items(&f, false, &[]);
    let lines: Vec<_> = v.iter().filter(|i| i.sep).map(|i| i.id).collect();
    assert_eq!(lines, ["open-folder-new-tab"]);
    let cover = wholeseek_core::RemoteFile {
        path: format!("{COAST}\\cover.jpg"),
        ..f.clone()
    };
    assert_eq!(
        ids(crate::menu::file_items(&cover, true, &[]))[0],
        "download",
        "no ▶ for non-audio"
    );
    assert_eq!(
        ids(crate::menu::folder_items("alice", COAST, true, &[])),
        [
            "play-folder",
            "download-folder",
            "download-folder-to",
            "open-folder",
            "browse-user",
            "open-folder-new-tab",
            "browse-user-new-tab",
            "copy-name",
            "copy-path"
        ]
    );
    assert_eq!(
        ids(crate::menu::folder_items("alice", COAST, false, &[])),
        [
            "download-folder",
            "download-folder-to",
            "open-folder",
            "browse-user",
            "open-folder-new-tab",
            "browse-user-new-tab",
            "copy-name",
            "copy-path"
        ],
        "nothing to play: no Play folder"
    );
}

#[test]
fn right_click_and_more_open_a_row_menu_and_escape_closes_it() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "03 Low Tide Radio.flac");
    let (x, y) = rt.find(&format!("row-{i}")).unwrap();
    rt.button_down(x, y, PointerButton::Secondary);
    rt.button_up(x, y, PointerButton::Secondary);
    run(&mut rt, 0.05);
    assert_eq!(rt.state.menu.as_ref().unwrap().ids()[0], "play");
    assert!(rt.find("menu").is_some());
    assert_eq!(rt.state.tab().selected.as_deref(), Some(f.path.as_str()));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
    );
    assert!(rt.state.menu.is_none());

    let more = crate::view::files_more_key(rt.state.tab().id, &f.path);
    click(&mut rt, &more);
    assert!(matches!(rt.state.menu.as_ref().unwrap().anchor, MenuAnchor::Key(ref k) if *k == more));
    click(&mut rt, "menu-download");
    assert!(calls(&rt).contains(&Call::Download(f)));
}

#[test]
fn folder_segments_open_the_user_tab_at_that_folder() {
    let mut rt = searched();
    // "Music / Hollow Lights / Coastline (2019)": the middle segment.
    click(&mut rt, "folder-0-0/seg-1");
    assert_eq!(rt.state.tab().username(), Some("alice"));
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.selected.as_deref(), Some(r"Music\Hollow Lights"));
    assert!(
        rt.find("browse-loading").is_some(),
        "Opening folder… until the listing arrives"
    );
    run(&mut rt, 0.6);
    let b = rt.state.active_browse().unwrap();
    assert_eq!(
        b.selected.as_deref(),
        Some(r"Music\Hollow Lights"),
        "kept when it lands"
    );
    assert!(b.expanded.contains("Music"));
}

#[test]
fn open_folder_deep_links_even_when_the_listing_arrives_late() {
    let mut rt = searched();
    let (_, f) = row_of(&rt, "alice", "03 Undertow.flac");
    rt.state.run_menu(MenuAction::OpenFolder {
        username: "alice".into(),
        folder: f.folder().into(),
        file: Some(f.path.clone()),
    });
    run(&mut rt, 0.02);
    assert_eq!(
        rt.state.take_reveal(),
        None,
        "nothing to reveal before the listing"
    );
    assert!(rt.state.reveal);
    run(&mut rt, 0.8);
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.selected.as_deref(), Some(FERRY));
    assert_eq!(b.highlight.as_deref(), Some(f.path.as_str()));
    assert!(
        !rt.state.reveal,
        "revealed once listed (poll scrolled it into view)"
    );
    assert!(rt.find("file-2").is_some());

    // The tab already open: focuses it and moves to the other folder.
    rt.update(|s, _| {
        s.activate(0);
    });
    rt.update(|s, _| {
        s.open_folder_at("alice", COAST, None);
    });
    assert_eq!(rt.state.tabs.len(), 2);
    assert_eq!(
        rt.state.active_browse().unwrap().selected.as_deref(),
        Some(COAST)
    );
}

#[test]
fn enter_plays_the_selected_row_and_shift_enter_downloads_it() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "04 Breakwater.flac");
    click_at(&mut rt, &format!("row-{i}"), 0.5);
    assert_eq!(rt.state.tab().selected.as_deref(), Some(f.path.as_str()));
    rt.update(|_, tx| tx.focus(format!("row-{i}"), true));
    run(&mut rt, 0.02);
    press(
        &mut rt,
        KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
    );
    assert_eq!(rt.state.playing.as_ref().unwrap().file, f);
    press(
        &mut rt,
        KeyEvent {
            key: Key::Enter,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        },
    );
    assert!(calls(&rt).contains(&Call::Download(f)));
}

// ---- CP2: queue and ▶ fallback ---------------------------------------------------------------

#[test]
fn play_queues_the_folder_and_next_previous_follow_it() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    click(&mut rt, &format!("row-{i}/play"));
    assert_eq!(
        rt.state.queue_position(),
        Some((2, 6)),
        "Coastline's six tracks, not the jpg/log"
    );
    run(&mut rt, 1.0);
    assert!(
        rt.find("next").is_some() && rt.find("prev").is_some() && rt.find("queue-pos").is_some()
    );

    press(&mut rt, ctrl(Key::Right));
    run(&mut rt, 0.1);
    assert_eq!(rt.state.queue_position(), Some((3, 6)));
    let now = rt.state.playing.as_ref().unwrap().file.clone();
    assert_eq!(now.basename(), "03 Low Tide Radio.flac");
    assert!(calls(&rt).contains(&Call::PlayFetch(now)));
    press(&mut rt, ctrl(Key::Left));
    run(&mut rt, 0.1);
    assert_eq!(rt.state.playing.as_ref().unwrap().file, f);

    // Space toggles pause when no control takes it.
    run(&mut rt, 1.0);
    rt.update(|_, tx| tx.blur());
    press(
        &mut rt,
        KeyEvent {
            key: Key::Text(" ".into()),
            mods: Mods::default(),
        },
    );
    assert_eq!(rt.state.player.snapshot().state, PlayState::Paused);
}

#[test]
fn the_next_track_is_prefetched_and_plays_on_by_itself() {
    let mut rt = searched();
    let (i, _) = row_of(&rt, "alice", "04 Breakwater.flac");
    click(&mut rt, &format!("row-{i}/play"));
    run(&mut rt, 1.0);
    // Near the end: seek there once it is all downloaded.
    run(&mut rt, 12.0);
    rt.update(|s, _| s.seek_fraction(0.97));
    run(&mut rt, 1.0);
    let fetched = |rt: &Runtime<App>, name: &str| {
        calls(rt)
            .iter()
            .any(|c| matches!(c, Call::PlayFetch(f) if f.basename() == name))
    };
    assert!(fetched(&rt, "05 A Lamp in the Dunes.flac"), "prefetched");
    run(&mut rt, 10.0);
    assert_eq!(rt.state.queue_position(), Some((5, 6)));
}

#[test]
fn a_silent_source_falls_back_to_others_then_gives_up() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let alts = rt.state.alternates_for(&f);
    assert!(!alts.is_empty());
    click(&mut rt, &format!("row-{i}/play"));
    let stall = |rt: &mut Runtime<App>| {
        let id = rt.state.playing.as_ref().unwrap().ticket.id;
        rt.state.apply(wholeseek_core::CoreEvent::TransferFailed {
            id,
            reason: format!("{} alice within 8s", wholeseek_core::PLAY_STALLED),
        });
        run(rt, 0.02);
    };
    stall(&mut rt);
    let p = rt.state.playing.as_ref().unwrap();
    assert_eq!(p.attempt, 2);
    assert_eq!(p.file, alts[0]);
    assert!(calls(&rt).contains(&Call::PlayFetch(alts[0].clone())));
    assert!(p.failed.is_none(), "silent");
    for _ in 1..alts.len() {
        stall(&mut rt);
    }
    // Out of sources mid-queue: marked failed and skipped to the next track.
    stall(&mut rt);
    run(&mut rt, 0.1);
    let q = rt.state.queue.as_ref().unwrap();
    assert_eq!(q.items[1].failed.as_deref(), Some("No source responded"));
    assert_eq!(rt.state.queue_position(), Some((3, 6)));

    // A lone ▶ gives up with one clear line.
    rt.update(|s, _| s.play(&f));
    run(&mut rt, 0.02);
    for _ in 0..=alts.len() {
        stall(&mut rt);
    }
    let p = rt.state.playing.as_ref().unwrap();
    assert_eq!(p.failed.as_deref(), Some("No source responded"));
    assert!(rt.find("notice").is_some());
}

// ---- CP2: settings ---------------------------------------------------------------------------

fn applied(rt: &Runtime<App>) -> wholeseek_core::Settings {
    calls(rt)
        .into_iter()
        .rev()
        .find_map(|c| match c {
            Call::ApplySettings(s) => Some(s),
            _ => None,
        })
        .expect("settings applied")
}

#[test]
fn folder_picker_sets_the_download_folder_and_saves_at_once() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    click(&mut rt, "choose-download");
    let (kind, _) = rt.file_dialog().expect("a dialog");
    assert_eq!(kind, rune::FileDialogKind::Folder);
    rt.resolve_file_dialog(Some("/home/demo/Music/in".into()))
        .unwrap();
    run(&mut rt, 0.05);
    assert_eq!(
        applied(&rt).download_dir,
        std::path::PathBuf::from("/home/demo/Music/in")
    );
    assert!(matches!(store_calls(&rt).last(),
        Some(StoreCall::SaveSettings(s)) if s.download_dir == std::path::Path::new("/home/demo/Music/in")));
    assert!(rt.find("saved").is_some(), "the Saved tick");
    run(&mut rt, 2.0);
    assert!(rt.find("saved").is_none());
}

#[test]
fn shares_are_added_by_picker_removed_and_rescanned() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    let before = rt.state.backend.settings().share_dirs.clone();
    click(&mut rt, "add-share");
    rt.resolve_file_dialog(Some("/srv/flac".into())).unwrap();
    run(&mut rt, 0.05);
    let dirs = rt.state.backend.settings().share_dirs.clone();
    assert_eq!(dirs.len(), before.len() + 1);
    assert!(calls(&rt).contains(&Call::SetShares(dirs.clone())));
    let i = dirs.len() - 1;
    let files = |rt: &Runtime<App>| {
        rt.state
            .status
            .shares
            .iter()
            .find(|x| x.dir == dirs[i])
            .map(|x| (x.files, x.scanning))
    };
    assert_eq!(
        files(&rt),
        Some((518, true)),
        "counts come from core status(), scanning first"
    );
    run(&mut rt, 0.6);
    assert_eq!(files(&rt), Some((518, false)));
    assert!(rt.find(&format!("set-share-{i}/files")).is_some());
    click(&mut rt, &format!("set-share-{i}/remove"));
    assert_eq!(rt.state.backend.settings().share_dirs, before);
    click(&mut rt, "rescan");
    assert_eq!(calls(&rt).last(), Some(&Call::RescanShares));
    run(&mut rt, 0.6);
    assert!(rt.state.status.shares.iter().all(|x| x.files == 521));
    // A cancelled dialog changes nothing.
    click(&mut rt, "add-share");
    rt.resolve_file_dialog(None).unwrap();
    run(&mut rt, 0.05);
    assert_eq!(rt.state.backend.settings().share_dirs, before);
}

#[test]
fn without_a_folder_dialog_paths_are_typed() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    rt.update(|s, _| {
        crate::actions::picked(
            s,
            Err(rune::FileDialogError("no portal".into())),
            App::add_share,
        );
    });
    run(&mut rt, 0.05);
    assert!(rt.find("set-share").is_some() && rt.find("add-share").is_none());
    let dir = std::env::temp_dir();
    rt.state
        .settings_form
        .share_input
        .replace_text(dir.display().to_string());
    rt.update(|s, _| {
        s.add_typed_share();
    });
    assert!(rt.state.backend.settings().share_dirs.contains(&dir));
}

#[test]
fn steppers_apply_slots_and_the_silent_source_timeout() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    let slots = rt.state.backend.settings().upload_slots;
    click(&mut rt, "set-slots/more");
    assert_eq!(applied(&rt).upload_slots, slots + 1);
    assert_eq!(rt.state.backend.settings().play_start_timeout_secs, 8);
    click(&mut rt, "set-timeout/more");
    assert_eq!(applied(&rt).play_start_timeout_secs, 9);
    assert!(
        matches!(store_calls(&rt).last(), Some(StoreCall::SaveSettings(s)) if s.play_start_timeout_secs == 9)
    );
}

#[test]
fn port_line_is_one_line_and_a_busy_port_says_so() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    run(&mut rt, 1.0);
    assert_eq!(rt.state.status.listen.bound, Some(2234));
    assert_eq!(
        rt.state.port_mapping,
        Some((2234, true)),
        "demo router maps it"
    );
    assert_eq!(text_of(&rt, "port-label").as_deref(), Some("Port 2234"));
    assert_eq!(text_of(&rt, "port-state").as_deref(), Some("open"));
    rt.state.apply(wholeseek_core::CoreEvent::PortMapping {
        port: 2234,
        outcome: wholeseek_core::PortMapOutcome::Unavailable("no router".into()),
    });
    assert_eq!(rt.state.port_mapping, Some((2234, false)));

    // Change → 2240 (busy in the demo) → Enter → confirm → reconnect on the fallback.
    click(&mut rt, "change-port");
    rt.state.settings_form.listen_port.replace_text("2240");
    rt.update(|s, _| {
        s.propose_port();
    });
    run(&mut rt, 0.05);
    assert!(rt.find("port-confirm").is_some());
    click(&mut rt, "port-reconnect");
    assert_eq!(applied(&rt).listen_port, 2240);
    run(&mut rt, 1.0);
    assert!(matches!(
        rt.state.conn,
        Conn::Connected {
            listen_port: Some(45543),
            ..
        }
    ));
    assert_eq!(rt.state.status.listen.bound, Some(45543));
    // One line: "Port 45543 ● 2240 busy", the why in the tooltip.
    assert_eq!(text_of(&rt, "port-label").as_deref(), Some("Port 45543"));
    assert_eq!(text_of(&rt, "port-state").as_deref(), Some("2240 busy"));
    assert!(rt.state.error.is_none(), "no banner for a busy port");

    rt.state.settings_form.port_editing = true;
    rt.state.settings_form.listen_port.replace_text("80");
    rt.update(|s, _| {
        s.propose_port();
    });
    assert!(rt.state.settings_form.port_invalid && rt.state.settings_form.port_confirm.is_none());
}

#[test]
fn clear_cache_is_disabled_while_a_track_streams_from_it() {
    let mut rt = searched();
    click(&mut rt, "settings");
    run(&mut rt, 0.2);
    assert!(rt.state.status.cache.used_bytes > 0 && rt.find("cache-usage").is_some());
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    rt.update(|s, _| {
        s.play(&f);
    });
    run(&mut rt, 0.5);
    assert!(rt.state.cache_in_use());
    click(&mut rt, "clear-cache");
    assert!(!calls(&rt).contains(&Call::ClearCache));
    rt.update(|s, _| s.stop_current());
    run(&mut rt, 0.05);
    click(&mut rt, "clear-cache");
    assert!(calls(&rt).contains(&Call::ClearCache));
    run(&mut rt, 0.2);
    assert_eq!(
        rt.state.status.cache.used_bytes,
        rt.state.status.cache.in_use_bytes
    );
}

#[test]
fn group_and_player_menus_follow_what_is_on_disk() {
    let mut rt = signed_in();
    rt.update(|s, _| s.download_folder("alice", FERRY));
    run(&mut rt, 0.2);
    let ids = |v: Vec<crate::menu::MenuItem>| v.iter().map(|i| i.id).collect::<Vec<_>>();
    assert!(
        rt.state.group_items("alice", FERRY).is_empty(),
        "nothing finished yet"
    );
    run(&mut rt, 90.0);
    assert_eq!(
        ids(rt.state.group_items("alice", FERRY)),
        ["show-in-folder", "play-album"]
    );

    // Play album: from disk, the whole folder in order, no fetches.
    let fetches = count_calls(&rt, |c| matches!(c, Call::PlayFetch(_)));
    let action = rt.state.group_items("alice", FERRY)[1].action.clone();
    rt.update(|s, _| {
        s.run_menu(action);
    });
    run(&mut rt, 0.3);
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::PlayFetch(_))),
        fetches
    );
    assert_eq!(rt.state.queue_position(), Some((1, 4)));
    // The track is a kept download: the player offers Show in folder.
    assert_eq!(
        ids(rt.state.player_items()),
        ["show-in-folder", "copy-path"]
    );

    // A streamed track offers Download instead.
    let mut rt = searched();
    let (i, _) = row_of(&rt, "vinyl_rips", "A2 Salt Roads.flac");
    click(&mut rt, &format!("row-{i}/play"));
    assert_eq!(ids(rt.state.player_items()), ["download", "copy-path"]);
    let (x, y) = rt.find("now").unwrap();
    rt.button_down(x, y, PointerButton::Secondary);
    rt.button_up(x, y, PointerButton::Secondary);
    run(&mut rt, 0.05);
    assert_eq!(
        rt.state.menu.as_ref().unwrap().ids(),
        ["download", "copy-path"]
    );
}

#[test]
fn the_menu_key_opens_the_focused_row_menu() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "01 Northern Glass.flac");
    rt.update(|_, tx| tx.focus(format!("row-{i}"), true));
    run(&mut rt, 0.02);
    press(
        &mut rt,
        KeyEvent {
            key: Key::ContextMenu,
            mods: Mods::default(),
        },
    );
    let m = rt.state.menu.as_ref().expect("menu");
    assert_eq!(m.ids()[0], "play");
    assert_eq!(rt.state.tab().selected.as_deref(), Some(f.path.as_str()));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
    );
    assert!(rt.state.menu.is_none());
    // Shift+F10 too.
    rt.update(|_, tx| tx.focus(format!("row-{i}"), true));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Function(10),
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        },
    );
    assert!(rt.state.menu.is_some());
}

#[test]
fn user_tab_downloads_the_open_folder_and_settings_forget_the_account() {
    let mut rt = signed_in();
    rt.update(|s, _| s.open_folder_at("alice", FERRY, None));
    run(&mut rt, 0.6);
    click(&mut rt, "folder-download");
    assert_eq!(
        calls(&rt).last(),
        Some(&Call::DownloadFolder("alice".into(), FERRY.into()))
    );

    click(&mut rt, "settings");
    assert_eq!(
        text_of(&rt, "set-account").as_deref(),
        Some("nadia · connected")
    );
    click(&mut rt, "forget");
    assert_eq!(store_calls(&rt).last(), Some(&StoreCall::ForgetCredentials));
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert!(calls(&rt).contains(&Call::Logout));
}

// ---- window width: nothing may push the layout past the window -----------------------------

/// Elements whose drawn box runs past the right edge of a `width`-wide window (ignoring
/// content inside scroll areas, which may legitimately be wider).
fn overflowing(rt: &Runtime<App>, width: f32) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| e.rect[0] + e.rect[2] > width + 0.5 && e.key.is_some())
        .map(|e| format!("{} [{:.0}+{:.0}]", e.path, e.rect[0], e.rect[2]))
        .collect()
}

/// The strip's scene from the bug report, at a window `w`×`h` points: a long search tab
/// and a user tab, plus `extra` more search tabs.
fn strip_scene(w: f32, h: f32, scale: f32, extra: usize) -> Runtime<App> {
    let mut rt = Runtime::new(
        folders_view(App::demo()),
        view::view,
        (w, h),
        Env {
            system_dark: true,
            scale,
        },
    );
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.state.demo_login();
    run(&mut rt, 1.0);
    rt.update(|s, _| s.search_for("roadblock iration 13"));
    run(&mut rt, 1.6);
    rt.update(|s, _| s.open_user("infectiphibian"));
    for k in 0..extra {
        rt.update(|s, _| {
            s.new_search_tab();
            s.search_for(&format!("a rather long search number {k}"));
        });
    }
    run(&mut rt, 0.6);
    rt
}

/// The right cluster is whole inside the window, and nothing in the strip runs past it.
fn assert_strip_fits(rt: &Runtime<App>, w: f32, what: &str) {
    for key in ["status", "transfers", "settings"] {
        let ([x, _, kw, _], opacity) = rt
            .presented(key)
            .unwrap_or_else(|| panic!("{what}: no {key}"));
        assert!(
            x >= 0.0 && x + kw <= w + 0.5 && kw > 0.0,
            "{what}: {key} at {x}+{kw} in {w}"
        );
        assert!(opacity > 0.99, "{what}: {key} visible");
    }
    let strip: Vec<_> = overflowing(rt, w)
        .into_iter()
        .filter(|p| p.contains("strip"))
        .collect();
    assert!(strip.is_empty(), "{what}: {strip:?}");
}

#[test]
fn the_strip_right_cluster_is_always_whole_at_every_size_and_scale() {
    for (pw, ph) in [
        (800.0, 600.0),
        (1100.0, 700.0),
        (1280.0, 800.0),
        (1920.0, 1080.0),
    ] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            // A window `pw` device pixels wide is `pw / scale` points: 1280 px at 2× is 640.
            let (w, h) = (pw / scale, ph / scale);
            for extra in [0, 6] {
                let rt = strip_scene(w, h, scale, extra);
                assert_strip_fits(&rt, w, &format!("{pw}x{ph}@{scale} +{extra} tabs"));
                let active = format!("tab-{}", rt.state.active);
                assert!(
                    rt.find(&active).is_some(),
                    "the active tab stays in the strip"
                );
            }
        }
    }
}

#[test]
fn tabs_shrink_to_a_minimum_then_fold_into_a_more_menu() {
    use crate::view::strip_tab_split as split;
    assert_eq!(split(3, 0, 1000.0), (vec![0, 1, 2], vec![]));
    let three = 3.0 * crate::view::TAB_MIN_W + 10.0;
    assert_eq!(split(8, 0, three), (vec![0, 1, 2], vec![3, 4, 5, 6, 7]));
    assert_eq!(
        split(8, 6, three),
        (vec![4, 5, 6], vec![0, 1, 2, 3, 7]),
        "active stays shown"
    );
    assert_eq!(
        split(8, 7, 10.0),
        (vec![7], (0..7).collect()),
        "at least the active tab"
    );

    let mut rt = strip_scene(640.0, 400.0, 2.0, 6);
    assert_eq!(rt.state.tabs.len(), 8);
    let shown: Vec<_> = (0..8)
        .filter(|i| rt.find(&format!("tab-{i}")).is_some())
        .collect();
    assert!(
        shown.len() < 8 && shown.contains(&rt.state.active),
        "{shown:?}"
    );
    for i in &shown {
        let ([_, _, tw, _], _) = rt.presented(&format!("tab-{i}")).unwrap();
        assert!(tw >= crate::view::TAB_MIN_W - 0.5, "tab-{i} is {tw} wide");
    }
    // The long title is cut with an ellipsis, not pushed out.
    click(&mut rt, "tabs-more");
    let menu = rt.state.menu.as_ref().unwrap();
    assert_eq!(menu.items.len(), 8 - shown.len());
    assert_eq!(menu.items[0].label, "roadblock iration 13");
    click(&mut rt, "menu-tab-0");
    assert_eq!(rt.state.active, 0);
    assert!(
        rt.find("tab-0").is_some(),
        "the chosen tab comes into the strip"
    );
    assert_strip_fits(&rt, 640.0, "after choosing");

    // Widening the window brings every tab back and drops "»".
    rt.set_viewport((1920.0, 1080.0), 1.0);
    run(&mut rt, 0.3);
    assert!((0..8).all(|i| rt.find(&format!("tab-{i}")).is_some()));
    assert!(rt.find("tabs-more").is_none());
    assert_strip_fits(&rt, 1920.0, "wide");
    // …and the content fills the new width (W/H in main.rs are only the opening size).
    let ([_, _, sw, _], _) = rt.presented("strip").unwrap();
    assert!((sw - 1920.0).abs() < 0.5, "strip is {sw} wide");
}

#[test]
fn strip_proof_png_1280x800() {
    let rt = strip_scene(1280.0, 800.0, 1.0, 0);
    assert_strip_fits(&rt, 1280.0, "1280x800");
}

// ---- ▶ only for what the player decodes -------------------------------------------------------

#[test]
fn rows_the_player_cannot_decode_get_no_play_but_keep_download() {
    let mut rt = searched();
    let (opus, of) = row_of(&rt, "kassi", "low tide radio (live).opus");
    let (ogg, _) = row_of(&rt, "kassi", "salt roads (demo).ogg");
    let (wav, _) = row_of(&rt, "tomasz", "Hollow Lights - Salt Roads.wav");
    assert!(
        rt.find(&format!("row-{opus}/play")).is_none(),
        "Opus: no decoder, no ▶"
    );
    assert!(
        rt.find(&format!("row-{ogg}/play")).is_some()
            && rt.find(&format!("row-{wav}/play")).is_some()
    );
    assert!(
        rt.find(&format!("row-{opus}/download")).is_some(),
        "↓ still offered"
    );
    let ids: Vec<_> = crate::menu::file_items(&of, false, &[])
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        ids,
        [
            "download",
            "download-to",
            "download-folder",
            "open-folder",
            "browse-user",
            "open-folder-new-tab",
            "browse-user-new-tab",
            "copy-name",
            "copy-path"
        ]
    );
    // Enter on it does nothing; play() refuses it outright.
    let fetches = count_calls(&rt, |c| matches!(c, Call::PlayFetch(_)));
    rt.update(|s, _| s.play(&of));
    run(&mut rt, 0.05);
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::PlayFetch(_))),
        fetches
    );
    assert!(rt.state.playing.is_none());
    // Downloading it works like any file.
    rt.update(|s, _| s.download(&of));
    assert!(calls(&rt).contains(&Call::Download(of)));
}

#[test]
fn queues_alternates_and_folder_menus_skip_undecodable_files() {
    let mut rt = searched();
    // kassi's folder: an .ogg (plays) and an .opus (doesn't).
    let (_, ogg) = row_of(&rt, "kassi", "salt roads (demo).ogg");
    let folder = rt.state.folder_files("kassi", ogg.folder());
    assert!(
        folder.iter().any(|f| f.path.ends_with(".opus")),
        "{folder:?}"
    );
    rt.update(|s, _| s.play_in(&ogg, &folder)); // what the row's ▶ does (row is off-screen)
    run(&mut rt, 0.05);
    let q = rt.state.queue.as_ref().unwrap();
    assert_eq!(q.items.len(), 1, "the .opus is not queued");
    assert_eq!(q.items[0].file, ogg);
    // A silent source never falls back to an undecodable copy.
    let (_, salt) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let alts = rt.state.alternates_for(&salt);
    assert!(!alts.is_empty() && alts.iter().all(crate::tabs::is_playable));
    // Folder menus: "Play folder" only where something plays.
    let ids = |v: Vec<crate::menu::MenuItem>| v.iter().map(|i| i.id).collect::<Vec<_>>();
    assert!(ids(rt.state.folder_menu("kassi", r"music\hollow lights")).contains(&"play-folder"));
    let opus_only = wholeseek_core::RemoteFile {
        username: "zed".into(),
        path: r"x\only.opus".into(),
        size: 1,
        attrs: Default::default(),
    };
    rt.update(|s, _| {
        s.tab_mut()
            .search_mut()
            .unwrap()
            .search
            .as_mut()
            .unwrap()
            .hits
            .push(wholeseek_core::SearchHit {
                file: opus_only.clone(),
                free_slot: true,
                speed: 1,
            });
    });
    assert!(!ids(rt.state.folder_menu("zed", "x")).contains(&"play-folder"));
}

#[test]
fn a_finished_undecodable_download_offers_no_play() {
    let mut rt = searched();
    let (_, of) = row_of(&rt, "kassi", "low tide radio (live).opus");
    rt.update(|s, _| s.download(&of));
    run(&mut rt, 60.0);
    let d = rt.state.download_of(&of).unwrap().clone();
    assert!(matches!(d.state, TState::Done(_)));
    let ids: Vec<_> = rt.state.transfer_items(d.id).iter().map(|i| i.id).collect();
    assert_eq!(ids, ["show-in-folder", "open-file", "copy-path"]);
    click(&mut rt, "transfers");
    assert!(rt.find("group-0/dl-0/show").is_some() && rt.find("group-0/dl-0/play").is_none());
    rt.update(|s, _| s.play_local(d.id));
    assert!(rt.state.playing.is_none());
    // Its group offers no "Play album".
    assert!(
        rt.state
            .group_items("kassi", r"music\hollow lights")
            .iter()
            .all(|i| i.id != "play-album")
    );
}

// ---- macOS: ⌘ for every Ctrl shortcut, ⌘ in tooltips ------------------------------------------

/// A key with ⌘ held, as Rune reports it on macOS (`cmd` with `logo`, Control up).
fn cmd(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        mods: Mods {
            cmd: true,
            logo: true,
            ..Mods::default()
        },
    }
}

#[test]
fn command_key_drives_every_shortcut_as_on_macos() {
    let mut rt = searched();
    rt.update(|_, tx| tx.blur());
    press(&mut rt, cmd(Key::Command('t')));
    assert_eq!((rt.state.tabs.len(), rt.state.active), (2, 1), "⌘T");
    press(&mut rt, cmd(Key::Tab));
    assert_eq!(rt.state.active, 0, "⌘Tab cycles like Ctrl+Tab");
    press(&mut rt, cmd(Key::Tab));
    press(&mut rt, cmd(Key::Command('w'))); // the empty new tab
    assert_eq!(rt.state.tabs.len(), 1, "⌘W");
    rt.update(|s, _| s.open_user("alice"));
    press(&mut rt, cmd(Key::Command('l')));
    assert!(
        rt.state.tab().search().is_some() && focused(&rt).as_deref() == Some("query"),
        "⌘L"
    );

    let (i, _) = row_of(&rt, "alice", "02 Salt Roads.flac");
    click(&mut rt, &format!("row-{i}/play"));
    run(&mut rt, 1.0);
    rt.update(|_, tx| tx.blur());
    press(&mut rt, cmd(Key::Right));
    assert_eq!(rt.state.queue_position(), Some((3, 6)), "⌘→ next");
    press(&mut rt, cmd(Key::Left));
    assert_eq!(rt.state.queue_position(), Some((2, 6)), "⌘← previous");
}

#[test]
fn shortcut_labels_follow_the_platform() {
    use crate::desktop::{Os, chord, shortcut};
    assert_eq!(shortcut(Os::MacOs, "T"), "⌘T");
    assert_eq!(shortcut(Os::MacOs, "→"), "⌘→");
    assert_eq!(shortcut(Os::Freedesktop, "T"), "Ctrl+T");
    assert_eq!(chord("W"), shortcut(crate::desktop::OS, "W"));
}

// ---- late player events for an abandoned queue -------------------------------------------------

#[test]
fn a_late_prefetch_for_an_abandoned_queue_fetches_nothing() {
    use wholeseek_audio::PlayerEvent;
    let mut rt = searched();
    let fetches = |rt: &Runtime<App>| count_calls(rt, |c| matches!(c, Call::PlayFetch(_)));
    // Coastline plays; the player is about to ask for track 2.
    let album = |rt: &Runtime<App>, folder: &str| {
        let files = rt.state.folder_files("alice", folder);
        let first = files
            .iter()
            .find(|f| crate::tabs::is_playable(f))
            .unwrap()
            .clone();
        (first, files)
    };
    let (first, files) = album(&rt, COAST);
    rt.update(|s, _| s.play_in(&first, &files));
    run(&mut rt, 0.05);
    let old = rt.state.queue.as_ref().unwrap().generation;
    let late = PlayerEvent::PrefetchNext {
        index: 1,
        id: crate::queue::item_id(old, 1),
    };
    // …but ▶ on another album comes first.
    let (first, files) = album(&rt, FERRY);
    rt.update(|s, _| s.play_in(&first, &files));
    run(&mut rt, 0.05);
    let q = rt.state.queue.as_ref().unwrap();
    assert_ne!(q.generation, old);
    assert!(
        q.items[1].ticket.is_none(),
        "Night Ferry's track 2 not asked for yet"
    );
    let before = fetches(&rt);
    rt.update(|s, _| s.apply_player(late.clone()));
    assert_eq!(fetches(&rt), before, "the old album's prefetch is dropped");
    assert!(rt.state.queue.as_ref().unwrap().items[1].ticket.is_none());
    // A stale TrackChanged doesn't move the new queue either.
    rt.update(|s, _| {
        s.apply_player(PlayerEvent::TrackChanged {
            index: 3,
            id: crate::queue::item_id(old, 3),
        })
    });
    assert_eq!(rt.state.queue.as_ref().unwrap().index, 0);
    // The current queue's own prefetch still works.
    let gen_now = rt.state.queue.as_ref().unwrap().generation;
    rt.update(|s, _| {
        s.apply_player(PlayerEvent::PrefetchNext {
            index: 1,
            id: crate::queue::item_id(gen_now, 1),
        })
    });
    assert_eq!(fetches(&rt), before + 1);
    // With no queue at all (stopped), nothing happens.
    rt.update(|s, _| s.stop_current());
    let before = fetches(&rt);
    rt.update(|s, _| s.apply_player(late));
    assert_eq!(fetches(&rt), before);
}

// ---- natural file order inside a folder ---------------------------------------------------------

#[test]
fn natural_order_by_number_disc_and_case() {
    use crate::tabs::natural_cmp;
    let mut v = vec![
        "10 Ten.flac",
        "2 Two.flac",
        "1-10 x.flac",
        "2-01 y.flac",
        "1-02 z.flac",
        "b.flac",
        "A.flac",
        "01 One.flac",
        "track9.mp3",
        "Track10.mp3",
    ];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(
        v,
        [
            "01 One.flac",
            "1-02 z.flac",
            "1-10 x.flac",
            "2 Two.flac",
            "2-01 y.flac",
            "10 Ten.flac",
            "A.flac",
            "b.flac",
            "track9.mp3",
            "Track10.mp3",
        ]
    );
    assert!(natural_cmp("2", "10").is_lt() && natural_cmp("1-02", "2-01").is_lt());
    assert!(natural_cmp("abc", "ABD").is_lt(), "case-insensitive");
    assert!(
        natural_cmp("02 a", "2 a").is_ne(),
        "a total order: ties fall back to bytes"
    );
}

/// Hit indices of a search folder group's rows, top to bottom as drawn.
fn drawn_rows(rt: &Runtime<App>, idx: &[usize]) -> Vec<usize> {
    let mut v: Vec<(f32, usize)> = idx
        .iter()
        .map(|&i| {
            (
                rt.presented(&format!("row-{i}")).expect("row drawn").0[1],
                i,
            )
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, i)| i).collect()
}

#[test]
fn files_in_a_search_folder_are_in_track_order_and_groups_keep_rank_order() {
    let rt = searched();
    let st = rt.state.tab().search().unwrap();
    let v = st.search.as_ref().unwrap();
    let groups = v.groups(&st.filters);
    // Groups still follow ranking: users by availability, alice's folders as they arrived.
    let users: Vec<_> = groups.iter().map(|g| g.username.as_str()).collect();
    assert_eq!(&users[..3], ["alice", "vinyl_rips", "dj_ivo"]);
    assert_eq!(groups[0].folders[0].0, COAST);
    assert_eq!(groups[0].folders[1].0, FERRY);
    // The hits arrive out of track order (03, 06, 01, …); the rows read 01…06.
    let coast = &groups[0].folders[0].1;
    let arrival: Vec<_> = {
        let mut a = coast.clone();
        a.sort();
        a.iter()
            .map(|&i| v.hits[i].file.basename().to_string())
            .collect()
    };
    assert_ne!(
        arrival[0], "01 Northern Glass.flac",
        "the demo really arrives shuffled"
    );
    let names: Vec<_> = drawn_rows(&rt, coast)
        .into_iter()
        .map(|i| v.hits[i].file.basename().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "01 Northern Glass.flac",
            "02 Salt Roads.flac",
            "03 Low Tide Radio.flac",
            "04 Breakwater.flac",
            "05 A Lamp in the Dunes.flac",
            "06 Coastline.flac",
        ]
    );
}

#[test]
fn the_album_queue_from_a_search_group_plays_in_the_rows_order() {
    let mut rt = searched();
    let st = rt.state.tab().search().unwrap();
    let v = st.search.as_ref().unwrap();
    let coast = v.groups(&st.filters)[0].folders[0].1.clone();
    let rows: Vec<_> = drawn_rows(&rt, &coast)
        .into_iter()
        .map(|i| v.hits[i].file.clone())
        .collect();
    // ▶ on the row drawn 3rd: the queue is the rows in order, starting there.
    let third = rows[2].clone();
    let (i, _) = row_of(&rt, "alice", third.basename());
    click(&mut rt, &format!("row-{i}/play"));
    let q = rt.state.queue.as_ref().unwrap();
    let queued: Vec<_> = q.items.iter().map(|it| it.file.clone()).collect();
    assert_eq!(
        queued, rows,
        "same order as the rows, the .jpg/.log left out"
    );
    assert_eq!(q.index, 2);
    // "Play folder" builds the same queue.
    rt.update(|s, _| s.play_folder("alice", COAST));
    let q = rt.state.queue.as_ref().unwrap();
    assert_eq!(
        q.items.iter().map(|it| it.file.clone()).collect::<Vec<_>>(),
        rows
    );
}

#[test]
fn user_tabs_list_files_in_the_same_natural_order() {
    let mut rt = signed_in();
    let mk = |n: &str| wholeseek_core::RemoteFile {
        username: "zed".into(),
        path: format!("A\\{n}"),
        size: 1,
        attrs: Default::default(),
    };
    rt.update(|s, _| {
        s.open_user("zed");
        s.apply(wholeseek_core::CoreEvent::BrowseResult {
            username: "zed".into(),
            dirs: vec![wholeseek_core::BrowseDir {
                path: "A".into(),
                files: ["10 c.flac", "2 b.flac", "1 A.flac", "cover.JPG"]
                    .map(mk)
                    .to_vec(),
            }],
        });
    });
    run(&mut rt, 0.3);
    let b = rt.state.tab().browse().unwrap();
    let names: Vec<_> = b.dirs.as_ref().unwrap()[0]
        .files
        .iter()
        .map(|f| f.basename())
        .collect();
    assert_eq!(names, ["1 A.flac", "2 b.flac", "10 c.flac", "cover.JPG"]);
    let q: Vec<_> = rt
        .state
        .folder_files("zed", "A")
        .iter()
        .map(|f| f.basename().to_string())
        .collect();
    assert_eq!(q, names, "the queue uses the listing's order");
}

// ---- wide windows: a capped table and the details pane ------------------------------------------

/// The "hollow lights" results in a `w`-point-wide window.
fn searched_at(w: f32) -> Runtime<App> {
    let mut rt = Runtime::new(
        folders_view(App::demo()),
        view::view,
        (w, 900.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.state.demo_login();
    run(&mut rt, 1.0);
    rt.update(|s, _| s.search_for("hollow lights"));
    run(&mut rt, 1.6);
    rt
}

fn esc(rt: &mut Runtime<App>) {
    press(
        rt,
        KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
    );
}

fn rect_of(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no {key}"))
        .rect
}

#[test]
fn the_inspector_opens_on_a_selection_and_closes_with_x_or_esc() {
    use crate::view::{INSPECTOR_W, RESULTS_MAX_W};
    let mut rt = searched_at(1920.0);
    assert!(
        rt.find("details").is_none(),
        "nothing selected: no inspector"
    );
    assert_eq!(rect_of(&rt, "results")[2], 1920.0, "results get the window");
    let row0 = rect_of(&rt, "row-0");
    assert!(
        row0[0] + row0[2] <= RESULTS_MAX_W + 0.5,
        "the table is capped"
    );

    click(&mut rt, "folder-0-0");
    let d = rect_of(&rt, "details");
    assert_eq!((d[0], d[2]), (1920.0 - INSPECTOR_W, INSPECTOR_W));
    assert_eq!(rect_of(&rt, "row-0"), row0, "the table doesn't move");
    assert!(
        d[0] - (row0[0] + row0[2]) <= 150.0,
        "the table reaches (nearly) to the inspector: gap {}",
        d[0] - (row0[0] + row0[2])
    );

    click(&mut rt, "details-close");
    assert!(rt.find("details").is_none());
    assert_eq!(rt.state.tab().group, None);
    run(&mut rt, 0.5);
    assert!(rt.find("details").is_none(), "stays closed");

    click(&mut rt, "folder-0-1");
    assert!(rt.find("details").is_some(), "the next selection opens it");
    esc(&mut rt);
    assert!(rt.find("details").is_none(), "Esc closes it");

    // Narrower: beside the results down to 1100 pt, over them below.
    for (w, overlay) in [
        (1200.0, false),
        (1280.0, false),
        (1000.0, true),
        (900.0, true),
    ] {
        let mut rt = searched_at(w);
        click(&mut rt, "folder-0-0");
        let d = rect_of(&rt, "details");
        assert_eq!(d[0] + d[2], w, "{w}: at the right edge");
        let results = rect_of(&rt, "results")[2];
        if overlay {
            assert_eq!(results, w, "{w}: over the results");
        } else {
            assert_eq!(results, w - INSPECTOR_W - 1.0, "{w}: beside the results");
        }
        assert_nothing_clipped(&rt, w);
    }
}

#[test]
fn a_one_file_folder_gets_a_compact_card() {
    let rt = jungle_at(1920.0);
    assert!(rt.find("details-card").is_some());
    assert_eq!(
        text_of(&rt, "details-title").as_deref(),
        Some("Put It On (Jungle Mix).mp3")
    );
    assert!(rt.find("track-0").is_none(), "no one-row track list");
    for k in [
        "details-play",
        "details-download",
        "details-download-to",
        "details-browse",
    ] {
        assert!(rt.find(k).is_some(), "{k}");
    }
}

#[test]
fn the_card_downloads_the_lone_file_flat() {
    let mut rt = jungle_at(1920.0);
    click(&mut rt, "details-download");
    let f = sv(&rt).hits[0].file.clone();
    assert!(
        calls(&rt).contains(&Call::Download(f)),
        "a plain ↓, not an album file"
    );
}

#[test]
fn previously_downloaded_files_are_marked_in_rows_and_the_inspector() {
    let mut rt = searched_at(1920.0);
    // The demo's history has dj_ivo's "Low Tide Radio" mp3 only (exact user + path).
    let marked: Vec<String> = rt
        .elements()
        .into_iter()
        .filter(|e| e.key.as_deref() == Some("downloaded"))
        .map(|e| e.path)
        .collect();
    let (i, _) = row_of(&rt, "dj_ivo", "Hollow Lights - Low Tide Radio.mp3");
    assert_eq!(marked.len(), 1, "{marked:?}");
    assert!(marked[0].contains(&format!("row-{i}/")), "{marked:?}");
    click(&mut rt, &format!("row-{i}"));
    let in_pane = rt
        .elements()
        .into_iter()
        .filter(|e| e.key.as_deref() == Some("downloaded") && e.path.contains("/details/"))
        .count();
    assert_eq!(in_pane, 1);
}

#[test]
fn the_inspector_shows_what_plays_and_keeps_the_album_folder_on_download() {
    let mut rt = searched_at(1920.0);
    click(&mut rt, "folder-0-0");
    let (_, f) = row_of(&rt, "alice", "01 Northern Glass.flac");
    rt.update(|s, _| {
        let q = s.folder_files("alice", COAST);
        s.play_in(&f, &q);
    });
    run(&mut rt, 1.0);
    let eq = rt.elements().into_iter().any(|e| {
        e.path.contains("/details/")
            && e.path.contains("track-0/")
            && e.key.as_deref() == Some("eq")
    });
    assert!(eq, "the playing track shows the equalizer");
    let (_, g) = row_of(&rt, "alice", "03 Low Tide Radio.flac");
    click(&mut rt, "track-2/download");
    assert!(
        calls(&rt).contains(&Call::DownloadAlbumFile(g, None)),
        "{:?}",
        calls(&rt)
    );
}

#[test]
fn clicking_a_row_or_folder_selects_its_group_per_tab_without_playing() {
    let mut rt = searched_at(1920.0);
    let fetches = count_calls(&rt, |c| matches!(c, Call::PlayFetch(_)));
    let (i, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    click(&mut rt, &format!("row-{i}"));
    assert_eq!(rt.state.tab().group, Some(("alice".into(), COAST.into())));
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::PlayFetch(_))),
        fetches,
        "select, not play"
    );
    assert!(rt.state.playing.is_none());
    assert_eq!(
        text_of(&rt, "details-title").as_deref(),
        Some("Coastline (2019)")
    );
    // The pane lists the folder (as the results know it) in track order, with totals.
    let names: Vec<_> = (0..8)
        .filter_map(|k| {
            rt.elements()
                .into_iter()
                .find(|e| e.path.ends_with(&format!("track-{k}/name")))
                .and_then(|e| e.text)
        })
        .collect();
    assert_eq!(names[0], "01 Northern Glass.flac");
    assert_eq!(names[5], "06 Coastline.flac");
    assert_eq!(names.len(), 6);
    assert_eq!(
        text_of(&rt, "details-summary").as_deref(),
        Some("6 files · 25:12 · 158.9 MB · FLAC")
    );
    // Enter on the selected row still plays.
    rt.update(|_, tx| tx.focus(format!("row-{i}"), false));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
    );
    assert_eq!(rt.state.playing.as_ref().map(|p| p.file.clone()), Some(f));

    // A folder line selects that folder; another tab keeps its own selection.
    click(&mut rt, "folder-0-1");
    assert_eq!(rt.state.tab().group, Some(("alice".into(), FERRY.into())));
    click(&mut rt, "new-tab");
    rt.update(|s, _| s.search_for("hollow lights"));
    run(&mut rt, 1.6);
    assert_eq!(rt.state.tab().group, None);
    assert!(rt.find("details").is_none());
    let (j, _) = row_of(&rt, "dj_ivo", "Hollow Lights - Salt Roads.mp3");
    click(&mut rt, &format!("row-{j}"));
    assert_eq!(rt.state.tab().group.as_ref().unwrap().0, "dj_ivo");
    click(&mut rt, "tab-0");
    assert_eq!(rt.state.tab().group, Some(("alice".into(), FERRY.into())));
    assert_eq!(
        text_of(&rt, "details-title").as_deref(),
        Some("Night Ferry EP (2021)")
    );
}

#[test]
fn pane_actions_do_what_the_folder_menu_does() {
    // Each pane button against the same item picked from the folder's ⋯ menu.
    for (button, item) in [
        ("details-download", "menu-download-folder"),
        ("details-play", "menu-play-folder"),
        ("details-browse", "menu-open-folder"),
    ] {
        let outcome = |via_pane: bool| {
            let mut rt = searched_at(1920.0);
            click(&mut rt, "folder-0-0");
            let before = calls(&rt).len();
            if via_pane {
                click(&mut rt, button);
            } else {
                let (x, y) = rt.find("folder-0-0").unwrap();
                rt.update(|s, _| {
                    let items = s.folder_menu("alice", COAST);
                    s.open_menu(crate::menu::MenuAnchor::Point(x, y), items);
                });
                run(&mut rt, 0.05);
                click(&mut rt, item);
            }
            run(&mut rt, 0.1);
            let q: Vec<_> = rt
                .state
                .queue
                .as_ref()
                .map(|q| q.items.iter().map(|i| i.file.path.clone()).collect())
                .unwrap_or_default();
            let tab = rt
                .state
                .tab()
                .browse()
                .map(|b| (b.username.clone(), b.selected.clone()));
            (calls(&rt)[before..].to_vec(), q, tab, rt.state.tabs.len())
        };
        assert_eq!(outcome(true), outcome(false), "{button} = {item}");
    }
}

#[test]
fn play_album_only_shows_for_a_folder_with_something_playable() {
    let mut rt = searched_at(1920.0);
    let opus = wholeseek_core::RemoteFile {
        username: "zed".into(),
        path: r"x\only.opus".into(),
        size: 1,
        attrs: Default::default(),
    };
    rt.update(|s, _| {
        let tab = s.tab_mut();
        tab.group = Some(("zed".into(), "x".into()));
        tab.search_mut()
            .unwrap()
            .search
            .as_mut()
            .unwrap()
            .hits
            .push(wholeseek_core::SearchHit {
                file: opus,
                free_slot: true,
                speed: 1,
            });
    });
    run(&mut rt, 0.05);
    assert!(rt.find("details-download").is_some() && rt.find("details-browse").is_some());
    assert!(rt.find("details-play").is_none(), "nothing there decodes");
    click(&mut rt, "folder-0-0");
    assert!(rt.find("details-play").is_some());
}

// ---- "Filter results": narrowing the received results -------------------------------------------

fn hit_at(user: &str, path: &str) -> wholeseek_core::SearchHit {
    wholeseek_core::SearchHit {
        file: wholeseek_core::RemoteFile {
            username: user.into(),
            path: path.into(),
            size: 1,
            attrs: Default::default(),
        },
        free_slot: true,
        speed: 1,
    }
}

#[test]
fn filter_words_match_word_prefixes_of_path_and_user() {
    use crate::tabs::{TextQuery, search_key};
    let key = search_key(&hit_at(
        "DJ_Ivo",
        r"Música\Beyoncé - Crazy in Love\01 Crazy_in-Love (Remix).FLAC",
    ));
    let m = |q: &str| TextQuery::parse(q).matches(&key);
    assert!(m("craz"), "prefix");
    assert!(m("CRAZY love"), "case-insensitive, every word, any order");
    assert!(!m("razy"), "not inside a word");
    assert!(m("beyonce") && m("musica"), "accents folded");
    assert!(m("beyoncé"), "accented query too");
    assert!(
        m("in-love remix"),
        "punctuation is a separator in the query"
    );
    assert!(m("dj ivo") && m("dj_iv"), "the username counts");
    assert!(m("flac") && m("01"), "extension and track number");
    assert!(!m("crazy jazz"), "all words must match");
    assert!(m("crazy -live") && !m("crazy -remix"), "-word excludes");
    assert!(m("-"), "a lone dash is no word");
    assert!(TextQuery::parse("  ").is_empty());
    let apos = search_key(&hit_at("u", r"x\Wazn't Me.mp3"));
    assert!(
        TextQuery::parse("waznt").matches(&apos) && TextQuery::parse("wazn't me").matches(&apos)
    );
}

#[test]
fn match_spans_mark_the_matched_prefix_in_the_original_text() {
    use crate::tabs::TextQuery;
    let q = TextQuery::parse("coast beyonce");
    assert_eq!(q.spans("06 Coastline.flac"), [(3, 8)]);
    let t = "Beyoncé — Halo";
    assert_eq!(
        q.spans(t),
        [(0, t.find(" —").unwrap())],
        "the whole folded word, é included"
    );
    assert!(
        TextQuery::parse("-coast").spans("Coastline").is_empty(),
        "excluded words aren't marked"
    );
}

fn type_find(rt: &mut Runtime<App>, text: &str) {
    rt.update(|s, _| {
        s.tab_mut().search_mut().unwrap().find.replace_text(text);
        s.find_changed();
    });
    run(rt, 0.05);
}

#[test]
fn the_filter_box_narrows_results_with_chips_and_counts() {
    let mut rt = searched();
    let searches = count_calls(&rt, |c| matches!(c, Call::Search(_)));
    assert!(rt.find("filter").is_some() && rt.find("find-count").is_none());
    type_find(&mut rt, "coast");
    let st = rt.state.tab().search().unwrap();
    let v = st.search.as_ref().unwrap();
    let f = st.shown_filters();
    assert_eq!(v.visible(&f), 10);
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("10 of 20"));
    // Groups without a matching row are gone; dj_ivo's MP3s don't say "coast".
    assert!(v.groups(&f).iter().all(|g| g.username != "dj_ivo"));
    // Chips count the filtered set, and a format left with nothing has no chip.
    assert_eq!(
        text_of(&rt, "fmt-flac/n")
            .or_else(|| {
                rt.elements()
                    .into_iter()
                    .find(|e| e.path.ends_with("fmt-flac/n"))
                    .and_then(|e| e.text)
            })
            .as_deref(),
        Some("8")
    );
    assert!(rt.find("fmt-mp3").is_none(), "no 0 chip");
    // Composes with chips: WAV ∩ "coast".
    click(&mut rt, "fmt-wav");
    let st = rt.state.tab().search().unwrap();
    assert_eq!(st.search.as_ref().unwrap().visible(&st.shown_filters()), 2);
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("2 of 20"));
    // The tab badge keeps the total under the chips only (not the filter box).
    assert_eq!(
        rt.state.tab().search().unwrap().filters.text,
        Default::default()
    );
    // New results keep being filtered.
    rt.update(|s, _| {
        let id = s.tab().search().unwrap().search.as_ref().unwrap().id;
        s.apply(wholeseek_core::CoreEvent::SearchResults {
            search: id,
            hits: vec![
                hit_at("zed", r"Coast\x.wav"),
                hit_at("zed", r"Inland\y.wav"),
            ],
        });
    });
    run(&mut rt, 0.05);
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("3 of 22"));
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Search(_))),
        searches,
        "all local"
    );
    click(&mut rt, "filter-clear");
    assert!(rt.state.tab().search().unwrap().find.text.is_empty());
    assert!(rt.find("find-count").is_none());
}

#[test]
fn nothing_matching_shows_one_line_and_clear_filter() {
    let mut rt = searched();
    type_find(&mut rt, "xyz");
    assert_eq!(
        text_of(&rt, "nothing-matches-title").as_deref(),
        Some("Nothing matches “xyz”")
    );
    assert!(rt.find("columns").is_none(), "no empty table header");
    assert!(
        rt.find("filter").is_some(),
        "the box stays to fix the words"
    );
    click(&mut rt, "clear-filter");
    assert!(rt.state.tab().search().unwrap().find.text.is_empty());
    assert!(rt.find("row-0").is_some() && rt.find("nothing-matches").is_none());
}

#[test]
fn zero_results_hide_the_tools_and_offer_search_again() {
    let mut rt = signed_in();
    rt.update(|s, _| s.search_for("Wazn't Me"));
    run(&mut rt, 0.05);
    // Still searching: the progress line only.
    assert!(rt.find("searching").is_some());
    assert!(rt.find("no-results-title").is_none() && rt.find("chips").is_none());
    run(&mut rt, 12.0);
    assert_eq!(
        text_of(&rt, "no-results-title").as_deref(),
        Some("No results for “Wazn't Me”")
    );
    for gone in [
        "chips",
        "fmt-flac",
        "free-slot",
        "filter",
        "columns",
        "search-bar/count",
    ] {
        assert!(
            rt.find(gone).is_none(),
            "{gone} hidden with nothing to show"
        );
    }
    let before = count_calls(&rt, |c| matches!(c, Call::Search(_)));
    click(&mut rt, "search-again");
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Search(_))),
        before + 1
    );
    assert_eq!(
        calls(&rt)
            .iter()
            .rev()
            .find(|c| matches!(c, Call::Search(_))),
        Some(&Call::Search("Wazn't Me".into()))
    );
}

#[test]
fn the_filter_is_per_tab_and_a_new_tab_starts_without_it() {
    let mut rt = searched();
    type_find(&mut rt, "coast");
    click(&mut rt, "new-tab");
    assert!(rt.state.tab().search().unwrap().find.text.is_empty());
    rt.update(|s, _| s.search_for("hollow lights"));
    run(&mut rt, 1.6);
    assert!(rt.find("find-count").is_none());
    click(&mut rt, "tab-0");
    assert_eq!(rt.state.tab().search().unwrap().find.text, "coast");
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("10 of 20"));
}

#[test]
fn the_details_pane_drops_a_folder_the_filter_hides() {
    let mut rt = searched_at(1920.0);
    click(&mut rt, "folder-0-0"); // alice's Coastline
    type_find(&mut rt, "coast");
    assert_eq!(
        rt.state.tab().group,
        Some(("alice".into(), COAST.into())),
        "still listed"
    );
    type_find(&mut rt, "ferry");
    assert_eq!(rt.state.tab().group, None, "filtered out: cleared");
    assert!(rt.find("details").is_none());
    // Chips can hide it too.
    type_find(&mut rt, "");
    click(&mut rt, "folder-0-0");
    click(&mut rt, "fmt-mp3");
    assert_eq!(rt.state.tab().group, None);
}

#[test]
fn filter_keys_find_select_all_escape_and_down() {
    let mut rt = searched();
    type_find(&mut rt, "coast");
    rt.update(|_, tx| tx.blur());
    press(&mut rt, ctrl(Key::Command('f')));
    assert_eq!(
        focused(&rt).as_deref(),
        Some("filter"),
        "⌘/Ctrl+F: the filter"
    );
    let st = rt.state.tab().search().unwrap();
    assert_eq!((st.find.anchor, st.find.focus), (0, 5), "all selected");
    // ↓ goes into the results, on the first row shown.
    press(
        &mut rt,
        KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        },
    );
    let first = {
        let st = rt.state.tab().search().unwrap();
        let g = st.search.as_ref().unwrap().groups(&st.shown_filters());
        format!("row-{}", g[0].folders[0].1[0])
    };
    assert_eq!(focused(&rt), Some(first));
    // Esc: clears, then a second Esc leaves the box. The query is never touched.
    press(&mut rt, ctrl(Key::Command('f')));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
    );
    assert!(rt.state.tab().search().unwrap().find.text.is_empty());
    assert_eq!(focused(&rt).as_deref(), Some("filter"));
    press(
        &mut rt,
        KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
    );
    assert_ne!(focused(&rt).as_deref(), Some("filter"));
    assert_eq!(rt.state.tab().search().unwrap().edit.text, "hollow lights");
    // ⌘L still goes to the query; with no results ⌘F does too.
    press(&mut rt, ctrl(Key::Command('l')));
    assert_eq!(focused(&rt).as_deref(), Some("query"));
    click(&mut rt, "new-tab");
    rt.update(|_, tx| tx.blur());
    press(&mut rt, ctrl(Key::Command('f')));
    assert_eq!(focused(&rt).as_deref(), Some("query"));
}

#[test]
fn tab_order_is_query_filter_chips_results() {
    let mut rt = searched();
    rt.update(|_, tx| tx.focus("query", true));
    let mut order = Vec::new();
    for _ in 0..4 {
        press(
            &mut rt,
            KeyEvent {
                key: Key::Tab,
                mods: Mods::default(),
            },
        );
        order.push(focused(&rt).unwrap_or_default());
    }
    assert_eq!(order[0], "filter", "{order:?}");
    assert_eq!(order[1], "sort", "{order:?}");
    assert!(order[2].starts_with("fmt-"), "{order:?}");
}

#[test]
fn filtering_5000_results_takes_under_16_ms_per_keystroke() {
    use crate::tabs::{Filters, SearchView, TextQuery};
    let words = [
        "coast", "salt", "harbour", "tide", "glass", "ferry", "dune", "lamp", "night", "road",
    ];
    let mut v = SearchView {
        id: wholeseek_core::SearchId(1),
        query: "x".into(),
        hits: (0..5000)
            .map(|i| {
                hit_at(
                    &format!("user_{}", i % 300),
                    &format!(
                        r"Music\Artist {}\{} {} ({})\{:02} {} {}.flac",
                        i % 97,
                        words[i % 10],
                        words[(i / 10) % 10],
                        1990 + i % 30,
                        i % 14,
                        words[(i / 7) % 10],
                        words[(i / 3) % 10]
                    ),
                )
            })
            .collect(),
        finished: true,
        keys: Vec::new(),
        matched: Default::default(),
    };
    v.reindex(); // once, as results arrive
    // Each keystroke's time is the median of several runs, so one preempted run under a
    // loaded parallel test run doesn't count. The 16 ms budget is a release build's; a debug
    // build only guards against gross regressions.
    let budget = std::time::Duration::from_millis(if cfg!(debug_assertions) { 250 } else { 16 });
    let mut worst = std::time::Duration::ZERO;
    for typed in [
        "c",
        "co",
        "coa",
        "coas",
        "coast",
        "coast s",
        "coast sa",
        "coast salt -1999",
    ] {
        let mut runs: Vec<std::time::Duration> = (0..7)
            .map(|_| {
                v.matched.take(); // a fresh keystroke: nothing cached
                let f = Filters {
                    text: TextQuery::parse(typed),
                    ..Default::default()
                };
                let t0 = std::time::Instant::now();
                let shown = v.visible(&f);
                let groups = v.groups(&f);
                let chips = v.format_counts(&f);
                std::hint::black_box((shown, groups, chips));
                t0.elapsed()
            })
            .collect();
        runs.sort();
        worst = worst.max(runs[runs.len() / 2]);
    }
    eprintln!("filter 5000 rows: slowest keystroke (median of 7) {worst:?}");
    assert!(
        worst < budget,
        "slowest keystroke {worst:?}, budget {budget:?}"
    );
}

// ---- icons, Download to…, album folders -------------------------------------------------------

#[test]
fn every_icon_draws_at_menu_and_row_sizes() {
    for i in crate::view::icons::ALL {
        for px in [12, 14, 28] {
            let cov = rune::text::icon_coverage(i, px);
            let inked = cov.iter().filter(|&&c| c > 0).count();
            assert!(inked > 0, "{} draws nothing at {px} px", i.name);
            assert!(inked < cov.len(), "{} fills its box at {px} px", i.name);
        }
    }
}

#[test]
fn every_menu_item_has_an_icon() {
    let mut rt = searched();
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let recent = [std::path::PathBuf::from("/music/vinyl")];
    let mut items = crate::menu::file_items(&f, false, &recent);
    items.extend(crate::menu::folder_items("alice", COAST, true, &recent));
    rt.update(|s, _| s.download(&f));
    run(&mut rt, 30.0);
    let id = rt.state.transfers.iter().find(|t| t.file == f).unwrap().id;
    items.extend(rt.state.transfer_items(id));
    for it in &items {
        assert!(it.icon.is_some(), "{} has no icon", it.id);
    }
    // And the open menu draws them (an icon element before each label).
    rt.update(|s, _| s.open_menu(crate::menu::MenuAnchor::Point(100.0, 100.0), items.clone()));
    run(&mut rt, 0.05);
    let all = rt.elements();
    let menu = all
        .iter()
        .find(|e| e.key.as_deref() == Some("menu"))
        .expect("menu open")
        .path
        .clone();
    let icons = all
        .iter()
        .filter(|e| e.path.starts_with(&menu) && e.role == "icon")
        .count();
    assert_eq!(icons, items.len());
}

fn demo_settings(rt: &Runtime<App>) -> wholeseek_core::Settings {
    rt.state.backend.settings().clone()
}

#[test]
fn download_to_a_recent_folder_saves_there_and_remembers_it() {
    let mut rt = searched();
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let (vinyl, usb) = (
        std::path::PathBuf::from("/music/vinyl"),
        std::path::PathBuf::from("/usb"),
    );
    rt.update(|s, _| {
        s.run_menu(MenuAction::DownloadTo {
            target: crate::menu::DlTarget::Folder {
                username: "alice".into(),
                folder: COAST.into(),
            },
            dir: Some(usb.clone()),
        });
        s.download_to(&crate::menu::DlTarget::File(f.clone()), &vinyl);
    });
    run(&mut rt, 0.05);
    assert!(calls(&rt).contains(&Call::DownloadTo(f.clone(), vinyl.clone())));
    assert!(calls(&rt).contains(&Call::DownloadFolderTo(
        "alice".into(),
        COAST.into(),
        usb.clone()
    )));
    // Most recent first, and saved right away.
    assert_eq!(
        demo_settings(&rt).recent_download_dirs,
        [vinyl.clone(), usb.clone()]
    );
    assert!(store_calls(&rt).iter().any(|c| matches!(c,
        StoreCall::SaveSettings(s) if s.recent_download_dirs == [vinyl.clone(), usb.clone()])));
    // A single song goes flat by default; the folder keeps its name.
    let dest = |name: &str| {
        rt.state
            .transfers
            .iter()
            .find(|t| t.file.basename() == name && t.file.username == "alice")
            .and_then(|t| t.dest.clone())
    };
    assert_eq!(dest("02 Salt Roads.flac"), Some(vinyl.clone()));
    assert_eq!(
        dest("01 Northern Glass.flac"),
        Some(usb.join("Coastline (2019)"))
    );
    // The menus now offer them, under a line, by path, newest first.
    let items = crate::menu::file_items(&f, false, &demo_settings(&rt).recent_download_dirs);
    let to: Vec<&str> = items
        .iter()
        .filter(|i| i.id.starts_with("download-to"))
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(to, ["Download to…", "/music/vinyl", "/usb"]);
    let first = items.iter().find(|i| i.id == "download-to-0").unwrap();
    assert!(first.sep);
    assert_eq!(first.help.as_deref(), Some("/music/vinyl"));
    let after = items.iter().find(|i| i.id == "download-folder").unwrap();
    assert!(after.sep);
    // At most five.
    rt.update(|s, _| {
        for i in 0..7 {
            s.download_to(
                &crate::menu::DlTarget::Folder {
                    username: "alice".into(),
                    folder: COAST.into(),
                },
                std::path::Path::new(&format!("/d{i}")),
            );
        }
    });
    assert_eq!(
        demo_settings(&rt).recent_download_dirs.len(),
        wholeseek_core::Settings::RECENT_DOWNLOAD_DIRS
    );
    assert_eq!(
        demo_settings(&rt).recent_download_dirs[0],
        std::path::PathBuf::from("/d6")
    );
}

#[test]
fn keep_while_an_album_plays_keeps_the_album_folder_with_the_setting_off() {
    let mut rt = searched();
    assert!(
        !demo_settings(&rt).single_file_subfolder,
        "default: single songs flat"
    );
    let files = rt.state.folder_files("alice", COAST);
    let second = files[1].clone();
    rt.update(|s, _| s.play_in(&second, &files));
    run(&mut rt, 1.0);
    click(&mut rt, "keep");
    let root = demo_settings(&rt).download_dir;
    assert!(
        calls(&rt).contains(&Call::DownloadAlbumFile(second.clone(), None)),
        "{:?}",
        calls(&rt)
    );
    assert!(!calls(&rt).iter().any(|c| matches!(c, Call::Download(_))));
    let kept = rt.state.download_of(&second).unwrap();
    assert_eq!(kept.dest, Some(root.join("Coastline (2019)")));
    // A lone track is a plain ↓: flat.
    let (_, lone) = row_of(&rt, "nightbus", "hollow_lights_salt_roads_live.mp3");
    rt.update(|s, _| s.play(&lone));
    run(&mut rt, 1.0);
    click(&mut rt, "keep");
    assert!(calls(&rt).contains(&Call::Download(lone.clone())));
    assert_eq!(rt.state.download_of(&lone).unwrap().dest, Some(root));
}

#[test]
fn set_sort_orders_the_shown_tab_and_new_tabs_follow() {
    use crate::tabs::SortBy;
    let mut rt = searched();
    rt.update(|s, _| {
        s.run_menu(crate::menu::MenuAction::SetSort(SortBy::Size));
    });
    assert_eq!(rt.state.tab().search().unwrap().filters.sort, SortBy::Size);
    assert_eq!(rt.state.last_filters.sort, SortBy::Size);
    assert!(rt.state.menu.is_none());
}

#[test]
fn recent_folder_labels_shorten_home_and_elide_the_middle() {
    use crate::menu::dir_label;
    use std::path::Path;
    let home = Some(Path::new("/home/nadia"));
    let short = |p: &str| dir_label(Path::new(p), home);
    assert_eq!(short("/home/nadia/Music/Vinyl"), "~/Music/Vinyl");
    assert_eq!(short("/home/nadia"), "~");
    assert_eq!(short("/home/nadiab/x"), "/home/nadiab/x");
    assert_eq!(short("/media/usb/Music"), "/media/usb/Music");
    let long = short("/home/nadia/Music/Archive/Electronic/Drum and Bass/1995 Dubplates");
    assert_eq!(long.chars().count(), crate::menu::DIR_LABEL_MAX);
    assert!(long.starts_with("~/Music/Arch"), "{long}");
    assert!(long.ends_with("1995 Dubplates"), "{long}");
    assert!(long.contains('…'));
}

#[test]
fn the_players_download_item_keeps_an_album_track_in_its_folder() {
    let mut rt = searched();
    let files = rt.state.folder_files("alice", COAST);
    let third = files[2].clone();
    rt.update(|s, _| s.play_in(&third, &files));
    run(&mut rt, 1.0);
    let items = rt.state.player_items();
    let dl = items.iter().find(|i| i.id == "download").unwrap();
    assert_eq!(dl.action, crate::menu::MenuAction::Keep(third.clone()));
    rt.update(|s, _| {
        s.run_menu(crate::menu::MenuAction::Keep(third.clone()));
    });
    assert!(calls(&rt).contains(&Call::DownloadAlbumFile(third.clone(), None)));
    let root = demo_settings(&rt).download_dir;
    assert_eq!(
        rt.state.download_of(&third).unwrap().dest,
        Some(root.join("Coastline (2019)"))
    );
}

#[test]
fn the_album_folder_setting_is_a_switch_that_saves_and_moves_single_songs() {
    let mut rt = searched();
    click(&mut rt, "settings");
    let switch = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("set-single-subfolder"))
        .expect("switch");
    assert_eq!(switch.role, "switch");
    assert_eq!(
        switch.label.as_deref(),
        Some("Put single songs in their album folder")
    );
    click(&mut rt, "set-single-subfolder");
    assert!(demo_settings(&rt).single_file_subfolder);
    assert!(
        store_calls(&rt)
            .iter()
            .any(|c| matches!(c, StoreCall::SaveSettings(s) if s.single_file_subfolder))
    );
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    rt.update(|s, _| s.download(&f));
    let root = demo_settings(&rt).download_dir;
    assert_eq!(
        rt.state.download_of(&f).unwrap().dest,
        Some(root.join("Coastline (2019)"))
    );
    click(&mut rt, "set-single-subfolder");
    assert!(!demo_settings(&rt).single_file_subfolder);
}

#[test]
fn the_details_pane_offers_download_to_with_recent_folders() {
    let mut rt = searched_at(1920.0);
    click(&mut rt, "folder-0-0");
    assert!(rt.find("details-download-to").is_some());
    rt.update(|s, _| {
        s.download_to(
            &crate::menu::DlTarget::Folder {
                username: "kassi".into(),
                folder: "x".into(),
            },
            std::path::Path::new("/music/vinyl"),
        );
    });
    click(&mut rt, "details-download-to");
    let m = rt.state.menu.as_ref().expect("menu of folders");
    assert_eq!(m.ids(), ["download-folder-to", "download-folder-to-0"]);
    click(&mut rt, "menu-download-folder-to-0");
    assert!(calls(&rt).contains(&Call::DownloadFolderTo(
        "alice".into(),
        COAST.into(),
        "/music/vinyl".into()
    )));
}

/// A search like the user's "put it on jungle": long scene-release folder names, a 1-file
/// folder, many users.
fn jungle_at(w: f32) -> Runtime<App> {
    let mut rt = searched_at(w);
    let paths = [
        ("K1RZA", r"@@xkzqp\Music\Singles\Put It On (Jungle Mix).mp3"),
        (
            "divided1980",
            r"Music\VA_-_Liondub_and_Deekline_Present_Welcome_To_The_Jungle-(JC175ALBUM)-WEB-FLAC-2023\01-deekline_and_ed_solo-put_it_on_(jungle_mix).flac",
        ),
        (
            "divided1980",
            r"Music\VA_-_Liondub_and_Deekline_Present_Welcome_To_The_Jungle-(JC175ALBUM)-WEB-FLAC-2023\02-liondub-babylon_burning_(feat_general_levy)_(original_mix)_extended_version_for_djs.flac",
        ),
        (
            "a_user_with_a_rather_long_name_2024",
            r"D:\Shared\Drum & Bass\Jungle\1994 - 1996\Classics Collection Vol. 3 (Remastered 2019) [FLAC 24-96]\CD2\07 - Put It On (Original 1995 Dubplate Version).flac",
        ),
    ];
    rt.update(|s, _| {
        s.new_search_tab();
        let st = s.tab_mut().search_mut().unwrap();
        let mut v = crate::tabs::SearchView {
            id: wholeseek_core::SearchId(777),
            query: "put it on jungle".into(),
            hits: paths.iter().map(|(u, p)| hit_at(u, p)).collect(),
            finished: true,
            keys: Vec::new(),
            matched: Default::default(),
        };
        v.reindex();
        st.search = Some(v);
        st.edit.replace_text("put it on jungle");
        s.tab_mut().group = Some(("K1RZA".into(), r"@@xkzqp\Music\Singles".into()));
    });
    run(&mut rt, 0.1);
    rt
}

/// Every element of the window lies inside it horizontally and starts at x ≥ 0 (text that
/// doesn't fit truncates; nothing is pushed off the left edge).
fn assert_nothing_clipped(rt: &Runtime<App>, w: f32) {
    let bad: Vec<String> = rt
        .elements()
        .into_iter()
        .filter(|e| e.opacity > 0.0 && e.rect[2] > 0.0)
        .filter(|e| e.rect[0] < -0.5 || e.rect[0] + e.rect[2] > w + 0.5)
        .map(|e| format!("{} {:?} {:?}", e.path, e.rect, e.text))
        .collect();
    assert!(
        bad.is_empty(),
        "outside the window at {w}:\n{}",
        bad.join("\n")
    );
}

#[test]
fn long_paths_never_push_the_search_tab_off_screen() {
    let mut fails = Vec::new();
    for w in [1280.0, 1500.0, 1700.0, 1920.0, 2000.0] {
        let mut rt = jungle_at(w);
        let folders: Vec<(String, String)> = sv(&rt)
            .hits
            .iter()
            .map(|h| (h.file.username.clone(), h.file.folder().to_string()))
            .collect();
        for g in folders {
            rt.update(|s, _| s.tab_mut().group = Some(g.clone()));
            run(&mut rt, 0.1);
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_nothing_clipped(&rt, w)
            })) {
                fails.push(format!(
                    "{g:?}: {}",
                    e.downcast_ref::<String>().cloned().unwrap_or_default()
                ));
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n\n"));
}

#[test]
fn shrinking_a_wide_window_keeps_the_search_tab_inside_it() {
    let mut rt = jungle_at(2400.0);
    for w in [1920.0, 1700.0, 1600.0, 1280.0] {
        rt.set_viewport((w, 900.0), 1.0);
        run(&mut rt, 0.2);
        assert_nothing_clipped(&rt, w);
    }
}

/// The index of `f`'s row (`file-{i}`) in the shown user tab's open folder.
fn browse_row(rt: &Runtime<App>, f: &wholeseek_core::RemoteFile) -> usize {
    rt.state
        .tab()
        .browse()
        .and_then(|b| b.selected_dir())
        .and_then(|d| d.files.iter().position(|x| x.path == f.path))
        .expect("file listed in the user tab")
}

#[test]
fn the_playing_row_shows_in_every_tab_listing_it_even_with_other_attributes() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    // The same file as another listing reports it: other size and attributes.
    let mut g = f.clone();
    g.size += 1;
    g.attrs.bitrate_kbps = Some(1);
    rt.update(|s, _| s.play(&g));
    run(&mut rt, 0.3);
    assert!(rt.find(&format!("row-{i}/play/eq")).is_some(), "search row");
    rt.update(|s, _| s.open_folder_of(&f));
    run(&mut rt, 0.8);
    let j = browse_row(&rt, &f);
    assert!(
        rt.find(&format!("file-{j}/play/eq")).is_some(),
        "user tab row"
    );
}

#[test]
fn show_in_list_goes_back_to_the_tab_it_was_played_from() {
    let mut rt = searched();
    let (i, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    let search_id = rt.state.tab().id;
    rt.update(|s, _| s.play(&f));
    rt.update(|s, _| s.open_folder_of(&f));
    run(&mut rt, 0.8);
    assert_ne!(rt.state.tab().id, search_id);
    click(&mut rt, "now-title");
    let tab = rt.state.tab();
    assert_eq!(tab.id, search_id, "back to the search it was played from");
    assert_eq!(tab.selected.as_deref(), Some(f.path.as_str()));
    assert_eq!(
        tab.group,
        Some(("alice".to_string(), f.folder().to_string()))
    );
    assert!(!rt.state.reveal, "scrolled into view");
    assert!(rt.find(&format!("row-{i}")).is_some());
}

#[test]
fn show_in_list_with_its_tab_closed_uses_another_tab_or_opens_the_folder() {
    let mut rt = searched();
    let (_, f) = row_of(&rt, "alice", "02 Salt Roads.flac");
    rt.update(|s, _| s.play(&f));
    rt.update(|s, _| s.open_folder_of(&f));
    run(&mut rt, 0.8);
    let alice = rt.state.tab().id;
    rt.update(|s, _| s.open_user("vinyl_rips"));
    rt.update(|s, _| s.close_tab(0));
    run(&mut rt, 0.8);
    click(&mut rt, "now-title");
    assert_eq!(rt.state.tab().id, alice, "another tab listing it");
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.selected.as_deref(), Some(f.folder()));
    assert_eq!(b.highlight.as_deref(), Some(f.path.as_str()));
    assert!(!rt.state.reveal, "scrolled into view");

    // No tab lists it any more: its user's tab opens at its folder.
    rt.update(|s, _| {
        let i = s.tabs.iter().position(|t| t.id == alice).unwrap();
        s.close_tab(i);
    });
    click(&mut rt, "now-title");
    run(&mut rt, 0.8);
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.username, "alice");
    assert_eq!(b.selected.as_deref(), Some(f.folder()));
    assert_eq!(b.highlight.as_deref(), Some(f.path.as_str()));
    assert!(!rt.state.reveal);
    let j = browse_row(&rt, &f);
    assert!(rt.find(&format!("file-{j}/play/eq")).is_some());
}

// ---- search batch: broadened searches, wishlist, sort --------------------------------------------

#[test]
fn a_broadened_search_says_what_it_shows_and_keeps_the_typed_title() {
    let mut rt = searched_at(1280.0);
    assert!(
        rt.find("broadened").is_none(),
        "an ordinary search says nothing"
    );
    rt.update(|s, _| s.search_for("hollów lights"));
    run(&mut rt, 1.6);
    assert!(!sv(&rt).hits.is_empty(), "found by the broader query");
    assert_eq!(
        text_of(&rt, "broadened-to").as_deref(),
        Some("hollow lights")
    );
    assert_eq!(rt.state.tab().title(), "hollów lights");
    assert_nothing_clipped(&rt, 1280.0);
}

#[test]
fn the_star_puts_the_search_on_the_wishlist_and_saves() {
    let mut rt = searched_at(1280.0);
    click(&mut rt, "wish");
    assert!(calls(&rt).contains(&Call::WishlistAdd("hollow lights".into())));
    assert_eq!(demo_settings(&rt).wishlist, ["hollow lights"]);
    assert!(
        store_calls(&rt)
            .iter()
            .any(|c| matches!(c, StoreCall::SaveSettings(s) if s.wishlist == ["hollow lights"]))
    );
    assert!(
        rt.state.wished("Hollow  Lights"),
        "case and spaces don't matter"
    );
    click(&mut rt, "wish");
    assert!(calls(&rt).contains(&Call::WishlistRemove("hollow lights".into())));
    assert!(demo_settings(&rt).wishlist.is_empty());
    assert!(
        store_calls(&rt)
            .iter()
            .any(|c| matches!(c, StoreCall::SaveSettings(s) if s.wishlist.is_empty()))
    );
}

#[test]
fn sorting_orders_users_and_folders() {
    use crate::tabs::SortBy;
    let rt = searched_at(1280.0);
    let v = sv(&rt);
    let sorted_groups = |by: SortBy| {
        let mut g = v.groups(&crate::tabs::Filters::default());
        v.sort_groups(&mut g, by);
        g
    };
    let order = |by: SortBy| -> Vec<String> {
        sorted_groups(by)
            .iter()
            .flat_map(|g| {
                g.folders
                    .iter()
                    .map(|(d, _)| d.rsplit('\\').next().unwrap().to_string())
            })
            .collect()
    };
    let size = |d: &str| -> u64 {
        v.hits
            .iter()
            .filter(|h| h.file.folder().ends_with(d))
            .map(|h| h.file.size)
            .sum()
    };
    let by_size = order(SortBy::Size);
    let first_size = size(&by_size[0]);
    assert!(by_size.iter().all(|d| size(d) <= first_size), "{by_size:?}");
    let by_name = order(SortBy::Name);
    let groups = sorted_groups(SortBy::Name);
    let firsts: Vec<String> = groups
        .iter()
        .map(|g| g.folders[0].0.rsplit('\\').next().unwrap().to_lowercase())
        .collect();
    let mut sorted = firsts.clone();
    sorted.sort_by(|a, b| crate::tabs::natural_cmp(a, b));
    assert_eq!(firsts, sorted, "{by_name:?}");
    // Every order lists the same folders.
    let mut all = order(SortBy::Availability);
    all.sort();
    for by in SortBy::ALL {
        let mut o = order(by);
        o.sort();
        assert_eq!(o, all, "{by:?}");
        assert_eq!(SortBy::from_name(by.name()), Some(by));
    }
    // Relevance (the default): a folder whose file names hold the words beats one that
    // only has them in a parent folder.
    assert_eq!(crate::tabs::Filters::default().sort, SortBy::Relevance);
}

// ---- the album view ------------------------------------------------------------------------------

/// "hollow lights" in the album view, relevance order (the app's defaults).
fn albums_at(w: f32) -> Runtime<App> {
    let mut rt = searched_at(w);
    rt.update(|s, _| {
        s.last_filters.layout = crate::tabs::ResultsLayout::Albums;
        s.last_filters.sort = crate::tabs::SortBy::Relevance;
        let st = s.tab_mut().search_mut().unwrap();
        st.filters.layout = crate::tabs::ResultsLayout::Albums;
        st.filters.sort = crate::tabs::SortBy::Relevance;
    });
    run(&mut rt, 0.1);
    rt
}

/// The text of the element keyed `key` inside the element keyed `parent`.
fn text_in(rt: &Runtime<App>, parent: &str, key: &str) -> Option<String> {
    let p = format!("/{parent}/");
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && e.path.contains(&p))
        .and_then(|e| e.text)
}

fn album_titles(rt: &Runtime<App>) -> Vec<String> {
    (0..20)
        .filter_map(|i| text_in(rt, &format!("album-{i}"), "title"))
        .collect()
}

fn sort_label(rt: &Runtime<App>) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("sort"))
        .and_then(|e| e.label)
}

#[test]
fn albums_and_relevance_are_the_defaults() {
    let f = crate::tabs::Filters::default();
    assert_eq!(f.layout, crate::tabs::ResultsLayout::Albums);
    assert_eq!(f.sort, crate::tabs::SortBy::Relevance);
}

#[test]
fn the_album_view_merges_copies_and_acts_on_the_best_one() {
    let mut rt = albums_at(1920.0);
    let titles = album_titles(&rt);
    assert!(
        titles.contains(&"Coastline (2019)".to_string()),
        "{titles:?}"
    );
    assert!(rt.find("columns").is_none(), "no file columns over albums");
    assert!(rt.find("single-0").is_some(), "single files below");
    let i = titles.iter().position(|t| t == "Coastline (2019)").unwrap();
    // Click selects alice's copy: the inspector shows it.
    click(&mut rt, &format!("album-{i}"));
    assert_eq!(rt.state.tab().group, Some(("alice".into(), COAST.into())));
    assert_eq!(
        text_of(&rt, "details-title").as_deref(),
        Some("Coastline (2019)")
    );
    // ▶ plays the copy in track order; ↓ downloads its folder.
    click(&mut rt, &format!("album-{i}/play"));
    let q: Vec<String> = rt
        .state
        .queue
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|it| it.file.basename().to_string())
        .collect();
    assert_eq!(
        q.first().map(String::as_str),
        Some("01 Northern Glass.flac")
    );
    assert_eq!(q.len(), 6);
    click(&mut rt, &format!("album-{i}/download"));
    assert!(calls(&rt).contains(&Call::DownloadFolder("alice".into(), COAST.into())));
    assert_nothing_clipped(&rt, 1920.0);
}

#[test]
fn chips_and_the_filter_narrow_the_albums() {
    let mut rt = albums_at(1280.0);
    let all = album_titles(&rt).len();
    click(&mut rt, "fmt-mp3");
    let mp3 = album_titles(&rt);
    assert!(!mp3.is_empty() && mp3.len() < all, "{mp3:?}");
    for i in 0..mp3.len() {
        let meta = text_in(&rt, &format!("album-{i}"), "meta").unwrap();
        assert!(meta.starts_with("MP3"), "{meta}");
    }
    click(&mut rt, "fmt-mp3");
    type_find(&mut rt, "ferry");
    assert_eq!(album_titles(&rt), ["Night Ferry EP (2021)"]);
}

#[test]
fn the_inspector_lists_other_copies_and_switches_to_one() {
    let mut rt = albums_at(1920.0);
    let titles = album_titles(&rt);
    let i = titles
        .iter()
        .position(|t| t.contains("[24-96]"))
        .expect("the album two users have");
    assert!(
        text_in(&rt, &format!("album-{i}"), "meta")
            .unwrap()
            .ends_with("2 copies")
    );
    click(&mut rt, &format!("album-{i}"));
    let first = rt.state.tab().group.clone().unwrap();
    assert!(rt.find("details-copies").is_some());
    click(&mut rt, "copy-0");
    let second = rt.state.tab().group.clone().unwrap();
    assert_ne!(first.0, second.0, "the other user's copy");
    assert!(rt.find("details").is_some());
}

#[test]
fn the_sort_menu_orders_the_albums_and_new_tabs_follow() {
    let mut rt = albums_at(1280.0);
    assert_eq!(sort_label(&rt).as_deref(), Some("Sort: Relevance"));
    click(&mut rt, "sort");
    let m = rt.state.menu.as_ref().expect("sort menu");
    assert_eq!(
        m.ids(),
        [
            "sort-relevance",
            "sort-availability",
            "sort-size",
            "sort-length",
            "sort-name"
        ]
    );
    click(&mut rt, "menu-sort-name");
    let titles = album_titles(&rt);
    let mut sorted = titles.clone();
    sorted.sort_by(|a, b| crate::tabs::natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
    assert_eq!(titles, sorted);
    assert_eq!(sort_label(&rt).as_deref(), Some("Sort: Name"));
    // Folders, then a new tab: both choices carry over.
    click(&mut rt, "layout-folders");
    assert!(rt.find("album-0").is_none() && rt.find("folder-0-0").is_some());
    click(&mut rt, "new-tab");
    let f = &rt.state.tab().search().unwrap().filters;
    assert_eq!(
        (f.layout, f.sort),
        (
            crate::tabs::ResultsLayout::Folders,
            crate::tabs::SortBy::Name
        )
    );
}

// ---- transfers drawer: History (Panels) ------------------------------------------------------

fn history_open() -> Runtime<App> {
    let mut rt = signed_in();
    click(&mut rt, "transfers");
    click(&mut rt, "view-history");
    run(&mut rt, 0.1);
    rt
}

fn history_names(rt: &Runtime<App>) -> Vec<String> {
    rt.state
        .hist
        .rows
        .iter()
        .map(|r| crate::history::entry_file(&r.entry).basename().to_string())
        .collect()
}

fn history_row_of(rt: &Runtime<App>, name: &str) -> usize {
    history_names(rt)
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no history row {name}"))
}

#[test]
fn history_lists_past_downloads_newest_first_and_adds_each_that_ends() {
    let mut rt = history_open();
    assert_eq!(
        history_names(&rt),
        [
            "03 Quiet Engines.flac",
            "02 Wire Garden.flac",
            "01 Paper Moons.flac",
            "04 Breakwater (live).mp3",
            "03 Tin Roof.flac",
            "Hollow Lights - Low Tide Radio.mp3",
            "A1 Signals.flac",
        ]
    );
    assert!(rt.find("hist-0").is_some());
    // The deleted file is "missing"; finished ones on disk have no tag.
    let gone = history_row_of(&rt, "A1 Signals.flac");
    assert!(!rt.state.hist.rows[gone].on_disk);
    assert!(rt.find(&format!("hist-{gone}/tag")).is_some());
    assert!(rt.find("hist-0/tag").is_none());

    // A ↓ that finishes shows on top without reopening; a ▶ play never does.
    rt.state.search_for("hollow lights");
    run(&mut rt, 1.6);
    rt.update(|s, _| s.set_drawer_view(crate::history::DrawerView::History));
    let (_, f) = row_of(&rt, "alice", "01 Northern Glass.flac");
    let (_, p) = row_of(&rt, "alice", "02 Salt Roads.flac");
    rt.update(|s, _| s.play(&p));
    rt.update(|s, _| {
        s.download(&f);
    });
    run(&mut rt, 12.0);
    let names = history_names(&rt);
    assert_eq!(names[0], "01 Northern Glass.flac");
    assert!(
        !names.contains(&"02 Salt Roads.flac".to_string()),
        "plays aren't history"
    );
    assert_eq!(
        rt.state.hist.rows[0].entry.outcome,
        wholeseek_core::Outcome::Done
    );
}

#[test]
fn history_search_narrows_and_more_pages_in() {
    let mut rt = history_open();
    rt.update(|s, _| s.hist.find.replace_text("lantern"));
    run(&mut rt, 0.1);
    assert_eq!(rt.state.hist.rows.len(), 4);
    rt.update(|s, _| s.hist.find.replace_text("NIGHTBUS"));
    run(&mut rt, 0.1);
    assert_eq!(history_names(&rt), ["04 Breakwater (live).mp3"]);
    rt.update(|s, _| s.hist.find.replace_text("zzz"));
    run(&mut rt, 0.1);
    assert!(rt.find("no-history").is_some());
    assert!(!rt.state.hist.more);
    assert!(rt.find("history-more").is_none());
}

#[test]
fn history_rows_remove_retry_and_clear_with_a_confirm() {
    let mut rt = history_open();
    // Retry a failed one: a new ↓ of the same remote file.
    let i = history_row_of(&rt, "03 Tin Roof.flac");
    let path = rt.state.hist.rows[i].entry.remote_path.clone();
    click(&mut rt, &format!("hist-{i}/retry"));
    assert!(
        calls(&rt)
            .iter()
            .any(|c| matches!(c, Call::Download(f) if f.path == path && f.username == "marten_k"))
    );
    // Remove one.
    let id = rt.state.hist.rows[0].entry.id;
    click(&mut rt, "hist-0/remove");
    assert!(calls(&rt).contains(&Call::HistoryRemove(id)));
    assert!(rt.state.hist.rows.iter().all(|r| r.entry.id != id));
    // Clear asks first; Keep backs out.
    click(&mut rt, "clear-history");
    assert!(rt.find("confirm-clear").is_some());
    click(&mut rt, "keep-history");
    assert!(!calls(&rt).contains(&Call::HistoryClear));
    assert!(!rt.state.hist.rows.is_empty());
    click(&mut rt, "clear-history");
    click(&mut rt, "confirm-clear-history");
    assert!(calls(&rt).contains(&Call::HistoryClear));
    assert!(rt.state.hist.rows.is_empty());
    assert!(rt.find("no-history").is_some());
    assert!(rt.find("clear-history").is_none());
}

#[test]
fn an_unreadable_history_is_one_line_in_the_view_not_the_banner() {
    let mut rt = history_open();
    rt.update(|s, _| {
        let e = wholeseek_core::CoreEvent::Error {
            context: "history".into(),
            message: "bad line 3".into(),
        };
        s.history_event(&e);
        s.apply(e);
    });
    run(&mut rt, 0.1);
    assert!(rt.state.error.is_none());
    assert!(rt.find("history-error").is_some());
    click(&mut rt, "view-active");
    assert!(rt.find("history-error").is_none());
}

#[test]
fn user_tab_rows_mark_files_downloaded_before() {
    let mut rt = searched();
    let (_, f) = row_of(&rt, "dj_ivo", "Hollow Lights - Low Tide Radio.mp3");
    rt.update(|s, _| s.open_folder_of(&f));
    run(&mut rt, 0.8);
    let j = browse_row(&rt, &f);
    assert!(rt.find(&format!("file-{j}/downloaded")).is_some());
    let other = (0..8)
        .filter(|&i| i != j)
        .find(|i| rt.find(&format!("file-{i}")).is_some())
        .expect("another row");
    assert!(rt.find(&format!("file-{other}/downloaded")).is_none());
}

#[test]
fn history_show_in_folder_selects_the_file_in_the_file_manager() {
    let mut rt = history_open();
    let local = rt.state.hist.rows[0]
        .entry
        .local_path
        .clone()
        .expect("done");
    click(&mut rt, "hist-0/show");
    assert_eq!(
        rt.state.desktop_log.borrow().last(),
        Some(&crate::desktop::DesktopCall::ShowItem(local))
    );
    // Gone from disk: no Show in folder, Copy path still.
    let gone = history_row_of(&rt, "A1 Signals.flac");
    assert!(rt.find(&format!("hist-{gone}/show")).is_none());
    assert!(rt.find(&format!("hist-{gone}/copy")).is_some());
}

#[test]
fn with_a_drawer_open_results_and_inspector_fit_beside_it() {
    use crate::tabs::ResultsLayout;
    let mut fails = Vec::new();
    for w in [1000.0, 1280.0, 1500.0, 1920.0] {
        for layout in [ResultsLayout::Albums, ResultsLayout::Folders] {
            for select in [false, true] {
                let mut rt = albums_at(w);
                rt.update(|s, _| {
                    s.panel = crate::app::Panel::Transfers;
                    s.tab_mut().search_mut().unwrap().filters.layout = layout;
                    if select {
                        s.tab_mut().group = Some(("alice".into(), COAST.into()));
                    }
                });
                run(&mut rt, 0.2);
                let edge = w - crate::view::DRAWER_W - 1.0;
                let bad: Vec<String> = rt
                    .elements()
                    .into_iter()
                    .filter(|e| e.path.contains("/results-split/") && e.rect[2] > 0.0)
                    .filter(|e| e.rect[0] < -0.5 || e.rect[0] + e.rect[2] > edge + 0.5)
                    .map(|e| format!("{} {:?} {:?}", e.path, e.rect, e.text))
                    .take(4)
                    .collect();
                if !bad.is_empty() {
                    fails.push(format!(
                        "{w} {layout:?} selected={select}:\n{}",
                        bad.join("\n")
                    ));
                }
                if select {
                    assert!(rt.find("details").is_some(), "{w}: inspector shown");
                }
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n\n"));
}

fn key(k: Key) -> KeyEvent {
    KeyEvent {
        key: k,
        mods: Mods::default(),
    }
}

fn recent_shown(rt: &Runtime<App>) -> Vec<String> {
    (0..10)
        .filter_map(|i| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(&format!("recent-{i}")))
                .and_then(|e| e.label)
        })
        .collect()
}

/// Searched "hollow lights", "night ferry", then "coastline", with the query box focused.
fn with_recent() -> Runtime<App> {
    let mut rt = searched();
    for q in ["night ferry", "coastline"] {
        rt.state.search_for(q);
        run(&mut rt, 0.3);
    }
    rt.update(|_, tx| tx.focus("query", true));
    run(&mut rt, 0.05);
    rt
}

#[test]
fn down_in_the_query_box_lists_recent_searches_newest_first() {
    let mut rt = with_recent();
    assert!(rt.find("recent").is_none(), "closed until ↓");
    press(&mut rt, key(Key::Down));
    assert_eq!(
        recent_shown(&rt),
        ["coastline", "night ferry", "hollow lights"]
    );
    press(&mut rt, key(Key::Escape));
    assert!(rt.find("recent").is_none(), "Esc closes it");
    // What's typed narrows the list.
    rt.update(|s, _| s.tab_mut().search_mut().unwrap().edit.replace_text("ni"));
    press(&mut rt, key(Key::Down));
    assert_eq!(recent_shown(&rt), ["night ferry"]);
}

#[test]
fn arrows_and_enter_search_a_recent_one_again() {
    let mut rt = with_recent();
    for _ in 0..3 {
        press(&mut rt, key(Key::Down)); // open, then the 1st, then the 2nd
    }
    press(&mut rt, key(Key::Enter));
    assert_eq!(calls(&rt).last(), Some(&Call::Search("night ferry".into())));
    assert_eq!(sv(&rt).query, "night ferry");
    assert!(rt.find("recent").is_none(), "searching closes the list");
    assert_eq!(
        rt.state.recent.items[0], "night ferry",
        "it moves to the front"
    );
    // A click on an entry searches it too.
    rt.update(|_, tx| tx.focus("query", true));
    press(&mut rt, key(Key::Down));
    click(&mut rt, "recent-2");
    assert_eq!(sv(&rt).query, "hollow lights");
}

#[test]
fn recent_searches_can_be_forgotten_one_by_one() {
    let mut rt = with_recent();
    press(&mut rt, key(Key::Down));
    click(&mut rt, "recent-forget-1");
    assert_eq!(
        recent_shown(&rt),
        ["coastline", "hollow lights"],
        "still open"
    );
    press(&mut rt, key(Key::Down)); // highlights "coastline"
    press(
        &mut rt,
        KeyEvent {
            key: Key::Delete,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        },
    );
    assert_eq!(recent_shown(&rt), ["hollow lights"]);
    assert_eq!(rt.state.recent.items, ["hollow lights"]);
    assert_eq!(
        count_calls(&rt, |c| matches!(c, Call::Search(_))),
        3,
        "forgetting doesn't search"
    );
}

/// The open menu's rect touches its trigger: it starts within 4 pt below (or above, when
/// flipped) the trigger, and overlaps it horizontally.
fn assert_menu_by(rt: &Runtime<App>, trigger: &str) {
    let b = rect_of(rt, trigger);
    let m = rect_of(rt, "menu");
    let below = (m[1] - (b[1] + b[3])).abs() <= 4.0;
    let above = ((m[1] + m[3]) - b[1]).abs() <= 4.0;
    let overlaps_x = m[0] < b[0] + b[2] && b[0] < m[0] + m[2];
    assert!(
        (below || above) && overlaps_x,
        "menu {m:?} is not by its trigger {trigger} {b:?}"
    );
}

#[test]
fn every_more_menu_opens_by_its_button_in_both_layouts() {
    use crate::tabs::ResultsLayout;
    // Albums (the default): album rows, track rows, then the inspector's tracks.
    let mut rt = albums_at(1280.0);
    for key in [
        "album-more-0",
        "album-more-2",
        "single-more-0",
        "single-more-1",
    ] {
        click(&mut rt, key);
        assert!(rt.state.menu.is_some(), "{key} opens a menu");
        assert_menu_by(&rt, key);
        rt.update(|s, _| s.menu = None);
        run(&mut rt, 0.05);
    }
    click(&mut rt, "album-0");
    click(&mut rt, "track-more-0");
    assert_menu_by(&rt, "track-more-0");
    rt.update(|s, _| s.menu = None);
    // Folders: a file row's and a folder line's ⋯.
    rt.update(|s, _| s.tab_mut().search_mut().unwrap().filters.layout = ResultsLayout::Folders);
    run(&mut rt, 0.1);
    let tab = rt.state.tab().id;
    let f = sv(&rt).hits[0].file.clone();
    for key in [
        crate::view::files_more_key(tab, &f.path),
        crate::view::files_more_key(tab, f.folder()),
    ] {
        rt.update(|s, _| s.tab_mut().group = Some((f.username.clone(), f.folder().to_string())));
        rt.update(|s, _| s.tab_mut().selected = Some(f.path.clone()));
        run(&mut rt, 0.05);
        click(&mut rt, &key);
        assert!(rt.state.menu.is_some(), "{key} opens a menu");
        assert_menu_by(&rt, &key);
        rt.update(|s, _| s.menu = None);
        run(&mut rt, 0.05);
    }
}

/// The selected folder's tree row, and whether it lies inside the tree's viewport.
fn tree_row_in_view(rt: &Runtime<App>) -> (String, bool) {
    let tab = rt.state.tab();
    let i = tab
        .browse()
        .and_then(|b| b.tree_row_of_selected())
        .expect("the selected folder has a row");
    let key = format!("dir-{i}");
    let (r, v) = (rect_of(rt, &key), rect_of(rt, &tab.tree_key()));
    let inside = r[1] >= v[1] - 0.5 && r[1] + r[3] <= v[1] + v[3] + 0.5;
    (key, inside)
}

fn deep_tab() -> Runtime<App> {
    use crate::demo::{DEEP_FOLDER, DEEP_USER};
    let mut rt = signed_in();
    rt.state.open_folder_at(DEEP_USER, DEEP_FOLDER, None);
    run(&mut rt, 1.0);
    rt
}

#[test]
fn opening_a_user_tab_at_a_deep_folder_reveals_it_in_the_tree() {
    use crate::demo::DEEP_FOLDER;
    let rt = deep_tab();
    let b = rt.state.active_browse().unwrap();
    for anc in [r"Music", r"Music\Jungle", r"Music\Jungle\Iration Steppas"] {
        assert!(b.expanded.contains(anc), "{anc} expanded");
    }
    let i = b.tree_row_of_selected().unwrap();
    assert!(i > 40, "far down the tree (row {i})");
    assert_eq!(
        b.visible_tree()[i].0,
        DEEP_FOLDER,
        "the computed row is the drawn one"
    );
    let (key, inside) = tree_row_in_view(&rt);
    assert!(inside, "{key} is scrolled into the tree's view");
    assert!(rt.state.tab().tree_scroll.get() > 0.0, "the tree scrolled");
}

#[test]
fn clicking_a_tree_row_never_scrolls_the_tree() {
    let mut rt = deep_tab();
    let before = rt.state.tab().tree_scroll.get();
    let i = rt
        .state
        .active_browse()
        .unwrap()
        .tree_row_of_selected()
        .unwrap();
    for k in [i - 3, i + 2, i - 3] {
        click(&mut rt, &format!("dir-{k}"));
        run(&mut rt, 0.3);
        assert_eq!(rt.state.tab().tree_scroll.get(), before, "click on dir-{k}");
    }
}

#[test]
fn reselecting_a_folder_already_in_view_keeps_the_tree_still() {
    let mut rt = deep_tab();
    let before = rt.state.tab().tree_scroll.get();
    let p = {
        let b = rt.state.active_browse().unwrap();
        b.visible_tree()[b.tree_row_of_selected().unwrap() - 2]
            .0
            .clone()
    };
    rt.update(|s, _| s.active_browse_mut().unwrap().select(&p));
    run(&mut rt, 0.3);
    assert_eq!(rt.state.tab().tree_scroll.get(), before);
    assert!(!rt.state.active_browse().unwrap().reveal_tree, "handled");
}

#[test]
fn clearing_the_tree_filter_reveals_the_selected_folder() {
    let mut rt = deep_tab();
    rt.update(|s, _| s.active_browse_mut().unwrap().filter.replace_text("techno"));
    run(&mut rt, 0.2);
    // Cleared from the box, as typing would: the change handler asks for the reveal.
    rt.update(|s, tx| {
        let key = s.tab().tree_key();
        tx.scroll_to_instant(key, 0.0, 0.0);
    });
    rt.update(|_, tx| tx.focus("tree-find", true));
    run(&mut rt, 0.1);
    press(&mut rt, key(Key::End));
    for _ in 0.."techno".len() {
        press(&mut rt, key(Key::Backspace));
    }
    run(&mut rt, 0.3);
    assert!(rt.state.active_browse().unwrap().filter.text.is_empty());
    let (k, inside) = tree_row_in_view(&rt);
    assert!(inside, "{k} back in view");
}

// ---- context menus: tree, user files, tabs, new tabs -----------------------------------------

fn press_button(rt: &mut Runtime<App>, key: &str, b: PointerButton) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
    rt.button_down(x, y, b);
    rt.button_up(x, y, b);
    run(rt, 0.05);
}

fn menu_ids(rt: &Runtime<App>) -> Vec<&'static str> {
    rt.state.menu.as_ref().expect("a menu is open").ids()
}

/// The tree row of `path` in the shown user tab.
fn dir_row(rt: &Runtime<App>, path: &str) -> String {
    let b = rt.state.active_browse().unwrap();
    let i = b
        .visible_tree()
        .iter()
        .position(|r| r.0 == path)
        .expect("a row");
    format!("dir-{i}")
}

/// Press the tree row of `path`, scrolling it into the tree's view first if it's out.
fn press_dir(rt: &mut Runtime<App>, path: &str, b: PointerButton) {
    let k = dir_row(rt, path);
    let tree = rt.state.tab().tree_key();
    let (r, v) = (rect_of(rt, &k), rect_of(rt, &tree));
    if r[1] < v[1] || r[1] + r[3] > v[1] + v[3] {
        let i: f32 = k[4..].parse().unwrap();
        rt.update(|_, tx| tx.scroll_to_instant(tree, 0.0, i * crate::tabs::TREE_ROW_H));
        run(rt, 0.05);
    }
    press_button(rt, &k, b);
}

#[test]
fn tree_rows_right_click_to_their_folder_menu() {
    use crate::demo::DEEP_FOLDER;
    let mut rt = deep_tab();
    press_dir(&mut rt, DEEP_FOLDER, PointerButton::Secondary);
    assert_eq!(
        menu_ids(&rt),
        [
            "play-folder",
            "download-folder",
            "download-folder-to",
            "open-in-new-tab",
            "copy-path"
        ],
        "an album folder: no Expand/Collapse all"
    );
    assert!(rt.find("menu").is_some());
    esc(&mut rt);
    // A folder with subfolders (and no files of its own: nothing to play).
    let jungle = r"Music\Jungle";
    press_dir(&mut rt, jungle, PointerButton::Secondary);
    assert_eq!(
        menu_ids(&rt),
        [
            "download-folder",
            "download-folder-to",
            "expand-all",
            "collapse-all",
            "open-in-new-tab",
            "copy-path"
        ]
    );
    click(&mut rt, "menu-expand-all");
    let b = rt.state.active_browse().unwrap();
    assert!(
        b.expanded.contains(r"Music\Jungle\Acid Collective"),
        "every subfolder opens"
    );
    assert!(!b.expanded.contains(r"Music\Techno"), "only under it");
    press_dir(&mut rt, jungle, PointerButton::Secondary);
    click(&mut rt, "menu-collapse-all");
    let b = rt.state.active_browse().unwrap();
    assert!(
        !b.expanded.iter().any(|p| p.starts_with(jungle)),
        "all closed"
    );
    assert!(b.expanded.contains("Music"), "above it stays");
    // Copy folder path.
    let board = rune::clipboard::memory();
    rt.use_clipboard(&board);
    press_dir(&mut rt, jungle, PointerButton::Secondary);
    click(&mut rt, "menu-copy-path");
    assert_eq!(board.text().as_deref().map(|s| s.as_str()), Some(jungle));
}

/// Open in new tab (menu or middle-click) from the deep tab's tree: a background tab at that
/// folder, the shown tab untouched, no new browse (the listing is reused).
fn assert_opened_beside(rt: &mut Runtime<App>, before: usize, folder: &str) {
    use crate::demo::{DEEP_FOLDER, DEEP_USER};
    assert_eq!(rt.state.tabs.len(), before + 1);
    assert_eq!(rt.state.active, 1, "the source tab stays shown");
    let src = rt.state.active_browse().unwrap();
    assert_eq!(
        src.selected.as_deref(),
        Some(DEEP_FOLDER),
        "source unchanged"
    );
    let new = rt.state.tabs[2].browse().expect("a user tab beside it");
    assert_eq!(
        (new.username.as_str(), new.selected.as_deref()),
        (DEEP_USER, Some(folder))
    );
    assert!(new.dirs.is_some(), "the listing is reused");
    let browses = count_calls(rt, |c| matches!(c, Call::Browse(u) if u == DEEP_USER));
    rt.update(|s, _| s.activate(2));
    run(rt, 0.5);
    assert_eq!(
        count_calls(rt, |c| matches!(c, Call::Browse(u) if u == DEEP_USER)),
        browses,
        "shown without fetching again"
    );
    let (k, inside) = tree_row_in_view(rt);
    assert!(inside, "{k}: the new tab's tree shows its folder");
}

#[test]
fn open_in_new_tab_and_middle_click_open_a_tree_folder_beside() {
    let mut rt = deep_tab();
    let n = rt.state.tabs.len();
    let folder = r"Music\Jungle\Iration Steppas";
    press_dir(&mut rt, folder, PointerButton::Secondary);
    click(&mut rt, "menu-open-in-new-tab");
    assert_opened_beside(&mut rt, n, folder);

    let mut rt = deep_tab();
    let n = rt.state.tabs.len();
    let scroll = rt.state.tab().tree_scroll.get();
    press_dir(&mut rt, folder, PointerButton::Middle);
    assert_eq!(
        rt.state.tab().tree_scroll.get(),
        scroll,
        "the source tree stays put"
    );
    assert_opened_beside(&mut rt, n, folder);
}

#[test]
fn search_folders_open_in_new_tabs_from_the_menu_and_middle_click() {
    let mut rt = searched_at(W);
    let folder = r"Music\Hollow Lights\Night Ferry EP (2021)";
    let line = rt
        .elements()
        .into_iter()
        .filter(|e| e.text.as_deref() == Some("Night Ferry EP (2021)"))
        .find_map(|e| {
            e.path
                .split('/')
                .find(|p| p.starts_with("folder-"))
                .map(str::to_string)
        })
        .expect("the folder line");
    let query = rt.state.active_search().unwrap().query.clone();
    press_button(&mut rt, &line, PointerButton::Middle);
    assert_eq!(rt.state.tabs.len(), 2);
    assert_eq!(rt.state.active, 0, "still on the results");
    assert_eq!(rt.state.active_search().unwrap().query, query);
    let b = rt.state.tabs[1].browse().unwrap();
    assert_eq!(
        (b.username.as_str(), b.selected.as_deref()),
        ("alice", Some(folder))
    );
    assert!(
        !calls(&rt).iter().any(|c| matches!(c, Call::Browse(_))),
        "nothing fetched until it's shown"
    );
    rt.update(|s, _| s.activate(1));
    run(&mut rt, 1.0);
    assert!(calls(&rt).contains(&Call::Browse("alice".into())));
    let b = rt.state.active_browse().unwrap();
    assert!(b.dirs.is_some() && b.selected.as_deref() == Some(folder));

    // The folder menu's "Browse user in new tab": the user's share, beside.
    rt.update(|s, _| s.activate(0));
    run(&mut rt, 0.1);
    press_button(&mut rt, &line, PointerButton::Secondary);
    assert!(menu_ids(&rt).contains(&"open-folder-new-tab"));
    click(&mut rt, "menu-browse-user-new-tab");
    assert_eq!(rt.state.tabs.len(), 3);
    assert_eq!(rt.state.active, 0);
    assert_eq!(rt.state.tabs[1].browse().unwrap().username, "alice");
}

#[test]
fn user_tab_files_offer_copy_name_and_show_in_folder_when_downloaded() {
    let mut rt = signed_in();
    rt.state
        .open_folder_at("dj_ivo", r"mp3\H\Hollow Lights", None);
    run(&mut rt, 1.0);
    let files = rt
        .state
        .active_browse()
        .unwrap()
        .selected_dir()
        .unwrap()
        .files
        .clone();
    let got = files
        .iter()
        .position(|f| f.basename() == "Hollow Lights - Low Tide Radio.mp3")
        .unwrap();
    let other = 1 - got;
    press_button(&mut rt, &format!("file-{got}"), PointerButton::Secondary);
    let ids = menu_ids(&rt);
    assert_eq!(
        ids,
        [
            "play",
            "download",
            "download-to",
            "show-in-folder",
            "copy-name",
            "copy-path"
        ],
        "downloaded before (history): Show in folder, last of this row's actions"
    );
    click(&mut rt, "menu-show-in-folder");
    assert!(matches!(
        rt.state.desktop_log.borrow().last(),
        Some(DesktopCall::ShowItem(p)) if p.ends_with("Hollow Lights - Low Tide Radio.mp3")
    ));
    press_button(&mut rt, &format!("file-{other}"), PointerButton::Secondary);
    assert!(
        !menu_ids(&rt).contains(&"show-in-folder"),
        "never downloaded"
    );
    // The ⋯ menu is the same one.
    let ids = menu_ids(&rt);
    esc(&mut rt);
    let more = crate::view::files_more_key(rt.state.tab().id, &files[other].path);
    rt.update(|s, _| s.tab_mut().selected = Some(files[other].path.clone()));
    run(&mut rt, 0.05);
    click(&mut rt, &more);
    assert_eq!(menu_ids(&rt), ids);
    let board = rune::clipboard::memory();
    rt.use_clipboard(&board);
    click(&mut rt, "menu-copy-name");
    assert_eq!(
        board.text().as_deref().map(|s| s.as_str()),
        Some(files[other].basename())
    );
}

#[test]
fn the_tab_strip_menu_duplicates_and_closes_tabs() {
    let mut rt = searched();
    rt.update(|s, _| {
        s.open_folder_at("alice", COAST, None);
        s.open_user("vinyl_rips");
    });
    run(&mut rt, 1.0);
    let titles = |rt: &Runtime<App>| rt.state.tabs.iter().map(|t| t.title()).collect::<Vec<_>>();
    assert_eq!(titles(&rt), ["hollow lights", "alice", "vinyl_rips"]);
    press_button(&mut rt, "tab-1", PointerButton::Secondary);
    assert_eq!(
        menu_ids(&rt),
        [
            "duplicate-tab",
            "close-tab",
            "close-other-tabs",
            "close-tabs-right"
        ]
    );
    esc(&mut rt);
    press_button(&mut rt, "tab-2", PointerButton::Secondary);
    assert_eq!(
        menu_ids(&rt),
        ["duplicate-tab", "close-tab", "close-other-tabs"],
        "nothing to its right"
    );
    esc(&mut rt);

    // Duplicate a user tab: beside it, shown, same folder, the listing kept.
    press_button(&mut rt, "tab-1", PointerButton::Secondary);
    click(&mut rt, "menu-duplicate-tab");
    assert_eq!(
        titles(&rt),
        ["hollow lights", "alice", "alice", "vinyl_rips"]
    );
    assert_eq!(rt.state.active, 2);
    let b = rt.state.active_browse().unwrap();
    assert!(b.dirs.is_some() && b.selected.as_deref() == Some(COAST));
    // Duplicate the search: the same query, run again once shown.
    press_button(&mut rt, "tab-0", PointerButton::Secondary);
    click(&mut rt, "menu-duplicate-tab");
    run(&mut rt, 1.0);
    assert_eq!(rt.state.active, 1);
    assert_eq!(rt.state.active_search().unwrap().query, "hollow lights");
    assert_eq!(count_calls(&rt, |c| matches!(c, Call::Search(_))), 2);

    // Close tabs to the right of the 2nd, then the others.
    press_button(&mut rt, "tab-1", PointerButton::Secondary);
    click(&mut rt, "menu-close-tabs-right");
    assert_eq!(titles(&rt), ["hollow lights", "hollow lights"]);
    assert_eq!(rt.state.active, 1);
    rt.update(|s, _| s.open_user("alice"));
    run(&mut rt, 0.5);
    let alice = rt.state.active;
    press_button(&mut rt, &format!("tab-{alice}"), PointerButton::Secondary);
    click(&mut rt, "menu-close-other-tabs");
    assert_eq!(titles(&rt), ["alice"]);
    assert_eq!(rt.state.active, 0);
    assert!(
        rt.state.active_browse().unwrap().dirs.is_some(),
        "its listing kept"
    );
    press_button(&mut rt, "tab-0", PointerButton::Secondary);
    assert_eq!(
        menu_ids(&rt),
        ["duplicate-tab", "close-tab"],
        "the only tab"
    );
    // Close tab: as × does (the last one leaves an empty search tab).
    click(&mut rt, "menu-close-tab");
    assert_eq!(rt.state.tabs.len(), 1);
    assert!(rt.state.tab().search().is_some());
}

// ---- tab strip: switching, hairlines, ⋯ ----------------------------------------------------

/// What the strip paints this frame (hover overlays aside): path, rect, fill, opacity.
fn strip_paint(rt: &Runtime<App>) -> Vec<(String, [i32; 4], [i32; 4], i32)> {
    let q = |v: f32| (v * 1000.0).round() as i32;
    rt.elements()
        .into_iter()
        .filter(|e| e.path.contains("/strip/"))
        .map(|e| {
            (
                e.path.clone(),
                e.rect.map(|v| v.round() as i32),
                [q(e.fill.r), q(e.fill.g), q(e.fill.b), q(e.fill.a)],
                q(e.opacity),
            )
        })
        .collect()
}

fn three_tabs() -> Runtime<App> {
    let mut rt = searched();
    rt.update(|s, _| {
        s.open_user("alice");
        s.open_user("vinyl_rips");
    });
    run(&mut rt, 1.0);
    rt
}

/// The reported "blinking left border": the marker and hairline were unkeyed children, so on
/// a switch the old tab's orange marker turned into its hairline and faded across, and the
/// fills cross-faded. Now a switch lands whole in the frame of the click.
#[test]
fn switching_tabs_lands_in_one_frame_with_no_stray_border() {
    let mut rt = three_tabs();
    assert_eq!(rt.state.active, 2);
    for target in [0, 1, 2, 0] {
        let (x, y) = rt.find(&format!("tab-{target}/title")).unwrap();
        rt.pointer_move(x, y);
        // Hover may fade out of the tab the pointer left (≤ 80 ms); let it, then click.
        run(&mut rt, 0.15);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x, y);
        let frames: Vec<_> = (0..12)
            .map(|_| {
                rt.frame(1.0 / 60.0);
                strip_paint(&rt)
            })
            .collect();
        assert_eq!(rt.state.active, target);
        for (n, f) in frames.iter().enumerate() {
            assert_eq!(
                f,
                frames.last().unwrap(),
                "frame {n} after the click to tab-{target} differs from the settled strip"
            );
            let thin: Vec<_> = f
                .iter()
                .filter(|(_, r, c, o)| r[2] <= 2 && r[3] >= 8 && c[3] > 0 && *o > 0)
                .map(|(p, ..)| p.clone())
                .collect();
            for p in &thin {
                assert!(p.ends_with("/sep-line"), "a stray vertical line: {p}");
                for beside in [target, target.wrapping_sub(1)] {
                    assert!(
                        !p.contains(&format!("/tab-{beside}/")),
                        "a hairline beside the active tab-{target}: {p}"
                    );
                }
            }
            // The orange top marker, by its shape: on the active tab only.
            let markers: Vec<_> = f
                .iter()
                .filter(|(_, r, c, o)| r[1] == 0 && r[3] == 2 && r[2] > 20 && c[3] > 0 && *o > 0)
                .collect();
            assert_eq!(markers.len(), 1, "{markers:?}");
            assert!(markers[0].0.contains(&format!("/tab-{target}/")));
        }
    }
    // A switch no click caused (a shortcut, a restore) lands the same way.
    rt.update(|s, _| s.activate(1));
    rt.frame(1.0 / 60.0);
    let first = strip_paint(&rt);
    run(&mut rt, 0.3);
    assert_eq!(first, strip_paint(&rt));
}

#[test]
fn the_hairlines_hide_beside_the_active_and_the_hovered_tab() {
    let mut rt = three_tabs();
    rt.update(|s, _| {
        s.open_user("dj_ivo");
        s.activate(0);
    });
    run(&mut rt, 1.0);
    let line = |rt: &Runtime<App>, i: usize| {
        rt.elements()
            .into_iter()
            .find(|e| e.path.contains(&format!("/tab-{i}/sep")) && e.path.ends_with("/sep-line"))
            .map(|e| e.opacity)
    };
    assert_eq!(line(&rt, 0), None, "none right of the active tab");
    assert_eq!([line(&rt, 1), line(&rt, 2), line(&rt, 3)], [Some(1.0); 3]);
    let (x, y) = rt.find("tab-2/title").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(
        [line(&rt, 1), line(&rt, 2), line(&rt, 3)],
        [Some(0.0), Some(0.0), Some(1.0)],
        "both sides of the hovered tab, at once"
    );
    rt.pointer_move(600.0, 400.0);
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        [line(&rt, 1), line(&rt, 2), line(&rt, 3)],
        [Some(1.0); 3],
        "back within 80 ms"
    );
    rt.update(|s, _| s.activate(2));
    run(&mut rt, 0.1);
    assert_eq!(
        [line(&rt, 1), line(&rt, 2)],
        [None, None],
        "none beside the active tab"
    );
}

#[test]
fn the_tab_more_button_opens_the_tab_menu() {
    let mut rt = three_tabs();
    let opacity = |rt: &Runtime<App>, k: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(k))
            .map(|e| e.opacity)
    };
    assert_eq!(opacity(&rt, "tab-more-2"), Some(1.0), "on the active tab");
    assert_eq!(opacity(&rt, "tab-more-1"), Some(0.0), "others: on hover");
    let r = rect_of(&rt, "tab-1");
    rt.pointer_move(r[0] + 20.0, r[1] + 20.0);
    run(&mut rt, 0.05);
    assert_eq!(opacity(&rt, "tab-more-1"), Some(1.0));
    let m = rect_of(&rt, "tab-more-1");
    assert!(m[2] >= 22.0 && m[3] >= 22.0, "a 22 pt target: {m:?}");
    let x = rt
        .elements()
        .into_iter()
        .find(|e| e.path.ends_with("/tab-1/close"))
        .unwrap()
        .rect;
    assert!(m[0] + m[2] <= x[0] && x[2] >= 22.0, "left of ×, both 22 pt");
    click(&mut rt, "tab-more-1");
    assert_eq!(rt.state.active, 2, "⋯ doesn't switch");
    assert_eq!(
        menu_ids(&rt),
        [
            "duplicate-tab",
            "close-tab",
            "close-other-tabs",
            "close-tabs-right"
        ]
    );
    assert!(
        matches!(rt.state.menu.as_ref().unwrap().anchor, MenuAnchor::Key(ref k) if k == "tab-more-1")
    );
    click(&mut rt, "menu-close-tabs-right");
    assert_eq!(rt.state.tabs.len(), 2);
}

#[test]
fn the_tree_more_button_opens_the_folder_menu() {
    use crate::demo::{DEEP_FOLDER, DEEP_USER};
    let mut rt = deep_tab();
    let sel = dir_row(&rt, DEEP_FOLDER);
    let more = sel.replace("dir-", "dir-more-");
    click(&mut rt, &more);
    let want: Vec<_> = rt
        .state
        .tree_menu(DEEP_USER, DEEP_FOLDER, false)
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(menu_ids(&rt), want);
    assert!(matches!(rt.state.menu.as_ref().unwrap().anchor, MenuAnchor::Key(ref k) if *k == more));
    esc(&mut rt);
    // Another row: ⋯ shows on hover, over the end of its name; its parent has subfolders.
    let parent = r"Music\Jungle\Iration Steppas";
    let k = dir_row(&rt, parent);
    let r = rect_of(&rt, &k);
    rt.pointer_move(r[0] + 40.0, r[1] + 10.0);
    run(&mut rt, 0.05);
    let more = k.replace("dir-", "dir-more-");
    let m = rect_of(&rt, &more);
    assert!(
        m[0] + m[2] <= r[0] + r[2] && m[0] > r[0] + r[2] - 40.0,
        "at the row's right edge"
    );
    click(&mut rt, &more);
    assert!(menu_ids(&rt).contains(&"expand-all"));
    assert_eq!(
        rt.state.active_browse().unwrap().selected.as_deref(),
        Some(DEEP_FOLDER),
        "⋯ doesn't select"
    );
}

#[test]
fn context_menus_stay_inside_the_window_near_its_edges() {
    let mut rt = deep_tab();
    let items = rt
        .state
        .tree_menu(crate::demo::DEEP_USER, r"Music\Jungle", true);
    for (x, y) in [
        (W - 4.0, H - 4.0),
        (W - 4.0, 40.0),
        (4.0, H - 4.0),
        (2.0, 2.0),
    ] {
        rt.update(|s, _| s.open_menu(MenuAnchor::Point(x, y), items.clone()));
        run(&mut rt, 0.05);
        let m = rect_of(&rt, "menu");
        assert!(
            m[0] >= 0.0 && m[1] >= 0.0 && m[0] + m[2] <= W && m[1] + m[3] <= H,
            "menu {m:?} at ({x}, {y}) leaves the {W}x{H} window"
        );
        assert!(
            (m[0] - x).abs() < m[2] + 8.0 && (m[1] - y).abs() < m[3] + 8.0,
            "menu {m:?} stays by the pointer ({x}, {y})"
        );
        esc(&mut rt);
    }
}
