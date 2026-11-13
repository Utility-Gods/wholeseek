use super::*;
use std::sync::atomic::AtomicBool;

fn entry(path: &str, tags: Option<Tags>) -> Entry {
    Entry {
        path: PathBuf::from(path),
        size: 1_000_000,
        mtime: 1,
        tags,
    }
}

fn tags(title: &str, artist: &str, album: &str, track: u32) -> Tags {
    Tags {
        title: Some(title.into()),
        artist: Some(artist.into()),
        album: Some(album.into()),
        track: Some(track),
        duration_secs: Some(200),
        codec: Some("FLAC".into()),
        bit_depth: Some(16),
        sample_rate_hz: Some(44_100),
        ..Tags::default()
    }
}

fn one(path: &str, t: Option<Tags>) -> Track {
    let ix = LibraryIndex::build(&[entry(path, t)], &[PathBuf::from("/m")]);
    ix.tracks[0].clone()
}

#[test]
fn tags_win_over_names() {
    let t = one(
        "/m/Folder Artist/Folder Album (1999)/07 file title.flac",
        Some(Tags {
            album_artist: Some("Band".into()),
            year: Some(2004),
            disc: Some(2),
            ..tags("Real Title", "Band feat. Guest", "Real Album", 3)
        }),
    );
    assert_eq!(t.title, "Real Title");
    assert_eq!(t.artist, "Band feat. Guest");
    assert_eq!(t.album_artist.as_deref(), Some("Band"));
    assert_eq!(t.album, "Real Album");
    assert_eq!((t.disc, t.track, t.year), (Some(2), Some(3), Some(2004)));
    assert_eq!(t.format, "FLAC");
    assert_eq!(t.bitrate_kbps, Some(40), "1 MB over 200 s");
    assert!(t.tagged && t.lossless());
}

#[test]
fn untagged_files_fall_back_to_folder_and_file_names() {
    // Artist/Album (year)/NN Title
    let t = one(
        "/m/Hollow Lights/Coastline (2019) [FLAC]/03 - Low Tide Radio.flac",
        None,
    );
    assert_eq!(
        (t.artist.as_str(), t.album.as_str(), t.title.as_str()),
        ("Hollow Lights", "Coastline", "Low Tide Radio")
    );
    assert_eq!((t.track, t.year), (Some(3), Some(2019)));
    assert!(!t.tagged);

    // "Artist - Album" folder right under the root; "NN. Title".
    let t = one("/m/Northern Glass - Tideline/01. Salt Roads.mp3", None);
    assert_eq!(
        (t.artist.as_str(), t.album.as_str(), t.title.as_str()),
        ("Northern Glass", "Tideline", "Salt Roads")
    );
    assert_eq!(t.track, Some(1));

    // Disc folder: album and disc from the folders, "1-02" style numbers too.
    let t = one("/m/Artist/Long Album/CD2/04 Song.flac", None);
    assert_eq!(
        (t.album.as_str(), t.artist.as_str()),
        ("Long Album", "Artist")
    );
    assert_eq!((t.disc, t.track), (Some(2), Some(4)));
    assert_eq!(t.album_dir, PathBuf::from("/m/Artist/Long Album"));
    let t = one("/m/Artist/Album/2-11 Late.flac", None);
    assert_eq!(
        (t.disc, t.track, t.title.as_str()),
        (Some(2), Some(11), "Late")
    );

    // A loose file in the root: "Artist - Title", no album.
    let t = one("/m/Some Band - A Single.mp3", None);
    assert_eq!(
        (t.artist.as_str(), t.title.as_str(), t.album.as_str()),
        ("Some Band", "A Single", "")
    );
    // Nothing to go on.
    let t = one("/m/untitled.wav", None);
    assert_eq!(
        (t.artist.as_str(), t.title.as_str()),
        ("Unknown artist", "untitled")
    );
    // Numbers that are titles stay titles.
    let t = one("/m/A/B/1979.flac", None);
    assert_eq!((t.title.as_str(), t.track), ("1979", None));

    // Tags present but empty: the names still count.
    let t = one(
        "/m/X/Y/02 Two.ogg",
        Some(Tags {
            title: Some("  ".into()),
            duration_secs: Some(10),
            ..Tags::default()
        }),
    );
    assert_eq!(
        (t.title.as_str(), t.album.as_str(), t.artist.as_str()),
        ("Two", "Y", "X")
    );
    assert!(!t.tagged);
}

