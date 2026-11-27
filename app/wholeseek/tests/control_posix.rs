#![cfg(unix)]

// The binary's shared dispatch is integrated separately. Compile and exercise
// the exact same host module in this target without starting the desktop.
#[allow(dead_code)]
#[path = "../src/control/posix.rs"]
mod control;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wholeseek_core::control::ownership::{HANDOVER_MAX_AGE, Ownership, read_state};

const CHILD_DIR: &str = "WHOLESEEK_TEST_KILLED_DAEMON_DIR";

struct RemoveDir(PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct KillChild(Child);

impl Drop for KillChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn fake_daemon_mid_yield_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let local = Ownership::new(PathBuf::from(dir)).unwrap();
    let _lease = local.session().unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !local.app_is_waiting().unwrap() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    // Model a daemon interrupted before the handover's atomic rename.
    std::fs::write(
        local.handover_path().with_extension("json.pending"),
        b"partial",
    )
    .unwrap();
    thread::sleep(Duration::from_secs(10));
}

fn wait_file(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before yield"
        );
        assert!(Instant::now() < deadline, "child never entered yield");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn crash_mid_yield_releases_session_across_processes_without_handover() {
    let tag = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("wc-crash-{}-{tag:x}", std::process::id()));
    let _clean = RemoveDir(root.clone());
    let local = Ownership::new(root.clone()).unwrap();
    let _intent = local.app_intent().unwrap().unwrap();
    let mut child = KillChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fake_daemon_mid_yield_child"])
            .env(CHILD_DIR, &root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let handover = local.handover_path();
    wait_file(&handover.with_extension("json.pending"), &mut child.0);
    assert!(local.session().unwrap().is_none());
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    let _app = local.session().unwrap().unwrap();
    assert!(
        read_state::<serde_json::Value>(&handover, Some(HANDOVER_MAX_AGE))
            .unwrap()
            .is_none()
    );
}
