//! The bottom player strip.

use goalseek_audio::PlayState;
use rune::tok::{Theme, type_};
use rune::{Cursor, El, Phase, col, el, row, text};

use super::{accent, amber, icons, red, tnum};
use crate::app::{App, TState};
use crate::fmt;

pub fn strip(s: &App, t: &Theme) -> El {
    let strip = row()
        .key("player")
        .h(60.0)
        .w_full()
        .px(16.0)
        .gap(14.0)
        .center_y()
        .fill(t.card);
    let Some(p) = &s.playing else {
        return strip.child(
            text("Nothing playing — ▶ any track to stream it from the cache")
                .key("idle")
                .font(type_::LABEL)
                .color(t.text_secondary),
        );
    };
    let snap = p.snapshot.as_ref();
    let state = snap.map(|sn| &sn.state);
    let running = matches!(state, Some(PlayState::Playing | PlayState::Buffering))
        || (!p.opened && p.failed.is_none());
    let toggle = row()
        .key("toggle")
        .size(34.0, 34.0)
        .center()
        .radius(17.0)
        .fill(t.control_off)
        .hover(t.control_off)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(if running { "Pause" } else { "Play" })
        .on_press(|s: &mut App| s.toggle_pause())
        .child(if running {
            icons::pause(14.0, t.text)
        } else {
            icons::play(14.0, t.text)
        });

    // Second line: a transient notice, else what the fetch / player is doing.
    let transfer = s.transfers.iter().find(|t| t.id == p.ticket.id);
    let status: Option<(String, rune::Color)> = if let Some((n, _)) = &s.notice {
        Some((n.clone(), amber()))
    } else if let Some(reason) = &p.failed {
        Some((format!("Transfer failed: {reason}"), red()))
    } else if let Some(PlayState::Error(e)) = state {
        Some((format!("Can't play: {e}"), red()))
    } else if !p.opened {
        Some(match transfer.map(|t| &t.state) {
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
    let mut title = col().key("now").w(320.0).gap(2.0).child(
        row()
            .gap(6.0)
            .center_y()
            .child(
                text(fmt::title(&p.file))
                    .key("now-title")
                    .font(type_::TITLE)
                    .color(t.text)
                    .truncate()
                    .shrink(),
            )
            .child(text("—").font(type_::BODY).color(t.text_secondary))
            .child(
                text(p.file.username.clone())
                    .key("now-user")
                    .font(type_::BODY)
                    .color(t.text_secondary),
            ),
    );
    if let Some((line, color)) = status {
        title = title.child(
            text(line)
                .key("notice")
                .font(type_::LABEL)
                .color(color)
                .truncate(),
        );
    }

    let duration = snap
        .and_then(|sn| sn.duration_secs)
        .or_else(|| p.file.attrs.duration_secs.map(f64::from));
    let position = snap.map_or(0.0, |sn| sn.position_secs);
    let played = duration.map_or(0.0, |d| if d > 0.0 { (position / d) as f32 } else { 0.0 });
    let buffered = snap.map_or_else(
        || transfer.map_or(0.0, crate::app::Transfer::fraction),
        |sn| sn.buffered_fraction as f32,
    );
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
                .h(4.0)
                .radius(2.0)
                .fill(t.control_off)
                .child(
                    el().key("buffered")
                        .absolute(0.0, 0.0)
                        .w_frac(buffered.clamp(0.0, 1.0))
                        .h(4.0)
                        .radius(2.0)
                        .fill(t.control_off),
                )
                .child(
                    el().key("played")
                        .absolute(0.0, 0.0)
                        .w_frac(played.clamp(0.0, 1.0))
                        .h(4.0)
                        .radius(2.0)
                        .fill(accent()),
                ),
        );
    let time = text(format!(
        "{} / {}",
        fmt::mmss(position),
        duration.map_or_else(|| "–:––".into(), fmt::mmss)
    ))
    .key("time")
    .font(tnum(type_::LABEL))
    .color(t.text_secondary);
    let volume = snap.map_or(s.volume, |sn| sn.volume);
    let vol = row()
        .key("volume")
        .gap(8.0)
        .center_y()
        .child(icons::speaker(14.0, t.text_secondary))
        .child(
            row()
                .key("volume-track")
                .w(76.0)
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
                    el().w_full().h(4.0).radius(2.0).fill(t.control_off).child(
                        el().absolute(0.0, 0.0)
                            .w_frac(volume.clamp(0.0, 1.0))
                            .h(4.0)
                            .radius(2.0)
                            .fill(t.text_secondary),
                    ),
                ),
        );
    strip
        .child(toggle)
        .child(title)
        .child(seek)
        .child(time)
        .child(vol)
}
