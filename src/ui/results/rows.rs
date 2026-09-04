//! The windowed row list both Layout modes render into, and the per-row
//! pieces they build with.
//!
//! Vertical virtualization lives here once: only the slice of Hits near the
//! viewport is given widgets, with spacers standing in for everything above
//! and below so the scrollbar still spans every loaded Hit. A mode supplies
//! its own row content and a width policy; everything around that — the
//! selection fill, the fixed row height, the affordance strip, the click
//! target, the scroll reporting — is the same either way, and is written here.

use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{button, column, container, mouse_area, row, scrollable, space, text};
use iced::{Element, Fill, Font, Length, Padding, Pixels};

use crate::Message;
use crate::advance_cache::AdvanceCache;
use crate::line::{self, Affordance};
use crate::results::{self, Msg, Paging, ResultTab, TotalHits, WRAP_LINE_H};
use crate::style::{self, ERR_RED};
use crate::ui::thousands;

/// How wide a Layout mode's rows are, and therefore which way its list
/// scrolls. The two are the same fact, which is why they are one choice.
pub(super) enum RowWidth {
    /// Rows fill the pane; the list scrolls vertically only. (Table)
    Fill,
    /// Rows are as wide as the widest loaded line, so the horizontal scrollbar
    /// still reaches the end of every line and the list scrolls both ways.
    ///
    /// That width is estimated as byte length × one monospace advance rather
    /// than shaped — bytes over-approximate column width for any non-ASCII
    /// run, so nothing scrollable-to is ever clipped; it only grows as longer
    /// lines page in, the way the vertical extent grows with paging. While
    /// wrapping, rows are viewport-wide and there is nothing to scroll to.
    ///
    /// Resolved inside [`windowed`] rather than passed in: it is not knowable
    /// until the row-height model has been prepared. (Raw text)
    WidestLine,
}

/// One Hit in the window, as [`windowed`] hands it to a Layout mode.
pub(super) struct Row<'a> {
    /// The Hit rendered through the tab's [`line::Layout`], cached per Hit.
    pub rendered: &'a line::Line,
    /// How many visual rows it occupies — 1 unless wrapping.
    pub disp_rows: u32,
}

/// The scrollable, vertically-windowed list both Layout modes render into.
///
/// Prepares the row-height model, works out which slice of `tab.hits` is near
/// the viewport ([`ResultTab::row_window`]), stands spacers in for everything
/// above and below it so the scrollbar still spans every loaded Hit, and wraps
/// each mode's content in the row shell they share: selection fill, fixed
/// height, clip, the expand/collapse affordance strip, and the [`Msg::HitClicked`]
/// mouse area.
///
/// `content` is called once per Hit in the window and returns only that mode's
/// own content — the table's cells, or raw text mode's sliced line. It must
/// not borrow from the [`Row`] it is handed: the row-height model is borrowed
/// for the length of the loop and released before this returns.
///
/// `span` names the [`crate::perf`] span the row loop is timed under. It is a
/// parameter rather than something derived from `width` because those two
/// names are what every benchmark recorded in
/// `docs/plans/wide-line-perf-followups.md` is keyed by.
pub(super) fn windowed<'a>(
    tab: &'a ResultTab,
    ctx: line::WrapCtx,
    width: RowWidth,
    pad: Padding,
    span: &'static str,
    content: impl Fn(&Row<'_>) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let layout = tab.layout();
    let cache = AdvanceCache::shared();

    let mut lines = tab.line_cache.borrow_mut();
    lines.prepare_heights(&tab.hits, &layout, ctx, cache);
    let (start, end) = tab.row_window(&lines);
    lines.prepare_lines((start, end));

    let row_w = match width {
        RowWidth::Fill => Length::Fill,
        RowWidth::WidestLine if ctx.on => Length::Fixed(tab.viewport_w),
        RowWidth::WidestLine => Length::Fixed(
            (lines.max_line_bytes() as f32 * cache.mono_advance()).max(tab.viewport_w),
        ),
    };

    // Only build widgets for the slice around the viewport; pad the rest
    // with spacers so the scrollbar still spans every loaded Hit.
    let mut body: Vec<Element<'a, Message>> = Vec::with_capacity(end - start + 2);
    if start > 0 {
        body.push(space().height(lines.offset(start)).into());
    }
    // Timed as a unit: this loop is the windowed-render cost the wide-line
    // work is chasing — `line::render` (cached per Hit, see `LineCache`) plus
    // whatever `content` shapes and the widget building. See
    // `docs/plans/wide-line-perf-followups.md`.
    let rows_span = crate::perf::span(span);
    for (offset, hit) in tab.hits[start..end].iter().enumerate() {
        let index = start + offset;
        let selected = tab.selected_hit == Some(index);
        // Render + upgrade this Hit's exact wrapped-row count, then read the
        // (now immutable) metrics for its row.
        let _ = lines.get(index, hit, &layout, cache);
        let affordance = lines.affordance(index);
        let row_h = lines.row_height(index);
        let hit_row = Row {
            rendered: lines.line(index),
            disp_rows: lines.disp_rows(index),
        };

        let mut stacked = column![content(&hit_row)];
        if let Some(strip) = affordance_strip(run_id, index, affordance) {
            stacked = stacked.push(strip);
        }
        let shell = container(stacked)
            .width(row_w)
            .height(Length::Fixed(row_h))
            .padding(pad)
            .clip(true)
            .style(move |_| {
                if selected {
                    style::panel(style::ACCENT)
                } else {
                    container::Style::default()
                }
            });

        body.push(
            mouse_area(shell)
                .on_press(Message::Result(run_id, Msg::HitClicked(index)))
                .into(),
        );
    }
    drop(rows_span);
    let trailing_h = lines.content_height() - lines.offset(end);
    drop(lines);
    if trailing_h > 0.5 {
        body.push(space().height(trailing_h).into());
    }

    let list = match width {
        RowWidth::Fill => scrollable(column(body).width(Fill)),
        RowWidth::WidestLine => scrollable(column(body)).direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::default(),
            horizontal: scrollable::Scrollbar::default(),
        }),
    };

    list.id(tab.scroll_id.clone())
        .width(Fill)
        .height(Fill)
        .on_scroll(move |viewport| {
            let offset = viewport.absolute_offset();
            Message::ResultScrolled {
                run_id,
                offset_y: offset.y,
                viewport_h: viewport.bounds().height,
                content_h: viewport.content_bounds().height,
                offset_x: offset.x,
                viewport_w: viewport.bounds().width,
            }
        })
        .into()
}

