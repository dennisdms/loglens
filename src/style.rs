//! Central place for the application's colour palette and widget styling.

use iced::widget::{button, container, text_editor};
use iced::{Background, Border, Color, Theme};

// --- Palette --------------------------------------------------------------

/// Window / log background.
pub const BG: Color = Color::from_rgb8(0x1e, 0x1e, 0x1e);
/// Default panel surface (file picker, toolbar).
pub const PANEL: Color = Color::from_rgb8(0x25, 0x25, 0x26);
/// Slightly raised surface (tab strip, inactive tabs).
pub const PANEL_ALT: Color = Color::from_rgb8(0x2d, 0x2d, 0x30);
/// Hairline borders.
pub const BORDER: Color = Color::from_rgb8(0x3c, 0x3c, 0x3c);
/// Primary text.
pub const TEXT: Color = Color::from_rgb8(0xd4, 0xd4, 0xd4);
/// Muted / secondary text.
pub const TEXT_DIM: Color = Color::from_rgb8(0x8a, 0x8a, 0x8a);
/// Selection / active highlight.
pub const ACCENT: Color = Color::from_rgb8(0x09, 0x47, 0x71);
/// Pointer-hover highlight for menu / list rows. Deliberately lighter than
/// `PANEL_ALT` so it stays visible on the floating menu card, which is itself
/// filled with `PANEL_ALT`.
pub const HOVER: Color = Color::from_rgb8(0x37, 0x37, 0x3d);

// --- Widget styles -------------------------------------------------------

/// A flat panel filled with `background`, using palette text and border colours.
pub fn panel(background: Color) -> container::Style {
    container::Style {
        background: Some(background.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// The log text editor: borderless, palette colours, accent selection.
pub fn editor(_theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(BG),
        border: Border {
            color: BORDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        placeholder: TEXT_DIM,
        value: TEXT,
        selection: ACCENT,
    }
}

/// A floating context-menu card: raised surface, hairline border, rounded.
pub fn menu_popup() -> container::Style {
    container::Style {
        background: Some(PANEL_ALT.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

/// A file-picker row; filled when `active`, faintly lit on hover.
pub fn picker_row(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match (active, status) {
            (true, _) => Some(ACCENT.into()),
            (false, button::Status::Hovered) => Some(HOVER.into()),
            _ => None,
        };
        button::Style {
            background,
            text_color: TEXT,
            border: Border::default(),
            ..button::Style::default()
        }
    }
}

/// A button with no chrome of its own (tab label, close affordance).
pub fn bare_button() -> impl Fn(&Theme, button::Status) -> button::Style {
    |_theme, _status| button::Style {
        background: None,
        text_color: TEXT,
        border: Border::default(),
        ..button::Style::default()
    }
}
