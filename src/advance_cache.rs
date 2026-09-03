//! A cache of grapheme-cluster advance widths, used to find how much of a
//! line of text is visible — and, eventually, how many rows it wraps to —
//! without shaping the whole line. See
//! `docs/wide-line-rendering-resources.md` for the background reading.
//!
//! Real text shaping (kerning, ligatures, bidi, contextual glyph forms) is
//! genuinely expensive: on this project's own log data, shaping one
//! 11,000-character line costs over a millisecond, and a scrolling table
//! rebuilds roughly a hundred rows on every frame — most of that text never
//! on screen. iced's `text`/`rich_text` widgets have no way to shape only a
//! visible slice; they shape whatever string they're handed.
//!
//! The way out: this app only ever renders Hit text in a monospace font, and
//! monospace fonts don't kern — each glyph (or, for a combined character like
//! an accent or an emoji, each grapheme cluster) advances the cursor by a
//! fixed amount regardless of its neighbours. So the width of any run of text
//! is the sum of its grapheme clusters' individual advances, and those can be
//! measured once and cached, then summed with plain arithmetic instead of
//! reshaping on every frame.
//!
//! The one case this doesn't hold: cursive/joining scripts (Arabic, Syriac,
//! ...), where a letter's shape — and width — depends on its neighbours.
//! Text in those scripts sizes slightly wrong under this cache. That's judged
//! an acceptable, self-correcting-per-line approximation for log data rather
//! than a correctness bug to chase — see the resources doc for the
//! measurements behind that call.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::Font;
use iced::font::Family;
use unicode_segmentation::UnicodeSegmentation;

/// Per-`(font, size)` cache of grapheme-cluster advance widths, in pixels.
pub struct AdvanceCache {
    font: Font,
    size: f32,
    // Keyed by the grapheme cluster's text. Entries are never mutated after
    // insertion, so `Box<str>` rather than `String`. A `Mutex` rather than a
    // `RefCell`: this needs to be `Sync` to live in a `static` (see
    // `shared`), even though iced only ever calls into it from its one UI
    // thread — contention is never real, so the lock is never a bottleneck.
    widths: Mutex<HashMap<Box<str>, f32>>,
}

impl AdvanceCache {
    pub fn new(font: Font, size: f32) -> Self {
        Self {
            font,
            size,
            widths: Mutex::new(HashMap::new()),
        }
    }