#[test]
fn helpers() {
    assert_eq!(disc_dir("CD1"), Some(1));
    assert_eq!(disc_dir("Disc 2"), Some(2));
    assert_eq!(disc_dir("disk_3"), Some(3));
    assert_eq!(disc_dir("CD Singles"), None);
    assert_eq!(disc_dir("Discography"), None);
    assert_eq!(
        strip_folder_extras("Album (2019) [FLAC 24-96]"),
        ("Album".into(), Some(2019))
    );
    assert_eq!(
        strip_folder_extras("2005 - Album"),
        ("Album".into(), Some(2005))
    );
    assert_eq!(
        strip_folder_extras("(Untitled)"),
        ("(Untitled)".into(), None)
    );
    assert_eq!(split_number("05 Song"), (None, Some(5), "Song"));
    assert_eq!(split_number("A1 Song"), (None, None, "A1 Song"));
    assert_eq!(split_number("10"), (None, None, "10"));
}

#[test]
fn albums_group_by_album_artist_and_album_within_a_folder() {
    let mut entries = vec![
        // One album over two disc folders, out of order.
        entry("/m/A/Big/CD2/01 c.flac", Some(tags("c", "A", "Big", 1))),
        entry("/m/A/Big/CD1/02 b.flac", Some(tags("b", "A", "Big", 2))),
        entry("/m/A/Big/CD1/01 a.flac", Some(tags("a", "A", "Big", 1))),
        // A compilation: one album, several artists, no album artist.
        entry("/m/Comp/01.flac", Some(tags("x", "P", "Hits", 1))),
        entry("/m/Comp/02.flac", Some(tags("y", "Q", "Hits", 2))),
        // Same album name in another folder (another copy): its own album.
        entry("/m/Copies/Big/01.mp3", Some(tags("a", "A", "Big", 1))),
        // Two album artists sharing a folder and an album name: two albums.
        entry(
            "/m/Mixed/1.flac",
            Some(Tags {
                album_artist: Some("R".into()),
                ..tags("r", "R", "Same", 1)
            }),
        ),
        entry(
            "/m/Mixed/2.flac",
            Some(Tags {
                album_artist: Some("S".into()),
                ..tags("s", "S", "Same", 1)
            }),
        ),
    ];
    for (i, e) in entries.iter_mut().enumerate() {
        if let Some(t) = &mut e.tags {
            t.disc = e.path.to_str().unwrap().contains("CD2").then_some(2);
            t.year = Some(2000 + i as u32 % 2);
        }
    }
    let ix = LibraryIndex::build(&entries, &[PathBuf::from("/m")]);
    let names: Vec<(String, String, usize)> = ix
        .albums
        .iter()
        .map(|a| (a.artist.clone(), a.title.clone(), a.tracks.len()))
        .collect();
    assert_eq!(
        names,
        [
            ("A".into(), "Big".into(), 3),
            ("A".into(), "Big".into(), 1),
            ("R".into(), "Same".into(), 1),
            ("S".into(), "Same".into(), 1),
            ("Various artists".into(), "Hits".into(), 2),
        ]
    );
    let big = &ix.albums[0];
    let order: Vec<&str> = big
        .tracks
        .iter()
        .map(|&i| ix.tracks[i].title.as_str())
        .collect();
    assert_eq!(order, ["a", "b", "c"], "disc, then track");
    assert_eq!(big.dir, PathBuf::from("/m/A/Big"));
    assert_eq!(big.quality.format, "FLAC");
    assert_eq!(
        (big.quality.bit_depth, big.quality.sample_rate_hz),
        (Some(16), Some(44_100))
    );
    assert_eq!(big.duration_secs, 600);
    for (ai, a) in ix.albums.iter().enumerate() {
        for &i in &a.tracks {
            assert_eq!(ix.tracks[i].album_index, ai);
        }
    }
    assert_ne!(ix.albums[0].key, ix.albums[1].key);
}

