//! ⇅ Transfers and ⚙ Settings: drawers over the right edge of the page. Copy is minimal:
//! one short line per status, details in tooltips.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, FileDialogOptions, ScrollSpec, col, el, row, text};
use wholeseek_core::UploadState;

use super::{
    META, accent, amber, field_box, green, hairline, icon_button, icons, input, progress, red,
    section, tnum,
};
use crate::app::{App, Conn, Panel, TState, Transfer};
use crate::fmt;
use crate::history::{DrawerView, HistoryRow};
use crate::menu::MenuAnchor;
use crate::transfers::{self, TFilter, TGroup};

use super::DRAWER_W as W;

fn panel(key: &str, title: &str, extra: El, body: El, t: &Theme) -> El {
    let close = icon_button("panel-close", icons::close(icons::MD, t.text_secondary), t)
        .label("Close")
        .on_press(|s: &mut App| s.panel = Panel::None);
    col()
        .key(key.to_string())
        .w(W)
        .h_full()
        .min_h(0.0)
        .fill(t.card)
        .child(
            row()
                .h(40.0)
                .w_full()
                .pl(14.0)
                .pr(8.0)
                .gap(6.0)
                .center_y()
                .child(text(title.to_string()).font(type_::TITLE).color(t.text))
                .child(el().grow(1.0))
                .child(extra)
                .child(close),
        )
        .child(hairline(t))
        .child(
            body.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
        )
}

/// A small text action with an icon before its text ("Cancel all").
fn link_icon(key: &str, icon: &'static rune::Icon, label: &str, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(22.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label.to_string())
        .child(icons::draw(icon, icons::SM, t.text_secondary))
        .child(text(label.to_string()).font(META).color(t.text_secondary))
}

/// What a transfer state looks like at a glance: its icon and colour.
fn state_icon(state: &TState, t: &Theme) -> El {
    let (i, c): (&'static rune::Icon, _) = match state {
        TState::Queued | TState::RemoteQueued(_) => (&icons::CLOCK, t.text_secondary),
        TState::Active => (&icons::ARROW_DOWN_TO_LINE, accent()),
        TState::Done(_) => (&icons::CIRCLE_CHECK, green()),
        TState::Failed(_) => (&icons::CIRCLE_ALERT, red()),
        TState::Cancelled => (&icons::CIRCLE_X, t.text_secondary),
    };
    icons::draw(i, icons::SM, c).key("state-icon")
}

/// "Saving to ~/Music/Vinyl/Coastline (2019)" until it is done; then where it is.
fn dest_line(d: &Transfer) -> Option<String> {
    match &d.state {
        TState::Done(p) => Some(p.display().to_string()),
        _ => d
            .dest
            .as_ref()
            .map(|p| format!("Saving to {}", fmt::home_short(&p.display().to_string()))),
    }
}

fn small(key: &str, icon: El, label: &str, t: &Theme) -> El {
    icon_button(key, icon, t)
        .size(22.0, 22.0)
        .label(label.to_string())
        .help(label.to_string())
}

// ---- transfers -----------------------------------------------------------------------------

pub fn transfers(s: &App, t: &Theme) -> El {
    let open = small(
        "open-downloads",
        icons::draw(&icons::FOLDER_OPEN, icons::MD, t.text_secondary),
        "Open downloads folder",
        t,
    )
    .on_press(|s: &mut App| s.open_downloads());
    if s.hist.view == DrawerView::History {
        return panel("transfers-panel", "Transfers", open, history(s, t), t);
    }
    let counts = transfers::counts(&s.transfers, &s.drawer.hidden);
    let live = s.live_count();
    let mut body = col().w_full().pb(12.0);

    // Bulk actions, or the cancel confirm in their place.
    let mut bar = row()
        .key("bulk")
        .h(32.0)
        .w_full()
        .px(8.0)
        .gap(4.0)
        .center_y();
    if let Some(n) = s.drawer.confirm_cancel {
        bar = bar
            .child(
                text(format!("Cancel {n} transfers?"))
                    .key("confirm-cancel")
                    .font(type_::LABEL)
                    .color(t.text)
                    .grow(1.0),
            )
            .child(
                super::button_icon("confirm-cancel-all", &icons::X, "Cancel all", true, t)
                    .h(24.0)
                    .on_press(|s: &mut App| s.cancel_all()),
            )
            .child(
                super::button_icon("keep-transfers", &icons::CHECK, "Keep", false, t)
                    .h(24.0)
                    .on_press(|s: &mut App| s.keep_transfers()),
            );
    } else {
        bar = bar.child(views(s, t)).child(el().grow(1.0));
        if live > 0 {
            bar = bar.child(
                link_icon("cancel-all", &icons::X, "Cancel all", t)
                    .on_press(|s: &mut App| s.cancel_all()),
            );
        }
        if counts[4] > 0 {
            bar = bar.child(
                link_icon("retry-all", &icons::REFRESH_CW, "Retry failed", t)
                    .on_press(|s: &mut App| s.retry_all()),
            );
        }
        if counts[3] + counts[4] > 0 {
            bar = bar.child(
                link_icon("clear-finished", &icons::TRASH_2, "Clear finished", t)
                    .help("Files stay on disk")
                    .on_press(|s: &mut App| s.clear_finished()),
            );
        }
    }

    // Filter chips with counts.
    let mut chips = row()
        .key("filters")
        .h(30.0)
        .w_full()
        .px(10.0)
        .gap(4.0)
        .center_y();
    for (f, n) in TFilter::EVERY.into_iter().zip(counts) {
        let on = s.drawer.filter == f;
        let mut c = row()
            .key(f.key())
            .h(22.0)
            .px(7.0)
            .gap(5.0)
            .center_y()
            .radius(4.0)
            .cursor(Cursor::Pointer)
            .role("tab")
            .label(format!("{} {n}", f.label()))
            .checked(on)
            .hover(t.hover)
            .on_press(move |s: &mut App| s.drawer.filter = f)
            .child(
                text(f.label())
                    .font(META)
                    .color(if on { t.text } else { t.text_secondary }),
            )
            .child(text(n.to_string()).font(tnum(META)).color(t.text_secondary));
        if on {
            c = c.fill(t.hover);
        }
        chips = chips.child(c);
    }
    body = body.child(bar).child(chips).child(hairline(t));

    let groups = transfers::groups(&s.transfers, &s.drawer.hidden, s.drawer.filter);
    if groups.is_empty() {
        body = body.child(
            text(if counts[0] == 0 {
                "No downloads"
            } else {
                "None"
            })
            .key("no-downloads")
            .font(type_::LABEL)
            .color(t.text_secondary)
            .px(14.0)
            .pt(12.0),
        );
    }
    for (gi, g) in groups.iter().enumerate() {
        body = body.child(group(s, gi, g, t));
    }

    body = body.child(el().h(12.0)).child(
        section(&format!("Uploads · {}", s.uploads.len()), t)
            .px(14.0)
            .pb(4.0),
    );
    if s.uploads.is_empty() {
        body = body.child(
            text("None")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .px(14.0),
        );
    }
    for (i, u) in s.uploads.iter().enumerate() {
        body = body.child(upload_row(i, u, t));
    }
    panel("transfers-panel", "Transfers", open, body, t)
}

