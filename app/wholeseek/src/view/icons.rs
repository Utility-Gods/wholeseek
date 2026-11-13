//! The app's icons: a subset of Lucide (https://lucide.dev), vendored as SVG path data and
//! drawn by Rune as vectors (no image files, no icon font).
//!
//! Lucide is ISC licensed:
//!
//! > ISC License
//! >
//! > Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2026 as part of Feather
//! > (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2026.
//! >
//! > Permission to use, copy, modify, and/or distribute this software for any purpose with or
//! > without fee is hereby granted, provided that the above copyright notice and this
//! > permission notice appear in all copies.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO
//! > THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT
//! > SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR
//! > ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF
//! > CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE
//! > OR PERFORMANCE OF THIS SOFTWARE.
//!
//! Path data is Lucide 0.577.0's, unchanged; `line`, `polyline` and `polygon` elements are
//! written as the same geometry in `path` form, which Rune's icon renderer draws. Every icon
//! takes Lucide's root attributes (24 view box, round caps and joins) with the stroke at 1.75
//! instead of 2, which reads crisper at 12–16 pt.
//!
//! Sizes: [`SM`] in dense rows and tabs, [`MD`] in buttons and menus, [`LG`] in the toolbar
//! and player. Colour: muted (`text_secondary`) by default, the accent only for what is on.

use rune::{Color, El, Icon};

/// Dense rows, tabs, inline marks.
pub const SM: f32 = 12.0;
/// Buttons, menus, chips, fields.
pub const MD: f32 = 14.0;
/// Toolbar and player.
pub const LG: f32 = 16.0;

macro_rules! lucide {
    ($name:literal, $body:literal) => {
        Icon {
            name: concat!("lucide-", $name),
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        }
    };
}

