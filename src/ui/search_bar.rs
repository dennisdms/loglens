//! The Search bar and the options strip: the two chrome strips that appear
//! above the tab strip while a Result Tab is active, and the three overlays
//! that hang off them.
//!
//! All five surfaces are a function of exactly one [`ResultTab`] and nothing
//! else in the application — which is why they take one rather than `&self`,
//! and why [`crate::LogLens::active_result`] exists to hand it to them. The
//! three overlays additionally take the [`Chrome`] metrics, since a stack
//! layer covers the whole window and cannot ask where the control it belongs
//! under landed. See [`crate::ui::chrome`].

use iced::widget::svg::Handle;
use iced::widget::{button, column, container, pick_list, row, svg, text, text_input};
use iced::{Element, Fill, Length, Padding};

use crate::config::TimeframeChoice;
use crate::results::{Msg, ResultTab};
use crate::style::{self, PANEL, TEXT, TEXT_DIM};
use crate::ui::chrome::{self, Anchor, Chrome, anchored};
use crate::{Message, icons, ui};

/// The options strip shown below the Search bar, directly above the tab
/// strip: the live Column + sort controls moved out of the Result Tab.
/// `None` while the tab has nothing to show them for.
pub(crate) fn options_bar<'a>(tab: &'a ResultTab) -> Option<Element<'a, Message>> {
    if !tab.strips_visible() {
        return None;
    }

    // The "Sort fields" popover is *not* pushed inline here — it floats as a
    // stack layer ([`sort_overlay`]) so opening it never reflows
    // the strips or table below.
    Some(ui::results::result_sort_bar(tab))
}

/// The Search bar shown at the top of the right column, above the options
/// strip and tab strip: the index/datastream target, query string, timeframe
/// and a Refresh control, in that order. The loaded-Hit count lives in the
/// bottom info bar.
pub(crate) fn search_bar<'a>(tab: &'a ResultTab) -> Element<'a, Message> {
    let run_id = tab.run_id;

    let selected = tab
        .search
        .timeframe
        .matches_preset()
        .unwrap_or(TimeframeChoice::Custom);
    let timeframe_ctl = pick_list(&TimeframeChoice::ALL[..], Some(selected), move |choice| {
        Message::Result(run_id, Msg::TimeframeChoice(choice))
    })
    .text_size(12.0)
    .padding(4.0);

    // The Target is edited inline here, Kibana-style: typing opens a
    // suggestion dropdown ([`target_overlay`]) and the caret
    // button toggles it; a pick or Enter re-points the tab. Free text
    // (patterns like `logs-*`) commits on Enter without appearing in the
    // list.
    let target_ctl = row![
        text_input("index or data stream", &tab.target_draft)
            .on_input(move |v| Message::Result(run_id, Msg::TargetDraft(v)))
            .on_submit(Message::ResultTargetSubmit(run_id))
            .size(12.0)
            .padding(4.0)
            .width(Length::Fixed(160.0)),
        button(text("\u{25be}").size(9.0).color(TEXT_DIM))
            .on_press(Message::Result(run_id, Msg::TargetPanelToggle))
            .padding(Padding::new(4.0).left(6.0).right(6.0))
            .style(style::picker_row(tab.target_panel_open)),
    ]
    .spacing(2.0)
    .align_y(iced::Alignment::Center);

    let row1 = container(
        row![
            target_ctl,
            text_input("Lucene Query", &tab.query_draft)
                .on_input(move |v| Message::Result(run_id, Msg::QueryDraft(v)))
                .on_submit(Message::Result(run_id, Msg::QuerySubmit))
                .size(12.0)
                .padding(4.0)
                .width(Fill),
            timeframe_ctl,
            button(
                svg(Handle::clone(&icons::REFRESH))
                    .width(Length::Fixed(chrome::SEARCH_ICON))
                    .height(Length::Fixed(chrome::SEARCH_ICON))
                    .style(|_theme, _status| svg::Style { color: Some(TEXT) }),
            )
            .on_press(Message::RefreshResult(run_id))
            .padding(
                Padding::new(chrome::SEARCH_BAR_BTN_PAD_Y)
                    .left(9.0)
                    .right(9.0)
            )
            .style(style::icon_button(false)),
        ]
        .spacing(12.0)
        .align_y(iced::Alignment::Center),
    )
    .style(|_| style::panel(PANEL))
    .width(Fill)
    // As with the Menu bar, `chrome::SEARCH_BAR_H` is derived from these
    // constants: the timeframe popover and the Target dropdown are
    // anchored under this row.
    .padding(
        Padding::new(chrome::SEARCH_BAR_PAD_Y)
            .left(chrome::CONTENT_PAD_LEFT)
            .right(chrome::CONTENT_PAD_RIGHT),
    );

    // The raw-text template is edited in the "Format" modal (opened from the
    // options strip), not here — the Search bar stays a single row.
    row1.into()
}

