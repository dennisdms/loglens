//! Where the window's fixed chrome ends, and how a floating overlay hangs off
//! it.
//!
//! An overlay is a [`iced::widget::stack`] layer covering the whole window, so
//! nothing tells it where the control it belongs under actually landed; the
//! only way to place one is to know the chrome's geometry outright. Six
//! overlays needed that and each carried its own copy of it, which is how they
//! came to disagree: three different figures for the Menu bar's height between
//! them, none of them the height it is, and none of them accounting for the
//! Update banner, which pushes everything below it down. This module is the
//! one copy.
//!
//! Every height here is **derived** from the same constants the widget is built
//! from — a text `size` times iced's default line height, plus the paddings —
//! and then imposed on that widget with `.height(Fixed(..))`. So the number is
//! not a measurement taken off a screenshot that quietly rots the next time a
//! padding changes: edit a constant and the bar and everything anchored under
//! it move together.

use iced::widget::{column, container, mouse_area, row, space};
use iced::{Element, Fill, Length, Padding};

use crate::Message;

/// iced's default [`iced::widget::text::LineHeight::Relative`] factor: the
/// multiplier from a text widget's `size` to the height it lays out at.
const LINE: f32 = 1.3;

/// A `rule::horizontal(1.0)` / `rule::vertical(1.0)` separator.
pub(crate) const RULE_H: f32 = 1.0;

// --- Sidebar ---------------------------------------------------------------

/// Width of the Connection tree sidebar.
pub(crate) const SIDEBAR_W: f32 = 240.0;

/// Left padding of the Search bar and options strip rows.
pub(crate) const CONTENT_PAD_LEFT: f32 = 12.0;

/// The x of the leftmost control in the Search bar or the options strip: past
/// the sidebar, its rule, and the row's own left padding. Anything anchored
/// under a control at the left of either strip starts here.
pub(crate) const CONTENT_LEFT: f32 = SIDEBAR_W + RULE_H + CONTENT_PAD_LEFT;

/// Right padding of the Search bar row, and so the inset of anything anchored
/// under a control at the right of it.
pub(crate) const CONTENT_PAD_RIGHT: f32 = 12.0;

// --- Menu bar --------------------------------------------------------------

/// Left padding of the Menu bar, and so the x of its first label.
pub(crate) const MENU_BAR_PAD_LEFT: f32 = 8.0;
/// Vertical padding of the Menu bar.
pub(crate) const MENU_BAR_PAD_Y: f32 = 4.0;
/// The width every Menu bar label occupies, whether or not its text fills it.
/// Uniform so that a dropdown's x is [`menu_anchor_x`] arithmetic rather than a
/// number re-measured whenever a label is renamed.
pub(crate) const MENU_LABEL_W: f32 = 46.0;
/// Gap between two Menu bar labels.
pub(crate) const MENU_LABEL_GAP: f32 = 12.0;
/// Text size of a Menu bar label.
pub(crate) const MENU_LABEL_SIZE: f32 = 13.0;
/// Padding around a Menu bar label's text, inside its cell.
pub(crate) const MENU_LABEL_PAD: f32 = 2.0;

/// Height of the Menu bar: a label's text in its padding, in the bar's padding.
pub(crate) const MENU_BAR_H: f32 =
    MENU_BAR_PAD_Y * 2.0 + MENU_LABEL_PAD * 2.0 + MENU_LABEL_SIZE * LINE;

/// The x of the `index`-th Menu bar label, for anchoring its dropdown.
pub(crate) fn menu_anchor_x(index: usize) -> f32 {
    MENU_BAR_PAD_LEFT + index as f32 * (MENU_LABEL_W + MENU_LABEL_GAP)
}

// --- Search bar ------------------------------------------------------------

/// Vertical padding of the Search bar row.
pub(crate) const SEARCH_BAR_PAD_Y: f32 = 6.0;
/// Vertical padding of the Refresh button, the tallest control in the row.
pub(crate) const SEARCH_BAR_BTN_PAD_Y: f32 = 5.0;
/// Side of the Refresh icon.
pub(crate) const SEARCH_ICON: f32 = 16.0;

/// Height of the Search bar: its tallest control (the Refresh button) in the
/// row's padding.
pub(crate) const SEARCH_BAR_H: f32 =
    SEARCH_BAR_PAD_Y * 2.0 + SEARCH_BAR_BTN_PAD_Y * 2.0 + SEARCH_ICON;

// --- Options strip ---------------------------------------------------------

