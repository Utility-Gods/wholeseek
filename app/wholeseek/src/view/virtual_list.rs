//! Long lists (10k+ rows) that stay smooth: only the rows in view are built, plus Rune's
//! overscan (the mounted window is the viewport and at most two viewports either side, filled
//! a little per frame), at one fixed row height, so the scrollbar and positions are exact
//! without measuring anything.
//!
//! ```ignore
//! // In the state: one handle per list, kept across view runs (it remembers the scroll).
//! pub songs_list: ListScroll,                      // ListScroll::new("songs")
//!
//! // In the view:
//! virtual_list(&s.songs_list, s.songs.len(), 28.0, |s: &App, i| song_row(s, &s.songs[i]))
//!     .row_key(|s: &App, i| s.songs[i].id.as_str().into())   // stable identity per row
//!     .version(s.songs_rev)                                  // bump when rows change in place
//!     .build()
//!
//! // From a handler (keyboard, "show in list", a restore):
//! s.songs_list.scroll_to_index(tx, i, Reveal::Center);
//! ```
//!
//! Row handlers must find their item by key or id, never by the index they were built with
//! (Rune may keep a built row while rows are inserted above it).

// The whole API is for the Library too; the user tab uses part of it so far.
#![allow(dead_code)]

use std::borrow::Cow;
use std::cell::Cell;
use std::hash::Hash;
use std::rc::Rc;

use rune::{El, GrowthSpec, ScrollPos, ScrollSpec, Spring, StreamEdge, Tx, col, edge};

/// Where [`ListScroll::scroll_to_index`] puts the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Centred in the viewport (a deep link, a reveal).
    Center,
    /// Only as far as it takes to bring it in view; nothing moves if it is (keyboard steps).
    Nearest,
    /// At the top.
    Top,
}

/// A list's scroll handle: its key, where it's scrolled to, and its viewport. Cheap to clone
/// (shared cells); keep one per list in the state.
#[derive(Clone, Debug)]
pub struct ListScroll {
    key: Rc<str>,
    offset: Rc<Cell<f32>>,
    view_h: Rc<Cell<f32>>,
    row_h: Rc<Cell<f32>>,
    pad: Rc<Cell<f32>>,
}

impl ListScroll {
    pub fn new(key: impl Into<Rc<str>>) -> Self {
        ListScroll {
            key: key.into(),
            offset: Rc::default(),
            view_h: Rc::default(),
            row_h: Rc::new(Cell::new(1.0)),
            pad: Rc::default(),
        }
    }

    /// A handle over cells the state already keeps (its offset and viewport height), for a
    /// list whose scroll other code saves and restores.
    pub fn sharing(key: impl Into<Rc<str>>, offset: Rc<Cell<f32>>, view_h: Rc<Cell<f32>>) -> Self {
        ListScroll {
            offset,
            view_h,
            ..Self::new(key)
        }
    }

    /// The scroll viewport's key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// How far the list is scrolled (as of its last layout).
    pub fn offset(&self) -> f32 {
        self.offset.get()
    }

    /// The viewport's height (as of its last layout; 0 before the first).
    pub fn view_h(&self) -> f32 {
        self.view_h.get()
    }

    /// The rows wholly or partly in view, as of the last layout.
    pub fn visible(&self, count: usize) -> std::ops::Range<usize> {
        let (h, pad) = (self.row_h.get(), self.pad.get());
        let top = ((self.offset() - pad) / h).floor().max(0.0) as usize;
        let bottom = ((self.offset() + self.view_h() - pad) / h).ceil().max(0.0) as usize;
        top.min(count)..bottom.min(count)
    }

    /// Scroll so row `index` shows, at once (no glide). The row needn't be built: with one row
    /// height its place is known.
    pub fn scroll_to_index(&self, tx: &mut Tx, index: usize, how: Reveal) {
        if let Some(y) = self.offset_for(index, how) {
            tx.scroll_to_instant(self.key.to_string(), 0.0, y);
            self.offset.set(y);
        }
    }

