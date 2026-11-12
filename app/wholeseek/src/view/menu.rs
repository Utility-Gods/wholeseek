//! The context menu layer: a click-away surface over the window and the menu itself, under
//! its ⋯ button or at the right-click point. Escape closes it.

use rune::tok::{Theme, type_};
use rune::{Align, Cursor, El, Key, Side, col, el, row, text};

use super::pal;
use crate::app::App;
use crate::menu::MenuAnchor;

/// Menus grow with their longest label ("Download to › ~/Music/Vinyl") up to [`MENU_MAX_W`].
const MENU_W: f32 = 200.0;
const MENU_MAX_W: f32 = 360.0;

pub fn layer(s: &App, t: &Theme) -> Option<El> {
    let m = s.menu.as_ref()?;
    let (anchor_key, point) = match &m.anchor {
        MenuAnchor::Key(k) => (k.clone(), None),
        MenuAnchor::Point(x, y) => ("menu-at".to_string(), Some((*x, *y))),
    };
    let mut items = col()
        .key("menu")
        .min_w(MENU_W)
        .max_w(MENU_MAX_W)
        .py(4.0)
        .radius(6.0)
        .fill(pal::raised())
        .border(1.0, pal::line_strong())
        .role("menu")
        .on_key(|s: &mut App, e, _| {
            if e.key == Key::Escape {
                s.menu = None;
                return true;
            }
            false
        })
        .anchored(anchor_key, Side::Bottom, Align::Start, 2.0);
    for it in &m.items {
        let action = it.action.clone();
        let glyph = match it.icon {
            Some(i) => super::icons::draw(i, super::icons::MD, t.text_secondary),
            None => el().size(super::icons::MD, super::icons::MD),
        };
        if it.sep {
            items = items.child(el().h(1.0).w_full().my(4.0).fill(pal::line()));
        }
        let mut entry = row()
            .key(match it.action {
                crate::menu::MenuAction::ShowTab(i) => format!("menu-tab-{i}"),
                _ => format!("menu-{}", it.id),
            })
            .h(28.0)
            .w_full()
            .px(10.0)
            .gap(10.0)
            .center_y()
            .hover(t.hover)
            .press(t.press)
            .cursor(Cursor::Pointer)
            .role("menuitem")
            .label(it.help.clone().unwrap_or_else(|| it.label.clone()))
            .on_press_tx(move |s: &mut App, tx| {
                if let crate::menu::MenuAction::DownloadTo { target, dir: None } = &action {
                    s.menu = None;
                    crate::actions::choose_download_dir(s, tx, target.clone());
                    return;
                }
                if let Some(text) = s.run_menu(action.clone()) {
                    tx.copy(text);
                }
                s.restore_scroll(tx);
            })
            .child(glyph)
            .child(
                text(it.label.clone())
                    .font(type_::BODY)
                    .color(t.text)
                    .truncate()
                    .shrink()
                    .min_w(0.0),
            );
        if let Some(h) = &it.help {
            entry = entry.help(h.clone());
        }
        items = items.child(entry);
    }
    let mut layer = el()
        .key("menu-layer")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|s: &mut App| s.menu = None)
        .on_secondary_press(|s: &mut App, _, _| s.menu = None);
    if let Some((x, y)) = point {
        layer = layer.child(el().key("menu-at").absolute(x, y).size(1.0, 1.0));
    }
    Some(layer.child(items))
}

/// Opens `items` at a right-click (or Menu key / Shift+F10) point.
pub fn at_point(s: &mut App, p: rune::PointerPress, items: Vec<crate::menu::MenuItem>) {
    s.open_menu(MenuAnchor::Point(p.x, p.y), items);
}
