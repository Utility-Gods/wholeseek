//! A search tab's album view: every user's copy of an album merged into one row (best copy,
//! its edition, how many copies), then single tracks the same way. Click selects the best
//! copy (the inspector shows it and the other copies); ▶ plays it, ↓ downloads it.

use std::rc::Rc;

use rune::tok::{Theme, type_};
use rune::{Cursor, El, Key, KeyEvent, PointerButton, col, el, row, text};
use wholeseek_core::RemoteFile;
use wholeseek_core::group::{Album, Edition, Grouped, Source, Track};

use super::files::{download_cell, equalizer, now_playing};
use super::{META, accent, amber, green, icon_button, icons, tnum};
use crate::app::{App, MAX_ROWS};
use crate::fmt;
use crate::menu::MenuAction;
use crate::tabs::Tab;

const PLAY_W: f32 = 24.0;
const WHO_W: f32 = 190.0;
const DL_W: f32 = 56.0;

/// "FLAC 16-bit 44.1 kHz".
pub fn edition_label(e: &Edition) -> String {
    if e.quality.is_empty() {
        e.format.clone()
    } else {
        format!("{} {}", e.format, e.quality)
    }
}

/// The (user, folder) a source's rows live in: its first file's folder, so a multi-disc
/// album opens at a folder the results list.
pub fn source_folder(src: &Source) -> String {
    src.files
        .first()
        .map_or_else(|| src.folder.clone(), |f| f.folder().to_string())
}

fn copies(n: usize) -> String {
    format!("{n} {}", if n == 1 { "copy" } else { "copies" })
}

/// The album and track sections (rows inside the results' scroll list).
pub fn list(
    s: &App,
    tab: &Tab,
    g: &Grouped,
    find: Option<&crate::tabs::TextQuery>,
    t: &Theme,
) -> El {
    let mut list = col().w_full().pb(8.0);
    let mut drawn = 0;
    if !g.albums.is_empty() {
        list = list.child(section("albums-head", "Albums", g.albums.len(), t));
        for (i, a) in g.albums.iter().enumerate() {
            if drawn == MAX_ROWS {
                return list;
            }
            drawn += 1;
            if let Some(r) = album_row(s, tab, i, a, find, t) {
                list = list.child(r);
            }
        }
    }
    if !g.tracks.is_empty() {
        list = list.child(section("tracks-head", "Tracks", g.tracks.len(), t));
        for (i, tr) in g.tracks.iter().enumerate() {
            if drawn == MAX_ROWS {
                return list;
            }
            drawn += 1;
            if let Some(r) = track_row(s, tab, i, tr, find, t) {
                list = list.child(r);
            }
        }
    }
    list
}

fn section(key: &str, title: &str, n: usize, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(30.0)
        .w_full()
        .max_w(super::details::RESULTS_MAX_W)
        .px(12.0)
        .pt(6.0)
        .gap(8.0)
        .center_y()
        .child(text(title.to_string()).font(type_::LABEL).color(t.text))
        .child(text(n.to_string()).font(tnum(META)).color(t.text_secondary))
}

/// The best copy's user: online dot, name, speed (and "queued" without a free slot).
fn who(src: &Source, find: Option<&crate::tabs::TextQuery>, t: &Theme) -> El {
    let slot = if src.free_slot {
        fmt::speed(f64::from(src.speed))
    } else {
        format!("{} · queued", fmt::speed(f64::from(src.speed)))
    };
    row()
        .w(WHO_W)
        .gap(6.0)
        .center_y()
        .child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(if src.free_slot { green() } else { amber() }),
        )
        .child(
            text(src.username.clone())
                .key("user")
                .font(META)
                .color(t.text)
                .spans(super::files::match_spans(find, &src.username))
                .truncate()
                .shrink(),
        )
        .child(text(slot).font(tnum(META)).color(t.text_secondary))
}

fn selected(tab: &Tab, editions: &[Edition]) -> bool {
    tab.group.as_ref().is_some_and(|(u, d)| {
        editions
            .iter()
            .flat_map(|e| &e.sources)
            .any(|s| &s.username == u && source_folder(s) == *d)
    })
}