    /// The offset [`Self::scroll_to_index`] goes to; `None` when nothing needs to move.
    pub fn offset_for(&self, index: usize, how: Reveal) -> Option<f32> {
        let (h, pad, view) = (self.row_h.get(), self.pad.get(), self.view_h.get());
        let top = pad + index as f32 * h;
        let y = match how {
            Reveal::Top => top,
            Reveal::Center => top - (view - h) / 2.0,
            Reveal::Nearest => {
                let at = self.offset();
                if top < at {
                    top
                } else if top + h > at + view {
                    top + h - view
                } else {
                    return None;
                }
            }
        };
        Some(y.max(0.0))
    }

    /// Put the list back where it was (after a tab switch rebuilt it).
    pub fn restore(&self, tx: &mut Tx) {
        tx.scroll_to_instant(self.key.to_string(), 0.0, self.offset());
    }
}

/// A list of `count` rows `row_h` points tall, row `i` built by `row(s, i)` only while it's in
/// the mounted window. See the module docs.
pub fn virtual_list<S: 'static>(
    scroll: &ListScroll,
    count: usize,
    row_h: f32,
    row: impl Fn(&S, usize) -> El + 'static,
) -> VirtualList<S> {
    VirtualList {
        scroll: scroll.clone(),
        count,
        row_h,
        pad: 0.0,
        row: Box::new(row),
        key: None,
        version: None,
    }
}

type KeyFn<S> = Box<dyn for<'a> Fn(&'a S, usize) -> Cow<'a, str>>;
type RowFn<S> = Box<dyn Fn(&S, usize) -> El>;

pub struct VirtualList<S> {
    scroll: ListScroll,
    count: usize,
    row_h: f32,
    pad: f32,
    row: RowFn<S>,
    key: Option<KeyFn<S>>,
    version: Option<u64>,
}