    /// The shared cache for this app's one cell-text `(font, size)` pair —
    /// `Font::MONOSPACE` at [`crate::results::CELL_TEXT_SIZE`]. If cell text
    /// ever needs a second font or size, this can grow into a small registry
    /// keyed on both; today there's exactly one, so a single static keeps
    /// call sites simple.
    pub fn shared() -> &'static AdvanceCache {
        static CACHE: OnceLock<AdvanceCache> = OnceLock::new();
        CACHE.get_or_init(|| AdvanceCache::new(Font::MONOSPACE, crate::results::CELL_TEXT_SIZE))
    }

    /// The longest prefix of `text` whose shaped width is no more than
    /// `max_width`, as a byte length (always on a grapheme-cluster boundary,
    /// so it's always a valid `&str` slice point) plus the pixel width that
    /// prefix takes up. `max_width` of `f32::INFINITY` returns the whole
    /// string.
    ///
    /// Takes the cache lock once for the whole scan rather than per grapheme:
    /// the windowed row loops call this once per visible cell (or, in raw text
    /// mode, once over a viewport-wide slice \u{2248} hundreds of graphemes)
    /// every frame, and the lock is never actually contended (one UI thread).
    pub fn take_width(&self, text: &str, max_width: f32) -> (usize, f32) {
        let mut widths = self.widths.lock().expect("lock advance cache");
        let mut consumed = 0.0f32;
        for (byte_idx, grapheme) in text.grapheme_indices(true) {
            let w = match widths.get(grapheme) {
                Some(&w) => w,
                None => {
                    let w = self.shape_advance(grapheme);
                    widths.insert(grapheme.into(), w);
                    w
                }
            };
            if consumed + w > max_width {
                return (byte_idx, consumed);
            }
            consumed += w;
        }
        (text.len(), consumed)
    }

    /// How many visual rows `text` wraps to at `width` pixels under glyph
    /// wrapping — break anywhere a grapheme would overflow, the
    /// [`cosmic_text::Wrap::Glyph`] model iced uses for
    /// [`iced::widget::text::Wrapping::Glyph`] — plus whether the count was
    /// clamped at `max_rows`. Empty text is one row; an embedded `\n` (or
    /// `\r\n`) forces a break. `width <= 0` yields `(1, false)`.
    ///
    /// Same trade as [`take_width`](Self::take_width): summed grapheme
    /// advances instead of shaping the whole line, one lock for the scan. The
    /// row-height model calls this per visible Hit every frame the window
    /// moves (see [`crate::line::LineCache`]); off-screen Hits use a cheaper
    /// byte-length estimate.
    pub fn wrap_rows(&self, text: &str, width: f32, max_rows: u32) -> (u32, bool) {
        if width <= 0.0 {
            return (1, false);
        }
        let mut widths = self.widths.lock().expect("lock advance cache");
        let mut rows = 1u32;
        let mut consumed = 0.0f32;
        for grapheme in text.graphemes(true) {
            if grapheme == "\n" || grapheme == "\r\n" {
                rows += 1;
                if rows > max_rows {
                    return (max_rows, true);
                }
                consumed = 0.0;
                continue;
            }
            let w = match widths.get(grapheme) {
                Some(&w) => w,
                None => {
                    let w = self.shape_advance(grapheme);
                    widths.insert(grapheme.into(), w);
                    w
                }
            };
            if consumed > 0.0 && consumed + w > width {
                rows += 1;
                if rows > max_rows {
                    return (max_rows, true);
                }
                consumed = w;
            } else {
                consumed += w;
            }
        }
        (rows, false)
    }

    /// The advance width of a single plain monospace character, in pixels —
    /// the width every ASCII cell char takes in this font. Used as an O(1)
    /// per-line width estimate (byte length \u{d7} this) for raw text mode's
    /// horizontal scrollbar extent, where shaping every full line just to size
    /// the scrollbar would defeat the point of the cache.
    pub fn mono_advance(&self) -> f32 {
        self.take_width("x", f32::INFINITY).1
    }

    /// Shapes `grapheme` alone through iced's own global font system — the
    /// same one its `text`/`rich_text` widgets use — and reads back the
    /// advance width. Always `Shaping::Advanced`: a single grapheme is cheap
    /// to shape either way (this isn't the cost `take_width` exists to
    /// avoid), and Advanced is the strategy iced itself falls back to for
    /// anything non-ASCII, so cached widths match what actually renders.
    fn shape_advance(&self, grapheme: &str) -> f32 {
        use cosmic_text::{Attrs, Buffer, Metrics, Shaping};

        let font_system = iced::advanced::graphics::text::font_system();
        let mut font_system = font_system.write().expect("lock font system");
        let raw = font_system.raw();

        // Line height doesn't affect a run's measured width; any value works.
        let metrics = Metrics::new(self.size, self.size * 1.3);
        let mut buffer = Buffer::new(raw, metrics);
        buffer.set_size(raw, None, None);
        let attrs = Attrs::new().family(to_cosmic_family(self.font.family));
        buffer.set_text(raw, grapheme, &attrs, Shaping::Advanced, None);

        let (size, _has_rtl) = iced::advanced::graphics::text::measure(&buffer);
        size.width
    }
}

