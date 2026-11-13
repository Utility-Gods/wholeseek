//! File rows and their column header, shared by search and user tabs:
//! ▶ | Name | Length | Size | Quality | ↓ (↓ shows on hover until a download exists).

use rune::tok::{Theme, type_};
use std::rc::Rc;
use wholeseek_audio::PlayState;
use wholeseek_core::RemoteFile;

use rune::{Color, Cursor, El, Key, KeyEvent, col, el, row, text};

use super::{META, accent, green, icon_button, icons, progress, red, tnum};
use crate::app::{App, TState};
use crate::fmt;
use crate::menu::MenuAnchor;

pub const ROW_H: f32 = 28.0;
const PLAY_W: f32 = 24.0;
const LEN_W: f32 = 52.0;
const SIZE_W: f32 = 72.0;
const QUALITY_W: f32 = 92.0;
const DL_W: f32 = 56.0;

/// Column titles over a file list. `indent` lines them up with indented rows.
pub fn columns(s: &App, indent: f32, t: &Theme) -> El {
    let h = |s: &str| text(s.to_string()).font(META).color(t.text_secondary);
    row()
        .key("columns")
        .h(24.0)
        .w_full()
        .max_w(table_max_w(s.tab().search().is_some()))
        .pl(12.0 + indent)
        .pr(6.0)
        .gap(10.0)
        .center_y()
        .child(el().w(PLAY_W))
        .child(h("Name").grow(1.0))
        .child(h("Length").w(LEN_W))
        .child(h("Size").w(SIZE_W))
        .child(h("Quality").w(QUALITY_W))
        .child(el().w(DL_W))
        .child(el().w(24.0))
}

/// Whether `f` is the current track, and whether it is running (not paused or ended).
pub fn now_playing(s: &App, f: &RemoteFile) -> (bool, bool) {
    let Some(p) = s
        .playing
        .as_ref()
        .filter(|p| crate::tabs::same_file(&p.file, f))
    else {
        return (false, false);
    };
    let running = match p.snapshot.as_ref().map(|sn| &sn.state) {
        Some(PlayState::Paused | PlayState::Ended | PlayState::Error(_)) => false,
        Some(_) => true,
        None => p.failed.is_none(),
    };
    (true, running)
}

/// How wide a file table gets: search results reach further (see
/// [`super::details::RESULTS_MAX_W`]); a user tab keeps the narrower table.
fn table_max_w(search: bool) -> f32 {
    if search {
        super::details::RESULTS_MAX_W
    } else {
        super::details::TABLE_MAX_W
    }
}

/// The one live indicator: three bars moving with the clock while a track runs, at rest
/// when paused.
pub fn equalizer(now: f64, running: bool, color: Color) -> El {
    let mut bars = row().key("eq").h(10.0).gap(1.5).items_end();
    for (i, (f, p)) in [(7.1, 0.0), (9.7, 1.9), (5.3, 4.2)].into_iter().enumerate() {
        let h = if running {
            3.0 + 7.0 * (0.5 + 0.5 * (now * f + p).sin()) as f32
        } else {
            [4.0, 7.0, 5.0][i]
        };
        bars = bars.child(el().w(2.0).h(h).fill(color));
    }
    bars
}

/// Where a file row sits: its folder (the ▶ queue), whether in a user tab, and the unique
/// key its ⋯ button anchors the menu to.
pub struct RowCtx {
    pub folder: Rc<[RemoteFile]>,
    pub in_user_tab: bool,
    pub selected: bool,
    pub highlighted: bool,
    pub more_key: String,
    /// The search tab's filter words: their matches in the name are drawn in the accent.
    pub find: Option<Rc<crate::tabs::TextQuery>>,
}