pub(super) fn paging_footer<'a>(tab: &'a ResultTab) -> Option<Element<'a, Message>> {
    let run_id = tab.run_id;
    let content: Element<'_, Message> = match &tab.paging {
        Paging::Idle | Paging::Exhausted => return None,
        Paging::Loading => text("Loading more\u{2026}")
            .size(12.0)
            .color(style::TEXT_DIM)
            .into(),
        Paging::Capped => {
            let cap = tab.max_results as u64;
            let msg = match tab.total_hits {
                TotalHits::Known(total) if total > cap => format!(
                    "Showing first {} of {} Hits — refine your search",
                    thousands(cap),
                    thousands(total),
                ),
                _ => format!("Showing first {cap} Hits — refine your search"),
            };
            text(msg).size(12.0).color(style::TEXT_DIM).into()
        }
        Paging::Failed(err) => row![
            text(format!("Failed to load more — {err}"))
                .size(12.0)
                .color(ERR_RED),
            button(text("Retry").size(12.0).color(style::TEXT))
                .on_press(Message::RetryPage(run_id))
                .padding(Padding::new(3.0).left(10.0).right(10.0))
                .style(style::picker_row(true)),
        ]
        .spacing(10.0)
        .align_y(iced::Alignment::Center)
        .into(),
    };

    Some(
        container(content)
            .style(|_| style::panel(style::PANEL_ALT))
            .width(Fill)
            .padding(Padding::new(5.0).left(12.0).right(12.0))
            .into(),
    )
}

/// Renders one Part, truncated to `max_width` pixels of shaped text before
/// being handed to iced — see [`AdvanceCache::take_width`] — so a widget
/// never shapes text no Column could ever show. With `wrapping` set to
/// [`Wrapping::Glyph`] the `max_width` budget bounds how much text a wrapped
/// cell shapes (`disp_rows` \u{d7} column width). `max_width: None` renders the
/// Part exactly as-is (used only by the Format modal's small fixed-size
/// preview).
pub(super) fn part_widget<'a>(
    part: &line::Part,
    max_width: Option<f32>,
    wrapping: Wrapping,
) -> Element<'a, Message> {
    let visible = match max_width {
        None => part.text.as_str(),
        Some(max_width) => {
            let (len, _) = AdvanceCache::shared().take_width(&part.text, max_width);
            &part.text[..len]
        }
    };
    let widget = text(visible.to_string())
        .size(results::CELL_TEXT_SIZE)
        .font(Font::MONOSPACE)
        .wrapping(wrapping);
    match wrapping {
        Wrapping::None => widget.into(),
        _ => widget
            .line_height(LineHeight::Absolute(Pixels(WRAP_LINE_H)))
            .into(),
    }
}

/// The expand / collapse / truncation affordance strip at the bottom of a
/// wrapped row, or `None` when the row shows in full. Its height is already
/// folded into the row-height model ([`results::WRAP_AFFORDANCE_H`]).
fn affordance_strip<'a>(
    run_id: u64,
    index: usize,
    affordance: Affordance,
) -> Option<Element<'a, Message>> {
    let label = match affordance {
        Affordance::None => return None,
        Affordance::Expand(n) => format!("\u{ff0b} {n} more line{}", if n == 1 { "" } else { "s" }),
        Affordance::Collapse => "\u{ff0d} collapse".to_string(),
        Affordance::Truncated => {
            return Some(
                text("line truncated \u{2014} open Hit detail for the rest")
                    .size(10.0)
                    .color(style::TEXT_DIM)
                    .into(),
            );
        }
    };
    Some(
        button(text(label).size(10.0).color(style::ACCENT))
            .on_press(Message::Result(run_id, Msg::HitExpand(index)))
            .padding(Padding::new(1.0).left(4.0).right(4.0))
            .style(style::bare_button())
            .into(),
    )
}
