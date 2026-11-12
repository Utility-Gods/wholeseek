//! Top bar, the line under it, search results and the browse view.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollSpec, col, el, row, text};

use goalseek_core::{RemoteFile, SearchHit};

use super::{
    accent, amber, field_box, green, hairline, icon_button, icons, input, progress, red, tnum,
    vline,
};
use crate::app::{App, BrowseView, Conn, MAX_ROWS, Panel, TState};
use crate::fmt;

pub fn top_bar(s: &App, t: &Theme) -> El {
    let search = field_box(
        "search",
        input(&s.search_edit, "Search music…", t)
            .on_edit(|s: &mut App| &mut s.search_edit)
            .on_submit(|s: &mut App, _| s.submit_search()),
        t,
    )
    .grow(1.0);
    let (dot, label) = match &s.conn {
        Conn::Connected { username, .. } => (green(), username.clone()),
        Conn::Connecting => (amber(), "connecting…".into()),
        Conn::Reconnecting(_) => (amber(), "reconnecting…".into()),
        Conn::LoginRejected => (red(), "login rejected".into()),
        Conn::Displaced => (red(), "signed in elsewhere".into()),
        Conn::Offline => (t.indicator, "offline".into()),
    };
    let status = row()
        .key("status")
        .gap(6.0)
        .center_y()
        .label(format!("Status: {label}"))
        .child(el().size(7.0, 7.0).radius(3.5).fill(dot))
        .child(text(label).font(type_::LABEL).color(t.text_secondary));
    let active = s.active_transfers();
    let open = s.panel == Panel::Transfers;
    let mut transfers = row()
        .key("transfers")
        .h(28.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(t.control_off)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Transfers")
        .on_press(|s: &mut App| s.toggle_panel(Panel::Transfers))
        .child(icons::transfers(
            14.0,
            if open { accent() } else { t.text_secondary },
        ));
    if active > 0 {
        transfers = transfers.child(
            text(active.to_string())
                .font(type_::LABEL)
                .color(t.text_secondary),
        );
    }
    if open {
        transfers = transfers.fill(t.control_off);
    }
    let settings_open = s.panel == Panel::Settings;
    let mut settings = icon_button(
        "settings",
        icons::gear(
            15.0,
            if settings_open {
                accent()
            } else {
                t.text_secondary
            },
        ),
        t,
    )
    .label("Settings")
    .on_press(|s: &mut App| s.toggle_panel(Panel::Settings));
    if settings_open {
        settings = settings.fill(t.control_off);
    }
    row()
        .key("top")
        .h(52.0)
        .w_full()
        .px(16.0)
        .gap(14.0)
        .center_y()
        .child(search)
        .child(status)
        .child(transfers)
        .child(settings)
}

pub fn content(s: &App, t: &Theme) -> El {
    let mut c = col()
        .key("content")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0);
    if let Some(e) = &s.error {
        c = c.child(
            row()
                .key("error")
                .h(30.0)
                .px(16.0)
                .gap(10.0)
                .center_y()
                .fill(red().alpha(0.08))
                .child(
                    text(e.clone())
                        .font(type_::LABEL)
                        .color(red())
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(
                    icon_button("dismiss", icons::close(9.0, t.text_secondary), t)
                        .size(22.0, 22.0)
                        .label("Dismiss")
                        .on_press(|s: &mut App| s.error = None),
                ),
        );
    }
    if let Some(b) = &s.browse {
        c.child(browse(s, b, t))
    } else if s.search.is_some() {
        c.child(results(s, t))
    } else {
        c.child(
            col()
                .grow(1.0)
                .center()
                .gap(6.0)
                .child(
                    text("Search the Soulseek network")
                        .font(type_::TITLE)
                        .color(t.text_secondary),
                )
                .child(
                    text("FLAC first · ▶ plays from cache · ↓ saves to your downloads")
                        .font(type_::LABEL)
                        .color(t.text_secondary.alpha(0.7)),
                ),
        )
    }
}

fn columns(t: &Theme, browse: bool, compact: bool) -> El {
    let h = |s: &str, w: Option<f32>| {
        let e = text(s.to_string())
            .font(type_::SECTION)
            .color(t.text_secondary);
        match w {
            Some(w) => e.w(w),
            None => e.grow(1.0),
        }
    };
    let mut r = row()
        .key("columns")
        .h(24.0)
        .w_full()
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(h("Name", None))
        .child(h("Quality", Some(96.0)))
        .child(h("Size", Some(72.0)));
    if browse {
        r = r.child(h("Length", Some(56.0)));
    } else {
        r = r.child(h("User", Some(120.0)));
        if !compact {
            r = r.child(h("Slot · speed", Some(100.0)));
        }
    }
    r.child(el().w(84.0))
}

fn results(s: &App, t: &Theme) -> El {
    let Some(search) = &s.search else { return el() };
    let n = search.hits.len();
    let mut summary = row()
        .key("context")
        .h(36.0)
        .w_full()
        .px(16.0)
        .gap(10.0)
        .center_y()
        .child(
            text(format!("{n} results"))
                .key("count")
                .font(type_::LABEL)
                .color(t.text),
        )
        .child(
            row()
                .key("audio-only")
                .h(22.0)
                .px(8.0)
                .gap(6.0)
                .center_y()
                .radius(11.0)
                .fill(if s.audio_only {
                    t.control_off
                } else {
                    el_none()
                })
                .border(1.0, t.card_border)
                .hover(t.control_off)
                .cursor(Cursor::Pointer)
                .role("switch")
                .label("Audio only")
                .checked(s.audio_only)
                .on_press(|s: &mut App| s.toggle_audio_only())
                .child(
                    text(if s.audio_only {
                        "audio only"
                    } else {
                        "all files"
                    })
                    .font(type_::LABEL)
                    .color(t.text_secondary),
                ),
        )
        .child(
            text("FLAC first")
                .font(type_::LABEL)
                .color(t.text_secondary),
        );
    if !search.finished {
        summary = summary.child(
            text("searching…")
                .key("searching")
                .font(type_::LABEL)
                .color(amber()),
        );
    }
    if n > MAX_ROWS {
        summary = summary.child(
            text(format!("showing the first {MAX_ROWS}"))
                .font(type_::LABEL)
                .color(t.text_secondary),
        );
    }
    let mut list = col().key("results").w_full().px(8.0).pb(8.0).gap(2.0);
    for (i, h) in search.hits.iter().take(MAX_ROWS).enumerate() {
        list = list.child(file_row(s, &format!("row-{i}"), &h.file, Some(h), t));
    }
    if search.finished && n == 0 {
        list = list.child(
            text(format!("No audio found for “{}”.", search.query))
                .font(type_::BODY)
                .color(t.text_secondary)
                .pl(8.0)
                .pt(12.0),
        );
    }
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(summary)
        .child(columns(t, false, s.panel != Panel::None))
        .child(
            list.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
        )
}

/// One file with its ▶ and ↓ controls; `hit` adds the search-only columns.
fn file_row(s: &App, key: &str, f: &RemoteFile, hit: Option<&SearchHit>, t: &Theme) -> El {
    let playing = s.playing.as_ref().is_some_and(|p| &p.file == f);
    let paused = playing
        && s.playing
            .as_ref()
            .and_then(|p| p.snapshot.as_ref())
            .is_some_and(|sn| {
                matches!(
                    sn.state,
                    goalseek_audio::PlayState::Paused | goalseek_audio::PlayState::Ended
                )
            });
    let mut name = row().grow(1.0).min_w(0.0).gap(8.0).center_y().child(
        text(f.basename().to_string())
            .font(if playing { type_::TITLE } else { type_::BODY })
            .color(if playing { accent() } else { t.text })
            .truncate()
            .shrink(),
    );
    // With a panel docked the row is narrow: drop the folder and fold slot/speed into the user.
    let compact = s.panel != Panel::None;
    if hit.is_some() && !compact {
        let file = f.clone();
        name = name.child(
            text(fmt::crumbs(f.folder()))
                .key("folder")
                .font(type_::LABEL)
                .color(t.text_secondary.alpha(0.6))
                .truncate()
                .shrink()
                .cursor(Cursor::Pointer)
                .help("Open this folder")
                .on_press(move |s: &mut App| s.open_folder_of(&file)),
        );
    }
    let q = fmt::quality(f);
    let quality = text(q.clone())
        .font(type_::LABEL)
        .color(if q.starts_with("FLAC") {
            t.text
        } else {
            t.text_secondary
        })
        .w(96.0);
    let size = text(fmt::size(f.size))
        .font(tnum(type_::LABEL))
        .color(t.text_secondary)
        .w(72.0);
    let mut r = row()
        .key(key.to_string())
        .h(36.0)
        .w_full()
        .px(8.0)
        .gap(12.0)
        .center_y()
        .radius(6.0)
        .hover(t.hover)
        .child(name)
        .child(quality)
        .child(size);
    if let Some(h) = hit {
        let user = h.file.username.clone();
        let dot = || {
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(if h.free_slot { green() } else { t.indicator })
        };
        let mut name = row()
            .key("user")
            .w(120.0)
            .gap(6.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .help("Browse this user")
            .on_press(move |s: &mut App| s.open_user(&user));
        if compact {
            name = name.child(dot());
        }
        r = r.child(
            name.child(
                text(h.file.username.clone())
                    .font(type_::LABEL)
                    .color(t.text_secondary)
                    .truncate()
                    .shrink(),
            ),
        );
        if !compact {
            r = r.child(
                row().w(100.0).gap(6.0).center_y().child(dot()).child(
                    text(fmt::speed(f64::from(h.speed)))
                        .font(tnum(type_::LABEL))
                        .color(t.text_secondary),
                ),
            );
        }
    } else {
        let len = f
            .attrs
            .duration_secs
            .map(|d| fmt::mmss(f64::from(d)))
            .unwrap_or_default();
        r = r.child(
            text(len)
                .font(tnum(type_::LABEL))
                .color(t.text_secondary)
                .w(56.0),
        );
    }
    let play_icon = if playing && !paused {
        icons::pause(12.0, accent())
    } else {
        icons::play(12.0, if playing { accent() } else { t.text })
    };
    let file = f.clone();
    let play = icon_button("play", play_icon, t)
        .label(if playing && !paused { "Pause" } else { "Play" })
        .on_press(move |s: &mut App| s.play(&file));
    let actions = row()
        .w(84.0)
        .gap(4.0)
        .center_y()
        .justify_end()
        .child(play)
        .child(row().w(48.0).center().child(download_cell(s, f, t)));
    if playing {
        r = r.fill(accent().alpha(0.08));
    }
    r.child(actions)
}

/// ↓, or the download's progress / done / failed mark.
fn download_cell(s: &App, f: &RemoteFile, t: &Theme) -> El {
    let file = f.clone();
    let start = move |s: &mut App| s.download(&file);
    match s
        .download_of(f)
        .map(|d| (d.state.clone(), d.fraction(), d.id))
    {
        None | Some((TState::Cancelled, ..)) => {
            icon_button("download", icons::down(13.0, t.text), t)
                .label("Download")
                .on_press(start)
        }
        Some((TState::Queued | TState::RemoteQueued(_), ..)) => row()
            .key("download")
            .w(48.0)
            .h(28.0)
            .center()
            .label("Download queued")
            .child(text("queued").font(type_::LABEL).color(t.text_secondary)),
        Some((TState::Active, frac, _)) => row()
            .key("download")
            .w(48.0)
            .h(28.0)
            .gap(3.0)
            .center()
            .label(format!("Downloading {:.0}%", frac * 100.0))
            .child(
                col()
                    .gap(3.0)
                    .items_center()
                    .child(
                        text(format!("{:.0}%", frac * 100.0))
                            .font(tnum(type_::LABEL))
                            .color(t.text_secondary),
                    )
                    .child(progress("bar", frac, 40.0, accent(), t)),
            ),
        Some((TState::Done(_), ..)) => row()
            .key("download")
            .size(28.0, 28.0)
            .center()
            .label("Downloaded")
            .help("Saved to your downloads")
            .child(icons::check(13.0, green())),
        Some((TState::Failed(reason), _, id)) => {
            icon_button("download", icons::retry(13.0, red()), t)
                .label("Retry download")
                .help(format!("Failed: {reason} — click to retry"))
                .on_press(move |s: &mut App| s.retry(id))
        }
    }
}

fn browse(s: &App, b: &BrowseView, t: &Theme) -> El {
    let back = row()
        .key("back")
        .h(26.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(t.control_off)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Back")
        .on_press(|s: &mut App| s.back())
        .child(icons::back(12.0, t.text))
        .child(text("Back").font(type_::LABEL).color(t.text));
    let mut crumbs = row()
        .key("context")
        .h(36.0)
        .w_full()
        .px(16.0)
        .gap(10.0)
        .center_y()
        .child(back)
        .child(
            text(b.username.clone())
                .key("crumb-user")
                .font(type_::TITLE)
                .color(t.text),
        );
    if let Some(sel) = &b.selected {
        crumbs = crumbs.child(
            text(format!("/ {}", fmt::crumbs(sel)))
                .key("crumb-path")
                .font(type_::BODY)
                .color(t.text_secondary)
                .truncate()
                .shrink(),
        );
    }
    let body: El = if let Some(e) = &b.error {
        col().grow(1.0).center().child(
            text(format!("Couldn't browse {}: {e}", b.username))
                .key("browse-error")
                .font(type_::BODY)
                .color(red()),
        )
    } else if b.dirs.is_none() {
        col().grow(1.0).center().child(
            text(format!("Asking {} for their shares…", b.username))
                .font(type_::BODY)
                .color(t.text_secondary),
        )
    } else {
        row()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(tree(b, t))
            .child(vline(t))
            .child(folder(s, b, t))
    };
    let mut page = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(crumbs)
        .child(hairline(t));
    if let Some(h) = super::profile::header(&b.username, s.profiles.get(&b.username), t) {
        page = page.child(h).child(hairline(t));
    }
    page.child(body)
}

fn tree(b: &BrowseView, t: &Theme) -> El {
    let mut list = col().key("tree").w_full().py(6.0).px(6.0).gap(1.0);
    for (i, (path, depth, has_children, open)) in b.visible_tree().into_iter().enumerate() {
        let selected = b.selected.as_deref() == Some(path.as_str());
        let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
        let chevron: El = if has_children {
            icons::chevron(10.0, open, t.text_secondary)
        } else {
            el().size(10.0, 10.0)
        };
        let p = path.clone();
        let mut r = row()
            .key(format!("dir-{i}"))
            .h(26.0)
            .pl(6.0 + 14.0 * depth as f32)
            .pr(6.0)
            .gap(6.0)
            .center_y()
            .radius(5.0)
            .hover(t.hover)
            .cursor(Cursor::Pointer)
            .role("treeitem")
            .label(name.clone())
            .on_press(move |s: &mut App| {
                if let Some(b) = &mut s.browse {
                    if b.selected.as_deref() == Some(p.as_str()) || !b.expanded.contains(&p) {
                        b.toggle(&p);
                    }
                    b.selected = Some(p.clone());
                }
            })
            .child(chevron)
            .child(
                text(name)
                    .font(type_::BODY)
                    .color(if selected { t.text } else { t.text_secondary })
                    .truncate()
                    .shrink(),
            );
        if selected {
            r = r.fill(t.control_off);
        }
        list = list.child(r);
    }
    col().w(260.0).h_full().min_h(0.0).child(
        list.scroll(ScrollSpec {
            stick_bottom: false,
        })
        .shrink()
        .min_h(0.0),
    )
}

fn folder(s: &App, b: &BrowseView, t: &Theme) -> El {
    let Some(dir) = b.selected_dir() else {
        return col().grow(1.0).center().child(
            text("Pick a folder")
                .font(type_::BODY)
                .color(t.text_secondary),
        );
    };
    let user = b.username.clone();
    let path = dir.path.clone();
    let header = row()
        .key("folder-head")
        .h(40.0)
        .w_full()
        .px(16.0)
        .gap(10.0)
        .center_y()
        .child(icons::folder(14.0, t.text_secondary))
        .child(
            text(fmt::crumbs(
                dir.path.rsplit('\\').next().unwrap_or(&dir.path),
            ))
            .font(type_::TITLE)
            .color(t.text)
            .truncate()
            .shrink(),
        )
        .child(
            text(format!("{} files", dir.files.len()))
                .font(type_::LABEL)
                .color(t.text_secondary),
        )
        .child(el().grow(1.0))
        .child(
            row()
                .key("download-folder")
                .h(28.0)
                .px(10.0)
                .gap(6.0)
                .center_y()
                .radius(6.0)
                .fill(t.control_off)
                .hover(t.control_off)
                .cursor(Cursor::Pointer)
                .role("button")
                .label("Download folder")
                .on_press(move |s: &mut App| s.download_folder(&user, &path))
                .child(icons::down(12.0, t.text))
                .child(text("Download folder").font(type_::LABEL).color(t.text)),
        );
    let mut list = col().key("files").w_full().px(8.0).pb(8.0).gap(2.0);
    for (i, f) in dir.files.iter().enumerate() {
        list = list.child(file_row(s, &format!("file-{i}"), f, None, t));
    }
    if dir.files.is_empty() {
        list = list.child(
            text("This folder has no files of its own.")
                .font(type_::BODY)
                .color(t.text_secondary)
                .pl(8.0)
                .pt(12.0),
        );
    }
    col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(columns(t, true, false))
        .child(
            list.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
        )
}

fn el_none() -> rune::Color {
    rune::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }
}
