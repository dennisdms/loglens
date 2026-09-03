//! Rendering for an open Result Tab: the Hit table, raw text mode, and the
//! floating panels/popovers that hang off them (Hit detail, header menu,
//! Sort fields, Format, Custom timeframe, paging footer).
//!
//! Extracted out of `main.rs`'s `impl LogLens` — these were originally
//! methods there purely to inherit `&self`; every one of them only ever
//! touched a `ResultTab` plus a handful of transient UI-hover fields, never
//! the rest of the app's state. They're free functions taking exactly what
//! they need instead, following the same pattern `ResultTab`'s own logic in
//! `results.rs` already uses.

use iced::widget::svg::Handle;
use iced::widget::{
    button, column, container, mouse_area, pick_list, radio, row, rule, scrollable, space, svg,
    text, text_editor, text_input, tooltip,
};
use iced::{Border, Color, Element, Fill, Font, Length, Padding};

use crate::advance_cache::AdvanceCache;
use crate::config::{TimeUnit, TimeframeMode};
use crate::line;
use crate::results::{self, FILL_COLUMN_MAX_W, Paging, ROW_H, ResultTab, RunState, TotalHits};
use crate::{ColumnDrag, ERR_RED, Message, WARN_AMBER, centered, field_label, icons, style};

// --- Result tab view ---------------------------------------------

pub(crate) fn result_view<'a>(
    tab: &'a ResultTab,
    header_hover: Option<usize>,
    grip_hover: Option<usize>,
    column_drag: Option<&ColumnDrag>,
) -> Element<'a, Message> {
    let hits_view = || -> Element<'a, Message> {
        match tab.mode {
            line::LayoutMode::Table => hit_table(tab, header_hover, grip_hover, column_drag),
            line::LayoutMode::RawText => raw_text_view(tab),
        }
    };
    let body: Element<'_, Message> = match &tab.state {
        // A refresh over an already-populated tab keeps the old table on
        // screen until the new first Page lands, so nothing flashes.
        _ if tab.table_visible() => hits_view(),
        RunState::Loading => centered("Running\u{2026}", style::TEXT_DIM),
        RunState::Empty => centered("No hits for this query and timeframe", style::TEXT_DIM),
        RunState::Error(err) => container(
            container(text(err.clone()).size(13.0).color(ERR_RED))
                .style(|_| {
                    let mut s = style::panel(style::PANEL_ALT);
                    s.border = Border {
                        color: ERR_RED,
                        width: 1.0,
                        radius: 3.0.into(),
                    };
                    s
                })
                .padding(12.0)
                .width(Fill),
        )
        .padding(12.0)
        .width(Fill)
        .into(),
        RunState::Loaded => hits_view(),
    };

    let mut layout = column![body].width(Fill).height(Fill);
    if tab.selected_hit.is_some() && matches!(tab.state, RunState::Loaded) {
        layout = layout.push(hit_detail(tab));
    }
    layout.into()
}

/// The bottom panel showing the selected Hit's full `_source`, resizable by
/// its top edge and dismissed with Esc or a second click on the row.
fn hit_detail<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let index = tab.selected_hit.unwrap_or(0);

    let grip = mouse_area(
        container(space().height(6.0))
            .width(Fill)
            .style(|_| style::panel(style::BORDER)),
    )
    .on_press(Message::DetailDragStart(run_id));

    let header = row![
        text(format!("Hit {} \u{b7} _source", index + 1))
            .size(11.0)
            .color(style::TEXT_DIM),
        space().width(Fill),
        button(text("Close (Esc)").size(11.0).color(style::TEXT_DIM))
            .on_press(Message::CloseHitDetail)
            .padding(2.0)
            .style(style::bare_button()),
    ]
    .align_y(iced::Alignment::Center);

    let editor = text_editor(&tab.detail_content)
        .on_action(move |action| Message::DetailEdit(run_id, action))
        .font(Font::MONOSPACE)
        .size(12.0)
        .height(Fill)
        .padding(Padding::new(4.0).left(8.0))
        .style(style::editor);

    column![
        grip,
        container(column![header, editor].spacing(4.0))
            .width(Fill)
            .height(Length::Fixed(tab.detail_height))
            .style(|_| style::panel(style::BG))
            .padding(Padding::new(6.0).left(10.0).right(10.0)),
    ]
    .width(Fill)
    .into()
}

