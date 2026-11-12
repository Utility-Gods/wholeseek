//! A search tab's inspector: shown only while a folder is selected (click a folder line or a
//! file), × or Esc closes it until the next selection. A fixed column right of the results
//! on a wide window; below [`OVERLAY_BELOW`] it lies over them from the right. An album
//! shows the shared [`album_header`](super::album_header) and its tracks; a one-file folder
//! a compact card for that file.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollSpec, col, el, row, text};
use wholeseek_core::RemoteFile;

use super::album_header;
use super::files::{action, download_cell, equalizer, now_playing, was_downloaded};
use super::{META, accent, amber, green, hairline, icon_button, icons, tnum};
use crate::app::App;
use crate::fmt;
use crate::menu::{DlTarget, MenuAction};
use crate::tabs::{SearchTab, Tab};

/// The inspector's width (points).
pub const INSPECTOR_W: f32 = 380.0;
/// Below this window width the inspector lies over the results instead of beside them.
pub const OVERLAY_BELOW: f32 = 1100.0;
/// The results table never gets wider than this (the name column tops out near 680 pt), so
/// Length, Size and Quality stay next to the names. A cap only: narrower windows shrink it.
pub const TABLE_MAX_W: f32 = 1110.0;

/// Whether the inspector lies over the results (narrow window) rather than beside them.
pub fn overlays(s: &App) -> bool {
    s.window_w > 0.0 && s.window_w < OVERLAY_BELOW
}

/// The selected folder, if it still has listed results.
pub fn selection<'a>(tab: &'a Tab, st: &SearchTab) -> Option<&'a (String, String)> {
    tab.group.as_ref().filter(|(u, d)| {
        st.search.as_ref().is_some_and(|v| {
            v.hits
                .iter()
                .any(|h| &h.file.username == u && h.file.folder() == d)
        })
    })
}

/// The inspector for the tab's selection, or `None` (no inspector at all).
pub fn inspector(s: &App, tab: &Tab, st: &SearchTab, t: &Theme) -> Option<El> {
    let (username, folder) = selection(tab, st)?;
    let files = s.folder_files(username, folder);
    let hit = st
        .search
        .as_ref()
        .and_then(|v| v.hits.iter().find(|h| &h.file.username == username));
    let (free, speed) = hit.map_or((false, 0), |h| (h.free_slot, h.speed));
    let close = icon_button(
        "details-close",
        icons::close(icons::MD, t.text_secondary),
        t,
    )
    .size(24.0, 24.0)
    .label("Close")
    .help("Close (Esc)")
    .on_press(|s: &mut App| {
        s.tab_mut().close_inspector();
    });
    let base = col().key("details").w(INSPECTOR_W).h_full().fill(t.bg);
    let body = if let [f] = files.as_slice() {
        let card = card(s, f, free, speed, t);
        let card = super::art::above(s, username, folder, album_header::INSET, card, t);
        base.child(top(card, close))
    } else {
        let header = album(username, folder, &files, free, speed, t);
        base.child(top(header, close))
            .child(hairline(t))
            .child(tracks(s, tab, &files, t))
    };
    Some(body)
}

