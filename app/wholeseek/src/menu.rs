//! Context menus (⋯ buttons, right-click, Menu key / Shift+F10): what each place offers.
//! Drawn in-app; Rune's native pop-up menus exist only on macOS and Windows.

use std::path::PathBuf;

use wholeseek_core::{RemoteFile, TransferId};

use crate::app::{App, TState};

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Play(RemoteFile),
    Download(RemoteFile),
    DownloadFolder {
        username: String,
        folder: String,
    },
    /// The user's tab at `folder`, with `file` (a remote path) highlighted.
    OpenFolder {
        username: String,
        folder: String,
        file: Option<String>,
    },
    BrowseUser(String),
    /// Stream a remote folder's audio files in order.
    PlayFolder {
        username: String,
        folder: String,
    },
    /// Text for the clipboard.
    CopyPath(String),
    /// ▶ a finished download from disk.
    PlayLocal(TransferId),
    ShowInFolder(PathBuf),
    OpenFile(PathBuf),
    /// ▶ a group's finished downloads in order, from disk.
    PlayAlbum {
        username: String,
        folder: String,
    },
    Cancel(TransferId),
    Retry(TransferId),
    /// A format folded under "+N": select or unselect it in the shown search tab.
    ToggleFormat(&'static str),
    /// A tab folded under the strip's "»": show it.
    ShowTab(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub id: &'static str,
    pub label: String,
    pub action: MenuAction,
}

fn item(id: &'static str, label: &str, action: MenuAction) -> MenuItem {
    MenuItem {
        id,
        label: label.to_string(),
        action,
    }
}

/// Where the menu opens: at a pointer (right-click), or under a keyed element (⋯).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAnchor {
    Point(f32, f32),
    Key(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub anchor: MenuAnchor,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn ids(&self) -> Vec<&'static str> {
        self.items.iter().map(|i| i.id).collect()
    }
}

/// A file row in search results (`in_user_tab` false) or a user tab.
pub fn file_items(f: &RemoteFile, in_user_tab: bool) -> Vec<MenuItem> {
    let username = f.username.clone();
    let folder = f.folder().to_string();
    let mut v = Vec::new();
    if crate::tabs::is_playable(f) {
        v.push(item("play", "Play", MenuAction::Play(f.clone())));
    }
    v.push(item(
        "download",
        "Download",
        MenuAction::Download(f.clone()),
    ));
    v.push(item(
        "download-folder",
        "Download folder",
        MenuAction::DownloadFolder {
            username: username.clone(),
            folder: folder.clone(),
        },
    ));
    v.push(item(
        "open-folder",
        "Open folder",
        MenuAction::OpenFolder {
            username: username.clone(),
            folder,
            file: Some(f.path.clone()),
        },
    ));
    if !in_user_tab {
        v.push(item(
            "browse-user",
            "Browse user",
            MenuAction::BrowseUser(username),
        ));
    }
    v.push(item(
        "copy-path",
        "Copy path",
        MenuAction::CopyPath(f.path.clone()),
    ));
    v
}

/// A folder line in grouped search results.
/// A folder line's menu; "Play folder" only when it holds something the player decodes.
pub fn folder_items(username: &str, folder: &str, playable: bool) -> Vec<MenuItem> {
    let (u, d) = (username.to_string(), folder.to_string());
    let mut v = vec![
        item(
            "open-folder",
            "Open folder",
            MenuAction::OpenFolder {
                username: u.clone(),
                folder: d.clone(),
                file: None,
            },
        ),
        item(
            "download-folder",
            "Download folder",
            MenuAction::DownloadFolder {
                username: u.clone(),
                folder: d.clone(),
            },
        ),
    ];
    if playable {
        v.push(item(
            "play-folder",
            "Play folder",
            MenuAction::PlayFolder {
                username: u,
                folder: d.clone(),
            },
        ));
    }
    v.push(item("copy-path", "Copy path", MenuAction::CopyPath(d)));
    v
}

