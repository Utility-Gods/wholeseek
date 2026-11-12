//! A search tab: query field and filter chips, then results grouped by user and folder
//! (Nicotine+-style), in rank order.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollPos, ScrollSpec, col, el, row, text};

use std::rc::Rc;

use wholeseek_core::RemoteFile;

use super::files::{RowCtx, action, columns, file_row, more_key};
use super::{META, accent, amber, field_box, green, hairline, icons, input, tnum, vline};
use crate::app::{App, MAX_ROWS};
use crate::fmt;
use crate::tabs::{Filters, ResultsLayout, SearchTab, Tab};

const FOLDER_INDENT: f32 = 14.0;
const RECENT_W: f32 = 400.0;
const FILE_INDENT: f32 = 26.0;

pub fn page(s: &App, tab: &Tab, st: &SearchTab, t: &Theme) -> El {
    let query = field_box(
        "search",
        input(&st.edit, "Search music…", t)
            .key("query")
            .on_edit(|s: &mut App| &mut s.tab_mut().search_mut().expect("search tab is shown").edit)
            .on_submit(|s: &mut App, _| s.submit_search())
            .on_input_key(recent_key)
            .on_input_change(|s: &mut App, _| {
                // Typing narrows the open list and drops its highlight.
                let n = s.recent_matches().len();
                if let Some(st) = s.tab_mut().search_mut()
                    && st.recent.is_some()
                {
                    st.recent = (n > 0).then_some(None);
                }
            }),
        t,
    )
    .grow(1.0)
    .max_w(560.0)
    .child(icons::search(icons::MD, t.text_secondary));
    let mut bar = row()
        .key("search-bar")
        .h(44.0)
        .w_full()
        .px(12.0)
        .gap(12.0)
        .center_y()
        .child(query);
    if let Some(v) = &st.search {
        bar = bar.child(wish_button(s, &v.query, t).ml(-6.0));
    }
    bar = bar.child(el().grow(1.0));
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
                    .gap(8.0)
                    .help({
                        use crate::desktop::chord;
                        format!(
                            "{} new tab · {} close · {} switch · {} search · {} filter results",
                            chord("T"),
                            chord("W"),
                            chord("Tab"),
                            chord("L"),
                            chord("F")
                        )
                    })
                    .child(icons::draw(
                        &icons::SEARCH,
                        24.0,
                        t.text_secondary.alpha(0.6),
                    ))
                    .child(
                        text("Search Soulseek")
                            .font(type_::TITLE)
                            .color(t.text_secondary),
                    ),
            );
    };
    // "128 results · 34 users · searching…" while the core's window is open.
    let searching = !search.finished || s.status.searches.iter().any(|p| p.id == search.id);
    let total = search.hits.len();
    let users = {
        let mut u: Vec<&str> = search
            .hits
            .iter()
            .map(|h| h.file.username.as_str())
            .collect();
        u.sort_unstable();
        u.dedup();
        u.len()
    };
    let mut summary = row().gap(8.0).center_y();
    if total > 0 {
        summary = summary.child(
            text(format!(
                "{total} {} · {users} {}",
                if total == 1 { "result" } else { "results" },
                if users == 1 { "user" } else { "users" }
            ))
            .key("count")
            .font(tnum(META))
            .color(t.text_secondary),
        );
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
    let bar = match broadened(s, search, t) {
        Some(line) => col().w_full().child(bar).child(line),
        None => col().w_full().child(bar),
    };

    // Nothing came back: no chips, filter or table, just what happened and what to do.
    if total == 0 {
        let mut body = col().key("no-results").grow(1.0).w_full().center().gap(6.0);
        if !searching {
            body = body
                .child(icons::draw(&icons::SEARCH, 20.0, t.text_secondary.alpha(0.6)).mb(4.0))
                .child(
                    text(format!("No results for “{}”", search.query))
                        .key("no-results-title")
                        .font(type_::BODY)
                        .color(t.text_secondary)
                        .max_w(640.0)
                        .truncate(),
                )
                .child(
                    text("Check the spelling or use fewer words")
                        .font(META)
                        .color(t.text_secondary.alpha(0.75)),
                )
                .child(
                    action(
                        "search-again",
                        "Search again",
                        icons::retry(icons::MD, t.text),
                        t,
                    )
                    .mt(8.0)
                    .on_press(|s: &mut App| s.search_again()),
                );
        }
        return col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(bar)
            .child(hairline(t))
            .child(body);
    }

    let f = st.shown_filters();
    let f = &f;
    let groups = search.groups(f);
    let shown = search.visible(f);
    let mut chips = row().key("chips").gap(6.0).center_y().shrink().min_w(0.0);
    {
        // No 0-count chip, except one that is on (so it can be turned off again).
        let counts: Vec<_> = search
            .format_counts(f)
            .into_iter()
            .filter(|&(l, n)| n > 0 || f.has(l))
            .collect();
        let (shown_chips, rest) = counts.split_at(counts.len().min(MAX_FORMAT_CHIPS));
        for &(label, n) in shown_chips {
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
    let first_row = groups
        .first()
        .and_then(|g| g.folders.first())
        .and_then(|(_, v)| v.first())
        .map(|i| format!("row-{i}"));
    // Chips left, the filter right. Built right-to-left so the filter comes first in Tab
    // order (query → filter → chips → results) while it sits at the right end.
    // The filter ends where the table it narrows ends.
    let tools = row()
        .key("tools")
        .h(36.0)
        .w_full()
        .max_w(super::details::RESULTS_MAX_W)
        .px(12.0)
        .gap(12.0)
        .center_y()
        .dir(rune::Direction::Rtl)
        .child(find_box(st, shown, total, first_row, t));
    let tools = if super::SEARCH_BATCH {
        tools.child(sort_button(st, t))
    } else {
        tools
    };
    let tools = tools
        .child(el().grow(1.0))
        .child(chips.dir(rune::Direction::Ltr));
    let tools = if super::SEARCH_BATCH {
        tools.child(layout_switch(st, t))
    } else {
        tools
    };
    let find = Some(std::rc::Rc::new(f.text.clone())).filter(|q| !q.is_empty());

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
                        .spans(super::files::match_spans(find.as_deref(), &g.username))
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
                s,
                tab.group
                    .as_ref()
                    .is_some_and(|(u, d)| u == &g.username && d == folder),
                find.as_deref(),
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
                    find: find.clone(),
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
    if shown == 0 {
        list = list.child(nothing_matches(st, t));
    }
    let albums = super::SEARCH_BATCH && f.layout == ResultsLayout::Albums && shown > 0;
    if albums && let Some(g) = st.grouped(f) {
        list = super::albums::list(s, tab, &g, find.as_deref(), t);
    }
    let saved = tab.scroll.clone();
    // The results take what the inspector leaves; their rows cap at RESULTS_MAX_W, left-aligned
    // so nothing moves when the inspector opens. Never wider than the window: no fixed
    // widths from the measured window here (a stale width pushed the page off-screen).
    let results = col().key("results").h_full().grow(1.0).shrink().min_h(0.0);
    let results = if shown > 0 && !albums {
        results.child(columns(s, FILE_INDENT, t)).child(hairline(t))
    } else {
        results
    };
    let results = results.child(
        list.key(tab.list_key())
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .on_scroll_offset(move |at: ScrollPos| saved.set(at.offset[1]))
            .shrink()
            .min_h(0.0),
    );
    let inspector = super::details::inspector(s, tab, st, t);
    // An open drawer lies over the right of the page: the results and the inspector fit in
    // what it leaves rather than running under it (an overlaid inspector is pinned left of
    // it too: pins ignore padding).
    let drawer = if matches!(s.panel, crate::app::Panel::None) {
        0.0
    } else {
        super::DRAWER_W + 1.0
    };
    let mut body = row()
        .key("results-split")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .pr(drawer)
        .child(results);
    if let Some(pane) = inspector {
        let pane = row().h_full().child(vline(t)).child(pane);
        body = if super::details::overlays(s) {
            // Narrow window: over the results from the right, which keep their layout.
            body.child(pane.pin(None, Some(0.0), Some(drawer), Some(0.0)).shadow(
                0.0,
                16.0,
                rune::Color::BLACK.alpha(0.35),
            ))
        } else {
            body.child(pane)
        };
    }
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(bar)
        .child(tools)
        .child(hairline(t))
        .child(body)
}

/// "↕ Relevance": the sort menu (the current order checked).
fn sort_button(st: &SearchTab, t: &Theme) -> El {
    let by = st.filters.sort;
    row()
        .key("sort")
        .h(26.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .radius(4.0)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(format!("Sort: {}", by.label()))
        .help("Sort results")
        .focusable(true)
        .hover(t.hover)
        .dir(rune::Direction::Ltr)
        .child(icons::draw(
            &icons::ARROW_UP_DOWN,
            icons::SM,
            t.text_secondary,
        ))
        .child(text(by.label()).font(META).color(t.text_secondary))
        .on_press(move |s: &mut App| {
            let items = sort_items(by);
            s.open_menu(crate::menu::MenuAnchor::Key("sort".into()), items);
        })
}

/// The sort menu's items: every order, the current one checked.
pub fn sort_items(current: crate::tabs::SortBy) -> Vec<crate::menu::MenuItem> {
    use crate::tabs::SortBy;
    SortBy::ALL
        .into_iter()
        .map(|by| crate::menu::MenuItem {
            id: match by {
                SortBy::Relevance => "sort-relevance",
                SortBy::Availability => "sort-availability",
                SortBy::Size => "sort-size",
                SortBy::Length => "sort-length",
                SortBy::Name => "sort-name",
            },
            label: by.label().to_string(),
            action: crate::menu::MenuAction::SetSort(by),
            icon: (by == current).then_some(&icons::CHECK),
            sep: false,
            help: None,
        })
        .collect()
}

/// Albums | Folders: how the results are listed.
fn layout_switch(st: &SearchTab, t: &Theme) -> El {
    let seg = |key: &str, label: &str, icon: &'static rune::Icon, l: ResultsLayout| {
        let on = st.filters.layout == l;
        let fg = if on { t.text } else { t.text_secondary };
        let mut b = row()
            .key(key.to_string())
            .h(22.0)
            .px(8.0)
            .gap(5.0)
            .center_y()
            .radius(3.0)
            .cursor(Cursor::Pointer)
            .role("button")
            .label(label.to_string())
            .focusable(true)
            .hover(t.hover)
            .child(icons::draw(icon, icons::SM, fg))
            .child(text(label.to_string()).font(META).color(fg))
            .on_press(move |s: &mut App| {
                if let Some(st) = s.tab_mut().search_mut() {
                    st.filters.layout = l;
                    s.last_filters.layout = l;
                }
            });
        if on {
            b = b.fill(t.control_track);
        }
        b
    };
    row()
        .key("layout")
        .h(26.0)
        .px(2.0)
        .gap(2.0)
        .center_y()
        .radius(4.0)
        .border(1.0, t.card_border)
        .dir(rune::Direction::Ltr)
        .child(seg(
            "layout-albums",
            "Albums",
            &icons::DISC_3,
            ResultsLayout::Albums,
        ))
        .child(seg(
            "layout-folders",
            "Folders",
            &icons::FOLDER,
            ResultsLayout::Folders,
        ))
}

/// ☆: the search's query on the wishlist or off it.
fn wish_button(s: &App, query: &str, t: &Theme) -> El {
    let on = s.wished(query);
    let q = query.to_string();
    super::icon_button(
        "wish",
        if on {
            icons::draw(&icons::STAR_SOLID, icons::LG, accent())
        } else {
            icons::draw(&icons::STAR, icons::LG, t.text_secondary)
        },
        t,
    )
    .size(28.0, 28.0)
    .label(if on {
        "Remove from wishlist"
    } else {
        "Add to wishlist"
    })
    .help(if on {
        "On your wishlist: searched again every so often. Click to remove"
    } else {
        "Add to wishlist: search for this again every so often"
    })
    .on_press(move |s: &mut App| s.toggle_wish(&q))
}

/// "Showing results for …": the typed words found nothing, so the core searched once more
/// with broader ones (accents folded, one-letter words dropped).
fn broadened(s: &App, search: &crate::tabs::SearchView, t: &Theme) -> Option<El> {
    let info = s.backend.search_info(search.id)?;
    let to = info.broadened_to?;
    Some(
        row()
            .key("broadened")
            .h(22.0)
            .w_full()
            .px(14.0)
            .pb(4.0)
            .gap(4.0)
            .center_y()
            .help(format!(
                "Nothing matched “{}”, so these are for a broader search",
                info.typed
            ))
            .child(
                text("Showing results for")
                    .font(META)
                    .color(t.text_secondary),
            )
            .child(
                text(to)
                    .key("broadened-to")
                    .font(META)
                    .color(t.text)
                    .truncate()
                    .shrink(),
            ),
    )
}

/// "Nothing matches …" when the filter box or chips hide every result.
fn nothing_matches(st: &SearchTab, t: &Theme) -> El {
    let words = st.find.text.trim();
    let (title, button, clear): (String, &str, fn(&mut App)) = if words.is_empty() {
        (
            "Nothing matches these filters".into(),
            "Clear filters",
            |s| {
                if let Some(st) = s.tab_mut().search_mut() {
                    st.filters.formats.clear();
                    st.filters.free_slot = false;
                    let chosen = st.filters.clone();
                    s.last_filters = chosen;
                }
            },
        )
    } else {
        (
            format!("Nothing matches “{words}”"),
            "Clear filter",
            |s| s.clear_find(),
        )
    };
    col()
        .key("nothing-matches")
        .w_full()
        .pt(64.0)
        .items_center()
        .gap(10.0)
        .child(icons::draw(
            &icons::FUNNEL,
            20.0,
            t.text_secondary.alpha(0.6),
        ))
        .child(
            text(title)
                .key("nothing-matches-title")
                .font(type_::BODY)
                .color(t.text_secondary)
                .max_w(560.0)
                .truncate(),
        )
        .child(
            action("clear-filter", button, icons::close(icons::MD, t.text), t).on_press_tx(
                move |s: &mut App, tx| {
                    clear(s);
                    tx.focus("filter", false);
                },
            ),
        )
}

/// The "Filter results" box: secondary to the query (smaller, muted icon); while it holds
/// words, "42 of 257" and × beside it. Esc clears, then leaves; ↓ goes to the first result.
fn find_box(
    st: &SearchTab,
    shown: usize,
    total: usize,
    first_row: Option<String>,
    t: &Theme,
) -> El {
    let active = !st.find.text.trim().is_empty();
    let field = input(&st.find, "Filter results", t)
        .key("filter")
        .font(META)
        .on_edit(|s: &mut App| &mut s.tab_mut().search_mut().expect("search tab is shown").find)
        .on_input_change(|s: &mut App, _| s.find_changed())
        .on_input_key(move |s: &mut App, e, tx| match e.key {
            rune::Key::Escape => {
                let filled = s.tab().search().is_some_and(|st| !st.find.text.is_empty());
                if filled {
                    s.clear_find();
                } else {
                    tx.blur();
                }
                true
            }
            rune::Key::Down => {
                if let Some(r) = &first_row {
                    tx.focus(r.clone(), true);
                }
                true
            }
            _ => false,
        });
    let mut b = row()
        .key("find")
        .h(28.0)
        .w(220.0)
        .pl(8.0)
        .pr(4.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .help("Narrow these results: words match the start of words in the path or user; -word excludes")
        .child(icons::filter(icons::SM, t.text_secondary.alpha(0.8)))
        .child(field.grow(1.0).shrink().min_w(0.0));
    if active {
        b = b.child(
            super::icon_button("filter-clear", icons::close(icons::SM, t.text_secondary), t)
                .size(18.0, 18.0)
                .label("Clear filter")
                .on_press_tx(|s: &mut App, tx| {
                    s.clear_find();
                    tx.focus("filter", false);
                }),
        );
    }
    let mut r = row().gap(8.0).center_y().dir(rune::Direction::Ltr);
    if active {
        r = r.child(
            text(format!("{shown} of {total}"))
                .key("find-count")
                .font(tnum(META))
                .color(t.text_secondary),
        );
    }
    r.child(b)
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
        .focusable(true)
        .focus_ring(1.0, -1.0, super::accent().alpha(0.6))
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
        .focusable(true)
        .focus_ring(1.0, -1.0, super::accent().alpha(0.6))
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
            s.find_changed();
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
    s: &App,
    selected: bool,
    find: Option<&crate::tabs::TextQuery>,
    t: &Theme,
) -> El {
    let (tab, max_w) = (s.tab().id, super::details::RESULTS_MAX_W);
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
        .child(icons::folder(icons::SM, t.text_secondary));
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
                .spans(super::files::match_spans(find, parts[i]))
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
        super::icon_button(&more, icons::more(icons::MD, t.text_secondary), t)
            .size(24.0, 20.0)
            .label("Folder actions")
            .group_opacity(0.0, 1.0)
            .on_press(move |s: &mut App| {
                let items = s.folder_menu(&u2, &d2);
                s.open_menu(crate::menu::MenuAnchor::Key(more2.clone()), items);
            }),
    )
}

/// ↓/↑ open and walk the recent searches under the query box, Enter searches the highlighted
/// one, Shift+Delete forgets it, Esc closes the list.
fn recent_key(s: &mut App, e: &rune::KeyEvent, _tx: &mut rune::Tx) -> bool {
    use rune::Key;
    let open = s.tab().search().and_then(|st| st.recent);
    match (&e.key, open) {
        (Key::Down, _) => s.recent_step(true),
        (Key::Up, Some(_)) => s.recent_step(false),
        (Key::Escape, Some(_)) => {
            s.close_recent();
            true
        }
        (Key::Enter, Some(Some(i))) => match s.recent_matches().get(i).cloned() {
            Some(q) => {
                s.pick_recent(&q);
                true
            }
            None => false,
        },
        (Key::Delete, Some(Some(i))) if e.mods.shift => {
            if let Some(q) = s.recent_matches().get(i).cloned() {
                s.forget_recent(&q);
            }
            true
        }
        _ => false,
    }
}

/// The recent searches under the query box, when open: a layer at the root (a click
/// outside closes it), anchored to the query box.
pub fn recent_layer(s: &App, t: &Theme) -> Option<El> {
    let sel = s.tab().search()?.recent?;
    let items = s.recent_matches();
    if items.is_empty() {
        return None;
    }
    let mut list = col()
        .key("recent")
        .w(RECENT_W)
        .py(4.0)
        .radius(6.0)
        .fill(super::pal::raised())
        .border(1.0, super::pal::line_strong())
        .role("listbox")
        .anchored("search", rune::Side::Bottom, rune::Align::Start, 2.0);
    for (i, q) in items.into_iter().enumerate() {
        let (pick, forget) = (q.clone(), q.clone());
        let mut entry = row()
            .key(format!("recent-{i}"))
            .h(28.0)
            .w_full()
            .pl(10.0)
            .pr(4.0)
            .gap(10.0)
            .center_y()
            .hover(t.hover)
            .press(t.press)
            .cursor(Cursor::Pointer)
            .role("option")
            .label(q.clone())
            .on_press(move |s: &mut App| s.pick_recent(&pick))
            .child(icons::draw(&icons::HISTORY, icons::MD, t.text_secondary))
            .child(
                text(q)
                    .font(type_::BODY)
                    .color(t.text)
                    .truncate()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0),
            )
            .child(
                super::icon_button(
                    &format!("recent-forget-{i}"),
                    icons::close(icons::SM, t.text_secondary),
                    t,
                )
                .help("Remove from recent searches")
                .on_press_tx(move |s: &mut App, tx| {
                    s.forget_recent(&forget);
                    tx.focus("query", true); // ↑/↓ keep working
                }),
            );
        if sel == Some(i) {
            entry = entry.fill(t.hover);
        }
        list = list.child(entry);
    }
    Some(
        el().key("recent-layer")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .on_press(|s: &mut App| s.close_recent())
            .child(list),
    )
}