impl<S: 'static> VirtualList<S> {
    /// Each row's identity (a path, an id). Without one, rows are keyed by index, which is
    /// right only for a list that never changes in place.
    pub fn row_key(mut self, key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static) -> Self {
        self.key = Some(Box::new(key));
        self
    }

    /// The rows' revision: while it and the count stay, Rune skips asking every row for its
    /// key (a 60k list commits in time independent of its length). Change it whenever rows
    /// are reordered or replaced in place.
    pub fn version(mut self, v: impl Hash) -> Self {
        let mut h = std::hash::DefaultHasher::new();
        v.hash(&mut h);
        self.version = Some(std::hash::Hasher::finish(&h));
        self
    }

    /// Room above the first row and below the last.
    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }

    /// The scroll viewport (keyed [`ListScroll::key`]) holding the rows; give it a size (it
    /// grows to fill by default).
    pub fn build(self) -> El {
        let VirtualList {
            scroll,
            count,
            row_h,
            pad,
            row,
            key,
            version,
        } = self;
        scroll.row_h.set(row_h);
        scroll.pad.set(pad);
        let key: KeyFn<S> = key.unwrap_or_else(|| Box::new(|_, i| Cow::Owned(format!("r{i}"))));
        // Rows land at once: no growth spring, no feather, no word fade.
        let still = GrowthSpec {
            spring: Spring::NONE,
            edge: StreamEdge {
                word_fade: 0.0,
                ..edge::NO_MASK
            },
        };
        let mut rows =
            col()
                .w_full()
                .growth(still)
                .virtual_rows(count, row_h, key, move |s: &S, i| row(s, i).h(row_h));
        if let Some(v) = version {
            rows = rows.virtual_rows_version(v);
        }
        let (offset, view_h) = (scroll.offset.clone(), scroll.view_h.clone());
        col()
            .key(scroll.key.to_string())
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .py(pad)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .on_scroll_offset(move |at: ScrollPos| {
                offset.set(at.offset[1]);
                view_h.set(at.viewport[1]);
            })
            .child(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, text};

    struct Long {
        names: Vec<String>,
        list: ListScroll,
    }

    const ROW: f32 = 28.0;
    const VIEW_H: f32 = 600.0;

    fn view(s: &Long, _: &mut rune::Cx<Long>) -> El {
        col().w_full().h_full().child(
            virtual_list(&s.list, s.names.len(), ROW, |s: &Long, i| {
                rune::row().w_full().child(text(s.names[i].clone()))
            })
            .row_key(|s: &Long, i| s.names[i].as_str().into())
            .version(s.names.len())
            .build(),
        )
    }

    fn long(n: usize) -> Runtime<Long> {
        let s = Long {
            names: (0..n).map(|i| format!("song {i:05}")).collect(),
            list: ListScroll::new("songs"),
        };
        let mut rt = Runtime::new(
            s,
            view,
            (400.0, VIEW_H),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        settle(&mut rt);
        rt
    }

    fn settle(rt: &mut Runtime<Long>) {
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
    }

    /// The built rows' texts.
    fn built(rt: &Runtime<Long>) -> Vec<String> {
        rt.elements()
            .into_iter()
            .filter_map(|e| e.text.filter(|t| t.starts_with("song ")))
            .collect()
    }

    /// At most the viewport and Rune's overscan of two viewports either side.
    fn assert_bounded(rt: &Runtime<Long>) {
        let rows = built(rt).len();
        let bound = (5.0 * VIEW_H / ROW).ceil() as usize + 2;
        assert!(rows <= bound, "{rows} rows built, at most {bound}");
        assert!(
            rows >= (VIEW_H / ROW) as usize,
            "the viewport is filled: {rows}"
        );
    }

    #[test]
    fn only_the_rows_in_view_are_built_of_60000() {
        let mut rt = long(60_000);
        assert_bounded(&rt);
        assert!(built(&rt).contains(&"song 00000".to_string()));
        // The scroll extent is the whole list's.
        let content = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("songs"))
            .unwrap();
        assert!(content.rect[3] <= VIEW_H + 0.5);
        for _ in 0..40 {
            rt.wheel(200.0, 300.0, -400.0);
            rt.frame(1.0 / 60.0);
            assert_bounded(&rt);
        }
        assert!(rt.state.list.offset() > 1000.0);
    }

    #[test]
    fn scroll_to_index_centres_a_row_that_was_never_built() {
        let mut rt = long(60_000);
        rt.update(|s, tx| s.list.scroll_to_index(tx, 42_000, Reveal::Center));
        settle(&mut rt);
        let want = 42_000.0 * ROW - (VIEW_H - ROW) / 2.0;
        assert!(
            (rt.state.list.offset() - want).abs() < 1.0,
            "{} vs {want}",
            rt.state.list.offset()
        );
        let row = rt
            .elements()
            .into_iter()
            .find(|e| e.text.as_deref() == Some("song 42000"))
            .expect("built once in view");
        let mid = row.rect[1] + row.rect[3] / 2.0;
        assert!((mid - VIEW_H / 2.0).abs() < 2.0, "centred: {mid}");
        assert_bounded(&rt);
        // Nearest: nothing moves for a row in view; one below lands at the bottom edge.
        rt.update(|s, tx| s.list.scroll_to_index(tx, 42_001, Reveal::Nearest));
        settle(&mut rt);
        assert!((rt.state.list.offset() - want).abs() < 1.0);
        rt.update(|s, tx| s.list.scroll_to_index(tx, 42_030, Reveal::Nearest));
        settle(&mut rt);
        let bottom = 42_031.0 * ROW - VIEW_H;
        assert!((rt.state.list.offset() - bottom).abs() < 1.0);
        assert_eq!(rt.state.list.visible(60_000).end, 42_031);
    }
}
