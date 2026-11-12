//! The album header shared by the search inspector and a user tab's folder view: title (full
//! path in its tooltip); online dot, user, speed and slot; "60 files · 360:27 · 2.76 GB ·
//! FLAC"; then the folder actions (Play album / Download folder / Download to… / Browse user).

use std::collections::BTreeSet;

use rune::tok::{Theme, type_};
use rune::{Cursor, El, col, el, row, text};
use wholeseek_core::RemoteFile;

use super::files::action;
use super::{META, amber, green, icons, tnum};
use crate::app::App;
use crate::fmt;
use crate::menu::MenuAction;

/// What is known about the folder's owner. `None` fields are left out of the line.
#[derive(Debug, Clone, Copy, Default)]
pub struct UserStatus {
    pub free_slot: Option<bool>,
    /// B/s.
    pub speed: Option<u32>,
}

/// Which folder actions the header offers. Play album shows only for a playable folder.
#[derive(Debug, Clone, Copy)]
pub struct Actions {
    pub play: bool,
    pub download_to: bool,
    pub browse_user: bool,
}

pub struct AlbumHeader<'a> {
    /// Element keys are `{prefix}title`, `{prefix}user`, `{prefix}summary`,
    /// `{prefix}actions`, `{prefix}play`, `{prefix}download`, `{prefix}download-to`,
    /// `{prefix}browse`.
    pub prefix: &'a str,
    pub username: &'a str,
    /// Remote folder path (the title is its last part; the tooltip is the whole path).
    pub folder: &'a str,
    pub files: &'a [RemoteFile],
    pub status: UserStatus,
    pub actions: Actions,
    /// Left padding: lines the text up with whatever sits below the header.
    pub inset: f32,
}

/// Left padding of the header in the search inspector.
pub const INSET: f32 = 16.0;

/// "60 files · 360:27 · 2.76 GB · FLAC".
pub fn summary(files: &[RemoteFile]) -> String {
    let total: u64 = files.iter().map(|f| f.size).sum();
    let secs: u64 = files
        .iter()
        .filter_map(|f| f.attrs.duration_secs)
        .map(u64::from)
        .sum();
    let formats: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| crate::tabs::format_of(&f.path))
        .collect();
    let mut parts = vec![format!(
        "{} {}",
        files.len(),
        if files.len() == 1 { "file" } else { "files" }
    )];
    if secs > 0 {
        parts.push(fmt::mmss(secs as f64));
    }
    parts.push(fmt::size(total));
    if !formats.is_empty() {
        parts.push(formats.into_iter().collect::<Vec<_>>().join(", "));
    }
    parts.join(" · ")
}

pub fn view(h: &AlbumHeader, t: &Theme) -> El {
    let p = h.prefix;
    let k = |s: &str| format!("{p}{s}");
    let name = h.folder.rsplit('\\').next().unwrap_or(h.folder).to_string();

    let dot = match h.status.free_slot {
        Some(true) => green(),
        Some(false) => amber(),
        None => t.indicator,
    };
    let mut meta = Vec::new();
    if let Some(sp) = h.status.speed {
        meta.push(fmt::speed(f64::from(sp)));
    }
    if let Some(free) = h.status.free_slot {
        meta.push(if free { "free slot" } else { "queued" }.to_string());
    }
    let user = h.username.to_string();
    let mut who = row()
        .key(k("user"))
        .gap(8.0)
        .center_y()
        .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
        .child(
            text(user.clone())
                .font(type_::LABEL)
                .color(t.text)
                .cursor(Cursor::Pointer)
                .role("link")
                .help("Open this user in a tab")
                .on_press(move |s: &mut App| s.open_user(&user)),
        );
    if !meta.is_empty() {
        who = who.child(
            text(meta.join(" · "))
                .font(tnum(META))
                .color(t.text_secondary),
        );
    }

    let run = |a: MenuAction| {
        move |s: &mut App| {
            s.run_menu(a.clone());
        }
    };
    let (u, d) = (h.username.to_string(), h.folder.to_string());
    let mut actions = row().key(k("actions")).gap(6.0).center_y();
    if h.actions.play && h.files.iter().any(crate::tabs::is_playable) {
        actions = actions.child(
            action(
                &k("play"),
                "Play album",
                icons::draw(&icons::LIST_MUSIC, icons::MD, t.text),
                t,
            )
            .on_press(run(MenuAction::PlayFolder {
                username: u.clone(),
                folder: d.clone(),
            })),
        );
    }
    actions = actions.child(
        action(
            &k("download"),
            "Download folder",
            icons::down(icons::MD, t.text),
            t,
        )
        .on_press(run(MenuAction::DownloadFolder {
            username: u.clone(),
            folder: d.clone(),
        })),
    );
    if h.actions.download_to {
        let target = crate::menu::DlTarget::Folder {
            username: u.clone(),
            folder: d.clone(),
        };
        let anchor = k("download-to");
        actions = actions.child(
            action(
                &anchor,
                "Download to…",
                icons::draw(&icons::FOLDER_DOWN, icons::MD, t.text),
                t,
            )
            .help("Save this folder somewhere else")
            .on_press_tx(move |s: &mut App, tx| {
                let recent = s.backend.settings().recent_download_dirs.clone();
                if recent.is_empty() {
                    crate::actions::choose_download_dir(s, tx, target.clone());
                } else {
                    let items = crate::menu::download_to_items(&target, &recent);
                    s.open_menu(crate::menu::MenuAnchor::Key(anchor.clone()), items);
                }
            }),
        );
    }
    if h.actions.browse_user {
        actions = actions.child(
            action(
                &k("browse"),
                "Browse user",
                icons::user(icons::MD, t.text),
                t,
            )
            .help(format!("{u}'s shares, at this folder"))
            .on_press(run(MenuAction::OpenFolder {
                username: u.clone(),
                folder: d.clone(),
                file: None,
            })),
        );
    }

    col()
        .key(k("header"))
        .w_full()
        .pl(h.inset)
        .pr(INSET)
        .pt(14.0)
        .pb(12.0)
        .gap(8.0)
        .child(
            text(name)
                .key(k("title"))
                .font(type_::TITLE)
                .color(t.text)
                .truncate()
                .help(fmt::crumbs(h.folder)),
        )
        .child(who)
        .child(
            text(summary(h.files))
                .key(k("summary"))
                .font(tnum(META))
                .color(t.text_secondary),
        )
        .child(actions)
}