/// The live options strip above a Result Tab's table, left-aligned:
/// "Sort fields", then the Layout options group — the Table/Text mode
/// toggle, joined by "Format" while in Text mode, wrapped in one bordered
/// surface so they read as a unit. (Column add / remove / reorder live in
/// each header's "\u{22ee}" menu.)
pub(crate) fn result_sort_bar<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;

    // The icon sits in a box as tall as the sort button's size-14 digit, so
    // every button in this strip comes out the same height.
    let icon_box = |handle: &Handle, color: Color| {
        container(
            svg(Handle::clone(handle))
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .style(move |_theme, _status| svg::Style { color: Some(color) }),
        )
        .center_y(Length::Fixed(18.0))
    };

    // A dropdown-style tooltip bubble shared by every button in the strip.
    let tip = |label: &'a str| {
        container(text(label).size(11.0).color(style::TEXT))
            .padding(Padding::new(4.0).left(6.0).right(6.0))
            .style(|_| style::menu_popup())
    };

    let sort_btn = button(
        row![
            icon_box(&icons::SORT_FIELDS, style::TEXT),
            text(format!("{}", tab.sort.len()))
                .size(14.0)
                .color(style::TEXT),
        ]
        .spacing(5.0)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::ResultSortPanel(run_id))
    .padding(Padding::new(5.0).left(9.0).right(9.0))
    .style(style::icon_button(tab.sort_panel_open));
    let sort_ctl = tooltip(sort_btn, tip("Sort fields"), tooltip::Position::Bottom).gap(4.0);

    // Layout options group: the Table/Text mode toggle, joined by the
    // "Format" button while in Text mode — Format edits the raw-text
    // template, so it is meaningless (and hidden) in Table mode. The shared
    // bordered surface ties the two together as one control.
    let is_raw = tab.mode == line::LayoutMode::RawText;
    let (mode_icon, mode_label, next_mode) = if is_raw {
        (&icons::RAW_TEXT, "Text", line::LayoutMode::Table)
    } else {
        (&icons::TABLE, "Table", line::LayoutMode::RawText)
    };
    let mode_btn = button(icon_box(mode_icon, style::TEXT))
        .on_press(Message::ResultLayoutMode(run_id, next_mode))
        .padding(Padding::new(5.0).left(9.0).right(9.0))
        .style(style::icon_button(false));
    let mut group = row![tooltip(mode_btn, tip(mode_label), tooltip::Position::Bottom).gap(4.0)]
        .spacing(6.0)
        .align_y(iced::Alignment::Center);

    if is_raw {
        let format_btn = button(icon_box(&icons::FORMAT, style::TEXT))
            .on_press(Message::OpenFormat(run_id))
            .padding(Padding::new(5.0).left(9.0).right(9.0))
            .style(style::icon_button(tab.format_open));
        group = group.push(tooltip(format_btn, tip("Format"), tooltip::Position::Bottom).gap(4.0));
    }

    let layout_group = container(group)
        .padding(3.0)
        .style(|_| style::options_group());

    let controls = row![sort_ctl, layout_group, space().width(Fill)]
        .spacing(8.0)
        .align_y(iced::Alignment::Center);

    container(controls)
        .style(|_| style::panel(style::PANEL))
        .width(Fill)
        .padding(Padding::new(4.0).left(12.0).right(12.0))
        .into()
}