/// Maps only the font *family* — the one attribute cell text actually varies
/// on today (always `Font::MONOSPACE`, default weight/style/stretch). Mirrors
/// iced_graphics's own (private) `to_family`.
fn to_cosmic_family(family: Family) -> cosmic_text::Family<'static> {
    match family {
        Family::Name(name) => cosmic_text::Family::Name(name),
        Family::Serif => cosmic_text::Family::Serif,
        Family::SansSerif => cosmic_text::Family::SansSerif,
        Family::Cursive => cosmic_text::Family::Cursive,
        Family::Fantasy => cosmic_text::Family::Fantasy,
        Family::Monospace => cosmic_text::Family::Monospace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> AdvanceCache {
        AdvanceCache::new(Font::MONOSPACE, 12.0)
    }

    #[test]
    fn empty_string_takes_nothing() {
        assert_eq!(cache().take_width("", 100.0), (0, 0.0));
    }

    #[test]
    fn whole_string_fits_under_a_generous_budget() {
        let s = "GET /api/v1/orders HTTP/1.1 200";
        let (len, _) = cache().take_width(s, 10_000.0);
        assert_eq!(len, s.len());
    }

    #[test]
    fn zero_budget_takes_nothing() {
        assert_eq!(cache().take_width("hello", 0.0), (0, 0.0));
    }

    #[test]
    fn truncates_partway_through_a_long_line_on_a_char_boundary() {
        let cache = cache();
        let s = "x".repeat(1000);
        let (len, width) = cache.take_width(&s, 100.0);
        assert!(len > 0 && len < s.len());
        assert!(s.is_char_boundary(len));
        // Consumed width never exceeds the budget...
        assert!(width <= 100.0);
        // ...but it's tight: one more character would have overflowed it.
        let (_, one_more) = cache.take_width(&s[..len + 1], f32::INFINITY);
        assert!(one_more > 100.0);
    }

    #[test]
    fn never_splits_a_grapheme_cluster() {
        // A base letter + combining accent is one grapheme cluster ("é"); a
        // budget that only fits the base letter should drop the whole
        // cluster, not slice inside it.
        let s = "e\u{0301}x";
        let (len, _) = cache().take_width(s, 0.1);
        assert_eq!(len, 0);
    }

    #[test]
    fn wrap_rows_short_line_is_one_row() {
        let cache = cache();
        assert_eq!(cache.wrap_rows("GET /orders 200", 1000.0, 400), (1, false));
        assert_eq!(cache.wrap_rows("", 1000.0, 400), (1, false));
    }

    #[test]
    fn wrap_rows_counts_a_long_line() {
        let cache = cache();
        let adv = cache.mono_advance();
        let line = "x".repeat(500);
        let (rows, clamped) = cache.wrap_rows(&line, 100.0, 400);
        assert!(!clamped);
        // Greedy glyph wrap: each row holds as many whole chars as fit under
        // 100px, so the count is 500 / (chars per row), rounded up.
        let per_row = (100.0 / adv).floor() as u32;
        assert_eq!(rows, 500_u32.div_ceil(per_row));
    }

    #[test]
    fn wrap_rows_breaks_on_newlines() {
        let cache = cache();
        assert_eq!(cache.wrap_rows("a\nb\nc\nd", 1000.0, 400), (4, false));
        // `\r\n` is one grapheme cluster — still one break.
        assert_eq!(cache.wrap_rows("a\r\nb", 1000.0, 400), (2, false));
    }

    #[test]
    fn wrap_rows_clamps_at_max_rows() {
        let cache = cache();
        let line = "y".repeat(5000);
        let (rows, clamped) = cache.wrap_rows(&line, 50.0, 8);
        assert_eq!(rows, 8);
        assert!(clamped);
    }

    #[test]
    fn wrap_rows_never_splits_a_grapheme_cluster() {
        let cache = cache();
        // Each "e\u{0301}" is one cluster; a width fitting ~2 of them must
        // still land every cluster whole (row count stays consistent with a
        // plain 2-per-row split of 6 clusters).
        let s = "e\u{0301}".repeat(6);
        let two_wide = cache.mono_advance() * 2.5;
        let (rows, _) = cache.wrap_rows(&s, two_wide, 400);
        assert_eq!(rows, 3);
    }

    #[test]
    fn wrap_rows_matches_cosmic_text_glyph_wrap() {
        use cosmic_text::{Attrs, Buffer, Metrics, Shaping, Wrap};

        let cache = cache();
        let json_blob = format!("{{\"k\":\"{}\"}}", "v".repeat(900));
        let samples = [
            "short line",
            &"abcdefghij ".repeat(40),
            &"/api/v1/orders?id=1234&trace=abcdef0123456789 ".repeat(12),
            &json_blob,
        ];
        for (i, text) in samples.iter().enumerate() {
            for width in [180.0f32, 420.0, 900.0] {
                let (ours, _) = cache.wrap_rows(text, width, 4000);

                let font_system = iced::advanced::graphics::text::font_system();
                let mut fs = font_system.write().unwrap();
                let raw = fs.raw();
                let metrics = Metrics::new(12.0, 12.0 * 1.3);
                let mut buffer = Buffer::new(raw, metrics);
                buffer.set_wrap(raw, Wrap::Glyph);
                buffer.set_size(raw, Some(width), None);
                buffer.set_text(
                    raw,
                    text,
                    &Attrs::new().family(cosmic_text::Family::Monospace),
                    Shaping::Advanced,
                    None,
                );
                let theirs = buffer.layout_runs().count() as u32;
                drop(fs);

                assert_eq!(
                    ours, theirs,
                    "sample {i} at width {width}: ours={ours} cosmic={theirs}"
                );
            }
        }
    }

    #[test]
    fn repeated_lookups_reuse_the_cache() {
        // Not a timing assertion (flaky in CI) — checks that re-scanning
        // already-cached graphemes doesn't grow the cache further.
        let cache = cache();
        cache.take_width("abcabc", 1000.0);
        assert_eq!(cache.widths.lock().unwrap().len(), 3); // a, b, c
    }
}
