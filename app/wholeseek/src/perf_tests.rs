//! Long lists: the `big-share` measurements. Ignored by default (timings need `--release`):
//!
//! `PERF_LABEL=after cargo test -p wholeseek --release -- --ignored --nocapture long_list_timings`
//!
//! Each operation's frames are timed (view, layout and scene of the headless runtime; the GPU
//! raster isn't included), along with the derived data the view asks for per run, and the
//! element count. A table goes to stdout and to `proof/long-lists/<PERF_LABEL>.md`.

use std::time::{Duration, Instant};

use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::app::App;
use crate::demo_big::{BIG_DUMP, BIG_FOLDER, BIG_USER};
use crate::{H, W, poll, view};

fn rt() -> Runtime<App> {
    let mut rt = Runtime::new(
        App::demo(),
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.state.demo_login();
    settle(&mut rt, 60);
    rt
}

fn settle(rt: &mut Runtime<App>, frames: usize) {
    for _ in 0..frames {
        rt.frame(1.0 / 60.0);
    }
}

/// Frame times of one operation.
#[derive(Default)]
struct Op {
    name: String,
    frames: Vec<Duration>,
    elements: usize,
}

impl Op {
    fn new(name: impl Into<String>) -> Self {
        Op {
            name: name.into(),
            ..Op::default()
        }
    }
    fn frame(&mut self, rt: &mut Runtime<App>) {
        let t = Instant::now();
        rt.frame(1.0 / 60.0);
        self.frames.push(t.elapsed());
    }
    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }
    fn max(&self) -> f64 {
        self.frames
            .iter()
            .copied()
            .map(Self::ms)
            .fold(0.0, f64::max)
    }
    fn mean(&self) -> f64 {
        self.frames.iter().copied().map(Self::ms).sum::<f64>() / self.frames.len().max(1) as f64
    }
    fn p95(&self) -> f64 {
        let mut v: Vec<f64> = self.frames.iter().copied().map(Self::ms).collect();
        v.sort_by(f64::total_cmp);
        v.get((v.len() * 95 / 100).min(v.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0)
    }
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed().as_secs_f64() * 1000.0)
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no {key}"))
        .rect
}