/// The album's header: title, user line, summary, then the folder actions (wrapping in the
/// narrow column). Same keys and content as [`album_header::view`] with prefix "details-".
// TODO(inspector): use album_header::view once its actions row wraps.
fn album(
    username: &str,
    folder: &str,
    files: &[RemoteFile],
    free: bool,
    speed: u32,
    t: &Theme,
) -> El {
    let name = folder.rsplit('\\').next().unwrap_or(folder).to_string();
    let (u, d) = (username.to_string(), folder.to_string());
    let run = |a: MenuAction| {
        move |s: &mut App| {
            s.run_menu(a.clone());
        }
    };
    let mut actions = row()
        .key("details-actions")
        .w_full()
        .flex_wrap()
        .gap(6.0)
        .center_y();
    if files.iter().any(crate::tabs::is_playable) {
        actions = actions.child(
            action(
                "details-play",
                "Play album",
                icons::draw(&icons::LIST_MUSIC, icons::MD, t.text),
                t,
            )
            .on_press(run(MenuAction::PlayFolder {
                username: u.clone(),
                folder: d.clone(),
            })),
        );
    }
    let target = DlTarget::Folder {
        username: u.clone(),
        folder: d.clone(),
    };
    actions = actions
        .child(
            action(
                "details-download",
                "Download folder",
                icons::down(icons::MD, t.text),
                t,
            )
            .on_press(run(MenuAction::DownloadFolder {
                username: u.clone(),
                folder: d.clone(),
            })),
        )
        .child(download_to_button(
            target,
            "Save this folder somewhere else",
            t,
        ))
        .child(
            action(
                "details-browse",
                "Browse user",
                icons::user(icons::MD, t.text),
                t,
            )
            .help(format!("{u}'s shares, at this folder"))
            .on_press(run(MenuAction::OpenFolder {
                username: u.clone(),
                folder: d.clone(),
                file: None,
            })),
        );
    col()
        .key("details-header")
        .w_full()
        .pl(album_header::INSET)
        .pr(album_header::INSET)
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
        .child(who(username, free, speed, t))
        .child(
            text(album_header::summary(files))
                .key("details-summary")
                .font(tnum(META))
                .color(t.text_secondary),
        )
        .child(actions)
}

/// "Download to…": the recent folders as a menu, or the folder picker when there are none.
fn download_to_button(target: DlTarget, help: &str, t: &Theme) -> El {
    action(
        "details-download-to",
        "Download to…",
        icons::draw(&icons::FOLDER_DOWN, icons::MD, t.text),
        t,
    )
    .help(help.to_string())
    .on_press_tx(move |s: &mut App, tx| {
        let recent = s.backend.settings().recent_download_dirs.clone();
        if recent.is_empty() {
            crate::actions::choose_download_dir(s, tx, target.clone());
        } else {
            let items = crate::menu::download_to_items(&target, &recent);
            s.open_menu(
                crate::menu::MenuAnchor::Key("details-download-to".into()),
                items,
            );
        }
    })
}

/// The header with × in the top-right corner (the header's text stops short of it).
fn top(header: El, close: El) -> El {
    row()
        .w_full()
        .child(col().grow(1.0).shrink().min_w(0.0).child(header))
        .child(col().pt(10.0).pr(8.0).child(close))
}

/// The online dot, user (opens their tab), speed and slot.
fn who(username: &str, free: bool, speed: u32, t: &Theme) -> El {
    let user = username.to_string();
    row()
        .key("details-user")
        .gap(8.0)
        .center_y()
        .child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(if free { green() } else { amber() }),
        )
        .child(
            text(username.to_string())
                .font(type_::LABEL)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
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
        )
}

