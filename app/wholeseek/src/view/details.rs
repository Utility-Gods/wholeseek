//! A search tab's details pane, right of the results on a wide window: the selected folder
//! group (user, track list in natural order, totals, formats) and its actions. The actions
//! run the same [`MenuAction`]s as the folder menu.

use std::collections::BTreeSet;

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollSpec, col, el, row, text};

use super::files::action;
use super::{META, amber, green, hairline, icon_button, icons, tnum};
use crate::app::App;
use crate::fmt;
use crate::menu::MenuAction;
use crate::tabs::{SearchTab, Tab};

/// Narrowest window (points) that gets the pane; below it the layout is the plain list.
pub const PANE_MIN_WINDOW_W: f32 = 1500.0;
/// The results table never gets wider than this (the name column tops out near 680 pt), so
/// Length, Size and Quality stay next to the names.
pub const TABLE_MAX_W: f32 = 1110.0;
/// The pane's content is no wider than this, whatever room is left.
const PANE_MAX_W: f32 = 720.0;

pub fn wide(s: &App) -> bool {
    s.window_w >= PANE_MIN_WINDOW_W
}

/// How wide a results or file table may get: capped on a wide window, else the full width
/// (the layout below the breakpoint is unchanged).
pub fn table_max_w(s: &App) -> f32 {
    if wide(s) { TABLE_MAX_W } else { f32::INFINITY }
}

/// The pane: the tab's selected group, or one muted line.
pub fn pane(s: &App, tab: &Tab, st: &SearchTab, t: &Theme) -> El {
    let room = (s.window_w - TABLE_MAX_W - 1.0).max(0.0);
    let base = col()
        .key("details")
        .w(room.min(PANE_MAX_W))
        .h_full()
        .fill(t.bg);
    let selected = tab.group.as_ref().filter(|(u, d)| {
        st.search.as_ref().is_some_and(|v| {
            v.hits
                .iter()
                .any(|h| &h.file.username == u && h.file.folder() == d)
        })
    });
    let Some((username, folder)) = selected else {
        return base.center().child(
            text("Select a folder to see it here")
                .key("details-empty")
                .font(META)
                .color(t.text_secondary),
        );
    };
    let files = s.folder_files(username, folder);
    let hit = st
        .search
        .as_ref()
        .and_then(|v| v.hits.iter().find(|h| &h.file.username == username));
    let (free, speed) = hit.map_or((false, 0), |h| (h.free_slot, h.speed));
    let total: u64 = files.iter().map(|f| f.size).sum();
    let secs: u64 = files
        .iter()
        .filter_map(|f| f.attrs.duration_secs)
        .map(u64::from)
        .sum();
    let formats: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| crate::tabs::format_of(&f.path))
        .collect();
    let playable = files.iter().any(crate::tabs::is_playable);
    let name = folder.rsplit('\\').next().unwrap_or(folder).to_string();

    let user = username.clone();
    let who = row()
        .key("details-user")
        .gap(8.0)
        .center_y()
        .child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(if free { green() } else { amber() }),
        )
        .child(
            text(username.clone())
                .font(type_::LABEL)
                .color(t.text)
                .cursor(Cursor::Pointer)
                .role("link")
                .help("Open this user in a tab")
                .on_press(move |s: &mut App| s.open_user(&user)),
        )
        .child(
            text(format!(
                "{} · {}",
                fmt::speed(f64::from(speed)),
                if free { "free slot" } else { "queued" }
            ))
            .font(tnum(META))
            .color(t.text_secondary),
        );
    let mut summary = vec![format!(
        "{} {}",
        files.len(),
        if files.len() == 1 { "file" } else { "files" }
    )];
    if secs > 0 {
        summary.push(fmt::mmss(secs as f64));
    }
    summary.push(fmt::size(total));
    if !formats.is_empty() {
        summary.push(formats.into_iter().collect::<Vec<_>>().join(", "));
    }

    let run = |a: MenuAction| {
        move |s: &mut App| {
            s.run_menu(a.clone());
        }
    };
    let (u, d) = (username.clone(), folder.clone());
    let mut actions = row().key("details-actions").gap(6.0).center_y();
    if playable {
        actions = actions.child(
            action("details-play", "Play album", icons::play(10.0, t.text), t).on_press(run(
                MenuAction::PlayFolder {
                    username: u.clone(),
                    folder: d.clone(),
                },
            )),
        );
    }
    actions = actions
        .child(
            action(
                "details-download",
                "Download folder",
                icons::down(11.0, t.text),
                t,
            )
            .on_press(run(MenuAction::DownloadFolder {
                username: u.clone(),
                folder: d.clone(),
            })),
        )
        .child(
            action(
                "details-browse",
                "Browse user",
                icons::user(10.0, t.text),
                t,
            )
            .help(format!("{u}'s shares, at this folder"))
            .on_press(run(MenuAction::OpenFolder {
                username: u.clone(),
                folder: d.clone(),
                file: None,
            })),
        );

    let mut tracks = col().key("details-tracks").w_full().pb(8.0);
    let queue: std::rc::Rc<[wholeseek_core::RemoteFile]> = files.clone().into();
    for (i, f) in files.iter().enumerate() {
        let audio = crate::tabs::is_audio(f);
        let play: El = if crate::tabs::is_playable(f) {
            let (file, q) = (f.clone(), queue.clone());
            icon_button("play", icons::play(9.0, t.text_secondary), t)
                .size(22.0, 22.0)
                .label("Play")
                .on_press(move |s: &mut App| s.play_in(&file, &q))
        } else {
            el().w(22.0)
        };
        let len = f
            .attrs
            .duration_secs
            .map(|d| fmt::mmss(f64::from(d)))
            .unwrap_or_default();
        tracks = tracks.child(
            row()
                .key(format!("track-{i}"))
                .h(24.0)
                .w_full()
                .px(12.0)
                .gap(8.0)
                .center_y()
                .hover(t.hover)
                .help(fmt::quality(f))
                .child(play)
                .child(
                    text(f.basename().to_string())
                        .key("name")
                        .font(type_::BODY)
                        .color(if audio {
                            t.text
                        } else {
                            t.text_secondary.alpha(0.6)
                        })
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(text(len).font(tnum(META)).color(t.text_secondary)),
        );
    }

    base.child(
        col()
            .w_full()
            .px(16.0)
            .pt(14.0)
            .pb(12.0)
            .gap(8.0)
            .child(
                text(name)
                    .key("details-title")
                    .font(type_::TITLE)
                    .color(t.text)
                    .truncate()
                    .help(fmt::crumbs(folder)),
            )
            .child(who)
            .child(
                text(summary.join(" · "))
                    .key("details-summary")
                    .font(tnum(META))
                    .color(t.text_secondary),
            )
            .child(actions),
    )
    .child(hairline(t))
    .child(
        tracks
            .pt(6.0)
            .key(format!("details-list-{}", tab.id))
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
    )
}