/// Active | History.
fn views(s: &App, t: &Theme) -> El {
    let mut seg = row()
        .key("views")
        .h(24.0)
        .px(1.0)
        .py(1.0)
        .gap(1.0)
        .center_y()
        .radius(5.0)
        .border(1.0, t.separator);
    for (key, label, v) in [
        ("view-active", "Active", DrawerView::Active),
        ("view-history", "History", DrawerView::History),
    ] {
        let on = s.hist.view == v;
        let mut c = row()
            .key(key)
            .h(20.0)
            .px(8.0)
            .center_y()
            .radius(4.0)
            .cursor(Cursor::Pointer)
            .role("tab")
            .label(label)
            .checked(on)
            .on_press(move |s: &mut App| s.set_drawer_view(v))
            .child(
                text(label)
                    .font(META)
                    .color(if on { t.text } else { t.text_secondary }),
            );
        c = if on {
            c.fill(t.hover)
        } else {
            c.hover(t.hover)
        };
        seg = seg.child(c);
    }
    seg
}

// ---- history -------------------------------------------------------------------------------

fn history(s: &App, t: &Theme) -> El {
    let h = &s.hist;
    let mut bar = row()
        .key("bulk")
        .h(32.0)
        .w_full()
        .px(8.0)
        .gap(4.0)
        .center_y();
    if h.confirm_clear {
        bar = bar
            .child(
                text("Clear the whole history?")
                    .key("confirm-clear")
                    .font(type_::LABEL)
                    .color(t.text)
                    .grow(1.0)
                    .help("Files stay on disk"),
            )
            .child(
                super::button_icon("confirm-clear-history", &icons::TRASH_2, "Clear", true, t)
                    .h(24.0)
                    .on_press(|s: &mut App| s.history_clear()),
            )
            .child(
                super::button_icon("keep-history", &icons::X, "Keep", false, t)
                    .h(24.0)
                    .on_press(|s: &mut App| s.history_keep()),
            );
    } else {
        bar = bar.child(views(s, t)).child(el().grow(1.0));
        if !h.rows.is_empty() {
            bar = bar.child(
                link_icon("clear-history", &icons::TRASH_2, "Clear history", t)
                    .help("Files stay on disk")
                    .on_press(|s: &mut App| s.history_clear()),
            );
        }
    }
    let find = field_box(
        "history-find",
        input(&h.find, "Search history", t).on_edit(|s: &mut App| &mut s.hist.find),
        t,
    )
    .w_full();
    let mut body = col()
        .w_full()
        .pb(12.0)
        .child(bar)
        .child(row().w_full().px(10.0).pb(8.0).child(find))
        .child(hairline(t));
    if let Some(e) = &h.error {
        body = body.child(
            text("History unavailable")
                .key("history-error")
                .font(META)
                .color(t.text_secondary)
                .help(e.clone())
                .px(14.0)
                .pt(8.0),
        );
    }
    if h.rows.is_empty() {
        body = body.child(
            text(if h.find.text.trim().is_empty() {
                "No history"
            } else {
                "Nothing matches"
            })
            .key("no-history")
            .font(type_::LABEL)
            .color(t.text_secondary)
            .px(14.0)
            .pt(12.0),
        );
    }
    let now = crate::history::unix_now();
    for (i, r) in h.rows.iter().enumerate() {
        body = body.child(history_row(i, r, now, t));
    }
    if h.more {
        body = body.child(
            row().px(8.0).pt(6.0).child(
                link_icon("history-more", &icons::CHEVRON_DOWN, "Show more", t)
                    .on_press(|s: &mut App| s.history_more()),
            ),
        );
    }
    body
}

