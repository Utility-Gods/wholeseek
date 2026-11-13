//! `--demo`'s big share for the long-list work: [`BIG_USER`] shares 5,000 album folders of
//! 10 tracks under 20 genres × 50 artists, plus one 10,000-file dump folder ([`BIG_DUMP`]):
//! 60,000 files in 6,022 folders. Generated, deterministic, no randomness.

use wholeseek_core::{AudioAttrs, BrowseDir, RemoteFile};

pub const BIG_USER: &str = "megashare";
/// One folder of 10,000 files: the longest file list.
pub const BIG_DUMP: &str = r"Music\_Unsorted dump";
/// An album deep in the share, as a search's "Open folder" would open.
pub const BIG_FOLDER: &str = r"Music\Trip Hop\Velvet Static\Velvet Static - 1996 - Volume 4";

const GENRES: [&str; 20] = [
    "Ambient",
    "Breakbeat",
    "Dub",
    "Electro",
    "Footwork",
    "Garage",
    "House",
    "Jungle",
    "Reggae",
    "Soul",
    "Techno",
    "Trip Hop",
    "Drone",
    "Funk",
    "Jazz",
    "Krautrock",
    "Minimal",
    "Post Rock",
    "Shoegaze",
    "Synthpop",
];
const FIRST: [&str; 10] = [
    "Amber", "Cobalt", "Dusty", "Ember", "Hollow", "Lunar", "Paper", "Quiet", "Silver", "Velvet",
];
const SECOND: [&str; 5] = ["Signals", "Harbour", "Collective", "Static", "Engines"];
const TITLES: [&str; 10] = [
    "Opening",
    "Northern Glass",
    "Salt Roads",
    "Low Tide",
    "Breakwater",
    "Lamplight",
    "Undertow",
    "Ferry",
    "Coastline",
    "Closing",
];

fn track(folder: &str, name: String, secs: u32) -> RemoteFile {
    RemoteFile {
        username: BIG_USER.into(),
        path: format!("{folder}\\{name}"),
        size: u64::from(secs) * 105_000 + u64::from(secs % 7) * 13_007,
        attrs: AudioAttrs {
            duration_secs: Some(secs),
            sample_rate_hz: Some(44_100),
            bit_depth: Some(16),
            ..AudioAttrs::default()
        },
    }
}

/// [`BIG_USER`]'s share, in listing order.
pub fn share() -> Vec<BrowseDir> {
    let mut dirs = Vec::with_capacity(5_001);
    for (g, genre) in GENRES.iter().enumerate() {
        for (a, artist) in FIRST
            .iter()
            .flat_map(|f| SECOND.map(|s| format!("{f} {s}")))
            .enumerate()
        {
            for v in 0..5 {
                let year = 1980 + (g * 7 + a * 3 + v * 5) % 44;
                let path = format!(
                    r"Music\{genre}\{artist}\{artist} - {year} - Volume {}",
                    v + 1
                );
                let files = TITLES
                    .iter()
                    .enumerate()
                    .map(|(n, t)| {
                        let secs = 150 + ((g + a + v + n) * 37 % 240) as u32;
                        track(&path, format!("{:02} {t}.flac", n + 1), secs)
                    })
                    .collect();
                dirs.push(BrowseDir { path, files });
            }
        }
    }
    let files = (0..10_000)
        .map(|i| {
            let t = TITLES[i % TITLES.len()];
            let who = FIRST[i / 10 % FIRST.len()];
            track(
                BIG_DUMP,
                format!("{who} - {t} {i:05}.flac"),
                120 + (i * 13 % 300) as u32,
            )
        })
        .collect();
    dirs.push(BrowseDir {
        path: BIG_DUMP.into(),
        files,
    });
    dirs
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_big_share_is_5000_albums_and_60000_files() {
        let dirs = super::share();
        assert_eq!(dirs.len(), 5_001);
        assert_eq!(dirs.iter().map(|d| d.files.len()).sum::<usize>(), 60_000);
        assert!(dirs.iter().any(|d| d.path == super::BIG_FOLDER));
        assert_eq!(super::share()[1234].files[3].path, dirs[1234].files[3].path);
    }
}