/// The Hit table: a header row, then a scrollable, windowed body over the
/// visible slice of `tab.hits` (see [`ResultTab::row_window`]).
///
/// Each cell's text is truncated to what its Column's width could ever show
/// before being handed to a `text` widget — see [`AdvanceCache::take_width`].
/// Without this, a cell holding an 11,000-character Hit message gets fully
/// shaped just to render ~30 visible characters, on every one of the ~100
/// rows rebuilt on every scroll frame.
fn hit_table<'a>(
    tab: &'a ResultTab,
    header_hover: Option<usize>,
    grip_hover: Option<usize>,
    column_drag: Option<&ColumnDrag>,
) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let last = tab.columns.len().saturating_sub(1);

    let multi_sort = tab.sort.len() > 1;
    let header = row(tab
        .columns
        .iter()
        .enumerate()
        .map(|(i, col)| -> Element<'_, Message> {
            let mut label_row = row![text(col.clone()).size(12.0).color(style::TEXT_DIM)]
                .spacing(3.0)
                .align_y(iced::Alignment::Center);
            if let Some(rank) = tab.sort_index(col) {
                let arrow = if tab.sort[rank].desc {
                    "\u{25be}"
                } else {
                    "\u{25b4}"
                };
                label_row = label_row.push(text(arrow).size(10.0).color(style::TEXT));
                if multi_sort {
                    label_row = label_row.push(
                        text(format!("{}", rank + 1))
                            .size(9.0)
                            .color(style::TEXT_DIM),
                    );
                }
            }
            let label = container(label_row)
                .width(Fill)
                .clip(true)
                .padding(Padding::new(4.0).left(6.0));

            // A "\u{22ee}" affordance that opens this Column's settings menu
            // (add / remove / reorder / sort). Like the resize grip, it only shows while
            // the pointer is over the header (or the menu is open); a
            // fixed-width slot keeps the header from reflowing on hover.
            let show_dots = header_hover == Some(i) || tab.header_menu == Some(i);
            let dots: Element<'_, Message> = if show_dots {
                button(text("\u{22ee}").size(12.0).color(style::TEXT_DIM))
                    .on_press(Message::ResultHeaderMenu(run_id, i))
                    .padding(Padding::new(0.0).left(2.0).right(2.0))
                    .style(style::bare_button())
                    .into()
            } else {
                space().width(12.0).into()
            };
            let dots = container(dots).width(Length::Fixed(14.0));

            // The last Column flexes to fill the pane, so it has no edge to
            // drag; every other Column gets a right-edge resize grip.
            let inner: Element<'_, Message> = if i == last {
                container(row![label, dots].align_y(iced::Alignment::Center))
                    .width(Fill)
                    .into()
            } else {
                // The hairline shows while the pointer is anywhere over this
                // Column's header (or its own grip, or it is being dragged) —
                // and only for that Column.
                let lit = grip_hover == Some(i)
                    || header_hover == Some(i)
                    || matches!(column_drag, Some(d) if d.run_id == run_id && d.index == i);
                let line = container(space().width(2.0).height(14.0)).style(move |_| {
                    style::panel(if lit {
                        style::TEXT_DIM
                    } else {
                        Color::TRANSPARENT
                    })
                });
                let grip =
                    mouse_area(container(line).padding(Padding::new(0.0).left(4.0).right(4.0)))
                        .interaction(iced::mouse::Interaction::ResizingColumn)
                        .on_enter(Message::GripHover(Some(i)))
                        .on_exit(Message::GripHover(None))
                        .on_press(Message::ColumnDragStart(run_id, i));

                container(row![label, dots, grip].align_y(iced::Alignment::Center))
                    .width(Length::Fixed(tab.col_width(col)))
                    .into()
            };

            mouse_area(inner)
                .on_enter(Message::HeaderHover(Some(i)))
                .on_exit(Message::HeaderHover(None))
                .into()
        }))
    .spacing(8.0);

    // Only build widgets for the slice around the viewport; pad the rest
    // with spacers so the scrollbar still spans every loaded Hit.
    let (start, end) = tab.row_window();
    let layout = tab.layout();
    let mut body: Vec<Element<'_, Message>> = Vec::with_capacity(end - start + 2);
    if start > 0 {
        body.push(space().height(start as f32 * ROW_H).into());
    }
    // Timed as a unit: this loop is the windowed-render cost the wide-line
    // work is chasing — `line::render` per Hit plus the per-cell truncation
    // and widget building. See `docs/plans/wide-line-perf-followups.md`.
    let rows_span = crate::perf::span("view.hit_table_rows");
    for (offset, hit) in tab.hits[start..end].iter().enumerate() {
        let index = start + offset;
        let selected = tab.selected_hit == Some(index);
        let rendered = line::render(hit, &layout);
        let cells = container(
            row(tab
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| -> Element<'_, Message> {
                    let (width, max_text_w) = if i == last {
                        (Length::Fill, FILL_COLUMN_MAX_W)
                    } else {
                        let w = tab.col_width(col);
                        (Length::Fixed(w), w)
                    };
                    container(part_widget(&rendered.parts[i], Some(max_text_w)))
                        .width(width)
                        .padding(Padding::new(3.0).left(6.0))
                        .clip(true)
                        .into()
                }))
            .spacing(8.0),
        )
        .width(Fill)
        .height(Length::Fixed(ROW_H))
        .clip(true)
        .style(move |_| {
            if selected {
                style::panel(style::ACCENT)
            } else {
                container::Style::default()
            }
        });

        body.push(
            mouse_area(cells)
                .on_press(Message::HitClicked(run_id, index))
                .into(),
        );
    }
    drop(rows_span);
    let trailing = tab.hits.len().saturating_sub(end);
    if trailing > 0 {
        body.push(space().height(trailing as f32 * ROW_H).into());
    }

    let table = scrollable(column(body).width(Fill))
        .id(tab.scroll_id.clone())
        .width(Fill)
        .height(Fill)
        .on_scroll(move |viewport| {
            let offset = viewport.absolute_offset();
            Message::ResultScrolled {
                run_id,
                offset_y: offset.y,
                viewport_h: viewport.bounds().height,
                content_h: viewport.content_bounds().height,
            }
        });

    let mut stacked = column![
        container(header)
            .style(|_| style::panel(style::PANEL_ALT))
            .width(Fill)
            .padding(Padding::new(2.0).left(6.0)),
        rule::horizontal(1.0),
        table,
    ]
    .width(Fill)
    .height(Fill);

    if let Some(footer) = paging_footer(tab) {
        stacked = stacked.push(rule::horizontal(1.0));
        stacked = stacked.push(footer);
    }

    let Some(menu_col) = tab.header_menu.filter(|i| *i < tab.columns.len()) else {
        return stacked.into();
    };
    iced::widget::stack(vec![stacked.into(), header_menu_overlay(tab, menu_col)]).into()
}