/// One file. `indent` nests it under a folder line; non-audio files are dimmed and get no ▶.
/// Click selects it; Enter plays, Shift+Enter downloads; ⋯, right-click, the Menu key and
/// Shift+F10 open its menu.
pub fn file_row(s: &App, key: &str, f: &RemoteFile, indent: f32, ctx: &RowCtx, t: &Theme) -> El {
    let (playing, running) = now_playing(s, f);
    let audio = crate::tabs::is_audio(f);
    let playable = crate::tabs::is_playable(f);
    let fg = if !audio {
        t.text_secondary.alpha(0.6)
    } else if playing {
        accent()
    } else {
        t.text
    };
    let play: El = if !playable {
        el().w(PLAY_W)
    } else {
        let icon = if playing && running {
            equalizer(s.now, true, accent())
        } else if playing {
            icons::play(icons::SM, accent())
        } else {
            icons::play(icons::SM, t.text_secondary)
        };
        let (file, folder) = (f.clone(), ctx.folder.clone());
        icon_button("play", icon, t)
            .size(PLAY_W, 22.0)
            .label(if playing && running { "Pause" } else { "Play" })
            .on_press(move |s: &mut App| s.play_in(&file, &folder))
    };
    let q = fmt::quality(f);
    let lossless = matches!(
        rune_ext(f).as_str(),
        "flac" | "wav" | "aiff" | "alac" | "ape"
    );
    let len = f
        .attrs
        .duration_secs
        .map(|d| fmt::mmss(f64::from(d)))
        .unwrap_or_default();
    let in_user = ctx.in_user_tab;
    let (file_m, file_k, file_s) = (f.clone(), f.clone(), f.clone());
    let folder_k = ctx.folder.clone();
    let mut more = icon_button(&ctx.more_key, icons::more(icons::MD, t.text_secondary), t)
        .size(24.0, 22.0)
        .label("More")
        .on_press(move |s: &mut App| {
            let items = s.file_menu(&file_m, in_user);
            let key = more_key_of(s, &file_m);
            s.open_menu(MenuAnchor::Key(key), items);
        });
    if !(playing || ctx.selected) {
        more = more.group_opacity(0.0, 1.0);
    }
    let path = f.path.clone();
    let group = (!in_user).then(|| (f.username.clone(), f.folder().to_string()));
    let mut r = row()
        .key(key.to_string())
        .h(ROW_H)
        .w_full()
        .max_w(table_max_w(!ctx.in_user_tab))
        .pl(12.0 + indent)
        .pr(6.0)
        .gap(10.0)
        .center_y()
        .hover(t.hover)
        .hover_group()
        .focusable(true)
        .focus_ring(1.0, -1.0, accent().alpha(0.6))
        .on_press(move |s: &mut App| {
            // Selects (never plays); in a search tab its folder fills the details pane.
            let tab = s.tab_mut();
            tab.selected = Some(path.clone());
            if group.is_some() {
                tab.group = group.clone();
            }
        })
        .on_key(move |s: &mut App, e: &KeyEvent, _| {
            if e.key != Key::Enter {
                return false;
            }
            if e.mods.shift {
                s.download(&file_k);
            } else if crate::tabs::is_playable(&file_k) {
                s.play_in(&file_k, &folder_k);
            }
            true
        })
        .on_secondary_press(move |s: &mut App, p, _| {
            s.tab_mut().selected = Some(file_s.path.clone());
            let items = s.file_menu(&file_s, in_user);
            super::menu::at_point(s, p, items);
        })
        .child(play)
        .child(
            text(f.basename().to_string())
                .key("name")
                .font(type_::BODY)
                .color(fg)
                .spans(match_spans(ctx.find.as_deref(), f.basename()))
                .truncate()
                .grow(1.0)
                .shrink(),
        )
        .child(text(len).font(tnum(META)).color(t.text_secondary).w(LEN_W))
        .child(
            text(fmt::size(f.size))
                .font(tnum(META))
                .color(t.text_secondary)
                .w(SIZE_W),
        )
        .child(
            text(q)
                .font(META)
                .color(if lossless { t.text } else { t.text_secondary })
                .w(QUALITY_W),
        )
        .child(
            row()
                .w(DL_W)
                .h(ROW_H)
                .center()
                .child(download_cell(s, f, false, t)),
        )
        .child(more);
    if playing {
        r = r.fill(accent().alpha(0.07));
    } else if ctx.highlighted || ctx.selected {
        r = r.fill(t.hover);
    }
    r
}

