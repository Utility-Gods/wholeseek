//! Views: pure functions of [`App`]; handlers call `App` methods.
//!
//! Look: flat and dense on Turable's navy palette, one orange accent (active tab, playing
//! track, primary buttons, focus). Motion: none on entry or exit — the whole window lands
//! changes at once (`instant_changes`); only hover/press colours move, in 60 ms.

pub(crate) mod album_header;
mod albums;
pub(crate) mod art;
mod details;
mod files;
pub mod icons;
mod menu;
mod onboarding;
mod panels;
mod player;
mod profile;
mod search;
mod strip;
pub(crate) mod update;
mod user;
pub(crate) mod virtual_list;
#[cfg(test)]
pub use details::{INSPECTOR_W, RESULTS_MAX_W};
pub use search::sort_items;

/// The search batch still in progress (sort menu, album view): off until it is finished, so
/// a release cut meanwhile shows none of it.
pub const SEARCH_BATCH: bool = true;
pub use files::more_key as files_more_key;
pub use search::MAX_FORMAT_CHIPS;
#[cfg(test)]
pub use strip::{TAB_MIN_W, tab_split as strip_tab_split};

use rune::tok::{TextStyle, Theme, type_};
use rune::{Color, Cursor, Curve, Cx, El, TextEdit, col, el, row, text};

use crate::app::{App, Conn, Panel, Screen};
use crate::tabs::TabKind;

/// Hover/press colour change, in and out.
const HOVER_SECS: f32 = 0.06;
pub const DRAWER_W: f32 = 380.0;

fn hex(rgb: u32) -> Color {
    let c = |s: u32| rune::theme::srgb_decode(((rgb >> s) & 0xff) as f32 / 255.0);
    Color {
        r: c(16),
        g: c(8),
        b: c(0),
        a: 1.0,
    }
}

/// Turable's dark palette.
pub mod pal {
    use super::hex;
    use rune::Color;
    pub fn bg() -> Color {
        hex(0x0a0e1a)
    }
    /// The tab strip's chrome behind inactive tabs, a step darker than [`bg`].
    pub fn chrome() -> Color {
        hex(0x070a13)
    }
    pub fn raised() -> Color {
        hex(0x0e1424)
    }
    pub fn hover() -> Color {
        hex(0x182033)
    }
    pub fn line() -> Color {
        hex(0x182033)
    }
    pub fn line_strong() -> Color {
        hex(0x2a3447)
    }
    pub fn text() -> Color {
        hex(0xe2e8f0)
    }
    pub fn muted() -> Color {
        hex(0x94a3b8)
    }
    pub fn accent() -> Color {
        hex(0xfb923c)
    }
}

pub fn theme() -> Theme {
    let mut t = rune::tok::theme(true);
    t.bg = pal::bg();
    t.card = pal::raised();
    t.card_border = pal::line_strong();
    t.text = pal::text();
    t.text_secondary = pal::muted();
    t.separator = pal::line();
    t.control_off = pal::hover();
    t.control_track = pal::line_strong();
    t.indicator = hex(0x475569);
    t.hover = pal::hover();
    t.press = pal::line_strong();
    t
}

pub fn view(s: &App, _: &mut Cx<App>) -> El {
    let t = theme();
    let page = match s.screen {
        Screen::Starting => starting(&t),
        Screen::Onboarding => onboarding::view(s, &t),
        Screen::Main => main(s, &t),
    };
    page.instant_changes()
        .hover_motion_within(Curve::linear(HOVER_SECS), Curve::linear(HOVER_SECS))
}