/// Raw text mode's answer to [`hit_table`]: one fixed-height, non-wrapping
/// row per Hit rendering the template's single Part, with no headers and
/// horizontal overflow reachable by scrolling. Row clicks open the same
/// Hit-detail panel as Table mode.
///
/// Unlike `hit_table`, cell text here is *not* truncated: this mode's
/// `scrollable` handles real horizontal scrolling internally, and the app
/// doesn't track its scroll offset — truncating without knowing what's
/// currently scrolled into view would hide content a user could otherwise
/// reach. Left as a known follow-up (see `docs/wide-line-rendering-resources.md`).
fn raw_text_view<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let layout = tab.layout();

    let (start, end) = tab.row_window();
    let mut body: Vec<Element<'_, Message>> = Vec::with_capacity(end - start + 2);
    if start > 0 {
        body.push(space().height(start as f32 * ROW_H).into());
    }
    // Timed as a unit, mirroring `hit_table` — raw text mode renders each
    // Hit's whole (untruncated) line, so this is where item 5's cost lives.
    let rows_span = crate::perf::span("view.raw_text_rows");
    for (offset, hit) in tab.hits[start..end].iter().enumerate() {
        let index = start + offset;
        let selected = tab.selected_hit == Some(index);
        let rendered = line::render(hit, &layout);
        let content: Element<'_, Message> = match rendered.parts.first() {
            Some(part) => part_widget(part, None),
            None => text("").size(12.0).font(Font::MONOSPACE).into(),
        };

        let row_el = container(content)
            .height(Length::Fixed(ROW_H))
            .padding(Padding::new(3.0).left(6.0))
            .style(move |_| {
                if selected {
                    style::panel(style::ACCENT)
                } else {
                    container::Style::default()
                }
            });

        body.push(
            mouse_area(row_el)
                .on_press(Message::HitClicked(run_id, index))
                .into(),
        );
    }
    drop(rows_span);
    let trailing = tab.hits.len().saturating_sub(end);
    if trailing > 0 {
        body.push(space().height(trailing as f32 * ROW_H).into());
    }

    let view = scrollable(column(body))
        .id(tab.scroll_id.clone())
        .width(Fill)
        .height(Fill)
        .direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::default(),
            horizontal: scrollable::Scrollbar::default(),
        })
        .on_scroll(move |viewport| {
            let offset = viewport.absolute_offset();
            Message::ResultScrolled {
                run_id,
                offset_y: offset.y,
                viewport_h: viewport.bounds().height,
                content_h: viewport.content_bounds().height,
            }
        });

    let mut stacked = column![view].width(Fill).height(Fill);
    if let Some(footer) = paging_footer(tab) {
        stacked = stacked.push(rule::horizontal(1.0));
        stacked = stacked.push(footer);
    }
    stacked.into()
}