/// [`lucide!`], filled as well as stroked: ▶ and ⏸ on buttons read as solid shapes.
macro_rules! lucide_solid {
    ($name:literal, $body:literal) => {
        Icon {
            name: concat!("lucide-", $name, "-solid"),
            view: 24.0,
            body: concat!(
                r#"<g fill="currentColor" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        }
    };
}

pub static PLAY: Icon = lucide!(
    "play",
    r#"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"#
);
pub static DOWNLOAD: Icon = lucide!(
    "download",
    r#"<path d="M12 15V3"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/>"#
);
pub static FOLDER_DOWN: Icon = lucide!(
    "folder-down",
    r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><path d="M12 10v6"/><path d="m15 13-3 3-3-3"/>"#
);
pub static FOLDER_OPEN: Icon = lucide!(
    "folder-open",
    r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#
);
pub static FOLDER: Icon = lucide!(
    "folder",
    r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
);
pub static USER: Icon = lucide!(
    "user",
    r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#
);
pub static COPY: Icon = lucide!(
    "copy",
    r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
);
pub static LIST_MUSIC: Icon = lucide!(
    "list-music",
    r#"<path d="M16 5H3"/><path d="M11 12H3"/><path d="M11 19H3"/><path d="M21 16V5"/><circle cx="18" cy="16" r="3"/>"#
);
pub static REFRESH_CW: Icon = lucide!(
    "refresh-cw",
    r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
);
pub static X: Icon = lucide!("x", r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#);
pub static TRASH_2: Icon = lucide!(
    "trash-2",
    r#"<path d="M10 11v6"/><path d="M14 11v6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#
);
pub static FUNNEL: Icon = lucide!(
    "funnel",
    r#"<path d="M10 20a1 1 0 0 0 .553.895l2 1A1 1 0 0 0 14 21v-7a2 2 0 0 1 .517-1.341L21.74 4.67A1 1 0 0 0 21 3H3a1 1 0 0 0-.742 1.67l7.225 7.989A2 2 0 0 1 10 14z"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static ARROW_DOWN_UP: Icon = lucide!(
    "arrow-down-up",
    r#"<path d="m3 16 4 4 4-4"/><path d="M7 20V4"/><path d="m21 8-4-4-4 4"/><path d="M17 4v16"/>"#
);
pub static SEARCH: Icon = lucide!(
    "search",
    r#"<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"#
);
pub static SETTINGS: Icon = lucide!(
    "settings",
    r#"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>"#
);
pub static CHEVRON_DOWN: Icon = lucide!("chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#);
pub static CHEVRON_RIGHT: Icon = lucide!("chevron-right", r#"<path d="m9 18 6-6-6-6"/>"#);
pub static CHECK: Icon = lucide!("check", r#"<path d="M20 6 9 17l-5-5"/>"#);
pub static CIRCLE_ALERT: Icon = lucide!(
    "circle-alert",
    r#"<circle cx="12" cy="12" r="10"/><path d="M12 8L12 12"/><path d="M12 16L12.01 16"/>"#
);
pub static CLOCK: Icon = lucide!(
    "clock",
    r#"<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>"#
);
pub static HARD_DRIVE: Icon = lucide!(
    "hard-drive",
    r#"<path d="M10 16h.01"/><path d="M2.212 11.577a2 2 0 0 0-.212.896V18a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-5.527a2 2 0 0 0-.212-.896L18.55 5.11A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/><path d="M21.946 12.013H2.054"/><path d="M6 16h.01"/>"#
);
pub static SHARE_2: Icon = lucide!(
    "share-2",
    r#"<circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="M8.59 13.51L15.42 17.49"/><path d="M15.41 6.51L8.59 10.49"/>"#
);
pub static LOG_OUT: Icon = lucide!(
    "log-out",
    r#"<path d="m16 17 5-5-5-5"/><path d="M21 12H9"/><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/>"#
);
pub static PLUS: Icon = lucide!("plus", r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#);
pub static ELLIPSIS: Icon = lucide!(
    "ellipsis",
    r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#
);
pub static VOLUME_2: Icon = lucide!(
    "volume-2",
    r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>"#
);
pub static EXTERNAL_LINK: Icon = lucide!(
    "external-link",
    r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#
);
pub static CIRCLE_CHECK: Icon = lucide!(
    "circle-check",
    r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#
);
pub static CIRCLE_X: Icon = lucide!(
    "circle-x",
    r#"<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>"#
);
pub static ARROW_DOWN_TO_LINE: Icon = lucide!(
    "arrow-down-to-line",
    r#"<path d="M12 17V3"/><path d="m6 11 6 6 6-6"/><path d="M19 21H5"/>"#
);
pub static GLOBE: Icon = lucide!(
    "globe",
    r#"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>"#
);
pub static UPLOAD: Icon = lucide!(
    "upload",
    r#"<path d="M12 3v12"/><path d="m17 8-5-5-5 5"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static PAUSE: Icon = lucide!(
    "pause",
    r#"<rect x="14" y="3" width="5" height="18" rx="1"/><rect x="5" y="3" width="5" height="18" rx="1"/>"#
);
pub static DISC_3: Icon = lucide!(
    "disc-3",
    r#"<circle cx="12" cy="12" r="10"/><path d="M6 12c0-1.7.7-3.2 1.8-4.2"/><circle cx="12" cy="12" r="2"/><path d="M18 12c0 1.7-.7 3.2-1.8 4.2"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static STAR: Icon = lucide!(
    "star",
    r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/>"#
);
pub static ARROW_UP_DOWN: Icon = lucide!(
    "arrow-up-down",
    r#"<path d="m21 16-4 4-4-4"/><path d="M17 20V4"/><path d="m3 8 4-4 4 4"/><path d="M7 4v16"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static UNDO_2: Icon = lucide!(
    "undo-2",
    r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static PENCIL: Icon = lucide!(
    "pencil",
    r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#
);
pub static FOLDER_PLUS: Icon = lucide!(
    "folder-plus",
    r#"<path d="M12 10v6"/><path d="M9 13h6"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
);
pub static LOG_IN: Icon = lucide!(
    "log-in",
    r#"<path d="m10 17 5-5-5-5"/><path d="M15 12H3"/><path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static USER_X: Icon = lucide!(
    "user-x",
    r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M17 8L22 13"/><path d="M22 8L17 13"/>"#
);
pub static CHEVRON_UP: Icon = lucide!("chevron-up", r#"<path d="m18 15-6-6-6 6"/>"#);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static HISTORY: Icon = lucide!(
    "history",
    r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static MUSIC: Icon = lucide!(
    "music",
    r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static INFO: Icon = lucide!(
    "info",
    r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#
);
#[allow(dead_code)] // vendored for the views; not drawn yet
pub static STAR_SOLID: Icon = lucide_solid!(
    "star",
    r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/>"#
);
pub static PLAY_SOLID: Icon = lucide_solid!(
    "play",
    r#"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"#
);
pub static PAUSE_SOLID: Icon = lucide_solid!(
    "pause",
    r#"<rect x="14" y="3" width="5" height="18" rx="1"/><rect x="5" y="3" width="5" height="18" rx="1"/>"#
);
pub static SKIP_BACK_SOLID: Icon = lucide_solid!(
    "skip-back",
    r#"<path d="M17.971 4.285A2 2 0 0 1 21 6v12a2 2 0 0 1-3.029 1.715l-9.997-5.998a2 2 0 0 1-.003-3.432z"/><path d="M3 20V4"/>"#
);
pub static SKIP_FORWARD_SOLID: Icon = lucide_solid!(
    "skip-forward",
    r#"<path d="M21 4v16"/><path d="M6.029 4.285A2 2 0 0 0 3 6v12a2 2 0 0 0 3.029 1.715l9.997-5.998a2 2 0 0 0 .003-3.432z"/>"#
);

// Context menus: open in a new tab, duplicate, expand/collapse all, close others/right.
pub static APP_WINDOW: Icon = lucide!(
    "app-window",
    r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/>"#
);
pub static USER_PLUS: Icon = lucide!(
    "user-plus",
    r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><line x1="19" x2="19" y1="8" y2="14"/><line x1="22" x2="16" y1="11" y2="11"/>"#
);
pub static COPY_PLUS: Icon = lucide!(
    "copy-plus",
    r#"<line x1="15" x2="15" y1="12" y2="18"/><line x1="12" x2="18" y1="15" y2="15"/><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
);
pub static CHEVRONS_UP_DOWN: Icon = lucide!(
    "chevrons-up-down",
    r#"<path d="m7 15 5 5 5-5"/><path d="m7 9 5-5 5 5"/>"#
);
pub static CHEVRONS_DOWN_UP: Icon = lucide!(
    "chevrons-down-up",
    r#"<path d="m7 20 5-5 5 5"/><path d="m7 4 5 5 5-5"/>"#
);
pub static SQUARE_X: Icon = lucide!(
    "square-x",
    r#"<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>"#
);
pub static ARROW_RIGHT_TO_LINE: Icon = lucide!(
    "arrow-right-to-line",
    r#"<path d="M17 12H3"/><path d="m11 18 6-6-6-6"/><path d="M21 5v14"/>"#
);

/// Every vendored icon (tests draw each one).
#[cfg(test)]
pub static ALL: &[&Icon] = &[
    &PLAY,
    &DOWNLOAD,
    &FOLDER_DOWN,
    &FOLDER_OPEN,
    &FOLDER,
    &USER,
    &COPY,
    &LIST_MUSIC,
    &REFRESH_CW,
    &X,
    &TRASH_2,
    &FUNNEL,
    &ARROW_DOWN_UP,
    &SEARCH,
    &SETTINGS,
    &CHEVRON_DOWN,
    &CHEVRON_RIGHT,
    &CHECK,
    &CIRCLE_ALERT,
    &CLOCK,
    &HARD_DRIVE,
    &SHARE_2,
    &LOG_OUT,
    &PLUS,
    &ELLIPSIS,
    &VOLUME_2,
    &EXTERNAL_LINK,
    &CIRCLE_CHECK,
    &CIRCLE_X,
    &ARROW_DOWN_TO_LINE,
    &GLOBE,
    &UPLOAD,
    &PAUSE,
    &DISC_3,
    &STAR,
    &ARROW_UP_DOWN,
    &UNDO_2,
    &PENCIL,
    &FOLDER_PLUS,
    &LOG_IN,
    &USER_X,
    &CHEVRON_UP,
    &HISTORY,
    &MUSIC,
    &INFO,
    &STAR_SOLID,
    &PLAY_SOLID,
    &PAUSE_SOLID,
    &SKIP_BACK_SOLID,
    &SKIP_FORWARD_SOLID,
    &APP_WINDOW,
    &USER_PLUS,
    &COPY_PLUS,
    &CHEVRONS_UP_DOWN,
    &CHEVRONS_DOWN_UP,
    &SQUARE_X,
    &ARROW_RIGHT_TO_LINE,
];

/// `i` at `size`, tinted `c`.
pub fn draw(i: &'static Icon, size: f32, c: Color) -> El {
    rune::icon(i, size).color(c)
}

pub fn play(size: f32, c: Color) -> El {
    draw(&PLAY_SOLID, size, c)
}

pub fn pause(size: f32, c: Color) -> El {
    draw(&PAUSE_SOLID, size, c)
}

pub fn down(size: f32, c: Color) -> El {
    draw(&DOWNLOAD, size, c)
}

pub fn transfers(size: f32, c: Color) -> El {
    draw(&ARROW_UP_DOWN, size, c)
}

pub fn gear(size: f32, c: Color) -> El {
    draw(&SETTINGS, size, c)
}

pub fn speaker(size: f32, c: Color) -> El {
    draw(&VOLUME_2, size, c)
}

pub fn close(size: f32, c: Color) -> El {
    draw(&X, size, c)
}

pub fn filter(size: f32, c: Color) -> El {
    draw(&FUNNEL, size, c)
}

pub fn check(size: f32, c: Color) -> El {
    draw(&CHECK, size, c)
}

pub fn retry(size: f32, c: Color) -> El {
    draw(&REFRESH_CW, size, c)
}

pub fn chevron(size: f32, open: bool, c: Color) -> El {
    draw(if open { &CHEVRON_DOWN } else { &CHEVRON_RIGHT }, size, c)
}

pub fn folder(size: f32, c: Color) -> El {
    draw(&FOLDER, size, c)
}

pub fn plus(size: f32, c: Color) -> El {
    draw(&PLUS, size, c)
}

pub fn search(size: f32, c: Color) -> El {
    draw(&SEARCH, size, c)
}

pub fn user(size: f32, c: Color) -> El {
    draw(&USER, size, c)
}

pub fn more(size: f32, c: Color) -> El {
    draw(&ELLIPSIS, size, c)
}

pub fn skip(size: f32, back: bool, c: Color) -> El {
    draw(
        if back {
            &SKIP_BACK_SOLID
        } else {
            &SKIP_FORWARD_SOLID
        },
        size,
        c,
    )
}

pub fn open(size: f32, c: Color) -> El {
    draw(&EXTERNAL_LINK, size, c)
}

pub fn copy(size: f32, c: Color) -> El {
    draw(&COPY, size, c)
}
