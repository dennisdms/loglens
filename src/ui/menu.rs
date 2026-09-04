//! The window's own furniture: the Menu bar and the two dropdowns that hang
//! under it, the Update banner below that, the tab strip, and the status and
//! info bars along the bottom. Plus the About dialog, which is only ever
//! reached from the Help menu.
//!
//! None of it belongs to a tab \u{2014} it is what surrounds whichever tab is open,
//! and it is drawn from a handful of scattered flags on `LogLens` rather than
//! from any one piece of state. The dropdowns are stack layers over the whole
//! window, so they are told where the chrome ended rather than measuring it;
//! see [`crate::ui::chrome`].

use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{Border, Color, Element, Fill, Length, Padding};

use crate::results::{ResultTab, TotalHits};
use crate::style::{self, ACCENT, BG, ERR_RED, PANEL_ALT, TEXT, TEXT_DIM};
use crate::tab::Tab;
use crate::ui::chrome::{self, Anchor, Chrome, anchored, menu_anchor_x};
use crate::ui::{error_pill, field_label, meta, modal_card, thousands};
use crate::{APP_NAME, Message, Updating, VERSION, crashlog, update};

/// The always-present Menu bar across the top of the window. `File` and
/// `Help` open dropdowns (see [`file_overlay`] and [`help_overlay`]);
/// `View` is still inert.
///
/// Every label occupies the same fixed cell width. A dropdown is a
/// free-floating overlay layer stacked over the whole window, so nothing
/// tells it where the label it hangs under actually landed; uniform cells
/// turn the anchor into [`menu_anchor_x`] arithmetic instead of a number
/// measured off a screenshot and re-measured whenever a label is renamed.
pub(crate) fn bar<'a>(file_open: bool, help_open: bool) -> Element<'a, Message> {
    container(
        row![
            bar_label("File", file_open, Message::FileMenuToggle),
            // Inert, so it is rendered as dimmed text in a cell of the same
            // width rather than as a button that does nothing when pressed.
            container(text("View").size(chrome::MENU_LABEL_SIZE).color(TEXT_DIM))
                .width(Length::Fixed(chrome::MENU_LABEL_W))
                .center_x(Fill),
            bar_label("Help", help_open, Message::HelpMenuToggle),
        ]
        .spacing(chrome::MENU_LABEL_GAP)
        .align_y(iced::Alignment::Center),
    )
    .style(|_| style::panel(PANEL_ALT))
    .width(Fill)
    // Height comes out of these constants, and `chrome::MENU_BAR_H` is
    // derived from the same ones — the dropdowns anchored under this bar
    // have no way to measure where it actually ended. See `ui::chrome`.
    .padding(
        Padding::new(chrome::MENU_BAR_PAD_Y)
            .left(chrome::MENU_BAR_PAD_LEFT)
            .right(12.0),
    )
    .into()
}

/// One Menu bar label that opens a dropdown, in a cell of the shared width so
/// [`menu_anchor_x`] can place that dropdown underneath it.
fn bar_label<'a>(label: &'a str, open: bool, toggle: Message) -> Element<'a, Message> {
    button(
        text(label)
            .size(chrome::MENU_LABEL_SIZE)
            .color(TEXT)
            .width(Fill)
            .center(),
    )
    .on_press(toggle)
    .width(Length::Fixed(chrome::MENU_LABEL_W))
    .padding(Padding::new(chrome::MENU_LABEL_PAD))
    .style(style::picker_row(open))
    .into()
}

/// The floating "File" dropdown, anchored under its Menu bar label.
pub(crate) fn file_overlay<'a>(open: bool, metrics: &Chrome) -> Option<Element<'a, Message>> {
    if !open {
        return None;
    }
    let block = chrome::menu_popup(
        button(text("Settings").size(12.0).color(TEXT))
            .on_press(Message::OpenSettings)
            .width(Fill)
            .padding(Padding::new(4.0).left(10.0).right(10.0))
            .style(style::picker_row(false)),
        150.0,
    );

    Some(anchored(
        block,
        // Index 0: the bar reads File, View, Help.
        Anchor::Left {
            x: menu_anchor_x(0),
            y: metrics.below_menu_bar,
        },
        Message::FileMenuDismiss,
    ))
}

