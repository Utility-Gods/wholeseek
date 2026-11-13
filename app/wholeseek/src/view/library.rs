//! The Library tab: the user's own music, Albums or Songs, a filter, ☆ stars, scan progress.
//!
//! Every list is a [`virtual_list`] over the index vectors of [`crate::library::Rows`]: only
//! the rows in view are built. Row handlers carry the song's path or the album's key, never
//! the row index. Each row's menu opens from right-click and from its ⋯.

use std::rc::Rc;

use rune::tok::{TextStyle, Theme, type_};
use rune::{Cursor, El, Key, KeyEvent, col, el, row, text};
use wholeseek_core::library::{Album, Track};

use super::files::{equalizer, now_playing};
use super::virtual_list::virtual_list;
use super::{META, accent, field_box, icon_button, icons, input, progress, tnum};
use crate::app::{App, Panel};
use crate::fmt;
use crate::library::{LOCAL_USER, LibView, SongSort, quality_label, track_quality};
use crate::tabs::TabKind;

const SONG_H: f32 = 28.0;
const ALBUM_H: f32 = 40.0;
const THUMB: f32 = 30.0;
const ARTIST_W: f32 = 190.0;
const ALBUM_W: f32 = 220.0;
const YEAR_W: f32 = 44.0;
const COUNT_W: f32 = 64.0;
const LEN_W: f32 = 52.0;
const FORMAT_W: f32 = 112.0;
const NUM_W: f32 = 28.0;
const BTN_W: f32 = 24.0;

/// The strip's Library button: shows (or opens) the Library tab.
pub(super) fn strip_button(s: &App, t: &Theme) -> El {
    let on = matches!(s.tab().kind, TabKind::Library);
    let b = row()
        .key("library")
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Library")
        .help("Your music folders")
        .on_press(|s: &mut App| s.open_library())
        .child(icons::draw(
            &icons::LIBRARY,
            icons::MD,
            if on { accent() } else { t.text_secondary },
        ))
        .child(text("Library").font(type_::LABEL).color(if on {
            t.text
        } else {
            t.text_secondary
        }));
    if on { b.fill(t.hover) } else { b }
}

pub(super) fn page(s: &App, t: &Theme) -> El {
    let ui = &s.library;
    let body = if s.backend.settings().library_dirs.is_empty() && ui.index.is_empty() {
        empty(
            "No library folders",
            "Add the folders your music is in. Files are played where they are.",
            t,
        )
    } else if ui.index.is_empty() && (ui.scanning() || !ui.loaded) {
        empty("Scanning your music…", "", t)
    } else if ui.index.is_empty() {
        empty(
            "No music found",
            "None of the library folders has audio files.",
            t,
        )
    } else {
        match (
            ui.view,
            ui.open_album.as_deref().and_then(|k| ui.album_by_key(k)),
        ) {
            (_, Some((_, a))) => album_page(s, a, t),
            (LibView::Albums, None) => albums(s, t),
            (LibView::Songs, None) => songs(s, t),
        }
    };
    col()
        .key("library-page")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(toolbar(s, t))
        .child(super::hairline(t))
        .child(body)
}

// ---- toolbar ------------------------------------------------------------------------------