/// Vertical padding of the options strip row.
pub(crate) const OPTIONS_BAR_PAD_Y: f32 = 4.0;
/// Height of the box each of the strip's icons sits in, sized to the sort
/// button's size-14 digit so every button in the strip comes out level.
pub(crate) const OPTIONS_ICON_BOX_H: f32 = 18.0;
/// Vertical padding of a strip button, around its icon box.
pub(crate) const OPTIONS_BTN_PAD_Y: f32 = 5.0;
/// Padding of the bordered surface the Layout buttons are grouped in — the
/// tallest thing in the strip, being a button plus this.
pub(crate) const OPTIONS_GROUP_PAD: f32 = 3.0;

/// Height of the options strip: the Layout group (a button in its bordered
/// surface) in the row's padding.
pub(crate) const OPTIONS_BAR_H: f32 = OPTIONS_BAR_PAD_Y * 2.0
    + OPTIONS_GROUP_PAD * 2.0
    + OPTIONS_BTN_PAD_Y * 2.0
    + OPTIONS_ICON_BOX_H;

// --- Update banner ---------------------------------------------------------

/// Vertical padding of the Update banner.
pub(crate) const BANNER_PAD_Y: f32 = 8.0;
/// Text size of the banner's headline.
pub(crate) const BANNER_TITLE_SIZE: f32 = 13.0;
/// Gap between the headline and the release notes.
pub(crate) const BANNER_SPACING: f32 = 4.0;
/// Height of the release-notes panel. Fixed rather than a maximum: the notes
/// are the one piece of chrome whose content has no bounded length, and an
/// overlay anchored below the banner cannot be placed under a bar whose height
/// depends on how much a stranger wrote in a changelog. Longer notes scroll.
pub(crate) const BANNER_NOTES_H: f32 = 72.0;
/// Vertical padding of the banner's trailing buttons, which are taller than
/// the headline and so set the height when there are no notes.
const BANNER_BTN_PAD_Y: f32 = 4.0;
/// Text size of those buttons.
const BANNER_BTN_SIZE: f32 = 12.0;

/// Height of the Update banner when the Release carries notes.
pub(crate) const BANNER_H_NOTES: f32 =
    BANNER_PAD_Y * 2.0 + BANNER_TITLE_SIZE * LINE + BANNER_SPACING + BANNER_NOTES_H;
/// Height of the Update banner when it is the headline and buttons alone.
pub(crate) const BANNER_H_BARE: f32 =
    BANNER_PAD_Y * 2.0 + BANNER_BTN_PAD_Y * 2.0 + BANNER_BTN_SIZE * LINE;

// --- The metrics an overlay needs ------------------------------------------

/// The undersides of the window's fixed chrome, in window coordinates, for the
/// frame being laid out. Built once by `main_view` and handed to every overlay
/// that hangs off something.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Chrome {
    /// Underside of the Menu bar: where a Menu bar dropdown starts.
    ///
    /// Unaffected by the Update banner. The banner sits below the bar, but a
    /// dropdown belongs to the label it was opened from and floats *over* the
    /// banner rather than being pushed past it.
    pub below_menu_bar: f32,
    /// Underside of the Search bar, or [`Self::below_menu_bar`] plus the banner
    /// when no Result Tab is active and there is no Search bar.
    pub below_search_bar: f32,
    /// Underside of the options strip, or [`Self::below_search_bar`] when the
    /// strip is hidden.
    pub below_options_bar: f32,
}

impl Chrome {
    /// `banner` is the Update banner's height when one is showing. `search_bar`
    /// and `options_bar` are whether those two strips are up; they are not the
    /// same question, since the options strip also hides while a tab has no
    /// results to offer options for.
    pub(crate) fn new(banner: Option<f32>, search_bar: bool, options_bar: bool) -> Self {
        let below_menu_bar = MENU_BAR_H + RULE_H;
        let below_banner = below_menu_bar + banner.map_or(0.0, |h| h + RULE_H);
        let below_search_bar = below_banner
            + if search_bar {
                SEARCH_BAR_H + RULE_H
            } else {
                0.0
            };
        let below_options_bar = below_search_bar
            + if options_bar {
                OPTIONS_BAR_H + RULE_H
            } else {
                0.0
            };
        Self {
            below_menu_bar,
            below_search_bar,
            below_options_bar,
        }
    }
}

/// Where a floating overlay's card sits, in window coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Anchor {
    /// `x` in from the left edge of the window.
    Left { x: f32, y: f32 },
    /// `inset` in from the right edge of the window.
    Right { inset: f32, y: f32 },
}

