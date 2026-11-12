//! Unit-level behaviour of `GrowingFile`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;
use wholeseek_audio::{GrowingError, GrowingFile};

fn append(path: &Path, data: &[u8]) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(data).unwrap();
}

#[test]
fn read_blocks_until_bytes_arrive_then_follows_rename() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.flac");
    let part = dir.path().join("a.flac.part");
    append(&part, b"hello");

    let mut g = GrowingFile::new(&dest, 10);
    let h = g.handle();
    let mut buf = [0u8; 16];
    assert_eq!(g.read(&mut buf).unwrap(), 5);
    assert_eq!(h.buffered_bytes(), 5);
    assert_eq!(g.byte_len(), Some(10));

    let p2 = part.clone();
    let d2 = dest.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        append(&p2, b"world");
        std::fs::rename(&p2, &d2).unwrap();
    });
    let t = Instant::now();
    assert_eq!(g.read(&mut buf).unwrap(), 5);
    assert!(
        t.elapsed() >= Duration::from_millis(100),
        "read should have waited"
    );
    assert_eq!(&buf[..5], b"world");
    assert_eq!(g.read(&mut buf).unwrap(), 0, "EOF at total size");
    writer.join().unwrap();
    assert!(h.is_complete());

    // Seek back and re-read across the (renamed) file.
    g.seek(SeekFrom::Start(3)).unwrap();
    let mut all = Vec::new();
    g.read_to_end(&mut all).unwrap();
    assert_eq!(all, b"loworld");
}

#[test]
fn cancel_unblocks_a_waiting_read() {
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("b.mp3.part");
    File::create(&part).unwrap();
    let mut g = GrowingFile::new(&part, 100);
    let h = g.handle();
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 8];
        let e = g.read(&mut buf).unwrap_err();
        GrowingError::from_io(&e).cloned()
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(h.is_waiting());
    let c = Instant::now();
    h.cancel();
    assert_eq!(t.join().unwrap(), Some(GrowingError::Cancelled));
    assert!(c.elapsed() < Duration::from_millis(100));
}

#[test]
fn interrupt_and_seek_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("c.flac");
    append(&dir.path().join("c.flac.part"), &[7u8; 40]);
    let mut g = GrowingFile::new(&dest, 100);
    let h = g.handle();
    let mut buf = [0u8; 64];
    assert_eq!(g.read(&mut buf).unwrap(), 40);

    // Seeking past the buffered range is lazy; the read waits, and interrupt releases it.
    assert_eq!(g.seek(SeekFrom::Start(90)).unwrap(), 90);
    h.interrupt();
    let e = g.read(&mut buf).unwrap_err();
    assert_eq!(GrowingError::from_io(&e), Some(&GrowingError::Interrupted));
    h.clear_interrupt();

    // End-relative seeks: total size normally, buffered end while probing a seek.
    assert_eq!(g.seek(SeekFrom::End(0)).unwrap(), 100);
    h.set_end_at_buffered(true);
    assert_eq!(g.seek(SeekFrom::End(-10)).unwrap(), 30);
    h.set_end_at_buffered(false);

    // Length is hidden only while probing an incomplete file.
    h.set_hide_len(true);
    assert_eq!(g.byte_len(), None);
    h.set_hide_len(false);
    assert_eq!(g.byte_len(), Some(100));
    assert!(g.seek(SeekFrom::Current(-1000)).is_err());
}

#[test]
fn missing_file_times_out_with_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut g = GrowingFile::new(dir.path().join("nope.flac"), 10)
        .with_missing_timeout(Duration::from_millis(80));
    let mut buf = [0u8; 4];
    let e = g.read(&mut buf).unwrap_err();
    assert!(matches!(
        GrowingError::from_io(&e),
        Some(GrowingError::Missing(_))
    ));
    assert!(e.to_string().contains("missing"), "{e}");
}

#[test]
fn discarded_download_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("d.flac.part");
    append(&part, b"abc");
    let mut g = GrowingFile::new(&part, 10);
    let mut buf = [0u8; 8];
    assert_eq!(g.read(&mut buf).unwrap(), 3);
    std::fs::remove_file(&part).unwrap();
    let e = g.read(&mut buf).unwrap_err();
    assert!(matches!(
        GrowingError::from_io(&e),
        Some(GrowingError::Missing(_))
    ));
}
