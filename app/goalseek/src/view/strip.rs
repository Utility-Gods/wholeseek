//! The top strip: tabs (browser-like), ＋, then connection, ⇅ Transfers and ⚙ on the right.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, PointerButton, el, row, text};

use super::{META, accent, amber, green, icon_button, icons, pal, red, tnum};
use crate::app::{App, Conn, Panel};
use crate::tabs::{Tab, TabKind};

const TAB_H: f32 = 34.0;
const TAB_MAX_W: f32 = 200.0;

pub fn view(s: &App, t: &Theme) -> El {
    let mut tabs = row().key("tabs").h(TAB_H).min_w(0.0).shrink();
    for (i, tab) in s.tabs.iter().enumerate() {
        tabs = tabs.child(tab_el(s, i, tab, t));
    }
    let new = icon_button("new-tab", icons::plus(11.0, t.text_secondary), t)
        .label("New search tab")
        .help("New search (Ctrl+T)")
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
            12.0,
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
            13.0,
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
    row()
        .key("strip")
        .h(TAB_H)
        .w_full()
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .fill(t.bg)
        .child(tabs)
        .child(new)
        .child(el().grow(1.0))
        .child(status)
        .child(el().w(8.0))
        .child(transfers)
        .child(settings)
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
            icons::search(10.0, t.text_secondary),
            st.search.as_ref().map(|v| v.visible(&st.filters)),
        ),
        TabKind::User(_) => (icons::user(10.0, t.text_secondary), None),
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
    let close = icon_button("close", icons::close(8.0, t.text_secondary), t)
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
        .min_w(0.0)
        .shrink()
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
