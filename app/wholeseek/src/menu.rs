//! Context menus (⋯ buttons, right-click, Menu key / Shift+F10): what each place offers.
//! Drawn in-app; Rune's native pop-up menus exist only on macOS and Windows.

use std::path::{Path, PathBuf};

use rune::Icon;
use wholeseek_core::{RemoteFile, TransferId};

use crate::app::{App, TState};
use crate::view::icons;

/// What "Download to…" saves: one file, or a remote folder.
#[derive(Debug, Clone, PartialEq)]
pub enum DlTarget {
    File(RemoteFile),
    Folder { username: String, folder: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Play(RemoteFile),
    Download(RemoteFile),
    /// ↓ on the playing track: an album track (queue of more than one) keeps its folder.
    Keep(RemoteFile),
    DownloadFolder {
        username: String,
        folder: String,
    },
    /// Save `target` into `dir`; `None` asks for a folder first (the folder picker).
    DownloadTo {
        target: DlTarget,
        dir: Option<PathBuf>,
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

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub id: &'static str,
    pub label: String,
    pub action: MenuAction,
    /// Drawn before the label; items without one keep the label in line with the rest.
    pub icon: Option<&'static Icon>,
    /// A thin line above the item, starting a group.
    pub sep: bool,
    /// Tooltip (a recent folder's full path).
    pub help: Option<String>,
}

impl PartialEq for MenuItem {
    fn eq(&self, o: &Self) -> bool {
        (
            self.id,
            &self.label,
            &self.action,
            self.icon.map(|i| i.name),
            self.sep,
            &self.help,
        ) == (
            o.id,
            &o.label,
            &o.action,
            o.icon.map(|i| i.name),
            o.sep,
            &o.help,
        )
    }
}

fn item(id: &'static str, label: &str, action: MenuAction) -> MenuItem {
    MenuItem {
        id,
        label: label.to_string(),
        icon: icon_for(id),
        action,
        sep: false,
        help: None,
    }
}

/// One icon per kind of item, the same wherever it appears.
fn icon_for(id: &str) -> Option<&'static Icon> {
    Some(match id {
        "play" => &icons::PLAY,
        "play-folder" => &icons::LIST_MUSIC,
        "play-album" => &icons::DISC_3,
        "download" | "download-folder" => &icons::DOWNLOAD,
        "download-to" | "download-folder-to" => &icons::FOLDER_DOWN,
        id if id.starts_with("download-to-") || id.starts_with("download-folder-to-") => {
            &icons::FOLDER
        }
        "open-folder" | "show-in-folder" => &icons::FOLDER_OPEN,
        "browse-user" => &icons::USER,
        "copy-path" => &icons::COPY,
        "open-file" => &icons::EXTERNAL_LINK,
        "retry" => &icons::REFRESH_CW,
        "cancel" => &icons::X,
        _ => return None,
    })
}

const FILE_TO: [&str; 5] = [
    "download-to-0",
    "download-to-1",
    "download-to-2",
    "download-to-3",
    "download-to-4",
];
const FOLDER_TO: [&str; 5] = [
    "download-folder-to-0",
    "download-folder-to-1",
    "download-folder-to-2",
    "download-folder-to-3",
    "download-folder-to-4",
];

/// Recent folders longer than this many characters lose their middle.
pub const DIR_LABEL_MAX: usize = 40;

/// `dir` for a menu label: home as "~", the middle elided past [`DIR_LABEL_MAX`] characters
/// ("~/Music/…/Vinyl Rips").
pub fn dir_label(dir: &Path, home: Option<&Path>) -> String {
    let full = match home.and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    };
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= DIR_LABEL_MAX {
        return full;
    }
    let keep = DIR_LABEL_MAX - 1;
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// "Download to…" (the folder picker), then, under a thin line, each recent folder as its
/// own item ("~/Music/Vinyl"), most recent first; the tooltip has the full path.
pub fn download_to_items(target: &DlTarget, recent: &[PathBuf]) -> Vec<MenuItem> {
    let (choose, ids, label) = match target {
        DlTarget::File(_) => ("download-to", FILE_TO, "Download to"),
        DlTarget::Folder { .. } => ("download-folder-to", FOLDER_TO, "Download folder to"),
    };
    let mut v = vec![item(
        choose,
        &format!("{label}…"),
        MenuAction::DownloadTo {
            target: target.clone(),
            dir: None,
        },
    )];
    let home = home_dir();
    for (i, (id, dir)) in ids.iter().zip(recent).enumerate() {
        v.push(MenuItem {
            sep: i == 0,
            help: Some(dir.display().to_string()),
            ..item(
                id,
                &dir_label(dir, home.as_deref()),
                MenuAction::DownloadTo {
                    target: target.clone(),
                    dir: Some(dir.clone()),
                },
            )
        });
    }
    v
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

/// A file row in search results (`in_user_tab` false) or a user tab. `recent`: the recent
/// "Download to…" folders.
pub fn file_items(f: &RemoteFile, in_user_tab: bool, recent: &[PathBuf]) -> Vec<MenuItem> {
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
    v.extend(download_to_items(&DlTarget::File(f.clone()), recent));
    v.push(MenuItem {
        // The recent folders end where they began: at a line.
        sep: !recent.is_empty(),
        ..item(
            "download-folder",
            "Download folder",
            MenuAction::DownloadFolder {
                username: username.clone(),
                folder: folder.clone(),
            },
        )
    });
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
pub fn folder_items(
    username: &str,
    folder: &str,
    playable: bool,
    recent: &[PathBuf],
) -> Vec<MenuItem> {
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
    v.extend(download_to_items(
        &DlTarget::Folder {
            username: u.clone(),
            folder: d.clone(),
        },
        recent,
    ));
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
    // The recent folders end where they began: at a line.
    if !recent.is_empty()
        && let Some(after) = v
            .iter_mut()
            .skip_while(|i| i.id != "download-folder-to")
            .find(|i| i.help.is_none() && i.id != "download-folder-to")
    {
        after.sep = true;
    }
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
                    MenuAction::Keep(p.file.clone()),
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
            .map(|(label, n)| MenuItem {
                icon: st.filters.has(label).then_some(&icons::CHECK),
                ..item(
                    crate::tabs::format_key(label),
                    &format!("{label} {n}"),
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
            .map(|(i, t)| MenuItem {
                icon: Some(if t.search().is_some() {
                    &icons::SEARCH
                } else {
                    &icons::USER
                }),
                ..item("tab", &t.title(), MenuAction::ShowTab(i))
            })
            .collect()
    }
}