/// The floating "Help" dropdown, anchored under its Menu bar label.
pub(crate) fn help_overlay<'a>(
    open: bool,
    checking: bool,
    metrics: &Chrome,
) -> Option<Element<'a, Message>> {
    if !open {
        return None;
    }

    // While a check is in flight the item says so and stops responding.
    // The unauthenticated GitHub API allows 60 requests an hour per IP,
    // shared by everyone behind an office NAT, and a menu item that looks
    // like it did nothing invites exactly the repeated clicking that spends
    // them.
    let check = button(
        text(if checking {
            "Checking for updates\u{2026}"
        } else {
            "Check for updates\u{2026}"
        })
        .size(12.0)
        .color(if checking { TEXT_DIM } else { TEXT }),
    )
    .on_press_maybe((!checking).then_some(Message::CheckForUpdates))
    .width(Fill)
    .padding(Padding::new(4.0).left(10.0).right(10.0))
    .style(style::picker_row(false));

    let block = chrome::menu_popup(
        column![
            check,
            button(text("About").size(12.0).color(TEXT))
                .on_press(Message::OpenAbout)
                .width(Fill)
                .padding(Padding::new(4.0).left(10.0).right(10.0))
                .style(style::picker_row(false)),
        ]
        .spacing(1.0),
        178.0,
    );

    Some(anchored(
        block,
        // Index 2: the bar reads File, View, Help.
        Anchor::Left {
            x: menu_anchor_x(2),
            y: metrics.below_menu_bar,
        },
        Message::HelpMenuDismiss,
    ))
}

/// The Update banner: a strip directly below the Menu bar naming the newer
/// Release, showing its notes, and carrying a \u{00d7} that hides it.
///
/// A banner rather than a modal, because a new Release is never urgent and
/// a modal would take the window away from whatever query the user is in
/// the middle of reading. A banner rather than an indicator dot, because a
/// dot is too easy to never notice at all.
///
/// Dismissing hides it for the rest of the session only \u{2014} see
/// `new_release`.
///
/// What it offers depends on the Install flavour. An installer-managed copy
/// gets an Update button. A Portable copy gets the releases page and no
/// button: running the installer from a copy on a USB stick would install a
/// second one into `%LOCALAPPDATA%` while the user carried on running this
/// one, so the honest offer is the download.
/// Returns the banner and the height it was built to occupy — the one
/// piece of chrome that cannot be a constant, since it is there or not
/// depending on whether a Release was found, and two different heights
/// depending on whether that Release carried notes. Everything anchored
/// below it is displaced by exactly this, so it is reported rather than
/// guessed at from the outside. See [`Chrome`].
pub(crate) fn update_banner<'a>(
    release: Option<&update::Release>,
    updating: Option<&Updating>,
    flavour: &update::Flavour,
) -> Option<(Element<'a, Message>, f32)> {
    let release = release?;

    let mut left = column![
        text(format!("{APP_NAME} {} is available.", release.version))
            .size(chrome::BANNER_TITLE_SIZE)
            .color(Color::WHITE),
    ]
    .spacing(chrome::BANNER_SPACING);

    let notes = release.notes.trim();
    let height = if notes.is_empty() {
        chrome::BANNER_H_BARE
    } else {
        // GitHub's generated notes are markdown of no fixed length, shown
        // as the plain text they are. Given a fixed height — not merely a
        // maximum — so that a long changelog neither pushes the tab strip
        // off the bottom of the window nor moves the overlays anchored
        // under this banner. Longer notes scroll within it.
        left = left.push(
            container(scrollable(text(notes.to_string()).size(12.0).color(
                Color {
                    a: 0.85,
                    ..Color::WHITE
                },
            )))
            .height(Length::Fixed(chrome::BANNER_NOTES_H)),
        );
        chrome::BANNER_H_NOTES
    };

    let portable = flavour.installed_exe().is_none();
    if portable {
        left = left.push(
            text(
                "This copy is portable, so it does not update itself. \
                 Download the new version from the releases page.",
            )
            .size(12.0)
            .color(Color {
                a: 0.85,
                ..Color::WHITE
            }),
        );
    }

    // A failed download or a failed apply is always shown: the user pressed
    // a button, and unlike a background Update check nobody is being
    // spared an interruption they did not ask for.
    if let Some(Updating::Failed(err)) = updating {
        left = left.push(
            text(format!("Update failed: {err}"))
                .size(12.0)
                .color(ERR_RED),
        );
    }

    // The Update button belongs to installer-managed copies only, and goes
    // away while one is running so it cannot be pressed twice.
    let mut trailing: Vec<Element<'_, Message>> = Vec::new();
    if let Some(Updating::Busy(step)) = updating {
        trailing.push(
            text(*step)
                .size(12.0)
                .color(Color {
                    a: 0.85,
                    ..Color::WHITE
                })
                .into(),
        );
    } else if !portable {
        // Pressing it again after a failure is the user's call to make;
        // what must never happen is a retry they did not ask for.
        let again = matches!(updating, Some(Updating::Failed(_)));
        trailing.push(
            button(
                text(if again { "Try again" } else { "Update" })
                    .size(12.0)
                    .color(TEXT),
            )
            .on_press(Message::ApplyUpdate)
            .padding(Padding::new(4.0).left(12.0).right(12.0))
            .style(style::icon_button(false))
            .into(),
        );
    }

    // The way to the Release for anyone who cannot, or could not, be
    // updated in place. Every failure path keeps this beside it.
    if portable || matches!(updating, Some(Updating::Failed(_))) {
        trailing.push(
            button(text("Releases page").size(12.0).color(TEXT))
                .on_press(Message::OpenReleasesPage)
                .padding(Padding::new(4.0).left(12.0).right(12.0))
                .style(style::icon_button(false))
                .into(),
        );
    }

    Some((
        container(
            row![
                left.width(Fill),
                space().width(12.0),
                row(trailing).spacing(6.0).align_y(iced::Alignment::Center),
                space().width(8.0),
                button(text("\u{00d7}").size(14.0).color(Color::WHITE))
                    .on_press(Message::DismissUpdateBanner)
                    .padding(Padding::new(2.0).left(6.0).right(6.0))
                    .style(style::bare_button()),
            ]
            .align_y(iced::Alignment::Start),
        )
        .style(|_| style::panel(ACCENT))
        .width(Fill)
        .padding(Padding::new(chrome::BANNER_PAD_Y).left(12.0).right(8.0))
        .into(),
        height,
    ))
}

