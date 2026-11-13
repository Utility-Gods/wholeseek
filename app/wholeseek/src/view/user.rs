//! A user tab: profile header, then a fixed folder tree (with a filter) on the left and the
//! selected folder's files on the right.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, col, el, row, text};

use std::rc::Rc;

use wholeseek_core::RemoteFile;

use super::album_header::{self, Actions, AlbumHeader, UserStatus};
use super::files::{ROW_H, RowCtx, columns, file_row, more_key};
use super::virtual_list::{ListScroll, virtual_list};
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
            .child(tree(tab, b, t))
            .child(vline(t))
            .child(folder(s, tab, b, t))
    };
    page.child(body)
}

/// The ⋯ on tree row `i` (`dir-{i}`), its folder menu's anchor.
fn tree_more_key(i: usize) -> String {
    format!("dir-more-{i}")
}

fn tree(tab: &Tab, b: &BrowseView, t: &Theme) -> El {
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
    // Only the rows in view are built (a share can have tens of thousands of folders); each
    // keeps its `dir-{i}` key, which reveals and tests address.
    let (rev, rows) = b.tree_rows();
    let scroll = ListScroll::sharing(
        tab.tree_key(),
        tab.tree_scroll.clone(),
        tab.tree_view_h.clone(),
    );
    let (id, theme) = (tab.id, *t);
    let list = virtual_list(
        &scroll,
        rows.len(),
        crate::tabs::TREE_ROW_H,
        move |s: &App, i| {
            let b = s.tabs.iter().find(|t| t.id == id).and_then(|t| t.browse());
            match b.zip(rows.get(i)) {
                Some((b, r)) => tree_row(s, b, i, r, &theme),
                None => el(),
            }
        },
    )
    .row_key(|_: &App, i| format!("dir-{i}").into())
    .version((rev, b.selected.as_deref()))
    .pad(crate::tabs::TREE_PAD)
    .build();
    col()
        .key("tree")
        .w(TREE_W)
        .h_full()
        .min_h(0.0)
        .child(row().w_full().px(8.0).py(8.0).child(filter))
        .child(list)
}

/// Tree row `i`: `(path, depth, has children, open)`.
fn tree_row(s: &App, b: &BrowseView, i: usize, r: &crate::tabs::TreeRow, t: &Theme) -> El {
    let (path, depth, has_children, open) = (&r.0, r.1, r.2, r.3);
    {
        let selected = b.selected.as_deref() == Some(path.as_str());
        let name = path.rsplit('\\').next().unwrap_or(path).to_string();
        let chevron: El = if has_children {
            icons::chevron(icons::SM, open, t.text_secondary)
        } else {
            el().size(9.0, 9.0)
        };
        let (p, menu_p, user) = (path.clone(), path.clone(), b.username.clone());
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
            .pointer_buttons([rune::PointerButton::Primary, rune::PointerButton::Middle])
            .on_press_tx(move |s: &mut App, tx| {
                // Middle-click: this folder in a new tab, as in a browser.
                if tx.button() == rune::PointerButton::Middle {
                    let user = s.active_browse().map(|b| b.username.clone());
                    if let Some(u) = user {
                        s.open_in_new_tab(&u, Some(&p));
                    }
                } else if let Some(b) = s.active_browse_mut() {
                    b.press_dir(&p);
                }
            })
            .on_secondary_press(move |s: &mut App, at, _| {
                let items = s.tree_menu(&user, &menu_p, has_children);
                super::menu::at_point(s, at, items);
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
        if let Some(th) = super::art::row_thumb(s, "thumb", &b.username, path, super::art::SMALL) {
            r = r.child(rune::el().grow(1.0)).child(th);
        }
        // ⋯ at the row's right edge, over the end of a long name: on the selected row, and on
        // the others while hovered. The same menu as a right-click.
        let more_key = tree_more_key(i);
        let (more_user, more_path) = (b.username.clone(), path.clone());
        let more = super::icon_button(&more_key, icons::more(icons::MD, t.text_secondary), t)
            .pin(None, Some(1.0), Some(4.0), None)
            .size(22.0, 22.0)
            .fill(t.hover)
            .label("More")
            .help("Folder options")
            .group_opacity(if selected { 1.0 } else { 0.0 }, 1.0)
            .on_press(move |s: &mut App| {
                let items = s.tree_menu(&more_user, &more_path, has_children);
                s.open_menu(crate::menu::MenuAnchor::Key(more_key.clone()), items);
            });
        r = r.hover_group().child(more);
        r = if selected {
            r.fill(t.hover).child(
                rune::el()
                    .key("marker")
                    .pin(Some(0.0), Some(0.0), None, Some(0.0))
                    .w(2.0)
                    .fill(super::accent()),
            )
        } else {
            r.hover(t.hover)
        };
        r
    }
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
    // Only the files in view are built (a folder can hold thousands).
    let files: Rc<[RemoteFile]> = b.selected_files().unwrap_or_else(|| Rc::from([]));
    let list: El = if files.is_empty() {
        col().key(tab.list_key()).grow(1.0).child(
            text("No files")
                .font(type_::BODY)
                .color(t.text_secondary)
                .px(12.0)
                .pt(12.0),
        )
    } else {
        let scroll = ListScroll::sharing(tab.list_key(), tab.scroll.clone(), Rc::default());
        let (id, theme, n) = (tab.id, *t, files.len());
        let rev = (n, Rc::as_ptr(&files) as *const () as usize);
        virtual_list(&scroll, n, ROW_H, move |s: &App, i| {
            let Some(tab) = s.tabs.iter().find(|t| t.id == id) else {
                return el();
            };
            let (b, f) = (tab.browse(), &files[i]);
            let ctx = RowCtx {
                folder: files.clone(),
                in_user_tab: true,
                selected: tab.selected.as_deref() == Some(f.path.as_str()),
                highlighted: b.and_then(|b| b.highlight.as_deref()) == Some(f.path.as_str()),
                more_key: more_key(id, &f.path),
                find: None,
            };
            file_row(s, &format!("file-{i}"), f, 0.0, &ctx, &theme)
        })
        .row_key(|_: &App, i| format!("file-{i}").into())
        .version(rev)
        .build()
        .pb(8.0)
    };
    col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(head)
        .child(hairline(t))
        .child(columns(s, 0.0, t))
        .child(hairline(t))
        .child(list)
}
