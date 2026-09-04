//! The Result Tab: the Hits from one Run of a Saved Search.
//!
//! Split by surface — the two Layout modes ([`table`] and [`raw_text`]) over
//! the windowed row list they share ([`rows`]), and the floating editors that
//! hang off them ([`popovers`]). What is left here is what composes those: the
//! mode dispatch and its Run-state placeholders, the Hit detail panel, and the
//! options strip above the table.

mod popovers;
mod raw_text;
mod rows;
mod table;

pub(crate) use popovers::{format_modal, sort_fields_popover, timeframe_popover};

use iced::widget::svg::Handle;
use iced::widget::{
    button, column, container, mouse_area, row, space, svg, text, text_editor, tooltip,
};
use iced::{Border, Color, Element, Fill, Font, Length, Padding};

use crate::line;
use crate::results::{Msg, ResultTab, RunState};
use crate::style::{self, ERR_RED};
use crate::ui::centered;
use crate::ui::chrome;
use crate::{ColumnDrag, Message, icons};
use raw_text::raw_text_view;
use table::hit_table;

pub(crate) fn result_view<'a>(
    tab: &'a ResultTab,
    header_hover: Option<usize>,
    grip_hover: Option<usize>,
    column_drag: Option<&ColumnDrag>,
    wrap_row_cap: Option<usize>,
) -> Element<'a, Message> {
    let hits_view = || -> Element<'a, Message> {
        match tab.search.mode {
            line::LayoutMode::Table => {
                hit_table(tab, header_hover, grip_hover, column_drag, wrap_row_cap)
            }
            line::LayoutMode::RawText => raw_text_view(tab, wrap_row_cap),
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
        .on_action(move |action| Message::Result(run_id, Msg::DetailEdit(action)))
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
        .center_y(Length::Fixed(chrome::OPTIONS_ICON_BOX_H))
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
            text(format!("{}", tab.search.sort.len()))
                .size(14.0)
                .color(style::TEXT),
        ]
        .spacing(5.0)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::Result(run_id, Msg::SortPanel))
    .padding(Padding::new(chrome::OPTIONS_BTN_PAD_Y).left(9.0).right(9.0))
    .style(style::icon_button(tab.sort_panel_open));
    let sort_ctl = tooltip(sort_btn, tip("Sort fields"), tooltip::Position::Bottom).gap(4.0);

    // Layout options group: the Table/Text mode toggle, joined by the
    // "Format" button while in Text mode — Format edits the raw-text
    // template, so it is meaningless (and hidden) in Table mode. The shared
    // bordered surface ties the two together as one control.
    let is_raw = tab.search.mode == line::LayoutMode::RawText;
    let (mode_icon, mode_label, next_mode) = if is_raw {
        (&icons::RAW_TEXT, "Text", line::LayoutMode::Table)
    } else {
        (&icons::TABLE, "Table", line::LayoutMode::RawText)
    };
    let mode_btn = button(icon_box(mode_icon, style::TEXT))
        .on_press(Message::Result(run_id, Msg::LayoutMode(next_mode)))
        .padding(Padding::new(chrome::OPTIONS_BTN_PAD_Y).left(9.0).right(9.0))
        .style(style::icon_button(false));
    let mut group = row![tooltip(mode_btn, tip(mode_label), tooltip::Position::Bottom).gap(4.0)]
        .spacing(6.0)
        .align_y(iced::Alignment::Center);

    // Wrap toggle: long Hit text onto multiple visual rows instead of
    // truncating (Table) / scrolling sideways (Text).
    let wrap_btn = button(text("Wrap").size(12.0).color(if tab.search.wrap {
        style::TEXT
    } else {
        style::TEXT_DIM
    }))
    .on_press(Message::Result(run_id, Msg::Wrap))
    .padding(Padding::new(chrome::OPTIONS_BTN_PAD_Y).left(9.0).right(9.0))
    .style(style::icon_button(tab.search.wrap));
    group = group.push(
        tooltip(
            wrap_btn,
            tip(if tab.search.wrap {
                "Wrap: on"
            } else {
                "Wrap: off"
            }),
            tooltip::Position::Bottom,
        )
        .gap(4.0),
    );

    if is_raw {
        let format_btn = button(icon_box(&icons::FORMAT, style::TEXT))
            .on_press(Message::Result(run_id, Msg::OpenFormat))
            .padding(Padding::new(chrome::OPTIONS_BTN_PAD_Y).left(9.0).right(9.0))
            .style(style::icon_button(tab.format_open));
        group = group.push(tooltip(format_btn, tip("Format"), tooltip::Position::Bottom).gap(4.0));
    }

    let layout_group = container(group)
        .padding(chrome::OPTIONS_GROUP_PAD)
        .style(|_| style::options_group());

    let controls = row![sort_ctl, layout_group, space().width(Fill)]
        .spacing(8.0)
        .align_y(iced::Alignment::Center);

    container(controls)
        .style(|_| style::panel(style::PANEL))
        .width(Fill)
        // `chrome::OPTIONS_BAR_H` is derived from these constants, so that the
        // Sort fields popover can be anchored under this strip without being
        // able to measure it. See `ui::chrome`.
        .padding(
            Padding::new(chrome::OPTIONS_BAR_PAD_Y)
                .left(chrome::CONTENT_PAD_LEFT)
                .right(12.0),
        )
        .into()
}
