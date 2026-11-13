//! The bottom player: ▶/❚❚, title and "from <user> · quality · streaming|cached", the seek
//! bar (play position over the downloaded region), times, ↓ Keep and volume.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, Phase, col, el, row, text};
use wholeseek_audio::PlayState;

use super::files::{action, equalizer};
use super::{META, accent, amber, green, icon_button, icons, red, tnum};
use crate::app::{App, TState, Transfer};
use crate::fmt;

pub fn strip(s: &App, t: &Theme) -> El {
    let strip = row()
        .key("player")
        .h(56.0)
        .w_full()
        .px(12.0)
        .gap(12.0)
        .center_y()
        .fill(t.card);
    let Some(p) = &s.playing else {
        return strip.child(
            text("Nothing playing")
                .key("idle")
                .font(META)
                .color(t.text_secondary),
        );
    };
    let snap = p.snapshot.as_ref();
    let state = snap.map(|sn| &sn.state);
    let running = matches!(state, Some(PlayState::Playing | PlayState::Buffering))
        || (!p.opened && p.failed.is_none());
    let toggle = row()
        .key("toggle")
        .size(30.0, 30.0)
        .center()
        .radius(15.0)
        .fill(t.hover)
        .hover(t.control_track)
        .press(t.control_track)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(if running { "Pause" } else { "Play" })
        .on_press(|s: &mut App| s.toggle_pause())
        .child(if running {
            icons::pause(icons::LG, t.text)
        } else {
            icons::play(icons::LG, t.text)
        });

    let fetch = s.transfers.iter().find(|x| x.id == p.ticket.id);
    let kept = s.download_of(&p.file);
    // Second line: a notice or problem if there is one, else where the sound comes from.
    let user = p.file.username.clone();
    // A Library file: its tags, from disk; nobody to open.
    let local = p.file.is_local().then(|| s.library_names(&p.file));
    let from = match &local {
        Some((_, artist, album)) => row().gap(4.0).center_y().child(
            text(if album.is_empty() {
                artist.clone()
            } else {
                format!("{artist} · {album}")
            })
            .key("now-artist")
            .font(META)
            .color(t.text)
            .truncate()
            .shrink(),
        ),
        None => row()
            .gap(4.0)
            .center_y()
            .child(text("from").font(META).color(t.text_secondary))
            .child(
                text(p.file.username.clone())
                    .key("now-user")
                    .font(META)
                    .color(t.text)
                    .cursor(Cursor::Pointer)
                    .role("link")
                    .help("Open this user in a tab")
                    .on_press(move |s: &mut App| s.open_user(&user)),
            ),
    };
    let source = match fetch.map(|x| &x.state) {
        _ if local.is_some() => "Library",
        Some(TState::Done(_)) => "cached",
        _ if kept.is_some_and(|k| matches!(k.state, TState::Done(_))) => "cached",
        _ => "streaming",
    };
    let item = s.queue.as_ref().and_then(|q| q.current());
    let status: Option<(String, rune::Color)> = if let Some((n, _)) = &s.notice {
        Some((n.clone(), amber()))
    } else if p.failed.is_none() && !p.opened && p.attempt > 1 {
        let of = p.attempt + item.map_or(0, |i| i.sources.len());
        Some((
            format!("Trying another source… ({}/{of})", p.attempt),
            t.text_secondary,
        ))
    } else if let Some(reason) = &p.failed {
        Some((short(reason, "Source unavailable"), red()))
    } else if let Some(PlayState::Error(e)) = state {
        Some((short(e, "Can't play this file"), red()))
    } else if !p.opened {
        Some(match fetch.map(|x| &x.state) {
            Some(TState::RemoteQueued(Some(n))) => {
                (format!("Queued at {} (#{n})", p.file.username), amber())
            }
            Some(TState::RemoteQueued(None)) => (format!("Queued at {}", p.file.username), amber()),
            _ => (
                format!("Connecting to {}…", p.file.username),
                t.text_secondary,
            ),
        })
    } else if matches!(state, Some(PlayState::Buffering)) {
        Some(("Buffering…".into(), t.text_secondary))
    } else {
        None
    };
    let second = match status {
        Some((line, color)) => row().child(
            text(line)
                .key("notice")
                .help(p.failed.clone().unwrap_or_default())
                .font(META)
                .color(color)
                .truncate()
                .shrink(),
        ),
        None => from.child(
            text(format!("· {} · {source}", fmt::quality(&p.file)))
                .key("source")
                .font(META)
                .color(t.text_secondary),
        ),
    };
    let title = col()
        .key("now")
        .w(300.0)
        .gap(2.0)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(equalizer(s.now, running && p.opened, accent()))
                .child(
                    text(
                        local
                            .as_ref()
                            .map_or_else(|| fmt::title(&p.file), |l| l.0.clone()),
                    )
                    .key("now-title")
                    .font(type_::TITLE)
                    .color(t.text)
                    .truncate()
                    .shrink()
                    .cursor(rune::Cursor::Pointer)
                    .role("link")
                    .help("Show in list")
                    .on_press_tx(|s: &mut App, tx| {
                        if s.playing.as_ref().is_some_and(|p| p.file.is_local()) {
                            s.reveal_library_playing(tx);
                        } else {
                            s.reveal_playing();
                        }
                    }),
                ),
        )
        .child(second);

    let duration = snap
        .and_then(|sn| sn.duration_secs)
        .or_else(|| p.file.attrs.duration_secs.map(f64::from));
    let position = snap.map_or(0.0, |sn| sn.position_secs);
    let played = duration.map_or(0.0, |d| if d > 0.0 { (position / d) as f32 } else { 0.0 });
    let buffered = snap.map_or_else(
        || fetch.map_or(0.0, Transfer::fraction),
        |sn| sn.buffered_fraction as f32,
    );
    let bar = |key: &str, frac: f32, color: rune::Color| {
        el().key(key.to_string())
            .absolute(0.0, 0.0)
            .w_frac(frac.clamp(0.0, 1.0))
            .h(3.0)
            .fill(color)
    };
    let seek = row()
        .key("seek")
        .grow(1.0)
        .h(18.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("slider")
        .label("Seek")
        .on_drag(|s: &mut App, d: &mut rune::Drag, _| {
            if d.phase == Phase::Up && !d.cancelled() && d.w > 0.0 {
                s.seek_fraction(d.x / d.w);
            }
        })
        .child(
            el().w_full()
                .h(3.0)
                .fill(t.hover)
                .child(bar("buffered", buffered, t.control_track))
                .child(bar("played", played, accent())),
        );
    let time = |key: &str, secs: Option<f64>| {
        text(secs.map_or_else(|| "–:––".into(), fmt::mmss))
            .key(key.to_string())
            .font(tnum(META))
            .color(t.text_secondary)
    };

    let file = p.file.clone();
    let keep: El = match kept.map(|k| (&k.state, k.fraction())) {
        // Already on disk: nothing to keep.
        _ if p.file.is_local() => el().key("keep"),
        None | Some((TState::Cancelled | TState::Failed(_), _)) => {
            action("keep", "Keep", icons::down(icons::MD, t.text), t)
                .help("Save this track to your downloads")
                .on_press(move |s: &mut App| s.keep(&file))
        }
        Some((TState::Done(_), _)) => row()
            .key("keep")
            .gap(5.0)
            .center_y()
            .label("Kept")
            .child(icons::check(icons::MD, green()))
            .child(text("Kept").font(META).color(t.text_secondary)),
        Some((_, frac)) => row().key("keep").label("Keeping").child(
            text(format!("Keeping {:.0}%", frac * 100.0))
                .font(tnum(META))
                .color(t.text_secondary),
        ),
    };

    let volume = snap.map_or(s.volume, |sn| sn.volume);
    let vol = row()
        .key("volume")
        .gap(6.0)
        .center_y()
        .child(icons::speaker(icons::MD, t.text_secondary))
        .child(
            row()
                .key("volume-track")
                .w(64.0)
                .h(18.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .role("slider")
                .label("Volume")
                .on_drag(|s: &mut App, d: &mut rune::Drag, _| {
                    if d.w > 0.0 && !d.cancelled() {
                        s.set_volume(d.x / d.w);
                    }
                })
                .child(
                    el().w_full().h(3.0).fill(t.hover).child(
                        el().absolute(0.0, 0.0)
                            .w_frac(volume.clamp(0.0, 1.0))
                            .h(3.0)
                            .fill(t.text_secondary),
                    ),
                ),
        );
    let skip = |key: &str, back: bool| {
        icon_button(key, icons::skip(icons::MD, back, t.text_secondary), t)
            .size(24.0, 24.0)
            .label(if back { "Previous" } else { "Next" })
    };
    let queue = s.queue_position();
    let mut controls = row().gap(2.0).center_y();
    if queue.is_some() {
        controls = controls.child(
            skip("prev", true)
                .help(format!("Previous ({})", crate::desktop::chord("←")))
                .on_press(|s: &mut App| s.previous_track()),
        );
    }
    controls = controls.child(toggle);
    if let Some((at, of)) = queue {
        let next = s.queue.as_ref().and_then(|q| q.next_title());
        controls = controls
            .child(
                skip("next", false)
                    .help(next.map_or_else(
                        || format!("Next ({})", crate::desktop::chord("→")),
                        |n| format!("Next: {n}"),
                    ))
                    .on_press(|s: &mut App| s.next_track()),
            )
            .child(
                text(format!("{at} / {of}"))
                    .key("queue-pos")
                    .font(tnum(META))
                    .color(t.text_secondary)
                    .pl(4.0),
            );
    }
    strip
        .on_secondary_press(|s: &mut App, p, _| {
            let items = s.player_items();
            super::menu::at_point(s, p, items);
        })
        .child(controls)
        .child(super::art::cover(
            s,
            "now-cover",
            &p.file.username,
            p.file.folder(),
            super::art::PLAYER,
            t,
        ))
        .child(title)
        .child(time("pos", Some(position)))
        .child(seek)
        .child(time("dur", duration))
        .child(keep)
        .child(vol)
}

/// One line max: a long reason becomes `fallback` (the full text is the tooltip).
fn short(reason: &str, fallback: &str) -> String {
    if reason.chars().count() <= 32 {
        reason.to_string()
    } else {
        fallback.to_string()
    }
}
