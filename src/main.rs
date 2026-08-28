mod sample;
mod style;
mod tab;

use std::collections::HashSet;

use iced::widget::{
    button, column, container, row, rule, scrollable, space, text, text_editor,
};
use iced::{Border, Element, Fill, Font, Padding, Theme};

use sample::{LogFile, PickerNode};
use style::{ACCENT, BG, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

pub fn main() -> iced::Result {
    iced::application(LogLens::new, LogLens::update, LogLens::view)
        .title("Log Lens")
        .theme(LogLens::theme)
        .window_size(iced::Size::new(1180.0, 760.0))
        .run()
}

// --- State -----------------------------------------------------------------

struct LogLens {
    files: Vec<LogFile>,
    tree: Vec<PickerNode>,
    /// Open tabs, in tab order.
    open_tabs: Vec<Tab>,
    /// Index into `open_tabs`.
    active_tab: Option<usize>,
    expanded: HashSet<String>,
    /// Selectable contents of the active tab.
    content: text_editor::Content,
}

#[derive(Debug, Clone)]
enum Message {
    OpenFile(usize),
    SelectTab(usize),
    CloseTab(usize),
    ToggleFolder(String),
    Edit(text_editor::Action),
}

impl LogLens {
    fn new() -> Self {
        let (files, tree) = sample::library();
        let mut expanded = HashSet::new();
        collect_folders(&tree, &mut expanded);

        Self {
            content: text_editor::Content::with_text(&file_text(&files[0])),
            files,
            tree,
            open_tabs: vec![Tab::File { file: 0 }],
            active_tab: Some(0),
            expanded,
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn active_file(&self) -> Option<usize> {
        self.active_tab
            .and_then(|t| self.open_tabs.get(t))
            .and_then(Tab::file)
    }

    /// Rebuilds the editor buffer from whichever tab is now active.
    fn reload_content(&mut self) {
        let text = self
            .active_file()
            .map(|f| file_text(&self.files[f]))
            .unwrap_or_default();
        self.content = text_editor::Content::with_text(&text);
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::OpenFile(file) => {
                let tab = self
                    .open_tabs
                    .iter()
                    .position(|t| t.file() == Some(file))
                    .unwrap_or_else(|| {
                        self.open_tabs.push(Tab::File { file });
                        self.open_tabs.len() - 1
                    });
                self.active_tab = Some(tab);
                self.reload_content();
            }
            Message::SelectTab(tab) => {
                if tab < self.open_tabs.len() {
                    self.active_tab = Some(tab);
                    self.reload_content();
                }
            }
            Message::CloseTab(tab) => {
                if tab >= self.open_tabs.len() {
                    return;
                }
                self.open_tabs.remove(tab);
                self.active_tab = match self.active_tab {
                    _ if self.open_tabs.is_empty() => None,
                    Some(active) if active > tab => Some(active - 1),
                    Some(active) if active == tab => {
                        Some(tab.min(self.open_tabs.len() - 1))
                    }
                    other => other,
                };
                self.reload_content();
            }
            Message::ToggleFolder(name) => {
                if !self.expanded.remove(&name) {
                    self.expanded.insert(name);
                }
            }
            Message::Edit(action) => {
                // Read-only viewer: allow cursor moves, click-drag selection
                // and copy, but drop anything that would mutate the buffer.
                if !action.is_edit() {
                    self.content.perform(action);
                }
            }
        }
    }

    // --- View --------------------------------------------------------------

    fn view(&self) -> Element<'_, Message> {
        let body = row![
            self.file_picker(),
            rule::vertical(1.0),
            column![
                self.tab_bar(),
                rule::horizontal(1.0),
                self.toolbar(),
                rule::horizontal(1.0),
                self.log_view(),
            ]
            .width(Fill),
        ]
        .height(Fill);

        container(body)
            .style(|_| style::panel(BG))
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn file_picker(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![section_label("FILES")];
        for node in &self.tree {
            items.push(self.view_node(node, 0));
        }

        container(scrollable(column(items).spacing(1.0).width(Fill)).height(Fill))
            .style(|_| style::panel(PANEL))
            .width(240.0)
            .height(Fill)
            .padding(6.0)
            .into()
    }

    fn view_node<'a>(
        &'a self,
        node: &'a PickerNode,
        depth: u16,
    ) -> Element<'a, Message> {
        let indent = 6.0 + f32::from(depth) * 14.0;

        match node {
            PickerNode::Folder { name, children } => {
                let open = self.expanded.contains(name);
                let marker = if open { "\u{25be}" } else { "\u{25b8}" };

                let header = button(
                    row![text(marker).size(11.0).color(TEXT_DIM), text(name).size(13.0)]
                        .spacing(6.0),
                )
                .on_press(Message::ToggleFolder(name.clone()))
                .width(Fill)
                .padding(Padding::new(4.0).left(indent).right(4.0))
                .style(style::picker_row(false));

                let mut rows: Vec<Element<'a, Message>> = vec![header.into()];
                if open {
                    for child in children {
                        rows.push(self.view_node(child, depth + 1));
                    }
                }
                column(rows).spacing(1.0).width(Fill).into()
            }
            PickerNode::File { name, file } => {
                let active = self.active_file() == Some(*file);
                button(text(name).size(13.0))
                    .on_press(Message::OpenFile(*file))
                    .width(Fill)
                    .padding(Padding::new(4.0).left(indent + 14.0).right(4.0))
                    .style(style::picker_row(active))
                    .into()
            }
        }
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        if self.open_tabs.is_empty() {
            return container(space().height(34.0))
                .style(|_| style::panel(PANEL_ALT))
                .width(Fill)
                .into();
        }

        let tabs = self.open_tabs.iter().enumerate().map(|(i, tab)| -> Element<'_, Message> {
            let active = self.active_tab == Some(i);
            let name = match tab {
                Tab::File { file } => self.files[*file].name.clone(),
            };

            container(
                row![
                    button(text(name).size(13.0).color(if active {
                        TEXT
                    } else {
                        TEXT_DIM
                    }))
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

    fn toolbar(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match self.active_file() {
            Some(file) => {
                let f = &self.files[file];
                row![
                    text(&f.path).size(12.0).color(TEXT),
                    space().width(Fill),
                    meta(&format!("{:.2} KiB", f.size_kib())),
                    meta(&format!("{} lines", f.lines.len())),
                ]
                .spacing(18.0)
                .align_y(iced::Alignment::Center)
                .into()
            }
            None => text("No file open").size(12.0).color(TEXT_DIM).into(),
        };

        container(content)
            .style(|_| style::panel(PANEL))
            .width(Fill)
            .padding(Padding::new(6.0).left(12.0).right(12.0))
            .into()
    }

    fn log_view(&self) -> Element<'_, Message> {
        if self.active_file().is_none() {
            return container(
                text("Open a file from the picker to view logs")
                    .size(14.0)
                    .color(TEXT_DIM),
            )
            .center_x(Fill)
            .center_y(Fill)
            .style(|_| style::panel(BG))
            .into();
        }

        container(
            text_editor(&self.content)
                .on_action(Message::Edit)
                .font(Font::MONOSPACE)
                .size(12.5)
                .padding(Padding::new(8.0).left(12.0))
                .height(Fill)
                .style(style::editor),
        )
        .style(|_| style::panel(BG))
        .height(Fill)
        .into()
    }
}

// --- Small view helpers ----------------------------------------------------

fn section_label<'a>(label: &'a str) -> Element<'a, Message> {
    container(text(label).size(11.0).color(TEXT_DIM))
        .padding(Padding::new(4.0).left(6.0))
        .into()
}

fn meta<'a>(value: &str) -> Element<'a, Message> {
    text(value.to_string()).size(12.0).color(TEXT_DIM).into()
}

fn file_text(file: &LogFile) -> String {
    file.lines.join("\n")
}

fn collect_folders(nodes: &[PickerNode], out: &mut HashSet<String>) {
    for node in nodes {
        if let PickerNode::Folder { name, children } = node {
            out.insert(name.clone());
            collect_folders(children, out);
        }
    }
}

