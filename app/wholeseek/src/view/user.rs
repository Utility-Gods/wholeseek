//! A user tab: profile header, then a fixed folder tree (with a filter) on the left and the
//! selected folder's files on the right.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollPos, ScrollSpec, col, el, row, text};

use std::rc::Rc;

use wholeseek_core::RemoteFile;

use super::album_header::{self, Actions, AlbumHeader, UserStatus};
use super::files::{RowCtx, columns, file_row, more_key};
use super::{field_box, hairline, icons, input, red, vline};
use crate::app::App;
use crate::tabs::{BrowseView, Tab};

const TREE_W: f32 = 240.0;
/// Left edge of the file names in [`file_row`] (12 pt padding, 24 pt ▶, 10 pt gap): the
/// folder header's text starts there.
const NAME_X: f32 = 46.0;

/// The owner's slot and speed: from a search result when there is one (fresher), else from
/// the profile.
fn user_status(s: &App, username: &str) -> UserStatus {
    let hit = s
        .tabs
        .iter()
        .filter_map(|t| t.search()?.search.as_ref())
        .flat_map(|v| v.hits.iter())
        .find(|h| h.file.username == username);
    if let Some(h) = hit {
        return UserStatus {
            free_slot: Some(h.free_slot),
            speed: Some(h.speed),
        };
    }
    let p = s.profiles.get(username).map(|e| &e.profile);
    UserStatus {
        free_slot: p.and_then(|p| p.free_slot),
        speed: p.and_then(|p| p.avg_speed),
    }
}

pub fn page(s: &App, tab: &Tab, b: &BrowseView, t: &Theme) -> El {
    let mut page = col().w_full().grow(1.0).min_h(0.0);
    match super::profile::header(&b.username, s.profiles.get(&b.username), t) {
        Some(h) => page = page.child(h).child(hairline(t)),
        // No profile to show: still name the tab's user above the folders.
        None => {
            page = page
                .child(
                    row()
                        .key("user-head")
                        .h(36.0)
                        .px(12.0)
                        .center_y()
                        .child(text(b.username.clone()).font(type_::TITLE).color(t.text)),
                )
                .child(hairline(t));
        }
    }
    let body: El = if let Some(e) = &b.error {
        col().grow(1.0).center().child(
            text("Couldn't browse this user")
                .key("browse-error")
                .help(e.clone())
                .font(type_::BODY)
                .color(red()),
        )
    } else if b.dirs.is_none() {
        col().grow(1.0).center().child(
            text(if b.selected.is_some() {
                "Opening folder…"
            } else {
                "Loading shares…"
            })
            .key("browse-loading")
            .font(type_::BODY)
            .color(t.text_secondary),
        )
    } else {
        row()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(tree(s, tab, b, t))
            .child(vline(t))
            .child(folder(s, tab, b, t))
    };
    page.child(body)
}

fn tree(s: &App, tab: &Tab, b: &BrowseView, t: &Theme) -> El {
    let filter = field_box(
        "tree-filter",
        input(&b.filter, "Filter folders", t)
            .key("tree-find")
            .on_edit(|s: &mut App| &mut s.active_browse_mut().expect("user tab is shown").filter)
            .on_input_change(|s: &mut App, _| {
                // Cleared: back to the whole tree, at the selected folder.
                if let Some(b) = s.active_browse_mut()
                    && b.filter.text.trim().is_empty()
                {
                    b.reveal_tree = true;
                }
            }),
        t,
    )
    .w_full();
    let mut list = col().w_full().py(crate::tabs::TREE_PAD);
    for (i, (path, depth, has_children, open)) in b.visible_tree().into_iter().enumerate() {
        let selected = b.selected.as_deref() == Some(path.as_str());
        let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
        let chevron: El = if has_children {
            icons::chevron(icons::SM, open, t.text_secondary)
        } else {
            el().size(9.0, 9.0)
        };
        let p = path.clone();
        let mut r = row()
            .key(format!("dir-{i}"))
            .h(crate::tabs::TREE_ROW_H)
            .w_full()
            .pl(8.0 + 12.0 * depth as f32)
            .pr(8.0)
            .gap(5.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .role("treeitem")
            .label(name.clone())
            .expanded(open)
            .on_press(move |s: &mut App| {
                if let Some(b) = s.active_browse_mut() {
                    b.press_dir(&p);
                }
            })
            .child(chevron)
            .child(
                text(name)
                    .font(type_::BODY)
                    .color(if selected { t.text } else { t.text_secondary })
                    .truncate()
                    .shrink(),
            );
        // Cached covers only, at the row's end (the name never moves).
        if let Some(th) = super::art::row_thumb(s, "thumb", &b.username, &path, super::art::SMALL) {
            r = r.child(rune::el().grow(1.0)).child(th);
        }
        r = if selected {
            r.fill(t.hover).child(
                rune::el()
                    .pin(Some(0.0), Some(0.0), None, Some(0.0))
                    .w(2.0)
                    .fill(super::accent()),
            )
        } else {
            r.hover(t.hover)
        };
        list = list.child(r);
    }
    let (saved, view_h) = (tab.tree_scroll.clone(), tab.tree_view_h.clone());
    col()
        .key("tree")
        .w(TREE_W)
        .h_full()
        .min_h(0.0)
        .child(row().w_full().px(8.0).py(8.0).child(filter))
        .child(
            list.key(tab.tree_key())
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_scroll_offset(move |at: ScrollPos| {
                    saved.set(at.offset[1]);
                    view_h.set(at.viewport[1]);
                })
                .shrink()
                .min_h(0.0),
        )
}

fn folder(s: &App, tab: &Tab, b: &BrowseView, t: &Theme) -> El {
    let Some(dir) = b.selected_dir() else {
        return col().grow(1.0).center().child(
            text("No folder selected")
                .font(type_::BODY)
                .color(t.text_secondary),
        );
    };
    let head = album_header::view(
        &AlbumHeader {
            prefix: "folder-",
            username: &b.username,
            folder: &dir.path,
            files: &dir.files,
            status: user_status(s, &b.username),
            actions: Actions {
                play: true,
                download_to: true,
                browse_user: false,
            },
            inset: super::art::GAP,
        },
        t,
    );
    let head = super::art::beside(s, &b.username, &dir.path, NAME_X, head, t);
    let mut list = col().w_full().pb(8.0);
    let files: Rc<[RemoteFile]> = dir.files.clone().into();
    for (i, f) in dir.files.iter().enumerate() {
        let ctx = RowCtx {
            folder: files.clone(),
            in_user_tab: true,
            selected: tab.selected.as_deref() == Some(f.path.as_str()),
            highlighted: b.highlight.as_deref() == Some(f.path.as_str()),
            more_key: more_key(tab.id, &f.path),
            find: None,
        };
        list = list.child(file_row(s, &format!("file-{i}"), f, 0.0, &ctx, t));
    }
    if dir.files.is_empty() {
        list = list.child(
            text("No files")
                .font(type_::BODY)
                .color(t.text_secondary)
                .px(12.0)
                .pt(12.0),
        );
    }
    let saved = tab.scroll.clone();
    col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(head)
        .child(hairline(t))
        .child(columns(s, 0.0, t))
        .child(hairline(t))
        .child(
            list.key(tab.list_key())
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_scroll_offset(move |at: ScrollPos| saved.set(at.offset[1]))
                .shrink()
                .min_h(0.0),
        )
}