/// The About dialog: what this build is, where it came from, and where it
/// leaves a trace when it crashes.
///
/// An overlay modal in the main window rather than a second OS window like
/// Settings. Settings earns a window of its own because it is an editor
/// people leave open beside the app while they work; About is read once and
/// dismissed, and giving four lines of text its own taskbar button and
/// alt-tab entry costs more than it returns.
pub(crate) fn about_modal<'a>() -> Element<'a, Message> {
    // Shown as a path rather than opened, so a user asked for the crash log
    // can find the file. `None` only on a system with no data directory at
    // all, where there is no crash log to point at.
    let crash_log = crashlog::log_path().map_or_else(
        || "unavailable: no data directory on this system".to_string(),
        |path| path.display().to_string(),
    );

    let card = column![
        text("About Log Lens").size(16.0).color(TEXT),
        space().height(2.0),
        // `VERSION`, not the crate version the Update check compares
        // against: the commit hash is what makes the first bug report
        // against "0.1.0" say which 0.1.0.
        text(format!("{APP_NAME} {VERSION}")).size(13.0).color(TEXT),
        space().height(8.0),
        field_label("Repository"),
        text(update::REPOSITORY_URL).size(12.0).color(TEXT_DIM),
        space().height(8.0),
        field_label("Crash log"),
        text(crash_log).size(12.0).color(TEXT_DIM),
        space().height(12.0),
        row![
            space().width(Fill),
            button(text("Close").size(13.0).color(TEXT))
                .on_press(Message::CloseAbout)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ],
    ]
    .spacing(4.0)
    .width(Fill);

    modal_card(card.into())
}

