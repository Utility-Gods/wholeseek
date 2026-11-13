//! The update notice in the top strip ("⊕ Update 0.1.4 ×") and the version line in
//! Settings. Logic: `crate::update`.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, Icon, row, text};

use super::{META, accent, icon_button, icons};
use crate::app::App;
use crate::update::CURRENT;

/// Lucide 0.577.0 `circle-arrow-up` (ISC; see `icons.rs`).
pub static CIRCLE_ARROW_UP: Icon = Icon {
    name: "lucide-circle-arrow-up",
    view: 24.0,
    body: concat!(
        r#"<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
        r#"<circle cx="12" cy="12" r="10"/><path d="m16 12-4-4-4 4"/><path d="M12 16V8"/>"#,
        "</g>"
    ),
};

/// The strip's notice when a newer release is out (and not dismissed); `None` otherwise.
/// In `strip.rs`'s right cluster, before Transfers.
pub fn pill(s: &App, t: &Theme) -> Option<El> {
    let v = s.update_offer()?;
    let tip = format!("wholeseek {v} is available (you have {CURRENT})");
    let open = row()
        .key("update-open")
        .h(24.0)
        .pl(8.0)
        .pr(4.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(t.hover)
        .press(t.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(tip.clone())
        .help(tip)
        .on_press(|s: &mut App| s.open_update_site())
        .child(icons::draw(&CIRCLE_ARROW_UP, icons::MD, accent()))
        .child(text(format!("Update {v}")).font(type_::LABEL).color(t.text));
    let dismiss = icon_button(
        "update-dismiss",
        icons::close(icons::SM, t.text_secondary),
        t,
    )
    .size(18.0, 18.0)
    .label("Dismiss update notice")
    .help("Hide until the next version")
    .on_press(|s: &mut App| s.dismiss_update());
    Some(
        row()
            .key("update")
            .gap(2.0)
            .center_y()
            .child(open)
            .child(dismiss),
    )
}

/// "wholeseek 0.1.3", muted, at the bottom of Settings.
pub fn version(t: &Theme) -> El {
    row().px(14.0).pt(14.0).child(
        text(format!("wholeseek {CURRENT}"))
            .key("app-version")
            .font(META)
            .color(t.text_secondary),
    )
}
