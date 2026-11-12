//! The profile header: picture, name + online dot, one stats line, description. Self-contained
//! (username + its `ProfileEntry`) so a user tab can put it on top as is.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, SurfaceFit, col, el, row, text};

use super::green;
use crate::app::App;
use crate::profile::{ProfileEntry, ProfileStatus, stats_line};

/// Above this many characters (or three line breaks) the description gets "more".
const CLAMP_CHARS: usize = 300;
const PICTURE: f32 = 56.0;

/// `None` when there is nothing to show: no request, or it failed (the folders still work).
pub fn header(username: &str, entry: Option<&ProfileEntry>, t: &Theme) -> Option<El> {
    let entry = entry?;
    let quiet = |s: String| text(s).font(type_::LABEL).color(t.text_secondary);
    match &entry.status {
        ProfileStatus::Failed(_) => return None,
        ProfileStatus::Loading => {
            return Some(
                row()
                    .key("profile-loading")
                    .w_full()
                    .h(28.0)
                    .px(16.0)
                    .center_y()
                    .child(quiet("loading profile…".into()).color(t.text_secondary.alpha(0.7))),
            );
        }
        ProfileStatus::Known => {}
    }
    let p = &entry.profile;

    let mut name = row().gap(7.0).center_y().child(
        text(username.to_string())
            .key("profile-name")
            .font(type_::TITLE)
            .color(t.text),
    );
    if let Some(online) = p.online {
        let (color, label) = if online {
            (green(), "online")
        } else {
            (t.text_secondary.alpha(0.6), "offline")
        };
        name = name.child(
            el().key("profile-online")
                .size(7.0, 7.0)
                .radius(3.5)
                .fill(color)
                .label(label),
        );
        if !online {
            name = name.child(quiet("offline".into()));
        }
    }

    let mut info = col().gap(3.0).grow(1.0).min_w(0.0).shrink().child(name);
    let stats = stats_line(p);
    if !stats.is_empty() {
        info = info.child(quiet(stats).key("profile-stats").truncate());
    }
    if let Some(d) = p
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        let long = d.chars().count() > CLAMP_CHARS || d.matches('\n').count() >= 3;
        let body = text(d.to_string())
            .key("profile-desc")
            .font(type_::BODY)
            .color(t.text_secondary)
            .wrap();
        info = info.child(if long && !entry.expanded {
            body.max_lines(3)
        } else {
            body
        });
        if long {
            let user = username.to_string();
            info = info.child(
                row()
                    .key("profile-more")
                    .cursor(Cursor::Pointer)
                    .role("button")
                    .label(if entry.expanded {
                        "Show less"
                    } else {
                        "Show more"
                    })
                    .on_press(move |s: &mut App| s.profiles.toggle_expanded(&user))
                    .gap(4.0)
                    .center_y()
                    .child(super::icons::draw(
                        if entry.expanded {
                            &super::icons::CHEVRON_UP
                        } else {
                            &super::icons::CHEVRON_DOWN
                        },
                        super::icons::SM,
                        t.text_secondary,
                    ))
                    .child(
                        text(if entry.expanded { "less" } else { "more" })
                            .font(type_::LABEL)
                            .color(t.text),
                    ),
            );
        }
    }

    let mut h = row().key("profile").w_full().px(16.0).py(10.0).gap(12.0);
    if let Some(img) = entry.shown_picture() {
        h = h.child(
            el().key("profile-picture")
                .size(PICTURE, PICTURE)
                .radius(6.0)
                .fill(t.card)
                .pixels_fit(img.pixels(), SurfaceFit::Fill)
                .label(format!("{username}'s picture")),
        );
    }
    Some(h.child(info))
}