fn history_row(i: usize, r: &HistoryRow, now: u64, t: &Theme) -> El {
    use wholeseek_core::Outcome;
    let e = &r.entry;
    let file = crate::history::entry_file(e);
    let (icon, color): (&'static rune::Icon, _) = match e.outcome {
        Outcome::Done if r.on_disk => (&icons::CIRCLE_CHECK, green()),
        Outcome::Done => (&icons::CIRCLE_CHECK, t.text_secondary),
        Outcome::Failed => (&icons::CIRCLE_ALERT, red()),
        Outcome::Cancelled => (&icons::CIRCLE_X, t.text_secondary),
    };
    let id = e.id;
    let mut actions = row().gap(1.0).center_y().group_opacity(0.0, 1.0);
    match (&e.local_path, e.outcome) {
        (Some(p), Outcome::Done) => {
            if r.on_disk {
                let p1 = p.clone();
                actions = actions.child(
                    small(
                        "show",
                        icons::draw(&icons::FOLDER_OPEN, icons::MD, t.text),
                        "Show in folder",
                        t,
                    )
                    .on_press(move |s: &mut App| s.desktop.show_item(&p1)),
                );
            }
            let p2 = p.display().to_string();
            actions = actions.child(
                small("copy", icons::copy(icons::MD, t.text), "Copy path", t)
                    .on_press_tx(move |_: &mut App, tx| tx.copy(p2.clone())),
            );
        }
        _ => {
            actions = actions.child(
                small(
                    "retry",
                    icons::retry(icons::MD, t.text),
                    "Download again",
                    t,
                )
                .on_press(move |s: &mut App| s.history_retry(id)),
            );
        }
    }
    actions = actions.child(
        small(
            "remove",
            icons::close(icons::MD, t.text_secondary),
            "Remove from history",
            t,
        )
        .on_press(move |s: &mut App| s.history_remove(id)),
    );
    let mut meta = row()
        .gap(6.0)
        .center_y()
        .child(user_link(&e.username, t))
        .child(
            text(format!(
                "{} · {} · {}",
                crate::history::ago(e.finished_at, now),
                fmt::size(e.size),
                fmt::quality(&file)
            ))
            .key("meta")
            .font(tnum(META))
            .color(t.text_secondary)
            .truncate()
            .shrink()
            .help(crate::history::utc(e.finished_at)),
        )
        .child(el().grow(1.0));
    let tag = match e.outcome {
        Outcome::Done if !r.on_disk => Some(
            text("missing")
                .help("Moved or deleted since")
                .color(t.text_secondary),
        ),
        Outcome::Failed => Some(
            text("failed")
                .help(e.reason.clone().unwrap_or_default())
                .color(red()),
        ),
        Outcome::Cancelled => Some(text("cancelled").color(t.text_secondary)),
        Outcome::Done => None,
    };
    if let Some(tag) = tag {
        meta = meta.child(tag.key("tag").font(META));
    }
    let where_ = match &e.local_path {
        Some(p) => format!("{}\n{}", fmt::crumbs(&e.remote_path), p.display()),
        None => fmt::crumbs(&e.remote_path),
    };
    col()
        .key(format!("hist-{i}"))
        .w_full()
        .px(10.0)
        .py(6.0)
        .gap(3.0)
        .hover(t.hover)
        .hover_group()
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(icons::draw(icon, icons::SM, color).key("outcome"))
                .child(
                    text(file.basename().to_string())
                        .key("hist-name")
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .grow(1.0)
                        .shrink()
                        .help(where_),
                )
                .child(actions),
        )
        .child(meta.pl(18.0))
}