/// The floating "Custom\u{2026}" timeframe editor, anchored under the Search
/// bar's timeframe control as a stack layer so it never reflows the strips
/// or main area below it (the options strip sits below the Search bar now).
/// Mirrors the sidebar right-click menu: a click anywhere outside dismisses
/// it.
pub(crate) fn timeframe_overlay<'a>(
    tab: &'a ResultTab,
    metrics: &Chrome,
) -> Option<Element<'a, Message>> {
    if !tab.tf.open {
        return None;
    }
    let run_id = tab.run_id;

    const CARD_W: f32 = 480.0;

    let card = container(ui::results::timeframe_popover(tab)).width(Length::Fixed(CARD_W));

    Some(anchored(
        card.into(),
        // Under the Search bar's timeframe control, at the right of the row.
        Anchor::Right {
            inset: chrome::CONTENT_PAD_RIGHT,
            y: metrics.below_search_bar,
        },
        Message::Result(run_id, Msg::TfCancel),
    ))
}

/// The Search bar's Target suggestion dropdown, floated as a stack layer
/// under the Target input so it never reflows the strips or table below.
/// Anchored with the same top offset as the timeframe popover; a click
/// anywhere outside dismisses it.
pub(crate) fn target_overlay<'a>(
    tab: &'a ResultTab,
    metrics: &Chrome,
) -> Option<Element<'a, Message>> {
    if !tab.target_panel_open {
        return None;
    }
    let run_id = tab.run_id;

    const CARD_W: f32 = 240.0;

    let body: Element<'_, Message> = if tab.targets_loading {
        text("Loading indices\u{2026}")
            .size(11.0)
            .color(TEXT_DIM)
            .into()
    } else {
        let matches = tab.target_matches();
        if matches.is_empty() {
            text("No matching indices")
                .size(11.0)
                .color(TEXT_DIM)
                .into()
        } else {
            let mut opts = column![].spacing(1.0);
            for name in matches {
                opts = opts.push(
                    button(text(name.clone()).size(12.0))
                        .on_press(Message::ResultTargetPicked(run_id, name.clone()))
                        .width(Fill)
                        .padding(Padding::new(3.0).left(8.0))
                        .style(style::picker_row(false)),
                );
            }
            opts.into()
        }
    };

    let card = container(container(body).padding(4.0))
        .style(|_| style::panel(PANEL))
        .width(Length::Fixed(CARD_W));

    Some(anchored(
        card.into(),
        // Under the Target input, the leftmost control in the Search bar.
        Anchor::Left {
            x: chrome::CONTENT_LEFT,
            y: metrics.below_search_bar,
        },
        Message::Result(run_id, Msg::TargetPanelDismiss),
    ))
}

/// The floating "Sort fields" editor, anchored under the options strip's
/// "Sort fields" button as a stack layer so it never reflows the strips or
/// main area below it. A click anywhere outside dismisses it.
pub(crate) fn sort_overlay<'a>(
    tab: &'a ResultTab,
    metrics: &Chrome,
) -> Option<Element<'a, Message>> {
    if !tab.sort_panel_open {
        return None;
    }
    if !tab.strips_visible() {
        return None;
    }
    let run_id = tab.run_id;

    const CARD_W: f32 = 460.0;

    let card = container(ui::results::sort_fields_popover(tab)).width(Length::Fixed(CARD_W));

    Some(anchored(
        card.into(),
        // Under the "Sort fields" button, the leftmost control in the strip.
        Anchor::Left {
            x: chrome::CONTENT_LEFT,
            y: metrics.below_options_bar,
        },
        Message::Result(run_id, Msg::SortPanelDismiss),
    ))
}
