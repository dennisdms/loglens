//! The Settings window.
//!
//! A second OS window rather than a modal: this is an editor people leave open
//! beside the app while they work. Its fields edit a [`SettingsDraft`], not the
//! `Config` itself \u{2014} nothing here takes effect until Save parses and clamps
//! the drafts back into it.

use iced::widget::{button, column, container, row, rule, scrollable, space, text, text_input};
use iced::{Element, Fill, Length, Padding};

use crate::style::{self, BG, ERR_RED, TEXT, TEXT_DIM};
use crate::ui::field_label;
use crate::{Message, SettingsDraft};

/// The Settings window body: an Elasticsearch page with the two fetch limits
/// and a Display page with the wrap cap. Rendered whenever `view` is asked for
/// the Settings window rather than the main one.
pub(crate) fn view<'a>(draft: &'a SettingsDraft) -> Element<'a, Message> {
    let max_results = column![
        field_label("Max Results"),
        text("Stop fetching once a tab has loaded this many documents.")
            .size(11.0)
            .color(TEXT_DIM),
        text_input("", &draft.max_results)
            .on_input(Message::SettingsMaxResults)
            .on_submit(Message::SettingsSave)
            .padding(6.0)
            .width(Length::Fixed(140.0)),
    ]
    .spacing(4.0);

    let fetch_size = column![
        field_label("Fetch size"),
        text("Documents per request while paging (max 10,000).")
            .size(11.0)
            .color(TEXT_DIM),
        text_input("", &draft.fetch_size)
            .on_input(Message::SettingsFetchSize)
            .on_submit(Message::SettingsSave)
            .padding(6.0)
            .width(Length::Fixed(140.0)),
    ]
    .spacing(4.0);

    let wrap_cap = column![
        field_label("Wrap row cap"),
        text(
            "With Wrap on, the most visual rows one Hit shows before a \
             \u{201c}\u{2026} more lines\u{201d} toggle. Blank = no cap."
        )
        .size(11.0)
        .color(TEXT_DIM),
        text_input("none", &draft.wrap_row_cap)
            .on_input(Message::SettingsWrapCap)
            .on_submit(Message::SettingsSave)
            .padding(6.0)
            .width(Length::Fixed(140.0)),
    ]
    .spacing(4.0);

    let mut col = column![
        text("Elasticsearch").size(16.0).color(TEXT),
        text(
            "How many log documents Log Lens pulls from a cluster, and in \
             what size batches."
        )
        .size(11.0)
        .color(TEXT_DIM),
        space().height(4.0),
        max_results,
        fetch_size,
        rule::horizontal(1.0),
        text("Display").size(16.0).color(TEXT),
        space().height(4.0),
        wrap_cap,
    ]
    .spacing(10.0);

    if let Some(err) = &draft.error {
        col = col.push(text(err.clone()).size(12.0).color(ERR_RED));
    }

    col = col.push(space().height(6.0));
    col = col.push(
        row![
            button(text("Save").size(13.0).color(TEXT))
                .on_press(Message::SettingsSave)
                .padding(Padding::new(6.0).left(16.0).right(16.0))
                .style(style::picker_row(true)),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::SettingsClose)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
        ]
        .spacing(8.0),
    );

    container(scrollable(col).height(Fill))
        .style(|_| style::panel(BG))
        .width(Fill)
        .height(Fill)
        .padding(20.0)
        .into()
}
