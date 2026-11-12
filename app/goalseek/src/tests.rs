//! Headless UI flows in demo mode: the real view and handlers on Rune's runtime, the fake core
//! on the virtual clock. Clicks go through hit testing, like a pointer would.

use goalseek_audio::PlayState;
use goalseek_core::{RemoteFile, TransferKind};
use rune::{Env, Runtime};

use crate::app::{App, Conn, Panel, Screen, TState};
use crate::demo::Call;
use crate::{H, W, poll, view};

fn rt() -> Runtime<App> {
    let mut rt = Runtime::new(
        App::demo(),
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt
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
    rt.state.search_edit.replace_text("hollow lights");
    rt.state.submit_search();
    run(&mut rt, 1.6);
    rt
}

fn row_of(rt: &Runtime<App>, user: &str, name: &str) -> (usize, RemoteFile) {
    let hits = &rt.state.search.as_ref().expect("search").hits;
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
fn search_results_stream_in_flac_first() {
    let mut rt = signed_in();
    rt.state.search_edit.replace_text("hollow lights");
    rt.state.submit_search();
    run(&mut rt, 0.35);
    let first = rt.state.search.as_ref().unwrap().hits.len();
    assert!(
        first > 0 && !rt.state.search.as_ref().unwrap().finished,
        "a first batch arrives before the search ends"
    );
    assert!(rt.find("searching").is_some());
    run(&mut rt, 1.3);
    let s = rt.state.search.as_ref().unwrap();
    assert!(
        s.finished && s.hits.len() > first,
        "later batches append: {first} → {}",
        s.hits.len()
    );
    let ranks: Vec<u8> = s
        .hits
        .iter()
        .map(|h| goalseek_core::rank::format_rank(&h.file.path))
        .collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "FLAC first, then MP3, then others: {ranks:?}"
    );
    assert_eq!(ranks[0], 0);
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
    let (i, file) = row_of(&rt, "marten_k", "Hollow Lights - Salt Roads.flac");
    click(&mut rt, &format!("row-{i}/download"));
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
fn transfers_and_settings_panels_open() {
    let mut rt = searched();
    click(&mut rt, "row-0/download");
    click(&mut rt, "transfers");
    assert!(rt.find("transfers-panel").is_some());
    assert!(rt.find("dl-0").is_some(), "the download is listed");
    assert!(rt.find("up-0").is_some(), "uploads are listed");
    click(&mut rt, "settings");
    assert_eq!(rt.state.panel, Panel::Settings);
    assert!(rt.find("transfers-panel").is_none() && rt.find("settings-panel").is_some());
    assert!(rt.find("share-counts").is_some());

    rt.state.settings_form.listen_port.replace_text("80");
    click(&mut rt, "apply");
    assert!(
        rt.state
            .settings_form
            .message
            .as_ref()
            .is_some_and(|(ok, _)| !ok),
        "port < 1024 is refused"
    );
    rt.state.settings_form.listen_port.replace_text("2240");
    rt.state.settings_form.upload_slots.replace_text("4");
    click(&mut rt, "apply");
    let applied = calls(&rt)
        .into_iter()
        .rev()
        .find_map(|c| {
            if let Call::ApplySettings(s) = c {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!((applied.listen_port, applied.upload_slots), (2240, 4));

    click(&mut rt, "rescan");
    assert_eq!(calls(&rt).last(), Some(&Call::RescanShares));
    click(&mut rt, "panel-close");
    assert_eq!(rt.state.panel, Panel::None);
}

#[test]
fn username_opens_browse_with_tree_and_back_returns() {
    let mut rt = searched();
    let (i, _) = row_of(&rt, "alice", "01 Northern Glass.flac");
    click(&mut rt, &format!("row-{i}/user"));
    assert!(calls(&rt).contains(&Call::Browse("alice".into())));
    run(&mut rt, 0.6);
    let b = rt.state.browse.as_ref().unwrap();
    assert!(b.dirs.is_some() && b.selected.is_some());
    assert!(rt.find("dir-0").is_some() && rt.find("file-0/play").is_some());
    assert!(rt.find("back").is_some() && rt.find("crumb-user").is_some());

    // ▶ and ↓ work in browse too.
    click(&mut rt, "file-0/play");
    assert!(matches!(calls(&rt).last(), Some(Call::PlayFetch(f)) if f.username == "alice"));
    click(&mut rt, "download-folder");
    let sel = rt.state.browse.as_ref().unwrap().selected.clone().unwrap();
    assert_eq!(
        calls(&rt).last(),
        Some(&Call::DownloadFolder("alice".into(), sel))
    );

    click(&mut rt, "back");
    assert!(rt.state.browse.is_none());
    assert!(rt.find("row-0").is_some(), "results are back");
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
use goalseek_core::Credentials;

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
    assert!(rt.find("keyring-note").is_some());
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
fn settings_save_persists_and_forget_account_clears_it() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    rt.state.settings_form.upload_slots.replace_text("3");
    click(&mut rt, "apply");
    assert!(
        matches!(store_calls(&rt).last(), Some(StoreCall::SaveSettings(s)) if s.upload_slots == 3)
    );
    click(&mut rt, "forget");
    assert_eq!(store_calls(&rt).last(), Some(&StoreCall::ForgetCredentials));
    assert_eq!(rt.state.screen, Screen::Onboarding);
    assert!(
        rt.state.onboarding.username.text.is_empty()
            && rt.state.onboarding.password.text.is_empty()
    );
    assert!(calls(&rt).contains(&Call::Logout));
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
        goalseek_core::UploadState::Cancelled
    );
    assert!(rt.find("up-0/cancel").is_none());
}

#[test]
fn audio_only_toggle_shows_other_files_and_tells_the_core() {
    let mut rt = searched();
    let audio = rt.state.search.as_ref().unwrap().hits.len();
    assert!(
        rt.state
            .search
            .as_ref()
            .unwrap()
            .hits
            .iter()
            .all(|h| goalseek_core::rank::is_audio(&h.file.path))
    );
    click(&mut rt, "audio-only");
    assert!(calls(&rt).contains(&Call::SetAudioOnly(false)));
    run(&mut rt, 1.6);
    let hits = &rt.state.search.as_ref().unwrap().hits;
    assert!(hits.len() > audio && hits.iter().any(|h| h.file.path.ends_with("cover.jpg")));
    let ranks: Vec<u8> = hits
        .iter()
        .map(|h| goalseek_core::rank::format_rank(&h.file.path))
        .collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "non-audio sorts last"
    );
}

#[test]
fn port_mapping_result_shows_in_settings() {
    let mut rt = signed_in();
    click(&mut rt, "settings");
    assert_eq!(
        rt.state.port_mapping.as_deref(),
        Some("Port 2234 mapped via UPnP (203.0.113.7:2234)")
    );
    rt.state.apply(goalseek_core::CoreEvent::PortMapping {
        port: 2234,
        outcome: goalseek_core::PortMapOutcome::Unavailable(
            "No UPnP or NAT-PMP router answered.".into(),
        ),
    });
    assert!(
        rt.state
            .port_mapping
            .as_deref()
            .unwrap()
            .starts_with("Port 2234 not mapped — forward it manually.")
    );
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
    assert_eq!(stats, "3.1 MB/s · 24,558 files · 2,524 folders");
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
    let other = goalseek_core::UserProfile {
        description: Some("not alice".into()),
        ..Default::default()
    };
    rt.update(|s, _| {
        s.apply(goalseek_core::CoreEvent::UserProfile {
            username: "mallory".into(),
            profile: other.clone(),
        })
    });
    rt.update(|s, _| {
        s.apply(goalseek_core::CoreEvent::UserProfileFailed {
            username: "mallory".into(),
            reason: "x".into(),
        })
    });
    run(&mut rt, 0.02);
    assert!(shown(&rt, "profile-loading"), "still waiting for alice");
    assert!(rt.state.profiles.get("mallory").is_none());
    // A late answer for a user we already left doesn't show up on the next one.
    click(&mut rt, "back");
    rt.update(|s, _| s.open_user("vinyl_rips"));
    rt.update(|s, _| {
        s.apply(goalseek_core::CoreEvent::UserProfile {
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
        s.apply(goalseek_core::CoreEvent::UserProfileFailed {
            username: "alice".into(),
            reason: "timeout".into(),
        })
    });
    run(&mut rt, 0.02);
    assert!(shown(&rt, "profile-stats"));
}