fn header_menu_overlay<'a>(tab: &'a ResultTab, index: usize) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let field = tab.columns[index].clone();
    let last = tab.columns.len().saturating_sub(1);
    let sorted = tab.sort_index(&field).is_some();

    const MENU_W: f32 = 200.0;

    // A 13px symbolic icon, recoloured to `tint` via the svg colour filter.
    let glyph = |handle: &'static std::sync::LazyLock<Handle>, tint: Color| {
        svg(Handle::clone(handle))
            .width(Length::Fixed(13.0))
            .height(Length::Fixed(13.0))
            .style(move |_theme, _status| svg::Style { color: Some(tint) })
    };

    // One "icon + label" menu row. `msg == None` renders it greyed and
    // inert — used when a move would run off the end of the Column list.
    let entry =
        |handle: &'static std::sync::LazyLock<Handle>, label: &str, msg: Option<Message>| {
            let (fg, tint) = match msg {
                Some(_) => (style::TEXT, style::TEXT_DIM),
                None => (style::TEXT_DIM, style::BORDER),
            };
            let mut b = button(
                row![
                    glyph(handle, tint),
                    text(label.to_string()).size(12.0).color(fg),
                ]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
            )
            .width(Fill)
            .padding(Padding::new(4.0).left(8.0).right(8.0))
            .style(style::picker_row(false));
            if let Some(msg) = msg {
                b = b.on_press(msg);
            }
            b
        };

    let mut items: Vec<Element<'_, Message>> = Vec::new();
    items.push(
        entry(
            &icons::ARROW_LEFT,
            "Move column left",
            (index > 0).then_some(Message::ResultColumnMove(run_id, index, -1)),
        )
        .into(),
    );
    items.push(
        entry(
            &icons::ARROW_RIGHT,
            "Move column right",
            (index < last).then_some(Message::ResultColumnMove(run_id, index, 1)),
        )
        .into(),
    );
    items.push(
        entry(
            &icons::TRASH,
            "Remove column",
            Some(Message::ResultColumnRemove(run_id, index)),
        )
        .into(),
    );

    // Fields not already shown as Columns; picked from the menu to add one.
    let available: Vec<String> = tab
        .all_fields
        .iter()
        .filter(|f| !tab.columns.iter().any(|c| c == *f))
        .cloned()
        .collect();
    let add_ctl: Element<'_, Message> = if !available.is_empty() {
        pick_list(available, None::<String>, move |f| {
            Message::ResultColumnAddField(run_id, f)
        })
        .placeholder("Add column\u{2026}")
        .text_size(12.0)
        .padding(Padding::new(4.0).left(6.0).right(6.0))
        .width(Fill)
        .into()
    } else {
        row![
            text_input("Add column\u{2026}", &tab.column_draft)
                .on_input(move |v| Message::ResultColumnDraft(run_id, v))
                .on_submit(Message::ResultColumnAdd(run_id))
                .size(12.0)
                .padding(4.0)
                .width(Fill),
            button(text("+").size(12.0).color(style::TEXT))
                .on_press(Message::ResultColumnAdd(run_id))
                .padding(Padding::new(4.0).left(8.0).right(8.0))
                .style(style::picker_row(true)),
        ]
        .spacing(4.0)
        .into()
    };
    items.push(
        container(
            row![glyph(&icons::PLUS, style::TEXT_DIM), add_ctl]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
        )
        .padding(Padding::new(2.0).left(8.0).right(8.0))
        .into(),
    );

    items.push(rule::horizontal(1.0).into());
    items.push(
        entry(
            &icons::SORT_ASCENDING,
            "Sort ascending",
            Some(Message::ResultSortSet(run_id, field.clone(), false)),
        )
        .into(),
    );
    items.push(
        entry(
            &icons::SORT_DESCENDING,
            "Sort descending",
            Some(Message::ResultSortSet(run_id, field.clone(), true)),
        )
        .into(),
    );
    if sorted {
        items.push(
            entry(
                &icons::SORT_REMOVE,
                "Remove from sort",
                Some(Message::ResultSortRemove(run_id, field.clone())),
            )
            .into(),
        );
    }
    let card = container(column(items).spacing(1.0).width(Length::Fixed(MENU_W)))
        .style(|_| style::menu_popup())
        .padding(3.0);

    // Header geometry: 6px container pad + each fixed Column's width + 8px
    // row spacing between Columns. Anchor the card's right edge near the
    // Column's right edge, then clamp into the pane.
    let anchored: Element<'_, Message> = if index == last {
        row![
            space().width(Fill),
            container(card).padding(Padding::new(0.0).right(6.0)),
        ]
        .into()
    } else {
        let mut right_edge = 6.0;
        for i in 0..=index {
            right_edge += tab.col_width(&tab.columns[i]) + 8.0;
        }
        let left = (right_edge - MENU_W).max(6.0);
        container(card).padding(Padding::new(0.0).left(left)).into()
    };

    mouse_area(
        container(column![space().height(26.0), anchored])
            .width(Fill)
            .height(Fill),
    )
    .on_press(Message::ResultHeaderMenuDismiss(run_id))
    .on_right_press(Message::ResultHeaderMenuDismiss(run_id))
    .into()
}