/// Floats `card` over the whole window at `at`, dismissing on a click — of
/// either button — anywhere outside it.
///
/// The layer fills the window so that the click-away target covers everything
/// behind it, and the card is placed within it by spacers, which is the only
/// positioning a `stack` layer has. This is why [`Chrome`] has to exist: the
/// offsets those spacers need are not something the layer can ask for.
pub(crate) fn anchored<'a>(
    card: Element<'a, Message>,
    at: Anchor,
    dismiss: Message,
) -> Element<'a, Message> {
    let (y, placed) = match at {
        Anchor::Left { x, y } => (y, row![space().width(x), card, space().width(Fill)]),
        Anchor::Right { inset, y } => (y, row![space().width(Fill), card, space().width(inset)]),
    };

    let layer = container(column![space().height(y), placed])
        .width(Fill)
        .height(Fill);

    mouse_area(layer)
        .on_press(dismiss.clone())
        .on_right_press(dismiss)
        .into()
}

/// Padding of a dropdown's popup surface. iced expands a `Fixed` size by the
/// container's padding rather than fitting the content inside it, so a popup
/// declared `width` wide occupies `width + MENU_POPUP_PAD * 2` on screen — the
/// reason the bar heights above are derived from their inputs rather than
/// imposed with `.height(Fixed(..))`, which would inflate them the same way.
pub(crate) const MENU_POPUP_PAD: f32 = 3.0;

/// A dropdown block hung under a Menu bar label, in a popup surface whose
/// content is `width` wide.
pub(crate) fn menu_popup<'a>(
    body: impl Into<Element<'a, Message>>,
    width: f32,
) -> Element<'a, Message> {
    container(body.into())
        .width(Length::Fixed(width))
        .padding(Padding::new(MENU_POPUP_PAD))
        .style(|_| crate::style::menu_popup())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three bar heights, checked against the pixels they actually occupy.
    ///
    /// Measured off a running window: the Menu bar fills rows 0..=28 with its
    /// rule on row 29, the Search bar 30..=67 with its rule on 68, and the
    /// options strip 69..=110 with its rule on 111. That is what these
    /// constants are *for* — an overlay is placed by arithmetic on them and
    /// cannot check the answer — so the arithmetic is pinned here rather than
    /// left to be re-measured by whoever next notices a popover sitting wrong.
    #[test]
    fn bar_heights_match_the_pixels_they_occupy() {
        assert_eq!(MENU_BAR_H.round(), 29.0);
        assert_eq!(SEARCH_BAR_H.round(), 38.0);
        assert_eq!(OPTIONS_BAR_H.round(), 42.0);
    }

    /// Sidebar (240) + its rule (1) + the row's left padding (12), which is
    /// where the leftmost control in either strip starts.
    #[test]
    fn content_starts_past_the_sidebar_and_its_padding() {
        assert_eq!(CONTENT_LEFT, 253.0);
    }

    #[test]
    fn each_strip_pushes_the_one_below_it_down() {
        let bare = Chrome::new(None, false, false);
        assert_eq!(bare.below_menu_bar, MENU_BAR_H + RULE_H);
        // With no strips there is nothing between them to anchor under.
        assert_eq!(bare.below_search_bar, bare.below_menu_bar);
        assert_eq!(bare.below_options_bar, bare.below_menu_bar);

        // The options strip hides on its own while the Search bar stays.
        let search_only = Chrome::new(None, true, false);
        assert_eq!(
            search_only.below_search_bar,
            search_only.below_menu_bar + SEARCH_BAR_H + RULE_H
        );
        assert_eq!(search_only.below_options_bar, search_only.below_search_bar);

        let both = Chrome::new(None, true, true);
        assert_eq!(
            both.below_options_bar,
            both.below_search_bar + OPTIONS_BAR_H + RULE_H
        );
    }

    /// The bug this module exists to stop: an Update banner appears between the
    /// Menu bar and the Search bar and pushes everything below it down, but a
    /// Menu bar dropdown belongs to its label and floats over the banner rather
    /// than being pushed past it.
    #[test]
    fn the_update_banner_displaces_the_strips_but_not_a_menu_dropdown() {
        let without = Chrome::new(None, true, true);
        let with = Chrome::new(Some(BANNER_H_NOTES), true, true);

        assert_eq!(with.below_menu_bar, without.below_menu_bar);

        let shift = BANNER_H_NOTES + RULE_H;
        assert_eq!(with.below_search_bar, without.below_search_bar + shift);
        assert_eq!(with.below_options_bar, without.below_options_bar + shift);
    }

    /// Both banner heights are fixed — an overlay below the banner cannot be
    /// placed under a bar whose height depends on how long a changelog someone
    /// wrote — and the one carrying notes displaces the strips further.
    #[test]
    fn a_banner_with_notes_displaces_more_than_one_without() {
        let bare = Chrome::new(Some(BANNER_H_BARE), true, true);
        let noted = Chrome::new(Some(BANNER_H_NOTES), true, true);

        assert!(noted.below_search_bar > bare.below_search_bar);
        assert_eq!(
            noted.below_search_bar - bare.below_search_bar,
            BANNER_H_NOTES - BANNER_H_BARE
        );
    }
}
