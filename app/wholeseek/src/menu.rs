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
    /// The sort menu: order the shown search tab's results.
    SetSort(crate::tabs::SortBy),
    /// A tab folded under the strip's "»": show it.
    ShowTab(usize),
    /// A new user tab beside the shown one, at `folder` (none: the user's share), left in
    /// the background like a browser's.
    OpenInNewTab {
        username: String,
        folder: Option<String>,
    },
    /// A user tab's tree: open (or close) a folder and every folder under it.
    ExpandAll(String),
    CollapseAll(String),
    /// The tab strip, on tab `i`.
    DuplicateTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
    CloseTabsRight(usize),
    /// The Library: ▶ a song (by path) in the list it's shown in.
    LibraryPlay(String),
    /// The Library: ▶ an album (by key).
    LibraryPlayAlbum(String),
    /// The Library: star or unstar a song (by path).
    LibraryStar(String),
    /// The Library: star or unstar an album (by key).
    LibraryStarAlbum(String),
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
        "copy-path" | "copy-name" => &icons::COPY,
        "open-in-new-tab" | "open-folder-new-tab" => &icons::APP_WINDOW,
        "browse-user-new-tab" => &icons::USER_PLUS,
        "expand-all" => &icons::CHEVRONS_UP_DOWN,
        "collapse-all" => &icons::CHEVRONS_DOWN_UP,
        "duplicate-tab" => &icons::COPY_PLUS,
        "close-tab" => &icons::X,
        "close-other-tabs" => &icons::SQUARE_X,
        "close-tabs-right" => &icons::ARROW_RIGHT_TO_LINE,
        "open-file" => &icons::EXTERNAL_LINK,
        "retry" => &icons::REFRESH_CW,
        "star" => &icons::STAR,
        "cancel" => &icons::X,
        // Sort items: the view puts the check on the current one.
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
    let mut common = Vec::new();
    // In a user tab the folder is already open, and its header downloads it.
    if !in_user_tab {
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
                folder: folder.clone(),
                file: Some(f.path.clone()),
            },
        ));
        v.push(item(
            "browse-user",
            "Browse user",
            MenuAction::BrowseUser(username.clone()),
        ));
        common.extend(new_tab_items(&username, &folder));
    }
    common.push(item(
        "copy-name",
        if in_user_tab {
            "Copy file name"
        } else {
            "Copy name"
        },
        MenuAction::CopyPath(f.basename().to_string()),
    ));
    common.push(item(
        "copy-path",
        "Copy path",
        MenuAction::CopyPath(f.path.clone()),
    ));
    finish(v, common)
}

/// A row menu in two parts: what this row does, then under a line the group that's the
/// same everywhere (new tabs, copying). The recent "Download to…" folders end at a line
/// too.
fn finish(mut v: Vec<MenuItem>, common: Vec<MenuItem>) -> Vec<MenuItem> {
    let first_common = v.len();
    v.extend(common);
    if let Some(c) = v.get_mut(first_common) {
        c.sep = true;
    }
    if let Some(last_recent) = v.iter().rposition(|i| i.help.is_some())
        && let Some(after) = v.get_mut(last_recent + 1)
    {
        after.sep = true;
    }
    v
}

/// Where a menu's common group starts (after [`finish`]'s line); its end if none.
fn common_start(v: &[MenuItem]) -> usize {
    v.iter()
        .position(|i| {
            matches!(
                i.id,
                "open-in-new-tab" | "open-folder-new-tab" | "browse-user-new-tab" | "copy-name"
            )
        })
        .unwrap_or(v.len())
}

/// A search result's "Open folder in new tab" and "Browse user in new tab".
fn new_tab_items(username: &str, folder: &str) -> [MenuItem; 2] {
    [
        item(
            "open-folder-new-tab",
            "Open folder in new tab",
            MenuAction::OpenInNewTab {
                username: username.to_string(),
                folder: Some(folder.to_string()),
            },
        ),
        item(
            "browse-user-new-tab",
            "Browse user in new tab",
            MenuAction::OpenInNewTab {
                username: username.to_string(),
                folder: None,
            },
        ),
    ]
}

/// The last part of a remote path (a folder's or file's name).
fn name_of(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_string()
}

/// A user tab's folder tree row: play, download, a new tab there, open or close everything
/// under it (`has_children`), copy its path.
pub fn tree_items(
    username: &str,
    folder: &str,
    playable: bool,
    has_children: bool,
    recent: &[PathBuf],
) -> Vec<MenuItem> {
    let (u, d) = (username.to_string(), folder.to_string());
    let mut v = Vec::new();
    if playable {
        v.push(item(
            "play-folder",
            "Play folder",
            MenuAction::PlayFolder {
                username: u.clone(),
                folder: d.clone(),
            },
        ));
    }
    v.push(item(
        "download-folder",
        "Download folder",
        MenuAction::DownloadFolder {
            username: u.clone(),
            folder: d.clone(),
        },
    ));
    v.extend(download_to_items(
        &DlTarget::Folder {
            username: u.clone(),
            folder: d.clone(),
        },
        recent,
    ));
    if has_children {
        v.push(item(
            "expand-all",
            "Expand all",
            MenuAction::ExpandAll(d.clone()),
        ));
        v.push(item(
            "collapse-all",
            "Collapse all",
            MenuAction::CollapseAll(d.clone()),
        ));
    }
    let common = vec![
        item(
            "open-in-new-tab",
            "Open in new tab",
            MenuAction::OpenInNewTab {
                username: u,
                folder: Some(d.clone()),
            },
        ),
        item("copy-path", "Copy folder path", MenuAction::CopyPath(d)),
    ];
    finish(v, common)
}