/// One file alone in its folder: what it is and what to do with it (no one-row track list).
fn card(s: &App, f: &RemoteFile, free: bool, speed: u32, t: &Theme) -> El {
    let (playing, running) = now_playing(s, f);
    let mut facts = vec![fmt::size(f.size), fmt::quality(f)];
    if let Some(d) = f.attrs.duration_secs {
        facts.push(fmt::mmss(f64::from(d)));
    }
    facts.retain(|x| !x.is_empty());
    let folder = f.folder().to_string();
    let mut actions = row()
        .key("details-actions")
        .w_full()
        .flex_wrap()
        .gap(6.0)
        .center_y();
    if crate::tabs::is_playable(f) {
        let file = f.clone();
        let icon = if playing && running {
            equalizer(s.now, true, accent())
        } else {
            icons::draw(&icons::PLAY, icons::MD, t.text)
        };
        actions = actions.child(
            action("details-play", "Play", icon, t)
                .on_press(move |s: &mut App| s.play_in(&file, std::slice::from_ref(&file))),
        );
    }
    actions = match s.download_of(f) {
        Some(_) => actions.child(download_cell(s, f, false, t)),
        None => {
            let file = f.clone();
            actions.child(
                action(
                    "details-download",
                    "Download",
                    icons::down(icons::MD, t.text),
                    t,
                )
                .on_press(move |s: &mut App| s.download(&file)),
            )
        }
    };
    actions = actions
        .child(download_to_button(
            DlTarget::File(f.clone()),
            "Save this file somewhere else",
            t,
        ))
        .child({
            let a = MenuAction::OpenFolder {
                username: f.username.clone(),
                folder: folder.clone(),
                file: Some(f.path.clone()),
            };
            action(
                "details-browse",
                "Browse user",
                icons::user(icons::MD, t.text),
                t,
            )
            .help(format!("{}'s shares, at this file", f.username))
            .on_press(move |s: &mut App| {
                s.run_menu(a.clone());
            })
        });
    let mut c = col()
        .key("details-card")
        .w_full()
        .pl(album_header::INSET)
        .pr(album_header::INSET)
        .pt(14.0)
        .pb(14.0)
        .gap(8.0)
        .child(
            text(f.basename().to_string())
                .key("details-title")
                .font(type_::TITLE)
                .color(if playing { accent() } else { t.text })
                .wrap_anywhere()
                .help(fmt::crumbs(&f.path)),
        )
        .child(who(&f.username, free, speed, t))
        .child(
            text(facts.join(" · "))
                .key("details-summary")
                .font(tnum(META))
                .color(t.text_secondary),
        )
        .child(
            row()
                .key("details-folder")
                .gap(6.0)
                .center_y()
                .w_full()
                .help(fmt::crumbs(&folder))
                .child(icons::draw(&icons::FOLDER, icons::SM, t.text_secondary))
                .child(
                    text(folder.rsplit('\\').next().unwrap_or(&folder).to_string())
                        .font(META)
                        .color(t.text_secondary)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                ),
        );
    if was_downloaded(s, f) {
        c = c.child(
            row()
                .key("details-downloaded")
                .gap(6.0)
                .center_y()
                .help("Downloaded")
                .child(icons::check(icons::SM, t.text_secondary))
                .child(text("Downloaded").font(META).color(t.text_secondary)),
        );
    }
    c.child(el().h(2.0)).child(actions)
}

/// The album's tracks in natural order: ▶ (the playing one shows the equalizer), name,
/// length, and ↓ / its download state (album folder kept) or the muted "Downloaded" ✓.
fn tracks(s: &App, tab: &Tab, files: &[RemoteFile], t: &Theme) -> El {
    let mut list = col().w_full().pb(8.0);
    let queue: std::rc::Rc<[RemoteFile]> = files.to_vec().into();
    for (i, f) in files.iter().enumerate() {
        let audio = crate::tabs::is_audio(f);
        let (playing, running) = now_playing(s, f);
        let play: El = if crate::tabs::is_playable(f) {
            let (file, q) = (f.clone(), queue.clone());
            let icon = if playing && running {
                equalizer(s.now, true, accent())
            } else {
                icons::play(icons::SM, if playing { accent() } else { t.text_secondary })
            };
            icon_button("play", icon, t)
                .size(22.0, 22.0)
                .label(if playing && running { "Pause" } else { "Play" })
                .on_press(move |s: &mut App| s.play_in(&file, &q))
        } else {
            el().w(22.0)
        };
        let len = f
            .attrs
            .duration_secs
            .map(|d| fmt::mmss(f64::from(d)))
            .unwrap_or_default();
        let fg = if !audio {
            t.text_secondary.alpha(0.6)
        } else if playing {
            accent()
        } else {
            t.text
        };
        let mut r = row()
            .key(format!("track-{i}"))
            .h(26.0)
            .w_full()
            .pl(10.0)
            .pr(8.0)
            .gap(8.0)
            .center_y()
            .hover(t.hover)
            .hover_group()
            .help(fmt::quality(f))
            .child(play)
            .child(
                text(f.basename().to_string())
                    .key("name")
                    .font(type_::BODY)
                    .color(fg)
                    .truncate()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0),
            )
            .child(text(len).font(tnum(META)).color(t.text_secondary))
            .child(
                row()
                    .w(40.0)
                    .h(24.0)
                    .center()
                    .child(download_cell(s, f, true, t)),
            );
        if playing {
            r = r.fill(accent().alpha(0.07));
        }
        list = list.child(r);
    }
    list.pt(6.0)
        .key(format!("details-list-{}", tab.id))
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .shrink()
        .min_h(0.0)
}
