//! The top strip: tabs (browser-like), ＋, then connection, ⇅ Transfers and ⚙ on the right.
//!
//! The right cluster never shrinks and is always whole. Tabs share what is left, each
//! shrinking (title ellipsized) to [`TAB_MIN_W`]; tabs that still don't fit fold into a "»"
//! menu, the active one always staying in the strip.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, PointerButton, el, row, text};

use super::{META, accent, amber, green, icon_button, icons, pal, red, tnum};
use crate::app::{App, Conn, Panel};
use crate::tabs::{Tab, TabKind};

const TAB_H: f32 = 34.0;
const TAB_MAX_W: f32 = 200.0;
/// Narrowest a tab gets: icon, a few letters of title, its count and ×.
pub const TAB_MIN_W: f32 = 96.0;
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
    let right = row()
        .key("strip-right")
        .gap(4.0)
        .center_y()
        .on_layout(|s: &mut App, r: [f32; 4]| s.strip_right_w = r[2])
        .child(status)
        .child(el().w(8.0))
        .child(transfers)
        .child(settings);
    let mut strip = row()
        .key("strip")
        .h(TAB_H)
        .w_full()
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .fill(t.bg)
        .on_layout(|s: &mut App, r: [f32; 4]| {
            s.strip_w = r[2];
            s.window_w = r[2];
        })
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

fn tab_el(s: &App, i: usize, tab: &Tab, t: &Theme) -> El {
    let active = i == s.active;
    let (icon, count) = match &tab.kind {
        TabKind::Search(st) => (
            icons::search(icons::SM, t.text_secondary),
            st.search.as_ref().map(|v| v.visible(&st.filters)),
        ),
        TabKind::User(_) => (icons::user(icons::SM, t.text_secondary), None),
    };
    let mut label = row()
        .gap(6.0)
        .center_y()
        .min_w(0.0)
        .shrink()
        .child(icon)
        .child(
            text(tab.title())
                .key("title")
                .font(type_::LABEL)
                .color(if active { t.text } else { t.text_secondary })
                .truncate()
                .shrink(),
        );
    if let Some(n) = count {
        label = label.child(badge("count", n, t));
    }
    let close = icon_button("close", icons::close(icons::SM, t.text_secondary), t)
        .size(18.0, 18.0)
        .label("Close tab")
        .on_press_tx(move |s: &mut App, tx| {
            s.close_tab(i);
            s.restore_scroll(tx);
        });
    let mut el = row()
        .key(format!("tab-{i}"))
        .h(TAB_H)
        .max_w(TAB_MAX_W)
        .shrink()
        .min_w(TAB_MIN_W)
        .pl(10.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .role("tab")
        .label(tab.title())
        .checked(active)
        .pointer_buttons([PointerButton::Primary, PointerButton::Middle])
        .on_press_tx(move |s: &mut App, tx| {
            if tx.button() == PointerButton::Middle {
                s.close_tab(i);
            } else {
                s.activate(i);
            }
            s.restore_scroll(tx);
        })
        .child(label)
        .child(close);
    // Active: raised surface and an orange marker on top; others only hover.
    el = if active {
        el.fill(pal::raised()).child(
            rune::el()
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(2.0)
                .fill(accent()),
        )
    } else {
        el.hover(t.hover)
    };
    el.child(
        rune::el()
            .pin(None, Some(8.0), Some(0.0), Some(8.0))
            .w(1.0)
            .fill(t.separator),
    )
}