fn paging_footer<'a>(tab: &'a ResultTab) -> Option<Element<'a, Message>> {
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
                    crate::thousands(cap),
                    crate::thousands(total),
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
/// never shapes text no Column could ever show. `max_width: None` renders
/// the Part exactly as-is (used by raw text mode's real horizontal scrolling
/// and the Format modal's small fixed-size preview, neither of which is safe
/// or worth truncating; see the doc comment on [`raw_text_view`]).
fn part_widget<'a>(part: &line::Part, max_width: Option<f32>) -> Element<'a, Message> {
    let visible = match max_width {
        None => part.text.as_str(),
        Some(max_width) => {
            let (len, _) = AdvanceCache::shared().take_width(&part.text, max_width);
            &part.text[..len]
        }
    };
    text(visible.to_string())
        .size(results::CELL_TEXT_SIZE)
        .font(Font::MONOSPACE)
        .wrapping(text::Wrapping::None)
        .into()
}

// --- Format modal --------------------------------------------------------

pub(crate) fn format_modal<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;

    let mut card = column![
        text("Format").size(16.0).color(style::TEXT),
        text(
            "Template for Text mode. \u{201c}%{field.path}\u{201d} inserts a \
             field; \u{201c}%{_source}\u{201d} inserts the whole document as \
             JSON."
        )
        .size(11.0)
        .color(style::TEXT_DIM),
    ]
    .spacing(8.0)
    .width(Fill);

    // 1. Fields available in this search — reference only, no click behaviour.
    card = card.push(rule::horizontal(1.0));
    card = card.push(text("Fields").size(12.0).color(style::TEXT_DIM));
    let fields: Element<'_, Message> = if tab.all_fields.is_empty() {
        text("Field list not loaded yet.")
            .size(11.0)
            .color(style::TEXT_DIM)
            .into()
    } else {
        let mut list = column![].spacing(1.0);
        for f in &tab.all_fields {
            list = list.push(
                text(f.clone())
                    .size(11.0)
                    .font(Font::MONOSPACE)
                    .color(style::TEXT_DIM),
            );
        }
        list = list.push(
            text("_source \u{2014} the whole document as JSON")
                .size(11.0)
                .color(style::TEXT_DIM),
        );
        scrollable(list)
            .height(Length::Fixed(140.0))
            .width(Fill)
            .into()
    };
    card = card.push(fields);

    // 2. Format — the template string, plus a non-blocking unknown-field warning.
    card = card.push(rule::horizontal(1.0));
    card = card.push(text("Format").size(12.0).color(style::TEXT_DIM));
    card = card.push(
        text_input("%{field.path} template", &tab.template_draft)
            .on_input(move |v| Message::ResultTemplateDraft(run_id, v))
            .on_submit(Message::ResultTemplateSubmit(run_id))
            .size(12.0)
            .padding(4.0)
            .font(Font::MONOSPACE)
            .width(Fill),
    );
    let unknown = line::unknown_template_fields(&tab.template_draft, &tab.all_fields);
    if !unknown.is_empty() {
        let list = unknown
            .iter()
            .map(|f| format!("\u{201c}{f}\u{201d}"))
            .collect::<Vec<_>>()
            .join(", ");
        card = card.push(
            text(format!("Unknown field: {list}"))
                .size(11.0)
                .color(WARN_AMBER),
        );
    }

    // 3. Display — a live preview of the first Hits under the current draft.
    card = card.push(rule::horizontal(1.0));
    card = card.push(text("Display").size(12.0).color(style::TEXT_DIM));
    let template = {
        let draft = tab.template_draft.trim();
        if draft.is_empty() {
            tab.template.clone()
        } else {
            draft.to_string()
        }
    };
    let preview_layout = line::Layout {
        mode: line::LayoutMode::RawText,
        columns: Vec::new(),
        template,
        timestamp_field: tab.timestamp_field.clone(),
        utc: tab.utc,
    };
    let preview: Element<'_, Message> = if tab.hits.is_empty() {
        text("Run a search to preview.")
            .size(11.0)
            .color(style::TEXT_DIM)
            .into()
    } else {
        let mut lines = column![];
        for hit in tab.hits.iter().take(10) {
            let rendered = line::render(hit, &preview_layout);
            let content: Element<'_, Message> = match rendered.parts.first() {
                Some(part) => part_widget(part, None),
                None => text("").size(12.0).font(Font::MONOSPACE).into(),
            };
            lines = lines.push(container(content).height(Length::Fixed(ROW_H)));
        }
        scrollable(lines)
            .width(Fill)
            .height(Length::Fixed(ROW_H * 10.0 + 6.0))
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::default(),
                horizontal: scrollable::Scrollbar::default(),
            })
            .into()
    };
    card = card.push(
        container(preview)
            .width(Fill)
            .padding(6.0)
            .style(|_| style::panel(style::BG)),
    );

    card = card.push(space().height(4.0));
    card = card.push(
        row![
            space().width(Fill),
            button(text("Cancel").size(13.0).color(style::TEXT_DIM))
                .on_press(Message::FormatCancel(run_id))
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            button(text("Done").size(13.0).color(style::TEXT))
                .on_press(Message::CloseFormat(run_id))
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ]
        .spacing(8.0),
    );

    crate::modal_card_sized(card.into(), 640.0)
}