fn group(s: &App, gi: usize, g: &TGroup, t: &Theme) -> El {
    let rows: Vec<&Transfer> = g.rows.iter().map(|&i| &s.transfers[i]).collect();
    let mut out = col().key(format!("group-{gi}")).w_full();
    // A lone download is just its row.
    if rows.len() == 1 {
        return out.child(download_row(s, 0, rows[0], false, t));
    }
    let collapsed = s.drawer.collapsed.contains(&g.key());
    let (u, d) = (g.username.clone(), g.folder.clone());
    let name = d.rsplit('\\').next().unwrap_or(&d).to_string();
    let live = rows.iter().any(|r| r.is_live());
    let retryable = rows.iter().any(|r| transfers::is_retryable(&r.state));
    let finished = rows.iter().any(|r| transfers::is_finished(&r.state));
    let done: u64 = rows.iter().map(|r| r.bytes).sum();
    let total: u64 = rows.iter().map(|r| r.total).sum();
    let frac = if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64) as f32
    };
    let more_key = format!("group-more-{gi}");

    let mut actions = row().gap(2.0).center_y().group_opacity(0.0, 1.0);
    if retryable {
        let (u, d) = (u.clone(), d.clone());
        actions = actions.child(
            small(
                "retry-group",
                icons::retry(icons::MD, t.text),
                "Retry failed",
                t,
            )
            .on_press(move |s: &mut App| s.retry_group(&u, &d)),
        );
    }
    if live {
        let (u, d) = (u.clone(), d.clone());
        actions = actions.child(
            small(
                "cancel-group",
                icons::close(icons::SM, t.text_secondary),
                "Cancel",
                t,
            )
            .on_press(move |s: &mut App| s.cancel_group(&u, &d)),
        );
    }
    if finished {
        let (u, d) = (u.clone(), d.clone());
        actions = actions.child(
            link_icon("clear-group", &icons::TRASH_2, "Clear", t)
                .on_press(move |s: &mut App| s.clear_group(&u, &d)),
        );
    }
    let menu_items = s.group_items(&u, &d);
    if !menu_items.is_empty() {
        let (u, d, k) = (u.clone(), d.clone(), more_key.clone());
        actions = actions.child(
            small(
                &more_key,
                icons::more(icons::MD, t.text_secondary),
                "More",
                t,
            )
            .on_press(move |s: &mut App| {
                let items = s.group_items(&u, &d);
                s.open_menu(MenuAnchor::Key(k.clone()), items);
            }),
        );
    }
    let (tu, td) = (u.clone(), d.clone());
    let (mu, md) = (u.clone(), d.clone());
    let head = col()
        .key("group-head")
        .w_full()
        .px(10.0)
        .py(6.0)
        .gap(4.0)
        .hover(t.hover)
        .hover_group()
        .cursor(Cursor::Pointer)
        .role("treeitem")
        .label(name.clone())
        .expanded(!collapsed)
        .on_press(move |s: &mut App| s.toggle_group(&tu, &td))
        .on_secondary_press(move |s: &mut App, p, _| {
            let items = s.group_items(&mu, &md);
            super::menu::at_point(s, p, items);
        })
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(icons::chevron(icons::SM, !collapsed, t.text_secondary))
                .child(
                    text(name)
                        .key("group-name")
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .shrink()
                        .help(match rows.iter().find_map(|r| r.dest.as_ref()) {
                            Some(dest) if live => format!(
                                "{}\nSaving to {}",
                                fmt::crumbs(&d),
                                fmt::home_short(&dest.display().to_string())
                            ),
                            _ => fmt::crumbs(&d),
                        }),
                )
                .child(user_link(&u, t))
                .child(el().grow(1.0))
                .child(actions),
        )
        .child(
            text(transfers::summary(&rows))
                .key("group-summary")
                .font(tnum(META))
                .color(t.text_secondary)
                .pl(15.0),
        )
        .child(
            row()
                .pl(15.0)
                .child(progress("bar", frac, W - 45.0, accent(), t)),
        );
    out = out.child(head);
    if !collapsed {
        for (i, r) in rows.iter().enumerate() {
            out = out.child(download_row(s, i, r, true, t));
        }
    }
    out
}

fn download_row(s: &App, i: usize, d: &Transfer, nested: bool, t: &Theme) -> El {
    let frac = d.fraction();
    let (status, color) = match &d.state {
        TState::Queued => ("queued".to_string(), t.text_secondary),
        TState::RemoteQueued(Some(n)) => (format!("queued #{n}"), t.text_secondary),
        TState::RemoteQueued(None) => ("queued".into(), t.text_secondary),
        TState::Active => (
            format!("{:.0}% · {}", frac * 100.0, fmt::speed(d.speed)),
            accent(),
        ),
        TState::Done(_) => ("done".into(), green()),
        TState::Failed(_) => ("failed".into(), red()),
        TState::Cancelled => ("cancelled".into(), t.text_secondary),
    };
    let id = d.id;
    // Row actions, shown on hover: everything a finished file offers, else ↻ / ×.
    let mut actions = row().gap(1.0).center_y().group_opacity(0.0, 1.0);
    match &d.state {
        TState::Done(path) => {
            if crate::tabs::is_playable(&d.file) {
                actions = actions.child(
                    small("play", icons::play(icons::MD, t.text), "Play", t)
                        .on_press(move |s: &mut App| s.play_local(id)),
                );
            }
            let (p1, p2, p3) = (path.clone(), path.clone(), path.display().to_string());
            actions = actions
                .child(
                    small(
                        "show",
                        icons::draw(&icons::FOLDER_OPEN, icons::MD, t.text),
                        "Show in folder",
                        t,
                    )
                    .on_press(move |s: &mut App| s.desktop.show_item(&p1)),
                )
                .child(
                    small(
                        "open",
                        icons::open(icons::MD, t.text),
                        "Open with default app",
                        t,
                    )
                    .on_press(move |s: &mut App| s.desktop.open(&p2)),
                )
                .child(
                    small("copy", icons::copy(icons::MD, t.text), "Copy path", t)
                        .on_press_tx(move |_: &mut App, tx| tx.copy(p3.clone())),
                );
        }
        TState::Failed(_) | TState::Cancelled => {
            actions = actions.child(
                small("retry", icons::retry(icons::MD, t.text), "Retry", t)
                    .on_press(move |s: &mut App| s.retry(id)),
            );
        }
        _ => {
            actions = actions.child(
                small(
                    "cancel",
                    icons::close(icons::MD, t.text_secondary),
                    "Cancel",
                    t,
                )
                .on_press(move |s: &mut App| s.cancel(id)),
            );
        }
    }
    let mut status_el = text(status).key("status").font(tnum(META)).color(color);
    if let TState::Failed(reason) = &d.state {
        status_el = status_el.help(reason.clone());
    }
    let mut meta = row().gap(6.0).center_y();
    if !nested {
        meta = meta.child(user_link(&d.file.username, t));
    }
    meta = meta
        .child(
            text(fmt::size(d.total))
                .font(tnum(META))
                .color(t.text_secondary),
        )
        .child(el().grow(1.0))
        .child(state_icon(&d.state, t))
        .child(status_el);
    let mut r = col()
        .key(format!("dl-{i}"))
        .w_full()
        .pl(if nested { 25.0 } else { 10.0 })
        .pr(10.0)
        .py(6.0)
        .gap(3.0)
        .hover(t.hover)
        .hover_group()
        .on_secondary_press(move |s: &mut App, p, _| {
            let items = s.transfer_items(id);
            super::menu::at_point(s, p, items);
        })
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child({
                    let name = text(d.file.basename().to_string())
                        .key("dl-name")
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .grow(1.0)
                        .shrink();
                    match dest_line(d) {
                        Some(h) => name.help(h),
                        None => name,
                    }
                })
                .child(actions),
        )
        .child(meta);
    if !matches!(d.state, TState::Done(_)) {
        let bar = match d.state {
            TState::Failed(_) => red(),
            _ => accent(),
        };
        r = r.child(progress("bar", frac, W - 40.0, bar, t));
    }
    let _ = s;
    r
}

