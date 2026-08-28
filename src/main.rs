mod config;
mod connection;
mod es;
mod sample;
mod secrets;
mod style;
mod tab;

use std::collections::HashSet;

use iced::widget::{
    button, checkbox, column, container, radio, row, rule, scrollable, space,
    stack, text, text_editor, text_input,
};
use iced::{Border, Color, Element, Fill, Font, Padding, Task, Theme};

use config::{Config, Connection};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use sample::{LogFile, PickerNode};
use style::{ACCENT, BG, BORDER, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, toggled through the same
/// `expanded` set as sample-log folders. The control char keeps it from
/// colliding with a real folder name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

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
    config: Config,
    /// Open tabs, in tab order.
    open_tabs: Vec<Tab>,
    /// Index into `open_tabs`.
    active_tab: Option<usize>,
    expanded: HashSet<String>,
    /// Selectable contents of the active file tab.
    content: text_editor::Content,
    /// The Connection form, when adding or editing one.
    connection_form: Option<ConnectionForm>,
    /// A prompt for a secret the keyring can't give us this session.
    secret_prompt: Option<SecretPrompt>,
    /// Transient status line (config save failures, keyring notices).
    status: Option<String>,
}

/// Asks the user for a Connection secret and retries what needed it.
struct SecretPrompt {
    connection_id: String,
    connection_name: String,
    value: String,
    then: PendingAction,
}

/// What to resume once a [`SecretPrompt`] is answered.
#[derive(Debug, Clone)]
enum PendingAction {
    TestConnection,
}

#[derive(Debug, Clone)]
enum Message {
    // Sample Logs
    OpenFile(usize),
    SelectTab(usize),
    CloseTab(usize),
    ToggleFolder(String),
    Edit(text_editor::Action),
    // Connection form
    NewConnection,
    ConnFormName(String),
    ConnFormUrl(String),
    ConnFormAuthKind(AuthKind),
    ConnFormUsername(String),
    ConnFormSecret(String),
    ConnFormSkipTls(bool),
    ConnFormTest,
    ConnFormTestDone(Result<es::ClusterInfo, String>),
    ConnFormSave,
    ConnFormCancel,
    // Secret prompt
    SecretPromptValue(String),
    SecretPromptSubmit,
    SecretPromptCancel,
    // Misc
    DismissStatus,
}