/// Before the saved account is read (off the UI thread).
fn starting(t: &Theme) -> El {
    col()
        .key("starting")
        .w_full()
        .h_full()
        .center()
        .fill(t.bg)
        .child(
            text("Signing in…")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
}

fn main(s: &App, t: &Theme) -> El {
    let content = match &s.tab().kind {
        TabKind::Search(st) => search::page(s, s.tab(), st, t),
        TabKind::User(u) => user::page(s, s.tab(), &u.browse, t),
    };
    let mut body = col()
        .key("body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(content);
    // Drawers lie over the page (it keeps its layout under them).
    let drawer = match s.panel {
        Panel::None => None,
        Panel::Transfers => Some(panels::transfers(s, t)),
        Panel::Settings => Some(panels::settings(s, t)),
    };
    if let Some(d) = drawer {
        body = body.child(
            row()
                .pin(None, Some(0.0), Some(0.0), Some(0.0))
                .w(DRAWER_W + 1.0)
                .child(vline(t))
                .child(d),
        );
    }
    let mut page = col()
        .w_full()
        .h_full()
        .fill(t.bg)
        // The strip draws its own bottom line, open under the active tab.
        .child(strip::view(s, t));
    if let Some(b) = banner(s, t) {
        page = page.child(b);
    }
    page = page
        .child(body)
        .child(hairline(t))
        .child(player::strip(s, t));
    if let Some(r) = search::recent_layer(s, t) {
        page = page.child(r);
    }
    // Last at the root: a context menu draws over everything and anchors to earlier keys.
    match menu::layer(s, t) {
        Some(m) => page.child(m),
        None => page,
    }
}

/// One line under the tab strip: signed in elsewhere (with Reconnect), else the last error.
fn banner(s: &App, t: &Theme) -> Option<El> {
    let line = |key: &str, msg: String, tint: Color| {
        row()
            .key(key.to_string())
            .h(32.0)
            .w_full()
            .px(12.0)
            .gap(10.0)
            .center_y()
            .fill(tint.alpha(0.10))
            .child(
                text(msg)
                    .font(type_::BODY)
                    .color(t.text)
                    .truncate()
                    .grow(1.0)
                    .shrink(),
            )
    };
    if let Some(r) = &s.status.reconnect {
        return Some(
            line(
                "reconnecting",
                format!("Reconnecting in {} s", r.in_secs()),
                amber(),
            )
            .help(format!("Attempt {}", r.attempt)),
        );
    }
    if s.conn == Conn::Displaced {
        return Some(
            line("displaced", "Signed in elsewhere".into(), pal::muted()).child(
                button_icon("reconnect", &icons::REFRESH_CW, "Reconnect", false, t)
                    .h(24.0)
                    .on_press(|s: &mut App| s.reconnect()),
            ),
        );
    }
    let e = s.error.as_ref()?;
    Some(
        line("error", e.clone(), red()).child(
            icon_button("dismiss", icons::close(icons::SM, t.text_secondary), t)
                .size(22.0, 22.0)
                .label("Dismiss")
                .on_press(|s: &mut App| s.error = None),
        ),
    )
}

pub fn accent() -> Color {
    pal::accent()
}
pub fn green() -> Color {
    hex(0x4ade80)
}
pub fn amber() -> Color {
    hex(0xfbbf24)
}
pub fn red() -> Color {
    hex(0xf87171)
}

/// 11 px meta text.
pub const META: TextStyle = TextStyle::new(11.0, 450.0);

pub fn tnum(style: TextStyle) -> TextStyle {
    TextStyle {
        features: "tnum",
        ..style
    }
}

pub fn hairline(t: &Theme) -> El {
    el().w_full().h(1.0).fill(t.separator)
}

pub fn vline(t: &Theme) -> El {
    el().w(1.0).h_full().fill(t.separator)
}

/// A small square button holding an icon.
pub fn icon_button(key: &str, icon: El, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .child(icon)
}

/// A button: an icon, then its label; `primary` fills it with the accent.
pub fn button_icon(
    key: &str,
    icon: &'static rune::Icon,
    label: &str,
    primary: bool,
    t: &Theme,
) -> El {
    let ink = if primary { pal::bg() } else { t.text };
    let b = row()
        .key(key.to_string())
        .h(28.0)
        .px(12.0)
        .gap(6.0)
        .center()
        .radius(4.0)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label.to_string())
        .child(icons::draw(icon, icons::MD, ink))
        .child(text(label.to_string()).font(type_::LABEL).color(ink));
    if primary {
        b.fill(accent()).hover(hex(0xfdba74)).press(hex(0xf97316))
    } else {
        b.fill(t.hover)
            .hover(t.control_track)
            .press(t.control_track)
    }
}

/// An on/off switch (flat, no motion): the knob sits right and the track is the accent when on.
pub fn switch(key: &str, on: bool, label: &str, t: &Theme) -> El {
    let knob =
        el().size(12.0, 12.0)
            .radius(6.0)
            .fill(if on { pal::bg() } else { t.text_secondary });
    let mut track = row()
        .key(key.to_string())
        .w(28.0)
        .h(16.0)
        .px(2.0)
        .radius(8.0)
        .center_y()
        .fill(if on { accent() } else { t.control_track })
        .cursor(Cursor::Pointer)
        .role("switch")
        .checked(on)
        .label(label.to_string())
        .focusable(true)
        .focus_ring(1.0, 1.0, accent().alpha(0.6));
    if on {
        track = track.child(el().grow(1.0));
    }
    track.child(knob)
}

/// A boxed single-line text field. `input` is the `text_input(..).on_edit(..)` element.
pub fn field_box(key: &str, input: El, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(28.0)
        .px(8.0)
        .center_y()
        .radius(4.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .child(input.grow(1.0))
}

pub fn input(edit: &TextEdit, placeholder: &str, t: &Theme) -> El {
    rune::text_input(edit, type_::BODY)
        .single_line()
        .placeholder(placeholder.to_string(), t.text_secondary.alpha(0.7))
        .caret(accent(), accent().alpha(0.3))
        .color(t.text)
}

pub fn section(label: &str, t: &Theme) -> El {
    text(label.to_string())
        .font(type_::SECTION)
        .color(t.text_secondary)
}

/// A thin progress bar `w` wide.
pub fn progress(key: &str, fraction: f32, w: f32, color: Color, t: &Theme) -> El {
    el().key(key.to_string())
        .w(w)
        .h(3.0)
        .fill(t.control_track)
        .child(
            el().absolute(0.0, 0.0)
                .w_frac(fraction.clamp(0.0, 1.0))
                .h(3.0)
                .fill(color),
        )
}