fn upload_row(i: usize, u: &wholeseek_core::UploadRow, t: &Theme) -> El {
    let frac = if u.size == 0 {
        0.0
    } else {
        (u.bytes_sent as f64 / u.size as f64) as f32
    };
    let (status, color) = match &u.state {
        UploadState::Queued { place } => (format!("queued #{place}"), t.text_secondary),
        UploadState::InProgress => (
            format!("{:.0}% · {}", frac * 100.0, fmt::speed(u.speed)),
            accent(),
        ),
        UploadState::Completed => ("done".into(), green()),
        UploadState::Cancelled => ("cancelled".into(), t.text_secondary),
        UploadState::Failed(_) => ("failed".into(), red()),
    };
    let (icon, icon_color): (&'static rune::Icon, _) = match &u.state {
        UploadState::Queued { .. } => (&icons::CLOCK, t.text_secondary),
        UploadState::InProgress => (&icons::UPLOAD, accent()),
        UploadState::Completed => (&icons::CIRCLE_CHECK, green()),
        UploadState::Cancelled => (&icons::CIRCLE_X, t.text_secondary),
        UploadState::Failed(_) => (&icons::CIRCLE_ALERT, red()),
    };
    let state_el = icons::draw(icon, icons::SM, icon_color).key("state-icon");
    let mut status_el = text(status).font(tnum(META)).color(color);
    if let UploadState::Failed(e) = &u.state {
        status_el = status_el.help(e.clone());
    }
    let action: El = if matches!(
        u.state,
        UploadState::Queued { .. } | UploadState::InProgress
    ) {
        let (user, path) = (u.username.clone(), u.path.clone());
        small(
            "cancel",
            icons::close(icons::SM, t.text_secondary),
            "Cancel upload",
            t,
        )
        .group_opacity(0.0, 1.0)
        .on_press(move |s: &mut App| s.cancel_upload(&user, &path))
    } else {
        el().size(22.0, 22.0)
    };
    let mut r = col()
        .key(format!("up-{i}"))
        .w_full()
        .px(10.0)
        .py(6.0)
        .gap(3.0)
        .hover(t.hover)
        .hover_group()
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(
                    text(wholeseek_core::paths::remote_basename(&u.path).to_string())
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(action),
        )
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(text("to").font(META).color(t.text_secondary))
                .child(user_link(&u.username, t))
                .child(el().grow(1.0))
                .child(state_el)
                .child(status_el),
        );
    if matches!(
        u.state,
        UploadState::Queued { .. } | UploadState::InProgress
    ) {
        r = r.child(progress("bar", frac, W - 40.0, accent(), t));
    }
    r
}

/// A username that opens (or shows) its user tab.
fn user_link(username: &str, t: &Theme) -> El {
    let user = username.to_string();
    text(username.to_string())
        .key("user")
        .font(META)
        .color(t.text)
        .cursor(Cursor::Pointer)
        .role("link")
        .help("Open in a tab")
        .on_press(move |s: &mut App| s.open_user(&user))
}

// ---- settings ------------------------------------------------------------------------------

/// A settings section: its icon and title, then its lines.
fn block(title: &str, icon: &'static rune::Icon, t: &Theme) -> El {
    col().w_full().px(14.0).pt(14.0).gap(6.0).child(
        row()
            .gap(6.0)
            .center_y()
            .child(icons::draw(icon, icons::SM, t.text_secondary))
            .child(section(title, t)),
    )
}