/// A row's menu, the same from its ⋯ and a right-click.
type MenuFn = Rc<dyn Fn(&App) -> Vec<crate::menu::MenuItem>>;

/// A row's frame: hover, selection, click selects `src` (the inspector shows it), Enter
/// plays, right-click opens `menu`, middle-click opens its folder in a new tab.
fn frame(
    key: String,
    h: f32,
    src: &Source,
    sel: bool,
    play: Rc<dyn Fn(&mut App)>,
    menu: MenuFn,
    t: &Theme,
) -> El {
    let pick = (src.username.clone(), source_folder(src));
    let mut r = row()
        .key(key)
        .h(h)
        .w_full()
        .max_w(super::details::RESULTS_MAX_W)
        .pl(12.0)
        .pr(6.0)
        .gap(10.0)
        .center_y()
        .hover(t.hover)
        .hover_group()
        .cursor(Cursor::Pointer)
        .focusable(true)
        .focus_ring(1.0, -1.0, accent().alpha(0.6))
        .pointer_buttons([PointerButton::Primary, PointerButton::Middle])
        .on_press_tx(move |s: &mut App, tx| {
            if tx.button() == PointerButton::Middle {
                s.open_in_new_tab(&pick.0, Some(&pick.1));
                return;
            }
            let tab = s.tab_mut();
            tab.group = Some(pick.clone());
            tab.selected = None;
        })
        .on_key(move |s: &mut App, e: &KeyEvent, _| {
            if e.key != Key::Enter {
                return false;
            }
            play(s);
            true
        })
        .on_secondary_press(move |s: &mut App, p, _| {
            let items = menu(s);
            super::menu::at_point(s, p, items);
        });
    if sel {
        r = r.fill(t.hover);
    }
    r
}

fn play_button(s: &App, files: &[RemoteFile], label: &str, t: &Theme) -> El {
    let Some(first) = files.iter().find(|f| crate::tabs::is_playable(f)).cloned() else {
        return el().w(PLAY_W);
    };
    let playing = files.iter().map(|f| now_playing(s, f)).find(|p| p.0);
    let icon = match playing {
        Some((_, true)) => equalizer(s.now, true, accent()),
        Some(_) => icons::play(icons::SM, accent()),
        None => icons::play(icons::SM, t.text_secondary),
    };
    let queue: Rc<[RemoteFile]> = files.to_vec().into();
    icon_button("play", icon, t)
        .size(PLAY_W, 22.0)
        .label(label.to_string())
        .on_press(move |s: &mut App| s.play_in(&first, &queue))
}

fn album_row(
    s: &App,
    tab: &Tab,
    i: usize,
    a: &Album,
    find: Option<&crate::tabs::TextQuery>,
    t: &Theme,
) -> Option<El> {
    let best = a.best_source()?;
    let edition = a.best_edition()?;
    let playing = best.files.iter().any(|f| now_playing(s, f).0);
    let sel = selected(tab, &a.editions);
    let mut facts = vec![
        edition_label(edition),
        format!(
            "{} {}",
            a.key.track_count(),
            if a.key.track_count() == 1 {
                "track"
            } else {
                "tracks"
            }
        ),
    ];
    if a.key.total_secs() > 0 {
        facts.push(fmt::mmss(f64::from(a.key.total_secs())));
    }
    facts.push(copies(a.source_count()));
    let (u, d) = (best.username.clone(), source_folder(best));
    let files: Rc<[RemoteFile]> = best.files.clone().into();
    let play_files = files.clone();
    let play: Rc<dyn Fn(&mut App)> = Rc::new(move |s: &mut App| {
        if let Some(f) = play_files.iter().find(|f| crate::tabs::is_playable(f)) {
            s.play_in(f, &play_files);
        }
    });
    let download = {
        let (u, d) = (u.clone(), best.folder.clone());
        icon_button("download", icons::down(icons::MD, t.text), t)
            .label("Download album")
            .help(format!("Download {}'s copy", u))
            .group_opacity(if sel { 1.0 } else { 0.0 }, 1.0)
            .on_press(move |s: &mut App| {
                s.run_menu(MenuAction::DownloadFolder {
                    username: u.clone(),
                    folder: d.clone(),
                });
            })
    };
    let menu: MenuFn = {
        let (u, d) = (u.clone(), d.clone());
        Rc::new(move |s: &App| s.album_menu(&u, &d))
    };
    let r = frame(format!("album-{i}"), 48.0, best, sel, play, menu.clone(), t)
        .help(fmt::crumbs(&d))
        .child(play_button(s, &files, "Play album", t))
        .child(super::art::row_slot(s, "thumb", &u, &d, super::art::ROW))
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(a.title.clone())
                        .key("title")
                        .font(type_::BODY)
                        .color(if playing { accent() } else { t.text })
                        .spans(super::files::match_spans(find, &a.title))
                        .truncate(),
                )
                .child(
                    text(facts.join(" · "))
                        .key("meta")
                        .font(tnum(META))
                        .color(t.text_secondary)
                        .truncate(),
                ),
        )
        .child(who(best, find, t))
        .child(row().w(DL_W).h(28.0).center().child(download))
        .child(more_button(
            format!("album-more-{i}"),
            sel,
            move |s: &App| menu(s),
            t,
        ));
    Some(r)
}