impl LogLens {
    fn new() -> Self {
        let (files, tree) = sample::library();
        let mut expanded = HashSet::new();
        collect_folders(&tree, &mut expanded);
        expanded.insert(ES_ROOT.to_string());

        Self {
            content: text_editor::Content::with_text(&file_text(&files[0])),
            files,
            tree,
            config: config::load(),
            open_tabs: vec![Tab::File { file: 0 }],
            active_tab: Some(0),
            expanded,
            connection_form: None,
            secret_prompt: None,
            status: None,
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

    fn update(&mut self, message: Message) -> Task<Message> {
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
                    return Task::none();
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

            Message::NewConnection => {
                self.connection_form = Some(ConnectionForm::adding());
                self.expanded.insert(ES_ROOT.to_string());
            }
            Message::ConnFormName(v) => {
                if let Some(f) = &mut self.connection_form {
                    f.name = v;
                    f.error = None;
                }
            }
            Message::ConnFormUrl(v) => {
                if let Some(f) = &mut self.connection_form {
                    f.url = v;
                    f.error = None;
                }
            }
            Message::ConnFormAuthKind(kind) => {
                if let Some(f) = &mut self.connection_form {
                    f.auth_kind = kind;
                    f.test = TestState::Idle;
                }
            }
            Message::ConnFormUsername(v) => {
                if let Some(f) = &mut self.connection_form {
                    f.username = v;
                }
            }
            Message::ConnFormSecret(v) => {
                if let Some(f) = &mut self.connection_form {
                    f.secret = v;
                }
            }
            Message::ConnFormSkipTls(v) => {
                if let Some(f) = &mut self.connection_form {
                    f.skip_tls_verify = v;
                }
            }
            Message::ConnFormTest => return self.start_connection_test(),
            Message::ConnFormTestDone(result) => {
                if let Some(f) = &mut self.connection_form {
                    f.test = match result {
                        Ok(info) => TestState::Ok(format!(
                            "{} · Elasticsearch {}",
                            info.cluster_name, info.version
                        )),
                        Err(err) => TestState::Failed(err),
                    };
                }
            }
            Message::ConnFormSave => return self.save_connection_form(),
            Message::ConnFormCancel => {
                self.connection_form = None;
            }

            Message::SecretPromptValue(v) => {
                if let Some(p) = &mut self.secret_prompt {
                    p.value = v;
                }
            }
            Message::SecretPromptSubmit => {
                if let Some(prompt) = self.secret_prompt.take() {
                    secrets::remember_session(&prompt.connection_id, &prompt.value);
                    if let Some(f) = &mut self.connection_form {
                        // Feed the answer back into the open form so the retry
                        // and a later Save both see it.
                        f.secret = prompt.value.clone();
                    }
                    match prompt.then {
                        PendingAction::TestConnection => {
                            return self.start_connection_test();
                        }
                    }
                }
            }
            Message::SecretPromptCancel => {
                self.secret_prompt = None;
            }

            Message::DismissStatus => self.status = None,
        }

        Task::none()
    }

    /// Kicks off a `GET /` for the current Connection form, or opens a secret
    /// prompt if the keyring can't supply the secret this session.
    fn start_connection_test(&mut self) -> Task<Message> {
        let Some(form) = &mut self.connection_form else {
            return Task::none();
        };
        match form.endpoint() {
            Ok(endpoint) => {
                form.test = TestState::Running;
                Task::perform(es::ping(endpoint), Message::ConnFormTestDone)
            }
            Err(EndpointError::MissingUrl) => {
                form.test = TestState::Failed("Enter a URL first".to_string());
                Task::none()
            }
            Err(EndpointError::MissingSecret) => {
                self.open_secret_prompt(PendingAction::TestConnection);
                Task::none()
            }
        }
    }

    /// Validates and persists the Connection form, then closes it.
    fn save_connection_form(&mut self) -> Task<Message> {
        let Some(form) = &mut self.connection_form else {
            return Task::none();
        };
        if form.name.trim().is_empty() {
            form.error = Some("Name is required".to_string());
            return Task::none();
        }
        if form.url.trim().is_empty() {
            form.error = Some("URL is required".to_string());
            return Task::none();
        }

        let form = self.connection_form.take().unwrap();
        let id = form
            .editing_id
            .clone()
            .unwrap_or_else(config::new_id);

        let connection = Connection {
            id: id.clone(),
            name: form.name.trim().to_string(),
            url: form.url.trim().to_string(),
            auth: form.auth(),
            skip_tls_verify: form.skip_tls_verify,
        };

        // Persist the secret. A freshly typed secret is always stored; an
        // untouched field on an edit leaves the existing secret alone.
        if form.auth_kind.needs_secret() && !form.secret.is_empty() {
            if secrets::set(&id, &form.secret) == secrets::Stored::Session {
                self.status = Some(
                    "Keyring unavailable — secret kept for this session only."
                        .to_string(),
                );
            }
        } else if !form.auth_kind.needs_secret() {
            secrets::delete(&id);
        }

        match self.config.connections.iter_mut().find(|c| c.id == id) {
            Some(existing) => *existing = connection,
            None => self.config.connections.push(connection),
        }

        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }

        Task::none()
    }

