//! A user tab: profile header, then a fixed folder tree (with a filter) on the left and the
//! selected folder's files on the right.

use rune::tok::{Theme, type_};
use rune::{Cursor, El, ScrollPos, ScrollSpec, col, el, row, text};

use std::rc::Rc;

use goalseek_core::RemoteFile;

use super::files::{RowCtx, action, columns, file_row, more_key};
use super::{META, field_box, hairline, icons, input, red, tnum, vline};
use crate::app::App;
use crate::fmt;
use crate::tabs::{BrowseView, Tab};

const TREE_W: f32 = 240.0;

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

fn tree(tab: &Tab, b: &BrowseView, t: &Theme) -> El {
    let filter = field_box(
        "tree-filter",
        input(&b.filter, "Filter folders", t)
            .on_edit(|s: &mut App| &mut s.active_browse_mut().expect("user tab is shown").filter),
        t,
    )
    .w_full();
    let mut list = col().w_full().py(4.0);
    for (i, (path, depth, has_children, open)) in b.visible_tree().into_iter().enumerate() {
        let selected = b.selected.as_deref() == Some(path.as_str());
        let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
        let chevron: El = if has_children {
            icons::chevron(9.0, open, t.text_secondary)
        } else {
            el().size(9.0, 9.0)
        };
        let p = path.clone();
        let mut r = row()
            .key(format!("dir-{i}"))
            .h(24.0)
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
    let saved = tab.tree_scroll.clone();
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
                .on_scroll_offset(move |at: ScrollPos| saved.set(at.offset[1]))
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
    let user = b.username.clone();
    let path = dir.path.clone();
    let total: u64 = dir.files.iter().map(|f| f.size).sum();
    let head = row()
        .key("folder-head")
        .h(36.0)
        .w_full()
        .px(12.0)
        .gap(10.0)
        .center_y()
        .child(
            text(fmt::crumbs(&dir.path))
                .key("crumb-path")
                .font(type_::TITLE)
                .color(t.text)
                .truncate()
                .shrink(),
        )
        .child(
            text(format!(
                "{} {} · {}",
                dir.files.len(),
                if dir.files.len() == 1 {
                    "file"
                } else {
                    "files"
                },
                fmt::size(total)
            ))
            .key("folder-stats")
            .font(tnum(META))
            .color(t.text_secondary),
        )
        .child(el().grow(1.0))
        .child(
            action(
                "download-folder",
                "Download folder",
                icons::down(11.0, t.text),
                t,
            )
            .on_press(move |s: &mut App| s.download_folder(&user, &path)),
        );
    let mut list = col().w_full().pb(8.0);
    let files: Rc<[RemoteFile]> = dir.files.clone().into();
    for (i, f) in dir.files.iter().enumerate() {
        let ctx = RowCtx {
            folder: files.clone(),
            in_user_tab: true,
            selected: tab.selected.as_deref() == Some(f.path.as_str()),
            highlighted: b.highlight.as_deref() == Some(f.path.as_str()),
            more_key: more_key(tab.id, &f.path),
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
        .child(columns(0.0, t))
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
