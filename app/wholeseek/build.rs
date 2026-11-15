//! Windows only: put the app icon and version info into `wholeseek.exe` (what Explorer, the
//! taskbar and the file's Properties → Details show). Uses the resource compiler the toolchain
//! already has: the Windows SDK's `rc.exe` (MSVC) or `windres` (GNU). No crates.
//!
//! Without one, the exe builds without resources (and the window falls back to the PNG icon,
//! see `main.rs`); set `WHOLESEEK_REQUIRE_RESOURCES=1` to make that an error (release builds).
//! `WHOLESEEK_RC=<path>` picks the compiler.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The icon's resource id; `main.rs` hands it to `rune::set_app_icon_resource`.
const ICON_ID: u16 = 1;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WHOLESEEK_RC");
    println!("cargo:rerun-if-env-changed=WHOLESEEK_REQUIRE_RESOURCES");
    println!("cargo:rustc-check-cfg=cfg(wholeseek_icon_resource)");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let required = env::var_os("WHOLESEEK_REQUIRE_RESOURCES").is_some_and(|v| v != "0");
    match embed() {
        Ok(()) => println!("cargo:rustc-cfg=wholeseek_icon_resource"),
        Err(e) if required => panic!("wholeseek.exe resources: {e}"),
        Err(e) => println!("cargo:warning=wholeseek.exe built without icon/version info: {e}"),
    }
}

fn embed() -> Result<(), String> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let out = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let icon = manifest.join("../../assets/wholeseek.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let icon = icon
        .canonicalize()
        .map_err(|e| format!("{}: {e}", icon.display()))?;

    let rc = out.join("wholeseek.rc");
    std::fs::write(&rc, script(&icon)).map_err(|e| e.to_string())?;

    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let compiled = if msvc {
        let res = out.join("wholeseek.res");
        let tool = rc_exe().ok_or("rc.exe not found (Windows SDK); set WHOLESEEK_RC")?;
        let mut c = Command::new(&tool);
        c.arg("/nologo").arg("/fo").arg(&res).arg(&rc);
        run(c, &tool)?;
        res
    } else {
        let obj = out.join("wholeseek-res.o");
        let tool = windres().ok_or("windres not found; set WHOLESEEK_RC")?;
        let mut c = Command::new(&tool);
        c.arg("--input-format=rc")
            .arg("--output-format=coff")
            .arg("-i")
            .arg(&rc)
            .arg("-o")
            .arg(&obj);
        run(c, &tool)?;
        obj
    };
    // Only the app binary: test and bench executables need no icon.
    println!("cargo:rustc-link-arg-bins={}", compiled.display());
    Ok(())
}

fn run(mut c: Command, tool: &Path) -> Result<(), String> {
    let out = c.output().map_err(|e| format!("{}: {e}", tool.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed ({}): {}{}",
            tool.display(),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// The resource script: the icon, then VERSIONINFO from the crate's version.
fn script(icon: &Path) -> String {
    let v = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let n = |k: &str| {
        env::var(k)
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0)
    };
    let (major, minor, patch) = (
        n("CARGO_PKG_VERSION_MAJOR"),
        n("CARGO_PKG_VERSION_MINOR"),
        n("CARGO_PKG_VERSION_PATCH"),
    );
    // rc string literals: backslashes doubled; `\\?\` verbatim prefixes from canonicalize dropped.
    let icon = icon.display().to_string();
    let icon = icon
        .strip_prefix(r"\\?\")
        .unwrap_or(&icon)
        .replace('\\', "\\\\");
    format!(
        r#"{ICON_ID} ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "wholeseek contributors"
      VALUE "FileDescription", "wholeseek"
      VALUE "FileVersion", "{v}"
      VALUE "InternalName", "wholeseek"
      VALUE "LegalCopyright", "MIT License, (c) 2026 wholeseek contributors"
      VALUE "OriginalFilename", "wholeseek.exe"
      VALUE "ProductName", "wholeseek"
      VALUE "ProductVersion", "{v}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}

/// `WHOLESEEK_RC`, `rc.exe` on PATH (a VS developer shell), else the newest Windows 10/11 SDK's
/// for this host's architecture.
fn rc_exe() -> Option<PathBuf> {
    if let Some(p) = env::var_os("WHOLESEEK_RC") {
        return Some(p.into());
    }
    if let Some(p) = on_path("rc.exe") {
        return Some(p);
    }
    let arch = match env::var("HOST").unwrap_or_default().split('-').next() {
        Some("aarch64") => "arm64",
        Some("i686") => "x86",
        _ => "x64",
    };
    let roots = [
        env::var_os("WindowsSdkDir").map(PathBuf::from),
        env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Windows Kits").join("10")),
        Some(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10")),
    ];
    for root in roots.into_iter().flatten() {
        let Ok(dirs) = std::fs::read_dir(root.join("bin")) else {
            continue;
        };
        // Version folders sort as `10.0.<build>.0`: the highest with an rc.exe wins.
        let mut found: Vec<(Vec<u32>, PathBuf)> = dirs
            .flatten()
            .filter_map(|d| {
                let name = d.file_name().to_string_lossy().to_string();
                let version: Vec<u32> = name
                    .split('.')
                    .map(|p| p.parse().ok())
                    .collect::<Option<_>>()?;
                let rc = d.path().join(arch).join("rc.exe");
                rc.is_file().then_some((version, rc))
            })
            .collect();
        found.sort();
        if let Some((_, rc)) = found.pop() {
            return Some(rc);
        }
    }
    None
}

/// `WHOLESEEK_RC`, the target's `windres` (`x86_64-w64-mingw32-windres` when cross-building),
/// else `windres`.
fn windres() -> Option<PathBuf> {
    if let Some(p) = env::var_os("WHOLESEEK_RC") {
        return Some(p.into());
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    [format!("{arch}-w64-mingw32-windres"), "windres".to_string()]
        .iter()
        .find_map(|n| on_path(n).or_else(|| on_path(&format!("{n}.exe"))))
}

fn on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}
