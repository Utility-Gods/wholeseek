//! A search tab: query field and filter chips, then results grouped by user and folder
//! (Nicotine+-style), in rank order.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollPos, ScrollSpec, col, el, row, text};

use std::rc::Rc;

use wholeseek_core::RemoteFile;

use super::files::{RowCtx, columns, file_row, more_key};
use super::{META, amber, field_box, green, hairline, icons, input, tnum, vline};
use crate::app::{App, MAX_ROWS};
use crate::fmt;
use crate::tabs::{Filters, SearchTab, Tab};

const FOLDER_INDENT: f32 = 14.0;
const FILE_INDENT: f32 = 26.0;

pub fn page(s: &App, tab: &Tab, st: &SearchTab, t: &Theme) -> El {
    let query = field_box(
        "search",
        input(&st.edit, "Search music…", t)
            .key("query")
            .on_edit(|s: &mut App| &mut s.tab_mut().search_mut().expect("search tab is shown").edit)
            .on_submit(|s: &mut App, _| s.submit_search()),
        t,
    )
    .grow(1.0)
    .max_w(560.0)
    .child(icons::search(11.0, t.text_secondary));
    let f = &st.filters;
    let mut chips = row().key("chips").gap(6.0).center_y();
    if let Some(v) = &st.search {
        let counts = v.format_counts(f);
        let (shown, rest) = counts.split_at(counts.len().min(MAX_FORMAT_CHIPS));
        for &(label, n) in shown {
            chips = chips.child(chip(
                crate::tabs::format_key(label),
                label,
                Some(n),
                f.has(label),
                move |f| f.toggle(label),
                t,
            ));
        }
        if !rest.is_empty() {
            chips = chips.child(more_formats(rest, f, t));
        }
    }
    let chips = chips.child(chip(
        "free-slot",
        "Free slot",
        None,
        f.free_slot,
        |f| f.free_slot = !f.free_slot,
        t,
    ));
    let mut bar = row()
        .key("search-bar")
        .h(44.0)
        .w_full()
        .px(12.0)
        .gap(12.0)
        .center_y()
        .child(query)
        .child(chips)
        .child(el().grow(1.0));
    let Some(search) = &st.search else {
        return col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(bar)
            .child(hairline(t))
            .child(
                col()
                    .key("empty")
                    .grow(1.0)
                    .center()
                    .gap(6.0)
                    .help({
                        use crate::desktop::chord;
                        format!(
                            "{} new tab · {} close · {} switch · {} search",
                            chord("T"),
                            chord("W"),
                            chord("Tab"),
                            chord("L")
                        )
                    })
                    .child(
                        text("Search Soulseek")
                            .font(type_::TITLE)
                            .color(t.text_secondary),
                    ),
            );
    };
    let groups = search.groups(f);
    let shown = search.visible(f);
    // "128 results · 34 users · searching…" while the core's window is open.
    let searching = !search.finished || s.status.searches.iter().any(|p| p.id == search.id);
    let mut summary = row().gap(8.0).center_y().child(
        text(format!(
            "{shown} {} · {} {}",
            if shown == 1 { "result" } else { "results" },
            groups.len(),
            if groups.len() == 1 { "user" } else { "users" }
        ))
        .key("count")
        .font(tnum(META))
        .color(t.text_secondary),
    );
    if shown < search.hits.len() {
        summary = summary.help(format!("{} hidden by filters", search.hits.len() - shown));
    }
    if searching {
        summary = summary.child(
            text("searching…")
                .key("searching")
                .font(META)
                .color(amber()),
        );
    }
    bar = bar.child(summary);

    let mut list = col().w_full().pb(8.0);
    let mut drawn = 0;
    'groups: for (gi, g) in groups.iter().enumerate() {
        let user = g.username.clone();
        let slot = if g.free_slot { "free slot" } else { "queued" };
        list = list.child(
            row()
                .key(format!("group-{gi}"))
                .h(28.0)
                .w_full()
                .px(12.0)
                .pt(4.0)
                .gap(8.0)
                .center_y()
                .child(el().size(6.0, 6.0).radius(3.0).fill(if g.free_slot {
                    green()
                } else {
                    amber()
                }))
                .child(
                    text(g.username.clone())
                        .key("user")
                        .font(type_::TITLE)
                        .color(t.text)
                        .cursor(Cursor::Pointer)
                        .role("link")
                        .help("Open this user in a tab")
                        .on_press(move |s: &mut App| s.open_user(&user)),
                )
                .child(
                    text(format!(
                        "{} · {slot} · {} {}",
                        fmt::speed(f64::from(g.speed)),
                        g.files,
                        if g.files == 1 { "file" } else { "files" }
                    ))
                    .key("meta")
                    .font(tnum(META))
                    .color(t.text_secondary),
                ),
        );
        for (fi, (folder, idx)) in g.folders.iter().enumerate() {
            list = list.child(folder_line(
                &format!("folder-{gi}-{fi}"),
                &g.username,
                folder,
                s.tab().id,
                super::details::wide(s)
                    && tab
                        .group
                        .as_ref()
                        .is_some_and(|(u, d)| u == &g.username && d == folder),
                super::details::table_max_w(s),
                t,
            ));
            let files: Rc<[RemoteFile]> = s.folder_files(&g.username, folder).into();
            for &i in idx {
                if drawn == MAX_ROWS {
                    break 'groups;
                }
                drawn += 1;
                let f = &search.hits[i].file;
                let ctx = RowCtx {
                    folder: files.clone(),
                    in_user_tab: false,
                    selected: tab.selected.as_deref() == Some(f.path.as_str()),
                    highlighted: false,
                    more_key: more_key(tab.id, &f.path),
                };
                list = list.child(file_row(s, &format!("row-{i}"), f, FILE_INDENT, &ctx, t));
            }
        }
    }
    if drawn == MAX_ROWS && shown > MAX_ROWS {
        list = list.child(
            text(format!("Showing the first {MAX_ROWS} of {shown}."))
                .font(META)
                .color(t.text_secondary)
                .px(12.0)
                .pt(10.0),
        );
    }
    if search.finished && shown == 0 {
        list = list.child(
            text(if search.hits.is_empty() {
                "No results"
            } else {
                "No matches for these filters"
            })
            .key("no-results")
            .font(type_::BODY)
            .color(t.text_secondary)
            .px(12.0)
            .pt(12.0),
        );
    }
    let saved = tab.scroll.clone();
    let wide = super::details::wide(s);
    let mut results = col().key("results").min_h(0.0);
    results = if wide {
        results.w(super::details::TABLE_MAX_W).h_full()
    } else {
        results.w_full().grow(1.0)
    };
    let results = results
        .child(columns(s, FILE_INDENT, t))
        .child(hairline(t))
        .child(
            list.key(tab.list_key())
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_scroll_offset(move |at: ScrollPos| saved.set(at.offset[1]))
                .shrink()
                .min_h(0.0),
        );
    let body = if wide {
        // Wide window: the table at its natural width, the selected folder beside it.
        row()
            .key("results-split")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(results)
            .child(vline(t))
            .child(super::details::pane(s, tab, st, t))
    } else {
        results
    };
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(bar)
        .child(hairline(t))
        .child(body)
}

