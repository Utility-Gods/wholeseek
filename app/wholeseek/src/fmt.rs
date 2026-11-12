//! Text for numbers: sizes, speeds, times, audio quality.

use wholeseek_core::RemoteFile;

pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

pub fn speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1e6 {
        format!("{:.1} MB/s", bytes_per_sec / 1e6)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1e3)
    }
}

/// `FLAC 16/44.1`, `FLAC 24/96`, `MP3 320`, `MP3 V 245`, `OGG 192`, `M4A`.
pub fn quality(f: &RemoteFile) -> String {
    let ext = f.extension().to_ascii_uppercase();
    let a = &f.attrs;
    match (a.bit_depth, a.sample_rate_hz, a.bitrate_kbps) {
        (Some(d), Some(r), _) => {
            let khz = f64::from(r) / 1000.0;
            let khz = if khz.fract() == 0.0 {
                format!("{khz:.0}")
            } else {
                format!("{khz:.1}")
            };
            format!("{ext} {d}/{khz}")
        }
        (_, _, Some(br)) if a.vbr == Some(true) => format!("{ext} V {br}"),
        (_, _, Some(br)) => format!("{ext} {br}"),
        _ => ext,
    }
}

/// `Music\Album (2019)` → `Music / Album (2019)`.
pub fn crumbs(path: &str) -> String {
    path.split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// File name without its extension, for the player strip.
pub fn title(f: &RemoteFile) -> String {
    let base = f.basename();
    base.rfind('.')
        .filter(|&i| i > 0)
        .map_or(base, |i| &base[..i])
        .to_string()
}

/// A local path with the home directory shown as `~`.
pub fn home_short(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}