pub(crate) fn tab_bar<'a>(open_tabs: &'a [Tab], active_tab: Option<usize>) -> Element<'a, Message> {
    if open_tabs.is_empty() {
        return container(space().height(34.0))
            .style(|_| style::panel(PANEL_ALT))
            .width(Fill)
            .into();
    }

    let tabs = open_tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| -> Element<'_, Message> {
            let active = active_tab == Some(i);
            let name = tab.title();

            container(
                row![
                    button(
                        text(name)
                            .size(13.0)
                            .color(if active { TEXT } else { TEXT_DIM })
                    )
                    .on_press(Message::SelectTab(i))
                    .padding(Padding::new(6.0).left(12.0).right(6.0))
                    .style(style::bare_button()),
                    button(text("\u{00d7}").size(13.0).color(TEXT_DIM))
                        .on_press(Message::CloseTab(i))
                        .padding(Padding::new(6.0).left(2.0).right(10.0))
                        .style(style::bare_button()),
                ]
                .align_y(iced::Alignment::Center),
            )
            .style(move |_| {
                let mut s = style::panel(if active { BG } else { PANEL_ALT });
                if active {
                    s.border = Border {
                        color: ACCENT,
                        width: 0.0,
                        ..Border::default()
                    };
                }
                s
            })
            .into()
        });

    container(row(tabs).width(Fill))
        .style(|_| style::panel(PANEL_ALT))
        .width(Fill)
        .into()
}

pub(crate) fn status_bar<'a>(status: Option<&str>) -> Element<'a, Message> {
    let Some(status) = status else {
        return space().height(0.0).into();
    };
    container(
        row![
            text(status.to_string()).size(12.0).color(TEXT),
            space().width(Fill),
            button(text("Dismiss").size(12.0).color(TEXT_DIM))
                .on_press(Message::DismissStatus)
                .padding(2.0)
                .style(style::bare_button()),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(|_| style::panel(PANEL_ALT))
    .width(Fill)
    .padding(Padding::new(4.0).left(12.0).right(12.0))
    .into()
}

/// A persistent info bar across the very bottom of the window, carrying
/// summary details for the active tab: the loaded-Hit count for a Result
/// Tab on the left, and a failed Target switch (a red outlined pill) on
/// the right.
pub(crate) fn info_bar<'a>(active: Option<&'a ResultTab>, spinner: usize) -> Element<'a, Message> {
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let Some(tab) = active {
        items.push(hit_count_readout(tab, spinner));
        if let Some(err) = &tab.target_error {
            items.push(space().width(Fill).into());
            items.push(error_pill(tab.run_id, err));
        }
    }
    // Fixed height so a transient `error_pill` (taller than the plain
    // hit-count readout) can't nudge the whole bar upward when it appears.
    container(row(items).spacing(12.0).align_y(iced::Alignment::Center))
        .style(|_| style::panel(PANEL_ALT))
        .width(Fill)
        .height(Length::Fixed(24.0))
        .align_y(iced::alignment::Vertical::Center)
        .padding(Padding::new(0.0).left(12.0).right(12.0))
        .into()
}

/// The bottom-bar Hit-count readout: how many Hits are loaded into the
/// table, then the total matching Hits once `_count` lands — an animated
/// spinner stands in for the total while it is still in flight, and it is
/// dropped entirely if the count failed.
fn hit_count_readout<'a>(tab: &'a ResultTab, spinner: usize) -> Element<'a, Message> {
    let loaded = thousands(tab.hits.len() as u64);
    match tab.total_hits {
        TotalHits::Loading => row![
            meta(&format!("Loaded {loaded} of")),
            text(spinner_frame(spinner)).size(12.0).color(TEXT_DIM),
            meta("hits"),
        ]
        .spacing(5.0)
        .align_y(iced::Alignment::Center)
        .into(),
        TotalHits::Known(total) => meta(&format!("Loaded {loaded} of {} hits", thousands(total))),
        TotalHits::Failed => meta(&format!("Loaded {loaded} hits")),
    }
}

/// One frame of the braille activity spinner, chosen by a monotonic counter.
fn spinner_frame(frame: usize) -> &'static str {
    const FRAMES: [&str; 10] = [
        "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
        "\u{2827}", "\u{2807}", "\u{280f}",
    ];
    FRAMES[frame % FRAMES.len()]
}
