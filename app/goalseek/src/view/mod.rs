//! Views: pure functions of [`App`]; handlers call `App` methods.

mod icons;
mod main_view;
mod onboarding;
mod panels;
mod player;
mod profile;

use rune::tok::{self, TextStyle, Theme, type_};
use rune::{Color, Cursor, Cx, El, TextEdit, col, el, oklch, row, text};

use crate::app::{App, Panel, Screen};

pub fn view(s: &App, _: &mut Cx<App>) -> El {
    let t = tok::theme(true);
    match s.screen {
        Screen::Onboarding => onboarding::view(s, &t),
        Screen::Main => {
            let mut body = row()
                .key("body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(main_view::content(s, &t));
            match s.panel {
                Panel::None => {}
                Panel::Transfers => body = body.child(vline(&t)).child(panels::transfers(s, &t)),
                Panel::Settings => body = body.child(vline(&t)).child(panels::settings(s, &t)),
            }
            col()
                .w_full()
                .h_full()
                .fill(t.bg)
                .child(main_view::top_bar(s, &t))
                .child(hairline(&t))
                .child(body)
                .child(hairline(&t))
                .child(player::strip(s, &t))
        }
    }
}

pub fn accent() -> Color {
    tok::accent(0, true)
}
pub fn green() -> Color {
    oklch(0.72, 0.14, 150.0)
}
pub fn amber() -> Color {
    oklch(0.8, 0.13, 75.0)
}
pub fn red() -> Color {
    oklch(0.7, 0.16, 25.0)
}

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
        .size(28.0, 28.0)
        .center()
        .radius(6.0)
        .hover(t.control_off)
        .press(t.control_track)
        .cursor(Cursor::Pointer)
        .role("button")
        .child(icon)
}

/// A text button; `primary` fills it with the accent.
pub fn button(key: &str, label: &str, primary: bool, t: &Theme) -> El {
    let b = row()
        .key(key.to_string())
        .h(30.0)
        .px(14.0)
        .center()
        .radius(7.0)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label.to_string())
        .child(
            text(label.to_string())
                .font(type_::LABEL)
                .color(if primary {
                    oklch(0.99, 0.0, 0.0)
                } else {
                    t.text
                }),
        );
    if primary {
        b.fill(accent()).hover(oklch(1.0, 0.0, 0.0).alpha(0.06))
    } else {
        b.fill(t.control_off)
            .hover(t.control_off)
            .press(t.control_track)
    }
}

/// A boxed single-line text field. `input` is the `text_input(..).on_edit(..)` element.
pub fn field_box(key: &str, input: El, t: &Theme) -> El {
    row()
        .key(key.to_string())
        .h(32.0)
        .px(10.0)
        .center_y()
        .radius(7.0)
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
        .h(4.0)
        .radius(2.0)
        .fill(t.control_off)
        .child(
            el().absolute(0.0, 0.0)
                .w_frac(fraction.clamp(0.0, 1.0))
                .h(4.0)
                .radius(2.0)
                .fill(color),
        )
}