/// A small button with an icon before its label.
fn mini_icon(key: &str, icon: &'static rune::Icon, label: &str, t: &Theme) -> El {
    super::files::action(
        key,
        label,
        icons::draw(icon, icons::MD, t.text_secondary),
        t,
    )
}

/// A label on the left, controls on the right.
fn line(key: &str, label: El) -> El {
    row()
        .key(key.to_string())
        .h(28.0)
        .w_full()
        .gap(6.0)
        .center_y()
        .child(label.grow(1.0).shrink())
}

fn label(s: impl Into<String>, t: &Theme) -> El {
    text(s.into()).font(type_::BODY).color(t.text).truncate()
}

fn disabled(b: El, why: &str, t: &Theme) -> El {
    let _ = t;
    b.disabled(true)
        .opacity(0.45)
        .cursor(Cursor::Default)
        .help(why.to_string())
}

/// − n unit +, applied on each press.
fn stepper(key: &str, n: String, step: fn(&mut App, i64), t: &Theme) -> El {
    row()
        .key(key.to_string())
        .gap(2.0)
        .center_y()
        .child(
            small("less", text("−").font(type_::BODY).color(t.text), "Less", t)
                .on_press(move |s: &mut App| step(s, -1)),
        )
        .child(
            text(n)
                .key("value")
                .font(tnum(type_::BODY))
                .color(t.text)
                .min_w(28.0)
                .align(rune::Align::Center),
        )
        .child(
            small("more", text("+").font(type_::BODY).color(t.text), "More", t)
                .on_press(move |s: &mut App| step(s, 1)),
        )
}

fn choose_folder(
    title: &str,
    then: fn(&mut App, std::path::PathBuf),
) -> impl Fn(&mut App, &mut rune::Tx) + 'static {
    let title = title.to_string();
    move |s: &mut App, tx: &mut rune::Tx| {
        if s.settings_form.manual.is_some() {
            return;
        }
        let options = FileDialogOptions {
            title: title.clone(),
            directory: dirs_home(),
            ..Default::default()
        };
        let _ = s;
        tx.open_folder(options, move |s: &mut App, result, _| {
            crate::actions::picked(s, result, then);
        });
    }
}

