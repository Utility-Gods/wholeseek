//! The top strip: tabs (browser-like), ＋, then connection, ⇅ Transfers and ⚙ on the right.
//!
//! The right cluster never shrinks and is always whole. Tabs share what is left, each
//! shrinking (title ellipsized) to [`TAB_MIN_W`]; tabs that still don't fit fold into a "»"
//! menu, the active one always staying in the strip.
//!
//! As a browser's: the strip sits on a darker chrome, the active tab is filled with the page
//! surface and joins the content below (no line under it), and the hairline between two tabs
//! hides beside the active one and the hovered one. Nothing animates: a switch lands in one
//! frame, and hover comes at once and leaves within 80 ms.

use rune::tok::{TextStyle, Theme, type_};
use rune::{Anim, Cursor, Curve, El, PointerButton, Spring, el, row, text};

use super::{META, accent, amber, green, icon_button, icons, pal, red, tnum};
use crate::app::{App, Conn, Panel};
use crate::tabs::{Tab, TabKind};

const TAB_H: f32 = 40.0;
const TAB_MAX_W: f32 = 240.0;
/// Narrowest a tab gets: icon, a few letters of title, its count, ⋯ and ×.
pub const TAB_MIN_W: f32 = 120.0;
/// The ⋯ and × hit targets on a tab.
const TAB_BUTTON: f32 = 22.0;

/// The ⋯ button on tab `i`, the tab menu's anchor.
pub fn tab_more_key(i: usize) -> String {
    format!("tab-more-{i}")
}
/// Fixed parts of the strip besides tabs and the right cluster: ＋, "»", gaps, padding.
const STRIP_FIXED_W: f32 = 28.0 + 28.0 + 8.0 * 4.0 + 8.0;

/// Which tabs the strip shows (in order) and which fold under "»", for `n` tabs with the
/// `active` one in `room` points.
pub fn tab_split(n: usize, active: usize, room: f32) -> (Vec<usize>, Vec<usize>) {
    let fit = ((room / TAB_MIN_W).floor().max(1.0) as usize).min(n);
    if fit >= n {
        return ((0..n).collect(), Vec::new());
    }
    let start = if active < fit { 0 } else { active + 1 - fit };
    let shown: Vec<usize> = (start..start + fit).collect();
    let hidden = (0..n).filter(|i| !shown.contains(i)).collect();
    (shown, hidden)
}