#[test]
fn filter_matches_words_across_fields_and_file_name() {
    let ix = LibraryIndex::build(
        &[
            entry(
                "/m/A/B/01 file-one.flac",
                Some(tags("Salt Roads", "Hollow Lights", "Coastline", 1)),
            ),
            entry(
                "/m/A/B/02 other.flac",
                Some(tags("Low Tide", "Hollow Lights", "Coastline", 2)),
            ),
            entry(
                "/m/C/D/x.mp3",
                Some(tags("Elsewhere", "Nobody", "Nothing", 1)),
            ),
        ],
        &[PathBuf::from("/m")],
    );
    let hits = |q: &str| {
        let q = Query::parse(q);
        (0..ix.tracks.len())
            .filter(|&i| ix.track_matches(i, &q))
            .map(|i| ix.tracks[i].title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(hits("").len(), 3);
    assert_eq!(hits("HOLLOW salt"), ["Salt Roads"]);
    assert_eq!(hits("coast"), ["Salt Roads", "Low Tide"]);
    assert_eq!(hits("file-one"), ["Salt Roads"]);
    assert!(hits("hollow nobody").is_empty());
    let q = Query::parse("elsewhere");
    let albums: Vec<usize> = (0..ix.albums.len())
        .filter(|&a| ix.album_matches(a, &q))
        .collect();
    assert_eq!(albums.len(), 1);
    assert_eq!(ix.albums[albums[0]].title, "Nothing");
}

// ------------------------------------------------------------------------------- scanning

/// A reader that counts which files it read; tags from the file's first line.
struct Counting {
    reads: Mutex<Vec<PathBuf>>,
}

fn counting() -> (Arc<Counting>, TagReader) {
    let c = Arc::new(Counting {
        reads: Mutex::new(Vec::new()),
    });
    let c2 = c.clone();
    let reader: TagReader = Arc::new(move |p: &Path| {
        c2.reads.lock().unwrap().push(p.to_path_buf());
        let text = std::fs::read_to_string(p).ok()?;
        Some(Tags {
            title: Some(text.lines().next()?.to_string()),
            ..Tags::default()
        })
    });
    (c, reader)
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn no_progress() -> impl FnMut(ScanProgress) {
    |_| {}
}

#[test]
fn walk_finds_audio_only_once_and_skips_hidden() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("music");
    write(&root.join("A/B/01.flac"), "x");
    write(&root.join("A/B/cover.jpg"), "x");
    write(&root.join("A/B/notes.txt"), "x");
    write(&root.join("A/.hidden/02.flac"), "x");
    write(&root.join(".wholeseek-partial/t1/03.flac"), "x");
    write(&root.join("A/B/04.flac.part"), "x");
    write(&root.join("C/05.MP3"), "x");
    // Overlapping roots: each file once.
    let roots = vec![root.clone(), root.join("A")];
    let found = walk(&roots, &AtomicBool::new(false));
    let mut names: Vec<String> = found
        .iter()
        .map(|(p, ..)| p.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["A/B/01.flac", "C/05.MP3"]);
}

#[test]
fn rescan_rereads_only_changed_files_and_drops_deleted_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("music");
    for i in 0..5 {
        write(
            &root.join(format!("Artist/Album/0{i} song.flac")),
            &format!("Song {i}"),
        );
    }
    let roots = vec![root.clone()];
    let (c, reader) = counting();
    let cancel = AtomicBool::new(false);
    let (first, r1) = scan(&roots, &[], &reader, &mut no_progress(), &cancel);
    assert_eq!((r1.tracks, r1.reread, r1.removed), (5, 5, 0));
    assert_eq!(c.reads.lock().unwrap().len(), 5);

    // Nothing changed: nothing read.
    c.reads.lock().unwrap().clear();
    let (second, r2) = scan(&roots, &first, &reader, &mut no_progress(), &cancel);
    assert_eq!((r2.tracks, r2.reread, r2.removed), (5, 0, 0));
    assert!(c.reads.lock().unwrap().is_empty());
    assert_eq!(second, first);

    // One file edited (size changes), one deleted.
    let edited = root.join("Artist/Album/02 song.flac");
    write(&edited, "Retitled song two");
    std::fs::remove_file(root.join("Artist/Album/04 song.flac")).unwrap();
    c.reads.lock().unwrap().clear();
    let (third, r3) = scan(&roots, &second, &reader, &mut no_progress(), &cancel);
    assert_eq!((r3.tracks, r3.reread, r3.removed), (4, 1, 1));
    assert_eq!(*c.reads.lock().unwrap(), std::slice::from_ref(&edited));
    let ix = LibraryIndex::build(&third, &roots);
    assert!(ix.tracks.iter().any(|t| t.title == "Retitled song two"));
    assert!(!ix.tracks.iter().any(|t| t.title == "Song 4"));

    // Same size, new mtime: read again too.
    let f = std::fs::File::options().write(true).open(&edited).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(f);
    c.reads.lock().unwrap().clear();
    let (_, r4) = scan(&roots, &third, &reader, &mut no_progress(), &cancel);
    assert_eq!((r4.reread, r4.removed), (1, 0));
    assert_eq!(*c.reads.lock().unwrap(), [edited]);

    // No folders: an empty library.
    let (none, r5) = scan(&[], &third, &reader, &mut no_progress(), &cancel);
    assert!(none.is_empty());
    assert_eq!(r5.removed, 4);
}

#[test]
fn a_panicking_reader_is_no_tags() {
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("A/B/01 x.flac"), "x");
    let reader: TagReader = Arc::new(|_: &Path| panic!("demuxer bug"));
    let (e, r) = scan(
        &[tmp.path().to_path_buf()],
        &[],
        &reader,
        &mut no_progress(),
        &AtomicBool::new(false),
    );
    assert_eq!(r.tracks, 1);
    assert_eq!(e[0].tags, None);
}