/// Settings → Library folders: what the Library indexes (read only), with its counts.
fn library_folders(s: &App, t: &Theme) -> El {
    let cfg = s.backend.settings();
    let ui = &s.library;
    let totals = if ui.progress.scanning {
        "scanning…".to_string()
    } else if ui.index.is_empty() {
        String::new()
    } else {
        format!(
            "{} songs · {} albums",
            ui.index.tracks.len(),
            ui.index.albums.len()
        )
    };
    let mut b = block("Library folders", &icons::LIBRARY, t).child(
        row()
            .w_full()
            .center_y()
            .child(
                text(totals)
                    .key("library-totals")
                    .font(tnum(META))
                    .color(t.text_secondary)
                    .help("Files play where they are; nothing is copied or changed")
                    .grow(1.0),
            )
            .child(
                link_icon("library-rescan", &icons::REFRESH_CW, "Rescan", t)
                    .on_press(|s: &mut App| s.rescan_library()),
            ),
    );
    for (i, dir) in cfg.library_dirs.iter().enumerate() {
        let path = dir.display().to_string();
        let (open, remove) = (dir.clone(), dir.clone());
        b = b.child(
            line(
                &format!("set-library-{i}"),
                row()
                    .gap(6.0)
                    .center_y()
                    .child(icons::folder(icons::MD, t.text_secondary))
                    .child(label(fmt::home_short(&path), t).shrink().help(path.clone())),
            )
            .hover_group()
            .child(
                mini_icon("open", &icons::FOLDER_OPEN, "Open", t)
                    .group_opacity(0.0, 1.0)
                    .on_press(move |s: &mut App| s.desktop.open(&open)),
            )
            .child(
                mini_icon("remove", &icons::X, "Remove", t)
                    .group_opacity(0.0, 1.0)
                    .help("Stop listing this folder (its files stay)")
                    .on_press(move |s: &mut App| s.remove_library_dir(&remove)),
            ),
        );
    }
    if s.settings_form.manual.is_some() {
        b.child(
            field_box(
                "set-library",
                input(&ui.folder_input, "Folder path", t)
                    .on_edit(|s: &mut App| &mut s.library.folder_input)
                    .on_submit(|s: &mut App, _| s.add_typed_library_dir()),
                t,
            )
            .w_full(),
        )
    } else {
        b.child(
            row().child(
                mini_icon("add-library", &icons::PLUS, "Add folder…", t)
                    .on_press_tx(choose_folder("Add a library folder", App::add_library_dir)),
            ),
        )
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

pub fn settings(s: &App, t: &Theme) -> El {
    let f = &s.settings_form;
    let cfg = s.backend.settings();
    let saved = f.saved_at.map_or_else(el, |_| {
        row()
            .key("saved")
            .gap(4.0)
            .center_y()
            .child(icons::check(icons::MD, green()))
            .child(text("Saved").font(META).color(t.text_secondary))
    });
    let mut body = col().w_full().pb(16.0);

    // Account
    let who = match &s.conn {
        Conn::Connected { username, .. } => format!("{username} · connected"),
        Conn::Connecting => "connecting…".into(),
        Conn::Reconnecting(_) => "reconnecting…".into(),
        Conn::LoginRejected => "login rejected".into(),
        Conn::Displaced => "signed in elsewhere".into(),
        Conn::Offline => "offline".into(),
    };
    let mut who_el = label(who, t).key("set-account");
    if !s.keyring_available {
        who_el = who_el.help("No system keyring: sign in each launch");
    }
    body = body.child(
        block("Account", &icons::USER, t).child(
            line("account", who_el)
                .child(
                    mini_icon("logout", &icons::LOG_OUT, "Log out", t)
                        .on_press(|s: &mut App| s.logout(false)),
                )
                .child(
                    mini_icon("forget", &icons::USER_X, "Forget account", t)
                        .help("Remove the saved password")
                        .on_press(|s: &mut App| s.logout(true)),
                ),
        ),
    );

    // Downloads
    let dl = cfg.download_dir.display().to_string();
    let mut downloads = block("Downloads", &icons::DOWNLOAD, t);
    if f.manual.is_some() {
        downloads = downloads.child(
            field_box(
                "set-download",
                input(&f.download_dir, "Folder path", t)
                    .on_edit(|s: &mut App| &mut s.settings_form.download_dir)
                    .on_submit(|s: &mut App, _| s.set_typed_download_dir()),
                t,
            )
            .w_full(),
        );
    } else {
        let open = cfg.download_dir.clone();
        downloads = downloads.child(
            line(
                "set-download",
                label(fmt::home_short(&dl), t)
                    .color(t.text)
                    .help(dl.clone()),
            )
            .child(
                mini_icon("choose-download", &icons::FOLDER, "Choose…", t)
                    .on_press_tx(choose_folder("Download folder", App::set_download_dir)),
            )
            .child(
                mini_icon("open-download", &icons::FOLDER_OPEN, "Open", t)
                    .on_press(move |s: &mut App| s.desktop.open(&open)),
            ),
        );
    }
    let single = cfg.single_file_subfolder;
    downloads = downloads.child(
        line(
            "single-subfolder",
            label("Put single songs in their album folder", t)
                .help("Albums always keep their folder"),
        )
        .help("Albums always keep their folder")
        .child(
            super::switch(
                "set-single-subfolder",
                single,
                "Put single songs in their album folder",
                t,
            )
            .on_press(|s: &mut App| {
                s.commit_settings(|c| c.single_file_subfolder = !c.single_file_subfolder);
            }),
        ),
    );
    body = body.child(downloads);

    // Shares
    let totals = s
        .share_counts
        .map_or_else(String::new, |(d, n)| format!("{d} folders · {n} files"));
    let mut shares = block("Shares", &icons::SHARE_2, t).child(
        row()
            .w_full()
            .center_y()
            .child(
                text(totals)
                    .key("share-totals")
                    .font(tnum(META))
                    .color(t.text_secondary)
                    .grow(1.0),
            )
            .child(
                link_icon("rescan", &icons::REFRESH_CW, "Rescan", t)
                    .on_press(|s: &mut App| s.rescan()),
            ),
    );
    for (i, dir) in cfg.share_dirs.iter().enumerate() {
        let path = dir.display().to_string();
        let count = match s.status.shares.iter().find(|x| &x.dir == dir) {
            Some(x) if x.scanning => "scanning…".to_string(),
            Some(x) => format!("{} files", x.files),
            None => String::new(),
        };
        let (open, remove) = (dir.clone(), dir.clone());
        shares = shares.child(
            line(
                &format!("set-share-{i}"),
                row()
                    .gap(6.0)
                    .center_y()
                    .child(icons::folder(icons::MD, t.text_secondary))
                    .child(label(fmt::home_short(&path), t).shrink().help(path.clone()))
                    .child(
                        text(count)
                            .key("files")
                            .font(tnum(META))
                            .color(t.text_secondary),
                    ),
            )
            .hover_group()
            .child(
                mini_icon("open", &icons::FOLDER_OPEN, "Open", t)
                    .group_opacity(0.0, 1.0)
                    .on_press(move |s: &mut App| s.desktop.open(&open)),
            )
            .child(
                mini_icon("remove", &icons::X, "Remove", t)
                    .group_opacity(0.0, 1.0)
                    .on_press(move |s: &mut App| s.remove_share(&remove)),
            ),
        );
    }
    if f.manual.is_some() {
        shares = shares.child(
            field_box(
                "set-share",
                input(&f.share_input, "Folder path", t)
                    .on_edit(|s: &mut App| &mut s.settings_form.share_input)
                    .on_submit(|s: &mut App, _| s.add_typed_share()),
                t,
            )
            .w_full(),
        );
    } else {
        shares = shares.child(
            row().child(
                mini_icon("add-share", &icons::PLUS, "Add folder…", t)
                    .on_press_tx(choose_folder("Share a folder", App::add_share)),
            ),
        );
    }
    if let Some(e) = &f.path_error {
        shares = shares.child(
            text(e.clone())
                .key("path-error")
                .font(META)
                .color(red())
                .truncate(),
        );
    }
    body = body.child(shares);

    // Uploads + playback
    body = body.child(block("Uploads", &icons::UPLOAD, t).child(
        line("slots", label("Upload slots", t)).child(stepper(
            "set-slots",
            cfg.upload_slots.to_string(),
            App::step_upload_slots,
            t,
        )),
    ));
    body = body.child(block("Playback", &icons::VOLUME_2, t).child(
        line("timeout", label("Skip silent sources after", t)).child(stepper(
            "set-timeout",
            format!("{} s", cfg.play_start_timeout_secs),
            App::step_play_timeout,
            t,
        )),
    ));

    // Cache
    let c = &s.status.cache;
    let cap = if c.cap_bytes == 0 {
        cfg.cache_cap_bytes
    } else {
        c.cap_bytes
    };
    let mut clear = mini_icon("clear-cache", &icons::TRASH_2, "Clear cache", t);
    clear = if s.cache_in_use() {
        disabled(clear, "In use by the playing track", t)
    } else {
        clear.on_press(|s: &mut App| s.clear_cache())
    };
    body = body.child(
        block("Cache", &icons::HARD_DRIVE, t).child(
            line(
                "cache",
                label(
                    format!("{} of {}", fmt::size(c.used_bytes), fmt::size(cap)),
                    t,
                )
                .key("cache-usage")
                .help(cfg.cache_dir.display().to_string()),
            )
            .child(clear),
        ),
    );

    // Connection
    body = body.child(block("Connection", &icons::GLOBE, t).child(port_line(s, t)));
    if let Some(p) = f.port_confirm {
        body = body.child(
            row()
                .key("port-confirm")
                .px(14.0)
                .pt(6.0)
                .gap(6.0)
                .center_y()
                .child(
                    text(format!("Reconnect on port {p}?"))
                        .font(type_::LABEL)
                        .color(t.text)
                        .grow(1.0),
                )
                .child(
                    super::button_icon("port-reconnect", &icons::REFRESH_CW, "Reconnect", true, t)
                        .h(24.0)
                        .on_press(|s: &mut App| s.confirm_port()),
                )
                .child(
                    mini_icon("port-undo", &icons::UNDO_2, "Undo", t)
                        .on_press(|s: &mut App| s.undo_port()),
                ),
        );
    }

    // Updates
    body = body.child(
        block("Updates", &super::update::CIRCLE_ARROW_UP, t).child(
            line("check-updates", label("Check for updates", t)).child(
                super::switch(
                    "set-check-updates",
                    cfg.check_updates,
                    "Check for updates",
                    t,
                )
                .on_press(|s: &mut App| s.toggle_update_checks()),
            ),
        ),
    );
    // Last, so the blocks above keep their place.
    body = body.child(library_folders(s, t));
    body = body.child(super::update::version(t));
    panel("settings-panel", "Settings", saved, body, t)
}

/// "Port 2234 ● open" / "● not reachable" / "Port 45543 ● 2234 busy"; details in the tooltip.
fn port_line(s: &App, t: &Theme) -> El {
    let f = &s.settings_form;
    let l = &s.status.listen;
    let requested = if l.requested == 0 {
        s.backend.settings().listen_port
    } else {
        l.requested
    };
    let connected = matches!(s.conn, Conn::Connected { .. });
    let bound = l.bound.or(match &s.conn {
        Conn::Connected { listen_port, .. } => *listen_port,
        _ => None,
    });
    let mapped = s
        .port_mapping
        .filter(|(p, _)| Some(*p) == bound)
        .map(|(_, m)| m);
    let (dot, state, tip): (rune::Color, String, String) = match bound {
        _ if !connected => (t.indicator, "offline".into(), String::new()),
        None => (
            red(),
            "not listening".into(),
            format!("Couldn't open port {requested}"),
        ),
        Some(b) if b != requested => (
            amber(),
            format!("{requested} busy"),
            format!("Port {requested} is in use by another program; using {b} for now."),
        ),
        Some(b) => match mapped {
            Some(true) => (green(), "open".into(), String::new()),
            Some(false) => (
                amber(),
                "not reachable".into(),
                format!("Forward TCP {b} on your router so more users can connect."),
            ),
            None => (t.indicator, "checking".into(), String::new()),
        },
    };
    let shown_port = match bound {
        Some(b) if connected => b,
        _ => requested,
    };
    let mut status = row()
        .key("port-status")
        .gap(6.0)
        .center_y()
        .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
        .child(
            text(state)
                .key("port-state")
                .font(META)
                .color(t.text_secondary),
        );
    if !tip.is_empty() {
        status = status.help(tip);
    }
    let mut out = line(
        "port",
        label(format!("Port {shown_port}"), t).key("port-label"),
    );
    if f.port_editing {
        out = out.child(
            field_box(
                "set-port",
                input(&f.listen_port, "2234", t)
                    .on_edit(|s: &mut App| &mut s.settings_form.listen_port)
                    .on_submit(|s: &mut App, _| s.propose_port()),
                t,
            )
            .w(72.0)
            .help("Enter to apply"),
        );
        if f.port_invalid {
            out = out.child(
                text("1024–65535")
                    .key("port-invalid")
                    .font(META)
                    .color(red()),
            );
        }
    } else {
        out = out.child(status).child(
            link_icon("change-port", &icons::PENCIL, "Change", t).on_press_tx(|s: &mut App, tx| {
                s.settings_form.port_editing = true;
                tx.focus("set-port", true);
            }),
        );
    }
    out
}
