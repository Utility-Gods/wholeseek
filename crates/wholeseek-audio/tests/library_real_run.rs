//! The Library on real files (`cargo test -p wholeseek-audio --test library_real_run --
//! --ignored --nocapture`): a temp music folder of ffmpeg-made audio is scanned by the real
//! scanner with the real tag reader, a song is played in place by the real player, and the
//! music folder is byte-for-byte unchanged afterwards; the only files written are the index,
//! the stars and the cover cache, all outside it. Then a 10k-file folder (copies) is timed.
//! Set `LIBRARY_RUN_FILES` for another size. Paths in the output are relative.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use wholeseek_audio::{ArtLookup, CaptureSink, Output, PlayState, Player, embedded_art, read_tags};
use wholeseek_core::Covers;
use wholeseek_core::library::{self, Library, ScanReport, Stars, TagReader, Tags};

fn reader() -> TagReader {
    Arc::new(|p: &Path| {
        read_tags(p).map(|t| Tags {
            title: t.title,
            artist: t.artist,
            album_artist: t.album_artist,
            album: t.album,
            track: t.track,
            disc: t.disc,
            year: t.year,
            duration_secs: t.duration_secs,
            codec: t.codec,
            bit_depth: t.bit_depth,
            sample_rate_hz: t.sample_rate_hz,
        })
    })
}

fn art() -> library::ArtReader {
    Arc::new(|p: &Path| match embedded_art(p) {
        ArtLookup::Found(a) => Some(a.data),
        _ => None,
    })
}

fn ffmpeg(args: &[&str], out: &Path) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .expect("ffmpeg must be installed");
    assert!(ok.success(), "ffmpeg failed: {}", out.display());
}

fn tone(out: &Path, secs: u32, meta: &[(&str, String)], codec: &[&str]) {
    let src = format!(
        "sine=frequency={}:sample_rate=44100:duration={secs}",
        300 + secs * 7
    );
    let mut args: Vec<String> = ["-f", "lavfi", "-i", &src]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(codec.iter().map(|s| s.to_string()));
    for (k, v) in meta {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg(&refs, out);
}

/// Every file under `dir`: relative path → (size, mtime).
fn listing(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(dir).unwrap().to_path_buf(),
                    (m.len(), m.modified().unwrap()),
                );
            }
        }
    }
    out
}