impl App {
    /// One download in the drawer.
    pub fn transfer_items(&self, id: TransferId) -> Vec<MenuItem> {
        let Some(t) = self.transfers.iter().find(|t| t.id == id) else {
            return Vec::new();
        };
        match &t.state {
            TState::Done(path) => {
                let mut v = Vec::new();
                if crate::tabs::is_playable(&t.file) {
                    v.push(item("play", "Play", MenuAction::PlayLocal(id)));
                }
                v.push(item(
                    "show-in-folder",
                    "Show in folder",
                    MenuAction::ShowInFolder(path.clone()),
                ));
                v.push(item(
                    "open-file",
                    "Open with default app",
                    MenuAction::OpenFile(path.clone()),
                ));
                v.push(item(
                    "copy-path",
                    "Copy path",
                    MenuAction::CopyPath(path.display().to_string()),
                ));
                v
            }
            TState::Failed(_) | TState::Cancelled => vec![
                item("retry", "Retry", MenuAction::Retry(id)),
                item(
                    "copy-path",
                    "Copy path",
                    MenuAction::CopyPath(t.file.path.clone()),
                ),
            ],
            _ => vec![
                item("cancel", "Cancel", MenuAction::Cancel(id)),
                item(
                    "copy-path",
                    "Copy path",
                    MenuAction::CopyPath(t.file.path.clone()),
                ),
            ],
        }
    }

    /// A (user, folder) group in the drawer.
    pub fn group_items(&self, username: &str, folder: &str) -> Vec<MenuItem> {
        let mut v = Vec::new();
        if let Some(path) = self.group_done_path(username, folder) {
            // The folder opens with its first finished file selected.
            v.push(item(
                "show-in-folder",
                "Show in folder",
                MenuAction::ShowInFolder(path.clone()),
            ));
            v.push(item(
                "play-album",
                "Play album",
                MenuAction::PlayAlbum {
                    username: username.into(),
                    folder: folder.into(),
                },
            ));
        }
        v
    }

    /// The player's current track.
    pub fn player_items(&self) -> Vec<MenuItem> {
        let Some(p) = &self.playing else {
            return Vec::new();
        };
        let mut v = Vec::new();
        match self.local_path_of(&p.file) {
            Some(path) => {
                v.push(item(
                    "show-in-folder",
                    "Show in folder",
                    MenuAction::ShowInFolder(path.clone()),
                ));
                v.push(item(
                    "copy-path",
                    "Copy path",
                    MenuAction::CopyPath(path.display().to_string()),
                ));
            }
            None => {
                v.push(item(
                    "download",
                    "Download",
                    MenuAction::Download(p.file.clone()),
                ));
                v.push(item(
                    "copy-path",
                    "Copy path",
                    MenuAction::CopyPath(p.file.path.clone()),
                ));
            }
        }
        v
    }

    /// The "+N" chip's menu: formats past the first few, "✓" on selected ones.
    pub fn more_format_items(&self) -> Vec<MenuItem> {
        let Some(st) = self.tab().search() else {
            return Vec::new();
        };
        let Some(v) = &st.search else {
            return Vec::new();
        };
        v.format_counts(&st.filters)
            .into_iter()
            .skip(crate::view::MAX_FORMAT_CHIPS)
            .map(|(label, n)| {
                let mark = if st.filters.has(label) { "✓ " } else { "" };
                item(
                    crate::tabs::format_key(label),
                    &format!("{mark}{label} {n}"),
                    MenuAction::ToggleFormat(label),
                )
            })
            .collect()
    }

    /// The strip's "»" menu: the tabs that don't fit, in order.
    pub fn hidden_tab_items(&self, hidden: &[usize]) -> Vec<MenuItem> {
        hidden
            .iter()
            .filter_map(|&i| self.tabs.get(i).map(|t| (i, t)))
            .map(|(i, t)| item("tab", &t.title(), MenuAction::ShowTab(i)))
            .collect()
    }
}