#[test]
fn index_and_stars_persist_across_a_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let roots = vec![PathBuf::from("/m")];
    let entries = vec![
        entry("/m/A/B/01.flac", Some(tags("a", "A", "B", 1))),
        entry("/m/A/B/02.flac", None),
    ];
    save_index(&data, &roots, &entries).unwrap();
    assert_eq!(load_index(&data), (roots.clone(), entries.clone()));
    // A torn last line loses only that line.
    let text = std::fs::read_to_string(data.join(INDEX_FILE)).unwrap();
    std::fs::write(data.join(INDEX_FILE), &text[..text.len() - 10]).unwrap();
    assert_eq!(load_index(&data).1, entries[..1]);

    let mut stars = Stars::load(&data);
    assert!(stars.tracks.is_empty());
    assert!(stars.toggle_track("/m/A/B/01.flac").unwrap());
    let key = LibraryIndex::build(&entries, &roots).albums[0].key.clone();
    assert!(stars.toggle_album(&key).unwrap());
    assert!(stars.toggle_track("/m/A/B/02.flac").unwrap());
    assert!(!stars.toggle_track("/m/A/B/02.flac").unwrap(), "unstar");
    let back = Stars::load(&data);
    assert!(back.track("/m/A/B/01.flac"));
    assert!(!back.track("/m/A/B/02.flac"));
    assert!(back.album(&key));
    // In memory: nothing written.
    let mut mem = Stars::in_memory();
    mem.toggle_track("x").unwrap();
    assert!(mem.track("x"));
}

/// Every file under `dir` with its size, to prove a scan wrote nothing there.
fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(listing(&p));
        } else {
            out.push((p.clone(), e.metadata().unwrap().len()));
        }
    }
    out.sort();
    out
}

#[test]
fn the_service_scans_in_the_background_and_never_writes_into_the_music_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let music = tmp.path().join("music");
    for i in 0..3 {
        write(
            &music.join(format!("Artist/Album/0{i} s.flac")),
            &format!("S{i}"),
        );
    }
    // A folder image the cover store picks.
    std::fs::write(
        music.join("Artist/Album/cover.png"),
        crate::cover::test_png(8, 8),
    )
    .unwrap();
    let before = listing(&music);
    let data = tmp.path().join("data");
    let covers = Covers::open(tmp.path().join("covers"));
    let (_, reader) = counting();
    let lib = Library::start(Config {
        data_dir: data.clone(),
        tags: reader,
        art: None,
        covers: Some(covers.clone()),
    });
    lib.rescan(vec![music.clone()]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while lib.snapshot().last.is_none()
        || covers
            .cached(LOCAL_USER, &lib.snapshot().index.albums[0].folder)
            .is_none()
    {
        assert!(Instant::now() < deadline, "scan did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let snap = lib.snapshot();
    assert_eq!(snap.index.tracks.len(), 3);
    assert_eq!(snap.last.unwrap().reread, 3);
    assert!(!snap.progress.scanning);
    assert_eq!(listing(&music), before, "nothing new in the music folder");
    assert!(data.join(INDEX_FILE).is_file());
    // A restart publishes the stored index before any scan.
    drop(lib);
    let (_, reader) = counting();
    let lib = Library::start(Config {
        data_dir: data,
        tags: reader,
        art: None,
        covers: None,
    });
    while !lib.snapshot().loaded {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(lib.snapshot().index.tracks.len(), 3);
}

/// 10k files: walk + stat + build + filter, with a reader that does no I/O. The real tag
/// reading times are in `wholeseek-audio/tests/library.rs`.
#[test]
fn ten_thousand_songs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("music");
    for a in 0..100 {
        for t in 0..100 {
            write(
                &root.join(format!("Artist {}/Album {a}/{t:02} Song {t}.flac", a % 37)),
                "",
            );
        }
    }
    let reader: TagReader = Arc::new(|_: &Path| Some(Tags::default()));
    let roots = vec![root];
    let cancel = AtomicBool::new(false);
    let (entries, first) = scan(&roots, &[], &reader, &mut no_progress(), &cancel);
    let (again, re) = scan(&roots, &entries, &reader, &mut no_progress(), &cancel);
    assert_eq!((first.tracks, re.reread), (10_000, 0));
    let t = Instant::now();
    let ix = LibraryIndex::build(&again, &roots);
    let build = t.elapsed();
    assert_eq!(ix.albums.len(), 100);
    let t = Instant::now();
    let q = Query::parse("song 42");
    let n = (0..ix.tracks.len())
        .filter(|&i| ix.track_matches(i, &q))
        .count();
    let filter = t.elapsed();
    // "Song 42" in every album, and every song of "Album 42" (one in both).
    assert_eq!(n, 199);
    eprintln!(
        "10k: first scan {:?}, rescan {:?}, build {build:?}, filter {filter:?}",
        first.elapsed, re.elapsed
    );
    assert!(filter < Duration::from_millis(200), "{filter:?}");
}