pub fn view(s: &App, t: &Theme) -> El {
    let room = s.strip_w - s.strip_right_w - STRIP_FIXED_W;
    let (shown, hidden) = tab_split(s.tabs.len(), s.active, room);
    let mut tabs = row().key("tabs").h(TAB_H).min_w(0.0).shrink();
    for &i in &shown {
        tabs = tabs.child(tab_el(s, i, &s.tabs[i], t));
    }
    let more = (!hidden.is_empty()).then(|| {
        let label = format!("{} more tabs", hidden.len());
        icon_button(
            "tabs-more",
            text("»").font(type_::LABEL).color(t.text_secondary),
            t,
        )
        .label(label.clone())
        .help(label)
        .on_press(move |s: &mut App| {
            let items = s.hidden_tab_items(&hidden);
            s.open_menu(crate::menu::MenuAnchor::Key("tabs-more".into()), items);
        })
    });
    let new = icon_button("new-tab", icons::plus(icons::MD, t.text_secondary), t)
        .label("New search tab")
        .help(format!("New search ({})", crate::desktop::chord("T")))
        .on_press_tx(|s: &mut App, tx| {
            s.new_search_tab();
            tx.focus("query", false);
        });

    let (dot, label) = match &s.conn {
        Conn::Connected { username, .. } => (green(), username.clone()),
        Conn::Connecting => (amber(), "connecting…".into()),
        Conn::Reconnecting(_) => (amber(), "reconnecting…".into()),
        Conn::LoginRejected => (red(), "login rejected".into()),
        Conn::Displaced => (t.indicator, "signed in elsewhere".into()),
        Conn::Offline => (t.indicator, "offline".into()),
    };
    let status = row()
        .key("status")
        .gap(6.0)
        .center_y()
        .label(format!("Status: {label}"))
        .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
        .child(text(label).font(META).color(t.text_secondary));

    let active = s.active_transfers();
    let open = s.panel == Panel::Transfers;
    let mut transfers = row()
        .key("transfers")
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Transfers")
        .on_press(|s: &mut App| s.toggle_panel(Panel::Transfers))
        .child(icons::transfers(
            icons::MD,
            if open { accent() } else { t.text_secondary },
        ))
        .child(text("Transfers").font(type_::LABEL).color(if open {
            t.text
        } else {
            t.text_secondary
        }));
    if active > 0 {
        transfers = transfers.child(badge("transfers-count", active, t));
    }
    if open {
        transfers = transfers.fill(t.hover);
    }
    let settings_open = s.panel == Panel::Settings;
    let mut settings = icon_button(
        "settings",
        icons::gear(
            icons::LG,
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
        settings = settings.fill(t.hover);
    }
    let mut right = row()
        .key("strip-right")
        .gap(4.0)
        .center_y()
        .on_layout(|s: &mut App, r: [f32; 4]| s.strip_right_w = r[2])
        .child(status)
        .child(el().w(8.0));
    if let Some(u) = super::update::pill(s, t) {
        right = right.child(u);
    }
    let right = right.child(transfers).child(settings);
    let mut strip = row()
        .key("strip")
        .h(TAB_H)
        .w_full()
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .fill(pal::chrome())
        .on_layout(|s: &mut App, r: [f32; 4]| {
            s.strip_w = r[2];
            s.window_w = r[2];
        })
        // The line under the strip, drawn first: the active tab covers it.
        .child(
            el().key("strip-line")
                .pin(Some(0.0), None, Some(0.0), Some(0.0))
                .h(1.0)
                .fill(t.separator),
        )
        .child(tabs);
    if let Some(more) = more {
        strip = strip.child(more);
    }
    strip
        .child(new)
        .child(el().grow(1.0).min_w(0.0).shrink())
        .child(right)
}

fn badge(key: &str, n: usize, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(15.0)
        .min_w(15.0)
        .px(4.0)
        .center()
        .radius(3.0)
        .fill(t.control_track)
        .child(text(n.to_string()).font(tnum(META)).color(t.text))
}

/// A tab's hover group, shared with the separator on its left (drawn by the tab before it), so
/// hovering a tab hides the hairlines on both its sides.
fn hover_key(i: usize) -> String {
    format!("tab-hover-{i}")
}

fn tab_el(s: &App, i: usize, tab: &Tab, t: &Theme) -> El {
    let active = i == s.active;
    let (icon, count) = match &tab.kind {
        TabKind::Search(st) => (
            icons::search(icons::MD, t.text_secondary),
            st.search.as_ref().map(|v| v.visible(&st.filters)),
        ),
        TabKind::User(_) => (icons::user(icons::MD, t.text_secondary), None),
    };
    let mut label = row()
        .gap(8.0)
        .center_y()
        .min_w(0.0)
        .shrink()
        .child(icon)
        .child(
            text(tab.title())
                .key("title")
                .font(if active {
                    TextStyle::new(type_::BODY.size, 500.0)
                } else {
                    type_::BODY
                })
                .color(if active { t.text } else { t.text_secondary })
                .truncate()
                .shrink(),
        );
    if let Some(n) = count {
        label = label.child(badge("count", n, t));
    }
    let more_key = tab_more_key(i);
    let more = icon_button(&more_key, icons::more(icons::MD, t.text_secondary), t)
        .size(TAB_BUTTON, TAB_BUTTON)
        .label("Tab options")
        .help("Tab options")
        // On the active tab, and on the others while hovered.
        .group_opacity(if active { 1.0 } else { 0.0 }, 1.0)
        .on_press(move |s: &mut App| {
            let items = crate::menu::tab_items(i, s.tabs.len());
            s.open_menu(crate::menu::MenuAnchor::Key(more_key.clone()), items);
        });
    let close = icon_button("close", icons::close(icons::SM, t.text_secondary), t)
        .size(TAB_BUTTON, TAB_BUTTON)
        .label("Close tab")
        .on_press_tx(move |s: &mut App, tx| {
            tx.instant();
            s.close_tab(i);
            s.restore_scroll(tx);
        });
    let mut el = row()
        .key(format!("tab-{i}"))
        .h(TAB_H)
        .max_w(TAB_MAX_W)
        .shrink()
        .min_w(TAB_MIN_W)
        .pl(14.0)
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("tab")
        .label(tab.title())
        .checked(active)
        // A switch lands in the frame it's clicked: no fill or marker fades between tabs.
        .motion(Spring::NONE)
        .hover_group_shared(hover_key(i))
        .hover_motion_within(Anim::INSTANT, Curve::linear(0.08))
        .pointer_buttons([PointerButton::Primary, PointerButton::Middle])
        .on_press_tx(move |s: &mut App, tx| {
            tx.instant();
            if tx.button() == PointerButton::Middle {
                s.close_tab(i);
            } else {
                s.activate(i);
            }
            s.restore_scroll(tx);
        })
        .on_secondary_press(move |s: &mut App, p, _| {
            let items = crate::menu::tab_items(i, s.tabs.len());
            super::menu::at_point(s, p, items);
        })
        .child(label)
        .child(el().grow(1.0).min_w(0.0).shrink())
        .child(more)
        .child(close);
    // Active: the page's surface (it covers the strip's bottom line) and an orange marker on
    // top; the others sit on the chrome and only hover. Keyed, so neither part ever turns
    // into the other when the active tab changes.
    el = if active {
        el.fill(t.bg).child(
            rune::el()
                .key("marker")
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(2.0)
                .fill(accent()),
        )
    } else {
        el.hover(t.hover)
    };
    // The hairline to the next tab, only between two inactive tabs, and hidden while either
    // is hovered: this tab's group fades the outer part, the next tab's the inner.
    let next_active = i + 1 == s.active;
    if !active && !next_active {
        el = el.child(
            rune::el()
                .key("sep")
                .pin(None, Some(11.0), Some(0.0), Some(11.0))
                .w(1.0)
                .group_opacity(1.0, 0.0)
                .child(
                    rune::el()
                        .w_full()
                        .h_full()
                        .hover_group_shared(hover_key(i + 1))
                        .child(
                            rune::el()
                                .key("sep-line")
                                .w_full()
                                .h_full()
                                .fill(t.separator)
                                .group_opacity(1.0, 0.0),
                        ),
                ),
        );
    }
    el
}
