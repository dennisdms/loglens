//! Table mode: one row of Columns per Hit, under a header that sorts,
//! resizes, and opens each Column's settings menu.

use iced::widget::svg::Handle;
use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, rule, space, svg, text, text_input,
};
use iced::{Color, Element, Fill, Length, Padding};

use super::rows::{RowWidth, paging_footer, part_widget, windowed};
use crate::results::{FILL_COLUMN_MAX_W, ResultTab};
use crate::style;
use crate::{ColumnDrag, Message, icons};

/// The Hit table: a header row, then a scrollable, windowed body over the
/// visible slice of `tab.hits` (see [`ResultTab::row_window`]).
///
/// Each cell's text is truncated to what its Column's width could ever show
/// before being handed to a `text` widget — see [`AdvanceCache::take_width`].
/// Without this, a cell holding an 11,000-character Hit message gets fully
/// shaped just to render ~30 visible characters, on every one of the ~100
/// rows rebuilt on every scroll frame.
pub(super) fn hit_table<'a>(
    tab: &'a ResultTab,
    header_hover: Option<usize>,
    grip_hover: Option<usize>,
    column_drag: Option<&ColumnDrag>,
    wrap_row_cap: Option<usize>,
) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let last = tab.search.columns.len().saturating_sub(1);

    let multi_sort = tab.search.sort.len() > 1;
    let header =
        row(tab
            .search
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| -> Element<'_, Message> {
                let mut label_row = row![text(col.clone()).size(12.0).color(style::TEXT_DIM)]
                    .spacing(3.0)
                    .align_y(iced::Alignment::Center);
                if let Some(rank) = tab.search.sort_index(col) {
                    let arrow = if tab.search.sort[rank].desc {
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

    // The bucketed width the flexible last column wraps at — matches the
    // height model so its estimate and the drawn text agree.
    let ctx = tab.wrap_ctx(wrap_row_cap);
    let fill_w = ctx.width;

    let table = windowed(
        tab,
        ctx,
        RowWidth::Fill,
        Padding::ZERO,
        "view.hit_table_rows",
        move |hit_row| {
            row(tab
                .search
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| -> Element<'_, Message> {
                    let wraps = ctx.on && i == last;
                    let (width, budget, wrapping) = if wraps {
                        (
                            Length::Fixed(fill_w),
                            hit_row.disp_rows.max(1) as f32 * fill_w,
                            Wrapping::Glyph,
                        )
                    } else if i == last {
                        (Length::Fill, FILL_COLUMN_MAX_W, Wrapping::None)
                    } else {
                        let w = tab.col_width(col);
                        (Length::Fixed(w), w, Wrapping::None)
                    };
                    container(part_widget(
                        &hit_row.rendered.parts[i],
                        Some(budget),
                        wrapping,
                    ))
                    .width(width)
                    .padding(Padding::new(3.0).left(6.0))
                    .clip(!wraps)
                    .into()
                }))
            .spacing(8.0)
            .align_y(iced::Alignment::Start)
            .into()
        },
    );

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

    let Some(menu_col) = tab.header_menu.filter(|i| *i < tab.search.columns.len()) else {
        return stacked.into();
    };
    iced::widget::stack(vec![stacked.into(), header_menu_overlay(tab, menu_col)]).into()
}

fn header_menu_overlay<'a>(tab: &'a ResultTab, index: usize) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let field = tab.search.columns[index].clone();
    let last = tab.search.columns.len().saturating_sub(1);
    let sorted = tab.search.sort_index(&field).is_some();

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
        .filter(|f| !tab.search.columns.iter().any(|c| c == *f))
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
            right_edge += tab.col_width(&tab.search.columns[i]) + 8.0;
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