fn wait_scan(lib: &Library, after: Option<ScanReport>) -> ScanReport {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let s = lib.snapshot();
        if let Some(r) = s.last
            && Some(r) != after
            && !s.progress.scanning
        {
            return r;
        }
        assert!(Instant::now() < deadline, "scan did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start(data: &Path, covers: &Covers) -> Library {
    Library::start(library::Config {
        data_dir: data.to_path_buf(),
        tags: reader(),
        art: Some(art()),
        covers: Some(covers.clone()),
    })
}

#[test]
#[ignore = "real-run proof: generates audio with ffmpeg, ~1 min"]
fn real_run_scans_plays_in_place_and_writes_nothing_into_the_music_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let music = tmp.path().join("music");
    let data = tmp.path().join("data");
    let covers_dir = tmp.path().join("covers");
    // 6 albums × 5 tracks, three formats; one album untagged (names only); one with a
    // folder cover image.
    let formats: [(&str, &[&str]); 3] = [
        ("flac", &["-sample_fmt", "s16"]),
        ("mp3", &["-b:a", "192k"]),
        ("ogg", &["-c:a", "libvorbis"]),
    ];
    for a in 0..6u32 {
        let (ext, codec) = formats[a as usize % 3];
        let artist = format!("Test Artist {}", a % 3);
        let album = format!("Album {a}");
        let dir = music
            .join(&artist)
            .join(format!("{album} (20{:02})", 10 + a));
        for t in 1..=5u32 {
            let title = format!("Song {a}-{t}");
            let meta: Vec<(&str, String)> = if a == 5 {
                Vec::new()
            } else {
                vec![
                    ("title", title.clone()),
                    ("artist", artist.clone()),
                    ("album", album.clone()),
                    ("track", t.to_string()),
                    ("date", format!("20{:02}", 10 + a)),
                ]
            };
            tone(&dir.join(format!("{t:02} {title}.{ext}")), 3, &meta, codec);
        }
        if a == 0 {
            ffmpeg(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=orange:s=64x64",
                    "-frames:v",
                    "1",
                ],
                &dir.join("cover.png"),
            );
        }
    }
    let before = listing(&music);
    println!("[library-real-run] music folder: {} files", before.len());

    let covers = Covers::open(&covers_dir);
    let lib = start(&data, &covers);
    lib.rescan(vec![music.clone()]);
    let first = wait_scan(&lib, None);
    let ix = lib.snapshot().index;
    println!(
        "[library-real-run] first scan: {} songs, {} albums, {} read, {:?}",
        first.tracks,
        ix.albums.len(),
        first.reread,
        first.elapsed
    );
    assert_eq!((first.tracks, ix.albums.len()), (30, 6));
    let tagged = ix.tracks.iter().find(|t| t.title == "Song 0-3").unwrap();
    assert_eq!(
        (tagged.artist.as_str(), tagged.album.as_str()),
        ("Test Artist 0", "Album 0")
    );
    assert_eq!((tagged.track, tagged.year), (Some(3), Some(2010)));
    assert_eq!(tagged.codec.as_deref(), Some("FLAC"));
    assert_eq!(tagged.duration_secs, Some(3));
    let untagged = ix
        .tracks
        .iter()
        .find(|t| t.file_name() == "02 Song 5-2.ogg")
        .unwrap();
    assert_eq!(
        (
            untagged.artist.as_str(),
            untagged.album.as_str(),
            untagged.title.as_str()
        ),
        ("Test Artist 2", "Album 5", "Song 5-2")
    );
    assert_eq!(
        (untagged.track, untagged.year, untagged.tagged),
        (Some(2), Some(2015), false)
    );

    // Rescan, nothing changed.
    lib.rescan(vec![music.clone()]);
    let again = wait_scan(&lib, Some(first));
    println!(
        "[library-real-run] rescan (unchanged): {} read, {:?}",
        again.reread, again.elapsed
    );
    assert_eq!((again.tracks, again.reread, again.removed), (30, 0, 0));

    // Star a song: the stars file goes to the data dir.
    let mut stars = Stars::load(&data);
    stars.toggle_track(&tagged.path_str).unwrap();
    assert!(Stars::load(&data).track(&tagged.path_str));

    // Play it in place: the player reads the file where it is.
    let file = tagged.remote_file();
    assert!(file.is_local());
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    player.play(&tagged.path, tagged.size);
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.snapshot().position_secs < 1.0 {
        assert!(
            Instant::now() < deadline,
            "did not play: {:?}",
            player.snapshot().state
        );
        assert!(!matches!(player.snapshot().state, PlayState::Error(_)));
        std::thread::sleep(Duration::from_millis(20));
    }
    player.stop();
    println!(
        "[library-real-run] played in place: {} ({} samples captured)",
        tagged.file_name(),
        capture.data().samples.len()
    );

    // Covers stored (folder image, and embedded art where present) outside the music folder.
    let deadline = Instant::now() + Duration::from_secs(20);
    let a0 = ix.albums.iter().find(|a| a.title == "Album 0").unwrap();
    while covers.cached(library::LOCAL_USER, &a0.folder).is_none() {
        assert!(Instant::now() < deadline, "folder cover not stored");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(lib);

    // The music folder is exactly as it was.
    assert_eq!(listing(&music), before, "the music folder changed");
    let mut written: Vec<String> = listing(&data)
        .keys()
        .map(|p| p.display().to_string())
        .collect();
    written.sort();
    println!("[library-real-run] data dir: {written:?}; music folder unchanged");
    assert_eq!(written, [library::STARS_FILE, library::INDEX_FILE]);

    // Incremental: one file re-tagged, one deleted.
    let edited = music.join("Test Artist 1/Album 1 (2011)/01 Song 1-1.mp3");
    tone(
        &edited,
        4,
        &[
            ("title", "Retitled".into()),
            ("artist", "Test Artist 1".into()),
        ],
        &["-b:a", "192k"],
    );
    std::fs::remove_file(music.join("Test Artist 2/Album 2 (2012)/03 Song 2-3.ogg")).unwrap();
    let lib = start(&data, &covers);
    lib.rescan(vec![music.clone()]);
    let third = wait_scan(&lib, None);
    println!(
        "[library-real-run] rescan after 1 edit + 1 delete: {} read, {} removed, {:?}",
        third.reread, third.removed, third.elapsed
    );
    assert_eq!((third.tracks, third.reread, third.removed), (29, 1, 1));
    assert!(
        lib.snapshot()
            .index
            .tracks
            .iter()
            .any(|t| t.title == "Retitled")
    );
}

#[test]
#[ignore = "timing proof: 10k files (copies of small ffmpeg files), ~1-2 min"]
fn real_run_ten_thousand_files_timing() {
    let n: usize = std::env::var("LIBRARY_RUN_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let tmp = tempfile::tempdir().unwrap();
    let seeds = tmp.path().join("seeds");
    let kinds: [(&str, &[&str]); 3] = [
        ("flac", &["-sample_fmt", "s16"]),
        ("mp3", &["-b:a", "128k"]),
        ("ogg", &["-c:a", "libvorbis"]),
    ];
    for (ext, codec) in kinds {
        let meta = vec![
            ("title", "Seed".to_string()),
            ("artist", "Seed Artist".to_string()),
            ("album", "Seed Album".to_string()),
            ("track", "1".to_string()),
        ];
        tone(&seeds.join(format!("seed.{ext}")), 1, &meta, codec);
    }
    let music = tmp.path().join("music");
    let t = Instant::now();
    for i in 0..n {
        let (ext, _) = kinds[i % 3];
        let dir = music.join(format!("Artist {}/Album {}", i / 600, i / 12));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            seeds.join(format!("seed.{ext}")),
            dir.join(format!("{:02} Track.{ext}", i % 12 + 1)),
        )
        .unwrap();
    }
    println!("[library-real-run] made {n} files in {:?}", t.elapsed());
    let data = tmp.path().join("data");
    let covers = Covers::open(tmp.path().join("covers"));
    let lib = start(&data, &covers);
    lib.rescan(vec![music.clone()]);
    let first = wait_scan(&lib, None);
    lib.rescan(vec![music.clone()]);
    let re = wait_scan(&lib, Some(first));
    let ix = lib.snapshot().index;
    println!(
        "[library-real-run] {n} files: first scan {:?} ({} read), rescan {:?} ({} read), {} albums",
        first.elapsed,
        first.reread,
        re.elapsed,
        re.reread,
        ix.albums.len()
    );
    assert_eq!((first.tracks, first.reread, re.reread), (n, n, 0));
    let size = std::fs::metadata(data.join(library::INDEX_FILE))
        .unwrap()
        .len();
    println!("[library-real-run] index file: {} KiB", size / 1024);
}
