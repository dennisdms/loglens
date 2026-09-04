//! The sidebar Connection tree and the right-click menu that hangs off it.
//!
//! The tree is a function of three things: the Connections it lists, which
//! nodes are expanded, and which Saved Search is showing in the active tab so
//! its row can be highlighted. Its right-click menu is a function of which
//! node was clicked and where the pointer was — and, being a stack layer that
//! covers the whole window, of nothing about the tree's own layout. See
//! [`crate::ui::chrome`].

use std::collections::HashSet;

use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Element, Fill, Padding, Point};

use crate::config::Connection;
use crate::style::{self, ERR_RED, PANEL, TEXT, TEXT_DIM};
use crate::ui::chrome::{self, Anchor, anchored};
use crate::workspace::ES_ROOT;
use crate::{Message, TreeMenu};

/// Inner width of the tree right-click menu.
const MENU_W: f32 = 130.0;
/// Its width on screen: iced expands a `Fixed` size by the container's padding,
/// so the popup surface is this much wider than the block inside it.
const MENU_OUTER_W: f32 = MENU_W + chrome::MENU_POPUP_PAD * 2.0;

/// The left-hand tree pane.
///
/// `active_saved` is the Saved Search showing in the active tab, if any — the
/// one row in the tree drawn as selected.
pub(crate) fn sidebar<'a>(
    connections: &'a [Connection],
    expanded: &'a HashSet<String>,
    active_saved: Option<&str>,
) -> Element<'a, Message> {
    let panel = container(
        scrollable(
            column![es_section(connections, expanded, active_saved)]
                .spacing(1.0)
                .width(Fill),
        )
        .height(Fill),
    )
    .style(|_| style::panel(PANEL))
    .width(chrome::SIDEBAR_W)
    .height(Fill)
    .padding(6.0);

    mouse_area(panel).on_move(Message::SidebarCursor).into()
}

/// The `Elasticsearch` tree root: its Connections plus a "＋" affordance.
fn es_section<'a>(
    connections: &'a [Connection],
    expanded: &'a HashSet<String>,
    active_saved: Option<&str>,
) -> Element<'a, Message> {
    let open = expanded.contains(ES_ROOT);
    let marker = if open { "\u{25be}" } else { "\u{25b8}" };

    let header = row![
        button(
            row![
                text(marker).size(11.0).color(TEXT_DIM),
                text("Elasticsearch").size(13.0),
            ]
            .spacing(6.0),
        )
        .on_press(Message::ToggleFolder(ES_ROOT.to_string()))
        .width(Fill)
        .padding(Padding::new(4.0).left(6.0).right(4.0))
        .style(style::picker_row(false)),
        button(text("\u{ff0b}").size(13.0).color(TEXT_DIM))
            .on_press(Message::NewConnection)
            .padding(Padding::new(4.0).left(6.0).right(6.0))
            .style(style::bare_button()),
    ]
    .align_y(iced::Alignment::Center);

    let mut rows: Vec<Element<'a, Message>> = vec![header.into()];
    if open {
        if connections.is_empty() {
            rows.push(
                container(
                    text("No connections yet — click ＋ to add one")
                        .size(12.0)
                        .color(TEXT_DIM),
                )
                .padding(Padding::new(4.0).left(26.0).right(4.0))
                .into(),
            );
        } else {
            for conn in connections {
                rows.push(connection_node(conn, expanded, active_saved));
            }
        }
    }
    column(rows).spacing(1.0).width(Fill).into()
}

fn connection_node<'a>(
    conn: &'a Connection,
    expanded: &'a HashSet<String>,
    active_saved: Option<&str>,
) -> Element<'a, Message> {
    let open = expanded.contains(&conn.id);
    let marker = if open { "\u{25be}" } else { "\u{25b8}" };

    let header = mouse_area(
        row![
            button(
                row![
                    text(marker).size(11.0).color(TEXT_DIM),
                    text(conn.name.clone()).size(13.0),
                ]
                .spacing(6.0),
            )
            .on_press(Message::ToggleFolder(conn.id.clone()))
            .width(Fill)
            .padding(Padding::new(4.0).left(20.0).right(4.0))
            .style(style::picker_row(false)),
            button(text("\u{ff0b}").size(12.0).color(TEXT_DIM))
                .on_press(Message::NewSearch(conn.id.clone()))
                .padding(Padding::new(4.0).left(6.0).right(6.0))
                .style(style::bare_button()),
        ]
        .align_y(iced::Alignment::Center),
    )
    .on_right_press(Message::TreeMenuToggle(TreeMenu::Connection(
        conn.id.clone(),
    )));

    let mut rows: Vec<Element<'a, Message>> = vec![header.into()];
    if open {
        if conn.searches.is_empty() {
            rows.push(
                container(
                    text("No saved searches — click ＋")
                        .size(11.0)
                        .color(TEXT_DIM),
                )
                .padding(Padding::new(3.0).left(40.0))
                .into(),
            );
        } else {
            for saved in &conn.searches {
                let active = active_saved == Some(saved.id.as_str());
                let menu_target = TreeMenu::Search {
                    connection: conn.id.clone(),
                    search: saved.id.clone(),
                };
                rows.push(
                    mouse_area(
                        button(text(saved.name.clone()).size(13.0))
                            .on_press(Message::OpenSavedSearch {
                                connection: conn.id.clone(),
                                search: saved.id.clone(),
                            })
                            .width(Fill)
                            .padding(Padding::new(4.0).left(40.0).right(4.0))
                            .style(style::picker_row(active)),
                    )
                    .on_right_press(Message::TreeMenuToggle(menu_target))
                    .into(),
                );
            }
        }
    }
    column(rows).spacing(1.0).width(Fill).into()
}

/// The floating right-click dropdown for the open tree menu, anchored at the
/// pointer `at` (sidebar coordinates) as a stack layer so it never reflows the
/// tree.
pub(crate) fn menu_overlay<'a>(menu: Option<&TreeMenu>, at: Point) -> Option<Element<'a, Message>> {
    let (edit, delete) = match menu? {
        TreeMenu::Connection(id) => (
            Message::EditConnection(id.clone()),
            Message::RequestDeleteConnection(id.clone()),
        ),
        TreeMenu::Search { connection, search } => (
            Message::EditSearch {
                connection: connection.clone(),
                search: search.clone(),
            },
            Message::DeleteSearch {
                connection: connection.clone(),
                search: search.clone(),
            },
        ),
    };

    // Anchored to the cursor rather than to the chrome, but kept inside
    // the sidebar it belongs to.
    let x = at.x.clamp(2.0, chrome::SIDEBAR_W - MENU_OUTER_W);
    let y = at.y.max(2.0);

    Some(anchored(
        menu_block(edit, delete),
        Anchor::Left { x, y },
        Message::TreeMenuDismiss,
    ))
}

/// The Edit / Delete block itself.
fn menu_block<'a>(edit: Message, delete: Message) -> Element<'a, Message> {
    chrome::menu_popup(
        column![
            button(text("Edit").size(12.0).color(TEXT))
                .on_press(edit)
                .width(Fill)
                .padding(Padding::new(4.0).left(10.0).right(10.0))
                .style(style::picker_row(false)),
            button(text("Delete").size(12.0).color(ERR_RED))
                .on_press(delete)
                .width(Fill)
                .padding(Padding::new(4.0).left(10.0).right(10.0))
                .style(style::picker_row(false)),
        ]
        .spacing(1.0),
        MENU_W,
    )
}
