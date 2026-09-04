//! Raw text mode: one rendered line per Hit, no headers, horizontal overflow
//! reachable by scrolling.
//!
//! Horizontally virtualized the way [`super::rows`] is vertically — a row is
//! shaped only over the slice of its line that is on screen.

use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{column, row, rule, space, text};
use iced::{Element, Fill, Font, Length, Padding, Pixels};

use super::rows::{RowWidth, paging_footer, windowed};
use crate::Message;
use crate::advance_cache::AdvanceCache;
use crate::results::{self, ResultTab, WRAP_LINE_H};

/// mode, so a horizontal fling doesn't outrun the sliced text before the next
/// frame catches up. The horizontal analogue of `results::WINDOW_BUFFER`.
const RAW_SLICE_SLACK: f32 = 400.0;

/// Raw text mode's answer to [`hit_table`]: one row per Hit rendering the
/// template's single Part, with no headers and horizontal overflow reachable
/// by scrolling. Row clicks open the same Hit-detail panel as Table mode.
///
/// Each row is shaped only over the slice within
/// `[scroll_x, scroll_x + viewport_w]` (plus [`RAW_SLICE_SLACK`]) — the
/// horizontal analogue of the vertical windowing [`windowed`] does. A leading
/// spacer of the scrolled-off prefix's exact width keeps every character at
/// its true position, so the slice never moves a visible pixel.
pub(super) fn raw_text_view<'a>(
    tab: &'a ResultTab,
    wrap_row_cap: Option<usize>,
) -> Element<'a, Message> {
    let ctx = tab.wrap_ctx(wrap_row_cap);
    let cache = AdvanceCache::shared();
    let offset_x = tab.scroll_x.max(0.0);
    let slice_w = tab.viewport_w.max(1.0) + RAW_SLICE_SLACK;

    let rows = windowed(
        tab,
        ctx,
        RowWidth::WidestLine,
        Padding::new(3.0).left(6.0),
        "view.raw_text_rows",
        move |hit_row| {
            let full = hit_row
                .rendered
                .parts
                .first()
                .map_or("", |p| p.text.as_str());
            if ctx.on {
                let budget = hit_row.disp_rows.max(1) as f32 * ctx.width;
                let (len, _) = cache.take_width(full, budget);
                text(full[..len].to_string())
                    .size(results::CELL_TEXT_SIZE)
                    .font(Font::MONOSPACE)
                    .wrapping(Wrapping::Glyph)
                    .line_height(LineHeight::Absolute(Pixels(WRAP_LINE_H)))
                    .into()
            } else {
                let (skip_w, visible) = raw_row_slice(cache, full, offset_x, slice_w);
                row![
                    space().width(Length::Fixed(skip_w)),
                    text(full[visible].to_string())
                        .size(results::CELL_TEXT_SIZE)
                        .font(Font::MONOSPACE)
                        .wrapping(Wrapping::None),
                ]
                .into()
            }
        },
    );

    let mut stacked = column![rows].width(Fill).height(Fill);
    if let Some(footer) = paging_footer(tab) {
        stacked = stacked.push(rule::horizontal(1.0));
        stacked = stacked.push(footer);
    }
    stacked.into()
}

/// The horizontally-virtualized slice of one raw-text line: the shaped width
/// of the prefix scrolled off the left — used as a leading spacer, taken on a
/// grapheme boundary so it never exceeds `offset_x` (the sub-character
/// remainder falls under the row's left pad) — and the byte range of the line
/// to hand a `text` widget: `slice_w` pixels starting at that boundary.
/// Everything outside is off-screen or covered by the fling slack in
/// `slice_w`. Because the spacer is the *exact* width of the skipped prefix,
/// every glyph in `visible` lands at its true position regardless of where the
/// grapheme boundary fell — the slice never shifts on screen as `offset_x`
/// moves.
fn raw_row_slice(
    cache: &AdvanceCache,
    full: &str,
    offset_x: f32,
    slice_w: f32,
) -> (f32, std::ops::Range<usize>) {
    let (skip_len, skip_w) = cache.take_width(full, offset_x);
    let (vis_len, _) = cache.take_width(&full[skip_len..], slice_w);
    (skip_w, skip_len..skip_len + vis_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> AdvanceCache {
        AdvanceCache::new(Font::MONOSPACE, results::CELL_TEXT_SIZE)
    }

    #[test]
    fn raw_slice_at_the_left_edge_starts_from_the_beginning() {
        let cache = cache();
        let line = "GET /api/v1/orders HTTP/1.1 200 1234";
        let (skip_w, range) = raw_row_slice(&cache, line, 0.0, 10_000.0);
        assert_eq!(skip_w, 0.0);
        assert_eq!(range.start, 0);
        // A generous slice budget keeps the whole line.
        assert_eq!(range.end, line.len());
    }

    #[test]
    fn raw_slice_drops_a_scrolled_off_prefix_without_overshooting_the_offset() {
        let cache = cache();
        let line = "x".repeat(2000);
        let offset_x = 300.0;
        let (skip_w, range) = raw_row_slice(&cache, &line, offset_x, 400.0);

        // The spacer never pushes past the real scroll offset...
        assert!(skip_w <= offset_x);
        // ...but it is tight: one more character would have exceeded it.
        let (_, one_more) = cache.take_width(&line[..range.start + 1], f32::INFINITY);
        assert!(one_more > offset_x);
        assert!(line.is_char_boundary(range.start));
    }

    #[test]
    fn raw_slice_keeps_only_a_slice_width_worth_of_text() {
        let cache = cache();
        let line = "y".repeat(5000);
        let slice_w = 250.0;
        let (_, range) = raw_row_slice(&cache, &line, 600.0, slice_w);

        assert!(range.start > 0 && range.end < line.len());
        let (_, shown) = cache.take_width(&line[range.clone()], f32::INFINITY);
        assert!(shown <= slice_w);
        // Tight on the right edge too.
        let (_, one_more) = cache.take_width(&line[range.start..range.end + 1], f32::INFINITY);
        assert!(one_more > slice_w);
    }

    #[test]
    fn raw_slice_of_a_short_line_is_the_whole_line() {
        let cache = cache();
        let line = "tiny";
        let (skip_w, range) = raw_row_slice(&cache, line, 0.0, 800.0);
        assert_eq!(skip_w, 0.0);
        assert_eq!(&line[range], "tiny");
    }
}