// --- Sort fields popover --------------------------------------------------

/// The "Sort fields" popover: one row per sort key (remove, direction
/// toggle, reorder) plus a picker to add a field and a "Clear sorting"
/// action. Floated by `LogLens::sort_fields_popover_overlay`.
pub(crate) fn sort_fields_popover<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let last = tab.sort.len().saturating_sub(1);

    let mut rows = column![].spacing(4.0);
    for (i, key) in tab.sort.iter().enumerate() {
        let field = key.field.clone();

        let remove = button(text("\u{00d7}").size(12.0).color(style::TEXT_DIM))
            .on_press(Message::ResultSortRemove(run_id, field.clone()))
            .padding(2.0)
            .style(style::bare_button());

        let name = container(text(key.field.clone()).size(12.0).color(style::TEXT))
            .width(Length::Fixed(220.0))
            .clip(true);

        let is_time = key.field == tab.timestamp_field;
        let (asc_label, desc_label) = if is_time {
            ("Old\u{2013}New", "New\u{2013}Old")
        } else {
            ("A\u{2013}Z", "Z\u{2013}A")
        };
        let asc = button(text(asc_label).size(11.0).color(style::TEXT))
            .on_press(Message::ResultSortSet(run_id, field.clone(), false))
            .padding(Padding::new(3.0).left(10.0).right(10.0))
            .style(style::picker_row(!key.desc));
        let desc = button(text(desc_label).size(11.0).color(style::TEXT))
            .on_press(Message::ResultSortSet(run_id, field.clone(), true))
            .padding(Padding::new(3.0).left(10.0).right(10.0))
            .style(style::picker_row(key.desc));

        let mut up = button(text("\u{25b4}").size(10.0).color(style::TEXT_DIM))
            .padding(1.0)
            .style(style::bare_button());
        if i > 0 {
            up = up.on_press(Message::ResultSortMove(run_id, i, -1));
        }
        let mut down = button(text("\u{25be}").size(10.0).color(style::TEXT_DIM))
            .padding(1.0)
            .style(style::bare_button());
        if i < last {
            down = down.on_press(Message::ResultSortMove(run_id, i, 1));
        }

        rows = rows.push(
            row![
                remove,
                name,
                row![asc, desc].spacing(1.0),
                space().width(Fill),
                column![up, down].spacing(0.0),
            ]
            .spacing(8.0)
            .align_y(iced::Alignment::Center),
        );
    }
    if tab.sort.is_empty() {
        rows = rows.push(
            text("No sort fields \u{2014} Hits fall back to the timestamp field, newest first.")
                .size(11.0)
                .color(style::TEXT_DIM),
        );
    }

    let pool = if !tab.sortable_fields.is_empty() {
        &tab.sortable_fields
    } else {
        &tab.all_fields
    };
    let available: Vec<String> = pool
        .iter()
        .filter(|f| tab.sort_index(f).is_none())
        .cloned()
        .collect();
    let picker: Element<'_, Message> = if available.is_empty() {
        text("Pick fields to sort by")
            .size(11.0)
            .color(style::TEXT_DIM)
            .into()
    } else {
        pick_list(available, None::<String>, move |f| {
            Message::ResultSortSet(run_id, f, true)
        })
        .placeholder("Pick fields to sort by")
        .text_size(11.0)
        .padding(3.0)
        .into()
    };

    let mut footer = row![picker, space().width(Fill)]
        .spacing(8.0)
        .align_y(iced::Alignment::Center);
    if !tab.sort.is_empty() {
        footer = footer.push(
            button(text("Clear sorting").size(11.0).color(style::ACCENT))
                .on_press(Message::ResultSortClear(run_id))
                .padding(Padding::new(3.0).left(8.0).right(8.0))
                .style(style::bare_button()),
        );
    }

    container(
        column![rows, rule::horizontal(1.0), footer]
            .spacing(8.0)
            .width(Fill),
    )
    .style(|_| {
        let mut s = style::panel(style::PANEL);
        s.border = Border {
            color: style::BORDER,
            width: 1.0,
            radius: 4.0.into(),
        };
        s
    })
    .width(Fill)
    .padding(Padding::new(8.0).left(12.0).right(12.0))
    .into()
}

