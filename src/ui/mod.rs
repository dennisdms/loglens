//! The view layer.
//!
//! Everything below here builds `iced` widgets and nothing else — no state, no
//! `Task`, no I/O. These surfaces were methods on `LogLens` purely to inherit
//! `&self`, though each only ever touched a small, disjoint slice of it; they
//! are free functions taking exactly what they need instead.
//!
//! This module holds the vocabulary they share: the card a modal sits in, a
//! field label, a number written the way a person reads one.

pub(crate) mod chrome;
pub(crate) mod results;

use iced::widget::svg::Handle;
use iced::widget::{button, container, mouse_area, opaque, row, svg, text};
use iced::{Border, Color, Element, Fill, Length, Padding};

use crate::Message;
use crate::icons;
use crate::style::{self, BORDER, ERR_RED, PANEL, TEXT_DIM};

pub(crate) fn field_label<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(12.0).color(TEXT_DIM).into()
}

pub(crate) fn centered<'a>(label: &'a str, color: Color) -> Element<'a, Message> {
    container(text(label.to_string()).size(14.0).color(color))
        .center_x(Fill)
        .center_y(Fill)
        .width(Fill)
        .height(Fill)
        .into()
}

/// Centres `content` in a panel card over a dimmed backdrop.
pub(crate) fn modal_card<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    modal_card_sized(content, 460.0)
}

/// [`modal_card`] with an explicit card width — for modals that need more room
/// than the default (e.g. the Format modal's log-line preview).
pub(crate) fn modal_card_sized<'a>(
    content: Element<'a, Message>,
    width: f32,
) -> Element<'a, Message> {
    let card = container(content).width(width).padding(20.0).style(|_| {
        let mut s = style::panel(PANEL);
        s.border = Border {
            color: BORDER,
            width: 1.0,
            radius: 4.0.into(),
        };
        s
    });

    let backdrop = container(card)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(|_| container::Style {
            background: Some(
                Color {
                    a: 0.6,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..container::Style::default()
        });

    // `opaque` swallows clicks on the backdrop; the `mouse_area` swallows scroll
    // wheel events. Without this a scroll while a modal is open reaches — and
    // moves — the Result Tab behind it. Inner scrollables still scroll: they
    // capture the event first, before it ever reaches this `mouse_area`.
    opaque(mouse_area(backdrop).on_scroll(|_| Message::Ignore))
}

pub(crate) fn meta<'a>(value: &str) -> Element<'a, Message> {
    text(value.to_string()).size(12.0).color(TEXT_DIM).into()
}

/// A solid-red alert pill for the info bar: a warning triangle, `msg`, and a
/// `\u{00d7}` button that clears the notice.
pub(crate) fn error_pill<'a>(run_id: u64, msg: &str) -> Element<'a, Message> {
    container(
        row![
            svg(Handle::clone(&icons::WARNING))
                .width(Length::Fixed(12.0))
                .height(Length::Fixed(12.0))
                .style(|_theme, _status| svg::Style {
                    color: Some(Color::WHITE)
                }),
            text(msg.to_string()).size(12.0).color(Color::WHITE),
            button(text("\u{00d7}").size(12.0).color(Color::WHITE))
                .on_press(Message::DismissTargetError(run_id))
                .padding(Padding::new(0.0).left(4.0).right(4.0))
                .style(style::bare_button()),
        ]
        .spacing(6.0)
        .align_y(iced::Alignment::Center),
    )
    // No vertical padding and a borderless (still rounded) fill so the pill
    // stays within a single line of the info bar — see `info_bar`.
    .padding(Padding::new(0.0).left(8.0).right(4.0))
    .style(|_| {
        let mut s = style::panel(ERR_RED);
        s.border = Border {
            color: ERR_RED,
            width: 0.0,
            radius: 3.0.into(),
        };
        s
    })
    .into()
}

/// Groups an integer into thousands: `1234567` → `1,234,567`.
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
