//! The Result Tab's floating editors: the Format modal (the raw-text
//! template), the Sort fields popover, and the Custom timeframe popover.
//!
//! Each builds only its own card. Where that card is anchored, and what
//! dismisses it, is the shell's business — see `main.rs`.

use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, pick_list, radio, row, rule, scrollable, space, text, text_input,
};
use iced::{Border, Element, Fill, Font, Length, Padding};

use super::rows::part_widget;
use crate::Message;
use crate::config::{TimeUnit, TimeframeMode};
use crate::line;
use crate::results::{ROW_H, ResultTab};
use crate::style::{self, WARN_AMBER};
use crate::ui::{field_label, modal_card_sized};

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
            tab.search.template.clone()
        } else {
            draft.to_string()
        }
    };
    let preview_layout = line::Layout {
        mode: line::LayoutMode::RawText,
        columns: Vec::new(),
        template,
        timestamp_field: tab.search.timestamp_field.clone(),
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
                Some(part) => part_widget(part, None, Wrapping::None),
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

    modal_card_sized(card.into(), 640.0)
}

// --- Sort fields popover --------------------------------------------------

/// The "Sort fields" popover: one row per sort key (remove, direction
/// toggle, reorder) plus a picker to add a field and a "Clear sorting"
/// action. Floated by `LogLens::sort_fields_popover_overlay`.
pub(crate) fn sort_fields_popover<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;
    let last = tab.search.sort.len().saturating_sub(1);

    let mut rows = column![].spacing(4.0);
    for (i, key) in tab.search.sort.iter().enumerate() {
        let field = key.field.clone();

        let remove = button(text("\u{00d7}").size(12.0).color(style::TEXT_DIM))
            .on_press(Message::ResultSortRemove(run_id, field.clone()))
            .padding(2.0)
            .style(style::bare_button());

        let name = container(text(key.field.clone()).size(12.0).color(style::TEXT))
            .width(Length::Fixed(220.0))
            .clip(true);

        let is_time = key.field == tab.search.timestamp_field;
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
    if tab.search.sort.is_empty() {
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
        .filter(|f| tab.search.sort_index(f).is_none())
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
    if !tab.search.sort.is_empty() {
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