// --- Custom timeframe popover ---------------------------------------------

/// The "Custom\u{2026}" timeframe popover card: a relative or absolute window
/// editor, applied or dismissed. Floated by `LogLens::timeframe_popover_overlay`.
pub(crate) fn timeframe_popover<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let tf = &tab.tf;

    let modes = row![
        radio(
            "Relative",
            TimeframeMode::Relative,
            Some(tf.mode),
            move |m| Message::ResultTfMode(run_id, m),
        )
        .size(14.0),
        radio(
            "Absolute",
            TimeframeMode::Absolute,
            Some(tf.mode),
            move |m| Message::ResultTfMode(run_id, m),
        )
        .size(14.0),
    ]
    .spacing(16.0);

    let detail: Element<'_, Message> = match tf.mode {
        TimeframeMode::Relative => {
            let units = row(TimeUnit::ALL.iter().map(move |&u| {
                radio(u.label(), u, Some(tf.rel_unit), move |u| {
                    Message::ResultTfRelUnit(run_id, u)
                })
                .size(14.0)
                .into()
            }))
            .spacing(12.0);
            row![
                text("Last").size(13.0).color(style::TEXT),
                text_input("15", &tf.rel_amount)
                    .on_input(move |v| Message::ResultTfRelAmount(run_id, v))
                    .on_submit(Message::ResultTfApply(run_id))
                    .width(60.0)
                    .padding(6.0),
                units,
            ]
            .spacing(10.0)
            .align_y(iced::Alignment::Center)
            .into()
        }
        TimeframeMode::Absolute => row![
            column![
                field_label("From"),
                text_input("2026-08-28T09:00:00", &tf.abs_from)
                    .on_input(move |v| Message::ResultTfAbsFrom(run_id, v))
                    .padding(6.0),
            ]
            .spacing(4.0),
            column![
                field_label("To"),
                text_input("2026-08-28T10:00:00", &tf.abs_to)
                    .on_input(move |v| Message::ResultTfAbsTo(run_id, v))
                    .padding(6.0),
            ]
            .spacing(4.0),
        ]
        .spacing(10.0)
        .into(),
    };

    let actions = row![
        space().width(Fill),
        button(text("Cancel").size(12.0).color(style::TEXT_DIM))
            .on_press(Message::ResultTfCancel(run_id))
            .padding(Padding::new(4.0).left(12.0).right(12.0))
            .style(style::bare_button()),
        button(text("Apply").size(12.0).color(style::TEXT))
            .on_press(Message::ResultTfApply(run_id))
            .padding(Padding::new(4.0).left(14.0).right(14.0))
            .style(style::picker_row(true)),
    ]
    .spacing(8.0);

    container(column![modes, detail, space().height(2.0), actions].spacing(8.0))
        .style(|_| {
            let mut s = style::panel(style::PANEL);
            s.border = Border {
                color: style::BORDER,
                width: 1.0,
                radius: 4.0.into(),
            };
            s
        })
        .width(Fill)
        .padding(Padding::new(10.0).left(12.0).right(12.0))
        .into()
}