/// The tab strip's menu on tab `i` of `n`: duplicate, then close it (as × does), the others
/// (with others) and those to its right (when some are).
/// A Library song: Play, Star/Unstar, Show in folder, Copy path.
pub fn library_song_items(path: &str, starred: bool) -> Vec<MenuItem> {
    vec![
        item("play", "Play", MenuAction::LibraryPlay(path.to_string())),
        item(
            "star",
            if starred { "Unstar" } else { "Star" },
            MenuAction::LibraryStar(path.to_string()),
        ),
        MenuItem {
            sep: true,
            ..item(
                "show-in-folder",
                "Show in folder",
                MenuAction::ShowInFolder(PathBuf::from(path)),
            )
        },
        item(
            "copy-path",
            "Copy path",
            MenuAction::CopyPath(path.to_string()),
        ),
    ]
}

/// A Library album: Play album, Star/Unstar, Show in folder, Copy path.
pub fn library_album_items(key: &str, dir: &Path, starred: bool) -> Vec<MenuItem> {
    vec![
        item(
            "play-album",
            "Play album",
            MenuAction::LibraryPlayAlbum(key.to_string()),
        ),
        item(
            "star",
            if starred {
                "Unstar album"
            } else {
                "Star album"
            },
            MenuAction::LibraryStarAlbum(key.to_string()),
        ),
        MenuItem {
            sep: true,
            ..item(
                "show-in-folder",
                "Show in folder",
                MenuAction::OpenFile(dir.to_path_buf()),
            )
        },
        item(
            "copy-path",
            "Copy path",
            MenuAction::CopyPath(dir.display().to_string()),
        ),
    ]
}

pub fn tab_items(i: usize, n: usize) -> Vec<MenuItem> {
    let mut v = vec![
        item(
            "duplicate-tab",
            "Duplicate tab",
            MenuAction::DuplicateTab(i),
        ),
        MenuItem {
            sep: true,
            ..item("close-tab", "Close tab", MenuAction::CloseTab(i))
        },
    ];
    if n > 1 {
        v.push(item(
            "close-other-tabs",
            "Close other tabs",
            MenuAction::CloseOtherTabs(i),
        ));
    }
    if i + 1 < n {
        v.push(item(
            "close-tabs-right",
            "Close tabs to the right",
            MenuAction::CloseTabsRight(i),
        ));
    }
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
    let mut v = Vec::new();
    if playable {
        v.push(item(
            "play-folder",
            "Play folder",
            MenuAction::PlayFolder {
                username: u.clone(),
                folder: d.clone(),
            },
        ));
    }
    v.push(item(
        "download-folder",
        "Download folder",
        MenuAction::DownloadFolder {
            username: u.clone(),
            folder: d.clone(),
        },
    ));
    v.extend(download_to_items(
        &DlTarget::Folder {
            username: u.clone(),
            folder: d.clone(),
        },
        recent,
    ));
    v.push(item(
        "open-folder",
        "Open folder",
        MenuAction::OpenFolder {
            username: u.clone(),
            folder: d.clone(),
            file: None,
        },
    ));
    v.push(item(
        "browse-user",
        "Browse user",
        MenuAction::BrowseUser(u.clone()),
    ));
    let mut common = new_tab_items(&u, &d).to_vec();
    common.push(item(
        "copy-name",
        "Copy name",
        MenuAction::CopyPath(name_of(&d)),
    ));
    common.push(item("copy-path", "Copy path", MenuAction::CopyPath(d)));
    finish(v, common)
}

impl App {
    /// A file row's menu (⋯ and right-click alike). In a user tab, a file downloaded
    /// before also gets "Show in folder" (from the download history, else this run's
    /// transfers).
    pub fn file_menu(&self, f: &RemoteFile, in_user_tab: bool) -> Vec<MenuItem> {
        let mut v = file_items(
            f,
            in_user_tab,
            &self.backend.settings().recent_download_dirs,
        );
        if in_user_tab && let Some(path) = self.downloaded_path(f) {
            // A contextual action: last before the common group's line.
            let at = common_start(&v);
            v.insert(
                at,
                item(
                    "show-in-folder",
                    "Show in folder",
                    MenuAction::ShowInFolder(path),
                ),
            );
        }
        v
    }

    /// Where `f` was saved, if a finished download of it is still on disk.
    fn downloaded_path(&self, f: &RemoteFile) -> Option<PathBuf> {
        if let Some(p) = self.local_path_of(f) {
            return Some(p);
        }
        if !self
            .downloaded
            .contains(&(f.username.clone(), f.path.clone()))
        {
            return None;
        }
        let q = wholeseek_core::HistoryQuery {
            text: f.path.clone(),
            limit: 50,
            offset: 0,
        };
        self.backend
            .history(&q)
            .into_iter()
            .filter(|e| e.username == f.username && e.remote_path == f.path)
            .find(|e| self.backend.history_on_disk(e))
            .and_then(|e| e.local_path)
    }

    /// A user tab's folder tree row.
    pub fn tree_menu(&self, username: &str, folder: &str, has_children: bool) -> Vec<MenuItem> {
        let playable = self
            .folder_files(username, folder)
            .iter()
            .any(crate::tabs::is_playable);
        let recent = &self.backend.settings().recent_download_dirs;
        tree_items(username, folder, playable, has_children, recent)
    }

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