    fn open_secret_prompt(&mut self, then: PendingAction) {
        let Some(form) = &self.connection_form else {
            return;
        };
        let id = form
            .editing_id
            .clone()
            .unwrap_or_else(|| "pending".to_string());
        let name = if form.name.trim().is_empty() {
            "this connection".to_string()
        } else {
            form.name.trim().to_string()
        };
        self.secret_prompt = Some(SecretPrompt {
            connection_id: id,
            connection_name: name,
            value: String::new(),
            then,
        });
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

        let base: Element<'_, Message> = container(column![
            container(body).width(Fill).height(Fill),
            self.status_bar(),
        ])
        .style(|_| style::panel(BG))
        .width(Fill)
        .height(Fill)
        .into();

        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if let Some(form) = &self.connection_form {
            layers.push(self.connection_form_modal(form));
        }
        if let Some(prompt) = &self.secret_prompt {
            layers.push(self.secret_prompt_modal(prompt));
        }

        if layers.len() == 1 {
            layers.pop().unwrap()
        } else {
            stack(layers).width(Fill).height(Fill).into()
        }
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let Some(status) = &self.status else {
            return space().height(0.0).into();
        };
        container(
            row![
                text(status.clone()).size(12.0).color(TEXT),
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

    fn file_picker(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![self.es_section()];
        items.push(space().height(8.0).into());
        items.push(section_label("SAMPLE LOGS"));
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

    /// The `Elasticsearch` tree root: its Connections plus a "＋" affordance.
    fn es_section(&self) -> Element<'_, Message> {
        let open = self.expanded.contains(ES_ROOT);
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

        let mut rows: Vec<Element<'_, Message>> = vec![header.into()];
        if open {
            if self.config.connections.is_empty() {
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
                for conn in &self.config.connections {
                    rows.push(
                        container(
                            text(conn.name.clone()).size(13.0).color(TEXT),
                        )
                        .width(Fill)
                        .padding(Padding::new(4.0).left(26.0).right(4.0))
                        .into(),
                    );
                }
            }
        }
        column(rows).spacing(1.0).width(Fill).into()
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

    // --- Connection form modal -------------------------------------------

    fn connection_form_modal<'a>(
        &'a self,
        form: &'a ConnectionForm,
    ) -> Element<'a, Message> {
        let mut fields: Vec<Element<'a, Message>> = vec![
            text(form.title()).size(16.0).color(TEXT).into(),
            field_label("Name"),
            text_input("Production logs", &form.name)
                .on_input(Message::ConnFormName)
                .padding(6.0)
                .into(),
            field_label("URL"),
            text_input("https://localhost:9200", &form.url)
                .on_input(Message::ConnFormUrl)
                .padding(6.0)
                .into(),
            field_label("Authentication"),
            row(AuthKind::ALL.iter().map(|&kind| {
                radio(
                    kind.label(),
                    kind,
                    Some(form.auth_kind),
                    Message::ConnFormAuthKind,
                )
                .size(14.0)
                .into()
            }))
            .spacing(16.0)
            .into(),
        ];

        if form.auth_kind == AuthKind::Basic {
            fields.push(field_label("Username"));
            fields.push(
                text_input("elastic", &form.username)
                    .on_input(Message::ConnFormUsername)
                    .padding(6.0)
                    .into(),
            );
        }
        if form.auth_kind.needs_secret() {
            let secret_label = if form.auth_kind == AuthKind::Basic {
                "Password"
            } else {
                "API key"
            };
            fields.push(field_label(secret_label));
            let placeholder = if form.editing_id.is_some() {
                "(unchanged)"
            } else {
                ""
            };
            fields.push(
                text_input(placeholder, &form.secret)
                    .on_input(Message::ConnFormSecret)
                    .secure(true)
                    .padding(6.0)
                    .into(),
            );
        }

        fields.push(
            checkbox(form.skip_tls_verify)
                .label("Skip TLS certificate verification")
                .on_toggle(Message::ConnFormSkipTls)
                .size(14.0)
                .into(),
        );

        // Test row + result.
        fields.push(space().height(4.0).into());
        fields.push(
            row![
                button(text("Test").size(13.0))
                    .on_press(Message::ConnFormTest)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                test_result(&form.test),
            ]
            .spacing(12.0)
            .align_y(iced::Alignment::Center)
            .into(),
        );

        if let Some(err) = &form.error {
            fields.push(text(err.clone()).size(12.0).color(Color::from_rgb8(0xe0, 0x6c, 0x6c)).into());
        }

        fields.push(space().height(8.0).into());
        fields.push(
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::ConnFormCancel)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text("Save").size(13.0).color(TEXT))
                    .on_press(Message::ConnFormSave)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0)
            .into(),
        );

        modal_card(column(fields).spacing(6.0).width(Fill).into())
    }

    fn secret_prompt_modal<'a>(
        &'a self,
        prompt: &'a SecretPrompt,
    ) -> Element<'a, Message> {
        let card = column![
            text("Secret required").size(16.0).color(TEXT),
            text(format!(
                "The keyring is unavailable. Enter the secret for {} for this session.",
                prompt.connection_name
            ))
            .size(12.0)
            .color(TEXT_DIM),
            space().height(4.0),
            text_input("", &prompt.value)
                .on_input(Message::SecretPromptValue)
                .on_submit(Message::SecretPromptSubmit)
                .secure(true)
                .padding(6.0),
            space().height(8.0),
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::SecretPromptCancel)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text("Continue").size(13.0).color(TEXT))
                    .on_press(Message::SecretPromptSubmit)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0),
        ]
        .spacing(6.0)
        .width(Fill);

        modal_card(card.into())
    }
}

// --- Small view helpers ----------------------------------------------------

fn section_label<'a>(label: &'a str) -> Element<'a, Message> {
    container(text(label).size(11.0).color(TEXT_DIM))
        .padding(Padding::new(4.0).left(6.0))
        .into()
}

fn field_label<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(12.0).color(TEXT_DIM).into()
}

fn test_result(state: &TestState) -> Element<'_, Message> {
    match state {
        TestState::Idle => space().width(0.0).into(),
        TestState::Running => text("Testing\u{2026}").size(12.0).color(TEXT_DIM).into(),
        TestState::Ok(msg) => text(format!("\u{2713} {msg}"))
            .size(12.0)
            .color(Color::from_rgb8(0x6c, 0xc0, 0x7a))
            .into(),
        TestState::Failed(err) => text(format!("\u{2717} {err}"))
            .size(12.0)
            .color(Color::from_rgb8(0xe0, 0x6c, 0x6c))
            .into(),
    }
}

/// Centres `content` in a panel card over a dimmed backdrop.
fn modal_card<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    let card = container(content)
        .width(460.0)
        .padding(20.0)
        .style(|_| {
            let mut s = style::panel(PANEL);
            s.border = Border {
                color: BORDER,
                width: 1.0,
                radius: 4.0.into(),
            };
            s
        });

    container(card)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(|_| container::Style {
            background: Some(Color { a: 0.6, ..Color::BLACK }.into()),
            ..container::Style::default()
        })
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