/// Format chips shown before the rest fold into "+N".
pub const MAX_FORMAT_CHIPS: usize = 7;

/// "+N": the less common formats, toggled from a menu. Lit while one of them is selected.
fn more_formats(rest: &[(&'static str, usize)], f: &Filters, t: &Theme) -> El {
    let on = rest.iter().any(|(l, _)| f.has(l));
    let label = format!("+{}", rest.len());
    let mut c = row()
        .key("fmt-more")
        .h(22.0)
        .px(8.0)
        .center_y()
        .radius(4.0)
        .border(1.0, if on { t.control_track } else { t.separator })
        .cursor(Cursor::Pointer)
        .role("button")
        .label(format!("{} more formats", rest.len()))
        .hover(t.hover)
        .press(t.press)
        .on_press(|s: &mut App| {
            let items = s.more_format_items();
            s.open_menu(crate::menu::MenuAnchor::Key("fmt-more".into()), items);
        })
        .child(
            text(label)
                .font(META)
                .color(if on { t.text } else { t.text_secondary }),
        );
    if on {
        c = c.fill(t.hover);
    }
    c
}

/// A filter toggle. Flipping it filters the results already in; nothing is re-searched, and
/// the choice carries over to the next new search tab. `count` (format chips) is shown after
/// the label; a chip with nothing to show is dimmed but still clickable.
fn chip(
    key: &str,
    label: &str,
    count: Option<usize>,
    on: bool,
    flip: impl Fn(&mut Filters) + 'static,
    t: &Theme,
) -> El {
    let spoken = count.map_or_else(|| label.to_string(), |n| format!("{label} {n}"));
    let mut c = row()
        .key(key.to_string())
        .h(22.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .radius(4.0)
        .border(1.0, if on { t.control_track } else { t.separator })
        .cursor(Cursor::Pointer)
        .role("switch")
        .label(spoken)
        .checked(on)
        .hover(t.hover)
        .press(t.press)
        .on_press(move |s: &mut App| {
            if let Some(st) = s.tab_mut().search_mut() {
                flip(&mut st.filters);
                let chosen = st.filters.clone();
                s.last_filters = chosen;
            }
        })
        .child(text(label.to_string()).font(META).color(if on {
            t.text
        } else {
            t.text_secondary
        }));
    if let Some(n) = count {
        c = c.child(
            text(n.to_string())
                .key("n")
                .font(tnum(META))
                .color(t.text_secondary),
        );
        if n == 0 && !on {
            c = c.opacity(0.45);
        }
    }
    if on {
        c = c.fill(t.hover);
    }
    c
}

/// Folders longer than this many segments keep the first and the last two, "…" between.
const MAX_SEGMENTS: usize = 4;

/// A result folder: its path in segments, each opening the user's tab at that folder; ⋯ and
/// right-click open its menu. Long paths elide in the middle; the full path is the tooltip.
fn folder_line(
    key: &str,
    username: &str,
    folder: &str,
    tab: crate::tabs::TabId,
    selected: bool,
    max_w: f32,
    t: &Theme,
) -> El {
    let parts: Vec<&str> = folder.split('\\').collect();
    let shown: Vec<Option<usize>> = if parts.len() > MAX_SEGMENTS {
        let n = parts.len();
        vec![Some(0), None, Some(n - 2), Some(n - 1)]
    } else {
        (0..parts.len()).map(Some).collect()
    };
    let more = more_key(tab, folder);
    let (u, d) = (username.to_string(), folder.to_string());
    let (u2, d2, more2) = (u.clone(), d.clone(), more.clone());
    let pick = (u.clone(), d.clone());
    let mut r = row()
        .key(key.to_string())
        .h(22.0)
        .w_full()
        .max_w(max_w)
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut App| s.tab_mut().group = Some(pick.clone()))
        .pl(12.0 + FOLDER_INDENT)
        .pr(6.0)
        .gap(4.0)
        .center_y()
        .hover_group()
        .help(fmt::crumbs(folder))
        .hover(t.hover)
        .on_secondary_press(move |s: &mut App, p, _| {
            let items = s.folder_menu(&u, &d);
            super::menu::at_point(s, p, items);
        })
        .child(icons::folder(11.0, t.text_secondary));
    for (k, seg) in shown.into_iter().enumerate() {
        if k > 0 {
            r = r.child(text("/").font(META).color(t.text_secondary.alpha(0.6)));
        }
        let Some(i) = seg else {
            r = r.child(text("…").font(META).color(t.text_secondary));
            continue;
        };
        let target = parts[..=i].join("\\");
        let user = username.to_string();
        r = r.child(
            text(parts[i].to_string())
                .key(format!("seg-{i}"))
                .font(META)
                .color(t.text_secondary)
                .truncate()
                .shrink()
                .cursor(Cursor::Pointer)
                .role("link")
                .px(2.0)
                .radius(3.0)
                .hover(t.hover)
                .on_press_tx(move |s: &mut App, tx| {
                    s.open_folder_at(&user, &target, None);
                    s.restore_scroll(tx);
                }),
        );
    }
    if selected {
        r = r.fill(t.hover);
    }
    r.child(el().grow(1.0)).child(
        super::icon_button(&more, icons::more(12.0, t.text_secondary), t)
            .size(24.0, 20.0)
            .label("Folder actions")
            .group_opacity(0.0, 1.0)
            .on_press(move |s: &mut App| {
                let items = s.folder_menu(&u2, &d2);
                s.open_menu(crate::menu::MenuAnchor::Key(more2.clone()), items);
            }),
    )
}
