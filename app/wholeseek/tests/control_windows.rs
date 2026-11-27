#![cfg(windows)]

use std::process::Command;

#[test]
fn agent_commands_are_unavailable_without_starting_the_desktop() {
    for command in [
        "search",
        "browse",
        "download",
        "transfers",
        "retry",
        "status",
        "cancel",
        "daemon",
        "--help",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_wholeseek"))
            .arg(command)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(output.stdout.is_empty(), "{command}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap().trim_end(),
            "the command line isn't available on Windows yet",
            "{command}"
        );
    }
}
