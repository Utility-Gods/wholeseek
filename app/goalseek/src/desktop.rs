//! The desktop around the app: opening files and folders and revealing a file in the file
//! manager. The real one shells out on background threads; the demo and tests record what
//! was asked, so nothing is ever launched.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

/// What was asked of the desktop (the demo's record, for tests).
#[derive(Debug, Clone, PartialEq)]
pub enum DesktopCall {
    Open(PathBuf),
    ShowItem(PathBuf),
}

pub trait Desktop {
    /// Open a file with its default app, or a folder in the file manager.
    fn open(&mut self, path: &Path);
    /// Show the file manager at `file`'s folder with `file` selected where supported.
    fn show_item(&mut self, file: &Path);
}

// ---- real ------------------------------------------------------------------------------------

pub struct RealDesktop;

/// Run `job` off the UI thread (helpers may block on D-Bus or a slow file manager).
fn background(job: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("goalseek-desktop".into())
        .spawn(job);
}

/// Run a helper; true when it exited successfully.
fn run(program: &str, args: &[String]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl Desktop for RealDesktop {
    fn open(&mut self, path: &Path) {
        let p = path.to_path_buf();
        background(move || {
            if !run("xdg-open", &[p.display().to_string()]) {
                eprintln!("goalseek: xdg-open {} failed", p.display());
            }
        });
    }
    fn show_item(&mut self, file: &Path) {
        let file = file.to_path_buf();
        background(move || {
            let shown = show_item_commands(&file)
                .into_iter()
                .any(|(program, args)| run(program, &args));
            if !shown {
                eprintln!("goalseek: could not show {}", file.display());
            }
        });
    }
}

/// How to reveal `file`, in order: the FileManager1 D-Bus interface (selects the file in
/// Nautilus, Dolphin, Nemo, …) via `gdbus` or `dbus-send`, else open its folder.
pub fn show_item_commands(file: &Path) -> Vec<(&'static str, Vec<String>)> {
    let uri = file_uri(file);
    let dir = file.parent().unwrap_or(file).display().to_string();
    vec![
        (
            "gdbus",
            vec![
                "call".into(),
                "--session".into(),
                "--dest".into(),
                "org.freedesktop.FileManager1".into(),
                "--object-path".into(),
                "/org/freedesktop/FileManager1".into(),
                "--method".into(),
                "org.freedesktop.FileManager1.ShowItems".into(),
                format!("['{}']", uri.replace('\'', "%27")),
                "".into(),
            ],
        ),
        (
            "dbus-send",
            vec![
                "--session".into(),
                "--print-reply".into(),
                "--dest=org.freedesktop.FileManager1".into(),
                "--type=method_call".into(),
                "/org/freedesktop/FileManager1".into(),
                "org.freedesktop.FileManager1.ShowItems".into(),
                format!("array:string:{}", uri.replace(',', "%2C")),
                "string:".into(),
            ],
        ),
        ("xdg-open", vec![dir]),
    ]
}

/// `file://` URI with everything but unreserved bytes and `/` percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---- demo / tests ----------------------------------------------------------------------------

/// Records calls; launches nothing.
pub struct DemoDesktop {
    pub calls: Rc<RefCell<Vec<DesktopCall>>>,
}

impl DemoDesktop {
    pub fn new(calls: Rc<RefCell<Vec<DesktopCall>>>) -> Self {
        Self { calls }
    }
}

impl Desktop for DemoDesktop {
    fn open(&mut self, path: &Path) {
        self.calls
            .borrow_mut()
            .push(DesktopCall::Open(path.to_path_buf()));
    }
    fn show_item(&mut self, file: &Path) {
        self.calls
            .borrow_mut()
            .push(DesktopCall::ShowItem(file.to_path_buf()));
    }
}