fn toggle(key: &str, icon: &'static rune::Icon, label: &str, on: bool, t: &Theme) -> El {
    let ink = if on { t.text } else { t.text_secondary };
    let b = row()
        .key(key.to_string())
        .h(26.0)
        .px(10.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .checked(on)
        .label(label.to_string())
        .child(icons::draw(
            icon,
            icons::MD,
            if on { accent() } else { ink },
        ))
        .child(text(label.to_string()).font(type_::LABEL).color(ink));
    if on { b.fill(t.hover) } else { b }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn toolbar(s: &App, t: &Theme) -> El {
    let ui = &s.library;
    let rows = ui.rows();
    let filter = field_box(
        "lib-filter-box",
        input(&ui.filter, "Filter library", t)
            .key("lib-filter")
            .font(META)
            .on_edit(|s: &mut App| &mut s.library.filter)
            .on_input_key(|s: &mut App, e, tx| {
                if e.key != Key::Escape {
                    return false;
                }
                if s.library.filter.text.is_empty() {
                    tx.blur();
                } else {
                    s.library.filter.replace_text("");
                }
                true
            }),
        t,
    )
    .w(240.0)
    .h(26.0);
    let status: El = if ui.progress.scanning {
        let p = ui.progress;
        let (line, frac) = if p.to_read > 0 {
            (
                format!("Reading {} / {}", thousands(p.read), thousands(p.to_read)),
                p.read as f32 / p.to_read as f32,
            )
        } else {
            (format!("Scanning… {}", thousands(p.found)), 0.0)
        };
        row()
            .key("lib-scan")
            .gap(8.0)
            .center_y()
            .child(text(line).font(tnum(META)).color(t.text_secondary))
            .child(progress("lib-scan-bar", frac, 80.0, accent(), t))
    } else {
        let (n, of, what) = match ui.view {
            LibView::Songs => (rows.songs.len(), ui.index.tracks.len(), "songs"),
            LibView::Albums => (rows.albums.len(), ui.index.albums.len(), "albums"),
        };
        let line = if n == of {
            format!("{} {what}", thousands(n))
        } else {
            format!("{} of {} {what}", thousands(n), thousands(of))
        };
        let help = ui.last.map_or_else(String::new, |r| {
            format!(
                "Last scan: {} files, {} read, {} gone, {:.1} s",
                thousands(r.tracks),
                thousands(r.reread),
                thousands(r.removed),
                r.elapsed.as_secs_f64()
            )
        });
        text(line)
            .key("lib-count")
            .font(tnum(META))
            .color(t.text_secondary)
            .help(help)
    };
    let listed = match ui.view {
        LibView::Songs => !rows.songs.is_empty(),
        LibView::Albums => !rows.albums.is_empty(),
    };
    let mut bar = row()
        .key("lib-toolbar")
        .h(40.0)
        .w_full()
        .px(12.0)
        .gap(6.0)
        .center_y()
        .child(
            toggle(
                "lib-albums",
                &icons::DISC_3,
                "Albums",
                ui.view == LibView::Albums,
                t,
            )
            .on_press(|s: &mut App| s.set_library_view(LibView::Albums)),
        )
        .child(
            toggle(
                "lib-songs",
                &icons::MUSIC,
                "Songs",
                ui.view == LibView::Songs,
                t,
            )
            .on_press(|s: &mut App| s.set_library_view(LibView::Songs)),
        )
        .child(el().w(6.0))
        .child(filter)
        .child(
            toggle(
                "lib-starred",
                if ui.starred_only {
                    &icons::STAR_SOLID
                } else {
                    &icons::STAR
                },
                "Starred",
                ui.starred_only,
                t,
            )
            .help("Only starred songs and albums")
            .on_press(|s: &mut App| s.toggle_library_starred()),
        )
        .child(el().grow(1.0))
        .child(status);
    if listed && ui.open_album.is_none() {
        bar = bar.child(
            super::files::action(
                "lib-play-all",
                "Play all",
                icons::play(icons::SM, t.text),
                t,
            )
            .help("Play the listed songs in order")
            .on_press(|s: &mut App| s.play_library_list()),
        );
    }
    bar.child(
        icon_button(
            "lib-rescan",
            icons::draw(&icons::REFRESH_CW, icons::MD, t.text_secondary),
            t,
        )
        .label("Rescan")
        .help("Rescan library folders (only changed files are read)")
        .on_press(|s: &mut App| s.rescan_library()),
    )
}

fn empty(title: &str, line: &str, t: &Theme) -> El {
    let mut c = col()
        .key("lib-empty")
        .w_full()
        .grow(1.0)
        .center()
        .gap(8.0)
        .child(text(title.to_string()).font(type_::TITLE).color(t.text));
    if !line.is_empty() {
        c = c
            .child(
                text(line.to_string())
                    .font(type_::BODY)
                    .color(t.text_secondary),
            )
            .child(
                super::files::action(
                    "lib-folders",
                    "Library folders…",
                    icons::draw(&icons::FOLDER, icons::MD, t.text_secondary),
                    t,
                )
                .help("Settings → Library folders")
                .on_press(|s: &mut App| {
                    if s.panel != Panel::Settings {
                        s.toggle_panel(Panel::Settings);
                    }
                }),
            );
    }
    c
}

fn nothing_matches(t: &Theme) -> El {
    col()
        .key("lib-none")
        .w_full()
        .grow(1.0)
        .center()
        .gap(6.0)
        .child(
            text("Nothing matches")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(
            super::files::action(
                "lib-clear",
                "Clear filter",
                icons::close(icons::SM, t.text_secondary),
                t,
            )
            .on_press(|s: &mut App| {
                s.library.filter.replace_text("");
                s.library.starred_only = false;
            }),
        )
}

// ---- shared row parts ---------------------------------------------------------------------

trait ChildAll {
    fn child_all<const N: usize>(self, kids: [El; N]) -> Self;
}

impl ChildAll for El {
    fn child_all<const N: usize>(self, kids: [El; N]) -> Self {
        kids.into_iter().fold(self, El::child)
    }
}

fn cell(s: impl Into<String>, w: f32, t: &Theme) -> El {
    text(s.into())
        .font(META)
        .color(t.text_secondary)
        .truncate()
        .w(w)
}

fn num_cell(s: impl Into<String>, w: f32, t: &Theme) -> El {
    text(s.into())
        .font(tnum(META))
        .color(t.text_secondary)
        .align(rune::Align::End)
        .w(w)
}

fn star_button(key: String, starred: bool, shown: bool, t: &Theme) -> El {
    let icon = if starred {
        icons::draw(&icons::STAR_SOLID, icons::SM, accent())
    } else {
        icons::draw(&icons::STAR, icons::SM, t.text_secondary)
    };
    icon_button(&key, icon, t)
        .size(BTN_W, 22.0)
        .label(if starred { "Unstar" } else { "Star" })
        .help(if starred { "Unstar" } else { "Star" })
        .group_opacity(if starred || shown { 1.0 } else { 0.0 }, 1.0)
}

fn more(
    key: String,
    shown: bool,
    items: impl Fn(&App) -> Vec<crate::menu::MenuItem> + 'static,
    t: &Theme,
) -> El {
    super::albums::more_button(key, shown, items, t)
}

/// A header cell: sorts the Songs list by `by`, the arrow on the current one.
fn head(s: &App, label: &str, by: SongSort, w: Option<f32>, end: bool, t: &Theme) -> El {
    let ui = &s.library;
    let on = ui.sort == by;
    let mut r = row()
        .key(format!("lib-sort-{}", label.to_lowercase()))
        .h(24.0)
        .gap(2.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("button")
        .label(format!("Sort by {}", label.to_lowercase()))
        .on_press(move |s: &mut App| s.sort_library(by));
    r = match w {
        Some(w) => r.w(w),
        None => r.grow(1.0).min_w(0.0).shrink(),
    };
    if end {
        r = r.child(el().grow(1.0));
    }
    r = r.child(
        text(label.to_string())
            .font(TextStyle::new(META.size, if on { 600.0 } else { 450.0 }))
            .color(if on { t.text } else { t.text_secondary }),
    );
    if on {
        r = r.child(icons::draw(
            if ui.desc {
                &icons::CHEVRON_DOWN
            } else {
                &icons::CHEVRON_UP
            },
            12.0,
            t.text_secondary,
        ));
    }
    r
}

/// The ▶ slot of a song row: the equalizer on the playing one, ▶ on hover.
fn play_slot(s: &App, tr: &Track, t: &Theme) -> El {
    let (playing, running) = now_playing(s, &tr.remote_file());
    let path = tr.path_str.clone();
    let icon = if playing {
        equalizer(s.now, running, accent())
    } else {
        icons::play(icons::SM, t.text_secondary)
    };
    icon_button("play", icon, t)
        .size(BTN_W, 22.0)
        .label("Play")
        .group_opacity(if playing { 1.0 } else { 0.0 }, 1.0)
        .on_press(move |s: &mut App| s.play_library_song(&path))
}

/// A song row's frame: hover, selection, click selects, Enter plays, right-click menu.
fn song_frame(s: &App, tr: &Track, t: &Theme) -> El {
    let path = tr.path_str.clone();
    let sel = s.library.selected.as_deref() == Some(path.as_str());
    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path);
    let r = row()
        .h(SONG_H)
        .w_full()
        .pl(8.0)
        .pr(8.0)
        .gap(10.0)
        .center_y()
        .hover(t.hover)
        .hover_group()
        .cursor(Cursor::Pointer)
        .focusable(true)
        .focus_ring(1.0, -1.0, accent().alpha(0.6))
        .help(fmt::home_short(&tr.path_str))
        .on_press(move |s: &mut App| s.library.selected = Some(p1.clone()))
        .on_double_press(move |s: &mut App| s.play_library_song(&p2))
        .on_key(move |s: &mut App, e: &KeyEvent, _| {
            if e.key != Key::Enter {
                return false;
            }
            s.play_library_song(&p3);
            true
        })
        .on_secondary_press(move |s: &mut App, p, _| {
            s.library.selected = Some(p4.clone());
            let items = s.library_song_menu(&p4);
            super::menu::at_point(s, p, items);
        });
    if sel { r.fill(t.hover) } else { r }
}

fn title_cell(tr: &Track, playing: bool, t: &Theme) -> El {
    text(tr.title.clone())
        .key("title")
        .font(type_::BODY)
        .color(if playing { accent() } else { t.text })
        .truncate()
        .grow(1.0)
        .min_w(0.0)
        .shrink()
}

fn song_tail(s: &App, i: usize, tr: &Track, t: &Theme) -> [El; 4] {
    let sel = s.library.selected.as_deref() == Some(tr.path_str.as_str());
    let starred = s.library.stars.track(&tr.path_str);
    let (p1, p2) = (tr.path_str.clone(), tr.path_str.clone());
    [
        num_cell(
            tr.duration_secs
                .map_or_else(String::new, |d| fmt::mmss(f64::from(d))),
            LEN_W,
            t,
        ),
        cell(track_quality(tr), FORMAT_W, t).pl(10.0),
        star_button(format!("lib-star-{i}"), starred, sel, t)
            .on_press(move |s: &mut App| s.toggle_star_song(&p1)),
        more(
            format!("lib-more-{i}"),
            sel,
            move |s: &App| s.library_song_menu(&p2),
            t,
        ),
    ]
}

// ---- Songs --------------------------------------------------------------------------------

fn song_row(s: &App, i: usize, t: &Theme) -> El {
    let ix = &s.library.index;
    let tr = &ix.tracks[i];
    let playing = now_playing(s, &tr.remote_file()).0;
    let album = &ix.albums[tr.album_index].title;
    song_frame(s, tr, t)
        .child(play_slot(s, tr, t))
        .child(title_cell(tr, playing, t))
        .child(cell(tr.artist.clone(), ARTIST_W, t))
        .child(cell(
            if album.is_empty() {
                "—".into()
            } else {
                album.clone()
            },
            ALBUM_W,
            t,
        ))
        .child_all(song_tail(s, i, tr, t))
}

fn songs(s: &App, t: &Theme) -> El {
    let rows = s.library.rows();
    let header = row()
        .key("lib-songs-head")
        .h(26.0)
        .w_full()
        .pl(8.0)
        .pr(8.0)
        .gap(10.0)
        .center_y()
        .child(el().w(BTN_W))
        .child(head(s, "Title", SongSort::Title, None, false, t))
        .child(head(
            s,
            "Artist",
            SongSort::Artist,
            Some(ARTIST_W),
            false,
            t,
        ))
        .child(head(s, "Album", SongSort::Album, Some(ALBUM_W), false, t))
        .child(head(s, "Length", SongSort::Length, Some(LEN_W), true, t))
        .child(head(s, "Format", SongSort::Format, Some(FORMAT_W), false, t).pl(10.0))
        .child(el().w(BTN_W * 2.0 + 10.0));
    if rows.songs.is_empty() {
        return col()
            .w_full()
            .grow(1.0)
            .child(header)
            .child(nothing_matches(t));
    }
    let version = (
        rows.fp,
        s.library_stars_rev(),
        playing_key(s),
        s.library.selected.clone(),
    );
    let r2 = rows.clone();
    let list = virtual_list(
        &s.library.songs_list,
        rows.songs.len(),
        SONG_H,
        move |s: &App, i| {
            let t = super::theme();
            match r2.songs.get(i) {
                Some(&ti) if ti < s.library.index.tracks.len() => song_row(s, ti, &t),
                _ => el().h(SONG_H),
            }
        },
    )
    .row_key(move |s: &App, i| {
        let rows = s.library.rows();
        rows.songs
            .get(i)
            .and_then(|&ti| s.library.index.tracks.get(ti))
            .map_or_else(|| format!("r{i}").into(), |t| t.path_str.clone().into())
    })
    .version(version)
    .pad(2.0)
    .build();
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(header)
        .child(super::hairline(t))
        .child(list)
}

fn playing_key(s: &App) -> Option<(String, bool)> {
    let p = s.playing.as_ref().filter(|p| p.file.is_local())?;
    Some((p.file.path.clone(), now_playing(s, &p.file).1))
}

// ---- Albums -------------------------------------------------------------------------------

fn album_row(s: &App, ai: usize, t: &Theme) -> El {
    let ui = &s.library;
    let a = &ui.index.albums[ai];
    let key = a.key.clone();
    let sel = ui.selected.as_deref() == Some(key.as_str());
    let playing = s
        .playing
        .as_ref()
        .filter(|p| p.file.is_local())
        .and_then(|p| ui.index.track_by_path(&p.file.path))
        .is_some_and(|i| a.tracks.contains(&i));
    let starred = ui.stars.album(&key);
    let (k1, k2, k3, k4, k5, k6) = (
        key.clone(),
        key.clone(),
        key.clone(),
        key.clone(),
        key.clone(),
        key,
    );
    let n = a.tracks.len();
    let r = row()
        .h(ALBUM_H)
        .w_full()
        .pl(8.0)
        .pr(8.0)
        .gap(10.0)
        .center_y()
        .hover(t.hover)
        .hover_group()
        .cursor(Cursor::Pointer)
        .focusable(true)
        .focus_ring(1.0, -1.0, accent().alpha(0.6))
        .help(fmt::home_short(&a.folder))
        .on_press(move |s: &mut App| s.open_library_album(&k1))
        .on_key(move |s: &mut App, e: &KeyEvent, _| {
            if e.key != Key::Enter {
                return false;
            }
            s.play_library_album(&k2);
            true
        })
        .on_secondary_press(move |s: &mut App, p, _| {
            s.library.selected = Some(k3.clone());
            let items = s.library_album_menu(&k3);
            super::menu::at_point(s, p, items);
        });
    let r = if sel { r.fill(t.hover) } else { r };
    let play = icon_button(
        "play",
        if playing {
            equalizer(s.now, playing_key(s).is_some_and(|p| p.1), accent())
        } else {
            icons::play(icons::SM, t.text_secondary)
        },
        t,
    )
    .size(BTN_W, 22.0)
    .label("Play album")
    .group_opacity(if playing { 1.0 } else { 0.0 }, 1.0)
    .on_press(move |s: &mut App| s.play_library_album(&k4));
    r.child(play)
        // No cover: the app's disc placeholder tile, so the column stays whole.
        .child(super::art::cover(
            s, "thumb", LOCAL_USER, &a.folder, THUMB, t,
        ))
        .child(
            text(if a.title.is_empty() {
                "Unknown album".to_string()
            } else {
                a.title.clone()
            })
            .key("title")
            .font(type_::BODY)
            .color(if playing { accent() } else { t.text })
            .truncate()
            .grow(1.0)
            .min_w(0.0)
            .shrink(),
        )
        .child(cell(a.artist.clone(), ARTIST_W, t))
        .child(num_cell(
            a.year.map_or_else(String::new, |y| y.to_string()),
            YEAR_W,
            t,
        ))
        .child(num_cell(
            format!("{n} {}", if n == 1 { "track" } else { "tracks" }),
            COUNT_W,
            t,
        ))
        .child(cell(quality_label(&a.quality), FORMAT_W, t).pl(10.0))
        .child(
            star_button(format!("lib-astar-{ai}"), starred, sel, t)
                .on_press(move |s: &mut App| s.toggle_star_album(&k5)),
        )
        .child(more(
            format!("lib-amore-{ai}"),
            sel,
            move |s: &App| s.library_album_menu(&k6),
            t,
        ))
}

fn albums(s: &App, t: &Theme) -> El {
    let rows = s.library.rows();
    if rows.albums.is_empty() {
        return nothing_matches(t);
    }
    let version = (
        rows.fp,
        s.library_stars_rev(),
        playing_key(s),
        s.library.selected.clone(),
    );
    let r2 = rows.clone();
    virtual_list(
        &s.library.albums_list,
        rows.albums.len(),
        ALBUM_H,
        move |s: &App, i| {
            let t = super::theme();
            match r2.albums.get(i) {
                Some(&ai) if ai < s.library.index.albums.len() => album_row(s, ai, &t),
                _ => el().h(ALBUM_H),
            }
        },
    )
    .row_key(move |s: &App, i| {
        let rows = s.library.rows();
        rows.albums
            .get(i)
            .and_then(|&ai| s.library.index.albums.get(ai))
            .map_or_else(|| format!("r{i}").into(), |a| a.key.clone().into())
    })
    .version(version)
    .pad(4.0)
    .build()
}

// ---- one album ----------------------------------------------------------------------------

fn album_track_row(s: &App, i: usize, t: &Theme) -> El {
    let ix = &s.library.index;
    let tr = &ix.tracks[i];
    let a = &ix.albums[tr.album_index];
    let playing = now_playing(s, &tr.remote_file()).0;
    let number = match (tr.disc, tr.track) {
        (Some(d), Some(n)) if a.tracks.iter().any(|&j| ix.tracks[j].disc.unwrap_or(1) > 1) => {
            format!("{d}.{n}")
        }
        (_, Some(n)) => n.to_string(),
        _ => String::new(),
    };
    let mut r = song_frame(s, tr, t)
        .child(play_slot(s, tr, t))
        .child(num_cell(number, NUM_W, t))
        .child(title_cell(tr, playing, t));
    // Compilations: each track's artist.
    if tr.artist != a.artist {
        r = r.child(cell(tr.artist.clone(), ARTIST_W, t));
    }
    r.child_all(song_tail(s, i, tr, t))
}

fn album_page(s: &App, a: &Album, t: &Theme) -> El {
    let key = a.key.clone();
    let starred = s.library.stars.album(&key);
    let n = a.tracks.len();
    let mut facts = vec![a.artist.clone()];
    if let Some(y) = a.year {
        facts.push(y.to_string());
    }
    facts.push(format!("{n} {}", if n == 1 { "track" } else { "tracks" }));
    if a.duration_secs > 0 {
        facts.push(fmt::mmss(a.duration_secs as f64));
    }
    facts.push(quality_label(&a.quality));
    let (k1, k2, k3) = (key.clone(), key.clone(), key);
    let header = row()
        .key("lib-album-head")
        .w_full()
        .px(12.0)
        .py(12.0)
        .gap(14.0)
        .center_y()
        .child(super::art::cover(
            s,
            "lib-cover",
            LOCAL_USER,
            &a.folder,
            88.0,
            t,
        ))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .shrink()
                .gap(4.0)
                .child(
                    text(if a.title.is_empty() {
                        "Unknown album".to_string()
                    } else {
                        a.title.clone()
                    })
                    .key("lib-album-title")
                    .font(type_::TITLE)
                    .color(t.text)
                    .truncate(),
                )
                .child(
                    text(facts.join(" · "))
                        .key("lib-album-facts")
                        .font(tnum(META))
                        .color(t.text_secondary)
                        .truncate(),
                )
                .child(
                    text(fmt::home_short(&a.folder))
                        .key("lib-album-folder")
                        .font(META)
                        .color(t.text_secondary.alpha(0.7))
                        .truncate(),
                )
                .child(
                    row()
                        .pt(6.0)
                        .gap(6.0)
                        .center_y()
                        .child(
                            super::button_icon(
                                "lib-album-play",
                                &icons::PLAY,
                                "Play album",
                                true,
                                t,
                            )
                            .on_press(move |s: &mut App| s.play_library_album(&k1)),
                        )
                        .child(
                            super::button_icon(
                                "lib-album-star",
                                if starred {
                                    &icons::STAR_SOLID
                                } else {
                                    &icons::STAR
                                },
                                if starred { "Starred" } else { "Star" },
                                false,
                                t,
                            )
                            .on_press(move |s: &mut App| s.toggle_star_album(&k2)),
                        )
                        .child(
                            super::albums::more_button(
                                "lib-album-more".into(),
                                true,
                                move |s: &App| s.library_album_menu(&k3),
                                t,
                            )
                            .size(28.0, 28.0),
                        ),
                ),
        );
    let back = row()
        .key("lib-back")
        .h(28.0)
        .px(8.0)
        .gap(4.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Back to albums")
        .on_press(|s: &mut App| s.close_library_album())
        .child(icons::draw(
            &icons::CHEVRON_LEFT,
            icons::SM,
            t.text_secondary,
        ))
        .child(text("Albums").font(type_::LABEL).color(t.text_secondary));
    let tracks: Rc<[usize]> = a.tracks.clone().into();
    let t2 = tracks.clone();
    let version = (
        a.key.clone(),
        s.library_stars_rev(),
        playing_key(s),
        s.library.selected.clone(),
    );
    let list = virtual_list(
        &s.library.tracks_list,
        tracks.len(),
        SONG_H,
        move |s: &App, i| {
            let t = super::theme();
            match t2.get(i) {
                Some(&ti) if ti < s.library.index.tracks.len() => album_track_row(s, ti, &t),
                _ => el().h(SONG_H),
            }
        },
    )
    .row_key(move |s: &App, i| {
        tracks
            .get(i)
            .and_then(|&ti| s.library.index.tracks.get(ti))
            .map_or_else(|| format!("r{i}").into(), |t| t.path_str.clone().into())
    })
    .version(version)
    .build();
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(row().px(4.0).pt(4.0).child(back))
        .child(header)
        .child(super::hairline(t))
        .child(list)
}