/// Accent-coloured [`rune::Span`]s over `text` where the filter words match (colour only, so
/// nothing shifts).
pub fn match_spans(q: Option<&crate::tabs::TextQuery>, text: &str) -> Vec<rune::Span> {
    q.map(|q| q.spans(text))
        .unwrap_or_default()
        .into_iter()
        .map(|(a, b)| rune::Span::new(a, b).color(accent()))
        .collect()
}

/// The ⋯ key for a file in the shown tab (unique across the page).
pub fn more_key_of(s: &App, f: &RemoteFile) -> String {
    more_key(s.tab().id, &f.path)
}

pub fn more_key(tab: crate::tabs::TabId, path: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("more-{tab}-{h:x}")
}

fn rune_ext(f: &RemoteFile) -> String {
    wholeseek_core::rank::extension(&f.path)
}

/// ↓ (on hover), or the download's progress / done / failed mark; a file downloaded
/// before (history, exact user + path) shows a muted ✓ until hovered. `album`: the ↓ keeps
/// the album folder (a track listed under its album).
pub fn download_cell(s: &App, f: &RemoteFile, album: bool, t: &Theme) -> El {
    let file = f.clone();
    let start = move |s: &mut App| {
        if album {
            s.download_album_track(&file)
        } else {
            s.download(&file)
        }
    };
    match s
        .download_of(f)
        .map(|d| (d.state.clone(), d.fraction(), d.id))
    {
        None | Some((TState::Cancelled, ..)) if was_downloaded(s, f) => row()
            .key("downloaded")
            .size(24.0, 24.0)
            .center()
            .label("Downloaded")
            .help("Downloaded")
            .child(icons::check(icons::SM, t.text_secondary)),
        None | Some((TState::Cancelled, ..)) => {
            icon_button("download", icons::down(icons::MD, t.text), t)
                .label("Download")
                .group_opacity(0.0, 1.0)
                .on_press(start)
        }
        Some((TState::Queued | TState::RemoteQueued(_), ..)) => row()
            .key("download")
            .label("Download queued")
            .child(text("queued").font(META).color(t.text_secondary)),
        Some((TState::Active, frac, _)) => col()
            .key("download")
            .gap(3.0)
            .items_center()
            .label(format!("Downloading {:.0}%", frac * 100.0))
            .child(
                text(format!("{:.0}%", frac * 100.0))
                    .font(tnum(META))
                    .color(t.text_secondary),
            )
            .child(progress("bar", frac, 40.0, accent(), t)),
        Some((TState::Done(_), ..)) => row()
            .key("download")
            .size(24.0, 24.0)
            .center()
            .label("Downloaded")
            .help("Saved to your downloads")
            .child(icons::check(icons::MD, green())),
        Some((TState::Failed(reason), _, id)) => {
            icon_button("download", icons::retry(icons::MD, red()), t)
                .label("Retry download")
                .help(format!("Failed: {reason} — click to retry"))
                .on_press(move |s: &mut App| s.retry(id))
        }
    }
}

/// Downloaded before (exact user + remote path).
pub fn was_downloaded(s: &App, f: &RemoteFile) -> bool {
    s.downloaded.contains(&(f.username.clone(), f.path.clone()))
}

/// A "↓ Download folder"-style text button with an icon.
pub fn action(key: &str, label: &str, icon: El, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(t.hover)
        .hover(t.control_track)
        .press(t.control_track)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label.to_string())
        .child(icon)
        .child(text(label.to_string()).font(type_::LABEL).color(t.text))
}