fn track_row(
    s: &App,
    tab: &Tab,
    i: usize,
    tr: &Track,
    find: Option<&crate::tabs::TextQuery>,
    t: &Theme,
) -> Option<El> {
    let best = tr.best_source()?;
    let file = best.files.first()?.clone();
    let edition = tr
        .editions
        .iter()
        .find(|e| e.sources.iter().any(|s| std::ptr::eq(s, best)))?;
    let (playing, _) = now_playing(s, &file);
    let sel = selected(tab, &tr.editions);
    let name = file.basename().to_string();
    let mut facts = vec![edition_label(edition)];
    if let Some(d) = file.attrs.duration_secs {
        facts.push(fmt::mmss(f64::from(d)));
    }
    facts.push(fmt::size(file.size));
    if tr.source_count() > 1 {
        facts.push(copies(tr.source_count()));
    }
    let one: Rc<[RemoteFile]> = vec![file.clone()].into();
    let f2 = file.clone();
    let play: Rc<dyn Fn(&mut App)> = Rc::new(move |s: &mut App| {
        s.play_in(&f2, std::slice::from_ref(&f2));
    });
    let menu: MenuFn = {
        let f = file.clone();
        Rc::new(move |s: &App| s.file_menu(&f, false))
    };
    let r = frame(
        format!("single-{i}"),
        30.0,
        best,
        sel,
        play,
        menu.clone(),
        t,
    )
    .help(fmt::crumbs(&file.path))
    .child(play_button(s, &one, "Play", t))
    .child(
        text(name.clone())
            .key("title")
            .font(type_::BODY)
            .color(if playing { accent() } else { t.text })
            .spans(super::files::match_spans(find, &name))
            .truncate()
            .grow(1.0)
            .shrink(),
    )
    .child(
        text(facts.join(" · "))
            .key("meta")
            .font(tnum(META))
            .color(t.text_secondary)
            .truncate()
            .shrink(),
    )
    .child(who(best, find, t))
    .child(
        row()
            .w(DL_W)
            .h(28.0)
            .center()
            .child(download_cell(s, &file, false, t)),
    )
    .child(more_button(
        format!("single-more-{i}"),
        sel,
        move |s: &App| menu(s),
        t,
    ));
    Some(r)
}

/// A row's ⋯: its menu, anchored to this button (its key is unique on the page, so the
/// menu finds it). Shown on hover and when the row is selected.
pub(super) fn more_button(
    key: String,
    shown: bool,
    items: impl Fn(&App) -> Vec<crate::menu::MenuItem> + 'static,
    t: &Theme,
) -> El {
    let anchor = key.clone();
    icon_button(&key, icons::more(icons::MD, t.text_secondary), t)
        .size(24.0, 22.0)
        .label("More")
        .group_opacity(if shown { 1.0 } else { 0.0 }, 1.0)
        .on_press(move |s: &mut App| {
            let items = items(s);
            s.open_menu(crate::menu::MenuAnchor::Key(anchor.clone()), items);
        })
}