/// Rows with keys starting `prefix` that are built.
fn built(rt: &Runtime<App>, prefix: &str) -> usize {
    rt.elements()
        .into_iter()
        .filter(|e| {
            e.key
                .as_deref()
                .and_then(|k| k.strip_prefix(prefix))
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count()
}

/// `view::files::ROW_H`.
const FILE_ROW_H: f32 = 28.0;

/// The viewport and Rune's overscan, two viewports either side, in rows of `row_h`.
fn bound(rt: &Runtime<App>, key: &str, row_h: f32) -> usize {
    (5.0 * rect(rt, key)[3] / row_h).ceil() as usize + 2
}

#[test]
fn a_60000_file_share_builds_only_the_rows_in_view() {
    let mut rt = rt();
    rt.update(|s, _| s.open_folder_at(BIG_USER, BIG_DUMP, None));
    settle(&mut rt, 60);
    rt.update(|s, _| {
        s.run_menu(crate::menu::MenuAction::ExpandAll("Music".into()));
    });
    settle(&mut rt, 30);
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.visible_tree().len(), 6_022);
    assert_eq!(b.selected_dir().unwrap().files.len(), 10_000);
    let (tree, list) = (rt.state.tab().tree_key(), rt.state.tab().list_key());
    for round in 0..2 {
        let (dirs, files) = (built(&rt, "dir-"), built(&rt, "file-"));
        let (dmax, fmax) = (
            bound(&rt, &tree, crate::tabs::TREE_ROW_H),
            bound(&rt, &list, FILE_ROW_H),
        );
        assert!(
            (10..=dmax).contains(&dirs),
            "round {round}: {dirs} tree rows, at most {dmax}"
        );
        assert!(
            (10..=fmax).contains(&files),
            "round {round}: {files} file rows, at most {fmax}"
        );
        // Far down both lists, then again.
        let (t, l) = (rect(&rt, &tree), rect(&rt, &list));
        for _ in 0..30 {
            rt.wheel(t[0] + 40.0, t[1] + 40.0, -2_000.0);
            rt.wheel(l[0] + 40.0, l[1] + 40.0, -2_000.0);
            rt.frame(1.0 / 60.0);
        }
        settle(&mut rt, 30);
    }
    assert!(rt.state.tab().tree_scroll.get() > 10_000.0);
    assert!(rt.state.tab().scroll.get() > 10_000.0);
}

#[test]
#[ignore = "timings: run with --release -- --ignored --nocapture"]
fn long_list_timings() {
    let mut ops = Vec::new();
    let mut rt = rt();

    // Open the 5k-folder share at a deep album: every frame until it's listed and revealed.
    let mut op = Op::new("open the share (5,001 folders)");
    rt.update(|s, _| s.open_folder_at(BIG_USER, BIG_FOLDER, None));
    for _ in 0..90 {
        op.frame(&mut rt);
    }
    assert!(rt.state.active_browse().unwrap().dirs.is_some());
    op.elements = rt.elements().len();
    ops.push(op);

    // Expand all from the top: ~6,000 tree rows.
    let mut op = Op::new("expand all (6,022 rows)");
    rt.update(|s, _| {
        s.run_menu(crate::menu::MenuAction::ExpandAll("Music".into()));
    });
    for _ in 0..10 {
        op.frame(&mut rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);

    // Scroll the tree: a wheel step a frame.
    let tree = rt.state.tab().tree_key();
    let t = rect(&rt, &tree);
    let (tx, ty) = (t[0] + t[2] / 2.0, t[1] + t[3] / 2.0);
    let mut op = Op::new("scroll the tree (wheel, 120 frames)");
    for i in 0..120 {
        rt.wheel(tx, ty, if i < 90 { -120.0 } else { 120.0 });
        op.frame(&mut rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);

    // Hover down the tree: the pointer on a new row each frame.
    let mut op = Op::new("hover tree rows (120 frames)");
    for i in 0..120 {
        rt.pointer_move(tx, t[1] + 8.0 + (i % 20) as f32 * 24.0);
        op.frame(&mut rt);
    }
    ops.push(op);

    // Click a few rows (select / open a folder).
    let mut op = Op::new("click a tree row (x6)");
    for i in 0..6 {
        let y = t[1] + 20.0 + i as f32 * 48.0;
        rt.pointer_down(tx, y);
        rt.pointer_up(tx, y);
        for _ in 0..3 {
            op.frame(&mut rt);
        }
    }
    ops.push(op);

    // Type in the tree's filter, a letter a frame, then clear it.
    rt.update(|_, tx| tx.focus("tree-find", false));
    settle(&mut rt, 2);
    let mut op = Op::new("type in the folder filter (\"velvet\")");
    for c in "velvet".chars() {
        rt.key(KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        });
        for _ in 0..2 {
            op.frame(&mut rt);
        }
    }
    op.elements = rt.elements().len();
    for _ in 0..6 {
        rt.key(KeyEvent {
            key: Key::Backspace,
            mods: Mods::default(),
        });
        op.frame(&mut rt);
    }
    ops.push(op);

    // The 10,000-file folder: open it, scroll its list, hover its rows.
    let mut op = Op::new("open a 10,000-file folder");
    rt.update(|s, _| s.active_browse_mut().unwrap().select(BIG_DUMP));
    for _ in 0..10 {
        op.frame(&mut rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);
    let list = [tx + 500.0, ty];
    let mut op = Op::new("scroll the file list (120 frames)");
    for i in 0..120 {
        rt.wheel(list[0], list[1], if i < 90 { -120.0 } else { 120.0 });
        op.frame(&mut rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);
    let mut op = Op::new("hover file rows (120 frames)");
    for i in 0..120 {
        rt.pointer_move(list[0], 260.0 + (i % 14) as f32 * 28.0);
        op.frame(&mut rt);
    }
    ops.push(op);

    search_ops(
        &mut rt,
        &mut ops,
        crate::tabs::ResultsLayout::Albums,
        "albums",
    );
    search_ops(
        &mut rt,
        &mut ops,
        crate::tabs::ResultsLayout::Folders,
        "folders",
    );

    // What the view derives from the share each time it runs (one call each).
    rt.update(|s, _| s.open_folder_at(BIG_USER, BIG_DUMP, None));
    settle(&mut rt, 30);
    let s = &rt.state;
    let b = s.active_browse().unwrap();
    let (rows, t_tree) = time(|| b.visible_tree().len());
    let (_, t_row) = time(|| b.tree_row_of_selected());
    let (_, t_sel) = time(|| b.selected_dir().map(|d| d.files.len()));
    let (n, t_files) = time(|| s.folder_files(BIG_USER, BIG_DUMP).len());
    let parts = [
        (format!("visible_tree() ({rows} rows)"), t_tree),
        ("tree_row_of_selected()".into(), t_row),
        ("selected_dir()".into(), t_sel),
        (format!("folder_files() ({n} files, sorted)"), t_files),
    ];

    let label = std::env::var("PERF_LABEL").unwrap_or_else(|_| "run".into());
    let mut out = format!(
        "# Long lists: {label}\n\n`big-share`: {BIG_USER}, 5,001 folders, 60,000 files; window {W}x{H} at 1x. \
         Frame = view + layout + scene of the headless runtime ({} build; no GPU raster).\n\n\
         | operation | frames | mean ms | p95 ms | max ms | elements |\n|---|---:|---:|---:|---:|---:|\n",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    for op in &ops {
        out += &format!(
            "| {} | {} | {:.2} | {:.2} | {:.2} | {} |\n",
            op.name,
            op.frames.len(),
            op.mean(),
            op.p95(),
            op.max(),
            if op.elements > 0 {
                op.elements.to_string()
            } else {
                String::new()
            }
        );
    }
    out += "\n| derived per view run | ms |\n|---|---:|\n";
    for (name, ms) in &parts {
        out += &format!("| {name} | {ms:.2} |\n");
    }
    println!("{out}");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proof/long-lists");
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(format!("{dir}/{label}.md"), &out);
    }
}

/// A search in `layout` gets 10,000 hits in 1,000 folders in one batch; then scroll and hover
/// them, and type in the results filter.
fn search_ops(
    rt: &mut Runtime<App>,
    ops: &mut Vec<Op>,
    layout: crate::tabs::ResultsLayout,
    name: &str,
) {
    rt.update(move |s, _| {
        s.last_filters.layout = layout;
        s.new_search_tab();
        s.search_for("hollow lights");
        if let Some(st) = s.tab_mut().search_mut() {
            st.filters.layout = layout;
        }
    });
    settle(rt, 90);
    let id = rt.state.tab().search().unwrap().search.as_ref().unwrap().id;
    search_hits(rt, ops, id, name);
}

fn search_hits(rt: &mut Runtime<App>, ops: &mut Vec<Op>, id: wholeseek_core::SearchId, name: &str) {
    let hits: Vec<_> = crate::demo_big::share()
        .into_iter()
        .take(1_000)
        .flat_map(|d| d.files)
        .map(|file| wholeseek_core::SearchHit {
            file,
            free_slot: true,
            speed: 2_000_000,
        })
        .collect();
    let mut op = Op::new(format!("{name}: 10,000 search hits arrive"));
    rt.update(move |s, _| s.apply(wholeseek_core::CoreEvent::SearchResults { search: id, hits }));
    for _ in 0..10 {
        op.frame(rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);
    let rows = rt
        .elements()
        .into_iter()
        .filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("row-")))
        .count();
    println!("{name}: {rows} file rows built");
    let list = rect(rt, &rt.state.tab().list_key());
    let (lx, ly) = (list[0] + 200.0, list[1] + list[3] / 2.0);
    let mut op = Op::new(format!("{name}: scroll the results (120 frames)"));
    for i in 0..120 {
        rt.wheel(lx, ly, if i < 90 { -120.0 } else { 120.0 });
        op.frame(rt);
    }
    op.elements = rt.elements().len();
    ops.push(op);
    let mut op = Op::new(format!("{name}: hover result rows (120 frames)"));
    for i in 0..120 {
        rt.pointer_move(lx, list[1] + 14.0 + (i % 14) as f32 * 28.0);
        op.frame(rt);
    }
    ops.push(op);
    rt.update(|_, tx| tx.focus("find", false));
    settle(rt, 2);
    let mut op = Op::new(format!("{name}: type in the results filter (\"static\")"));
    for c in "static".chars() {
        rt.key(KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        });
        for _ in 0..2 {
            op.frame(rt);
        }
    }
    op.elements = rt.elements().len();
    ops.push(op);
}
