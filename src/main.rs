mod config;
mod connection;
mod es;
mod results;
mod sample;
mod search;
mod secrets;
mod style;
mod tab;

use std::collections::HashSet;

use iced::widget::{
    button, checkbox, column, container, pick_list, radio, row, rule, scrollable,
    space, stack, text, text_editor, text_input,
};
use iced::{Border, Color, Element, Fill, Font, Length, Padding, Task, Theme};

use config::{Auth, Config, Connection};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use results::{Paging, ResultTab, RunState, RETENTION_CAP, ROW_H};
use sample::{LogFile, PickerNode};
use search::{Fields, SearchForm, TimeframeMode};
use config::TimeUnit;
use style::{ACCENT, BG, BORDER, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, toggled through the same
/// `expanded` set as sample-log folders. The control char keeps it from
/// colliding with a real folder name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

const OK_GREEN: Color = Color::from_rgb8(0x6c, 0xc0, 0x7a);
const ERR_RED: Color = Color::from_rgb8(0xe0, 0x6c, 0x6c);

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
    /// Source of stable ids for Search forms and Result Tabs.
    id_seq: u64,
}

/// Asks the user for a Connection secret and resumes what needed it.
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
    RunSearch { run_id: u64 },
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
    // Search form
    NewSearch(String),
    OpenSavedSearch { connection: String, search: String },
    SearchTargetsLoaded { form_id: u64, result: Result<Vec<String>, String> },
    SearchName(String),
    SearchTargetInput(String),
    SearchTargetPicked(String),
    SearchQuery(String),
    SearchTimeframeMode(TimeframeMode),
    SearchRelAmount(String),
    SearchRelUnit(TimeUnit),
    SearchAbsFrom(String),
    SearchAbsTo(String),
    SearchTimestampField(String),
    SearchFieldsLoaded {
        form_id: u64,
        result: Result<es::FieldCaps, String>,
    },
    SearchColumnDraft(String),
    SearchColumnAdd,
    SearchColumnAddField(String),
    SearchColumnRemove(usize),
    SearchColumnMove(usize, isize),
    SearchSortField(String),
    SearchSortDir(bool),
    SearchSave,
    // Result tab: live columns + sort
    ResultFieldsLoaded {
        run_id: u64,
        result: Result<es::FieldCaps, String>,
    },
    ResultColumnDraft(u64, String),
    ResultColumnAdd(u64),
    ResultColumnAddField(u64, String),
    ResultColumnRemove(u64, usize),
    ResultColumnMove(u64, usize, isize),
    ResultSortField(u64, String),
    ResultSortDir(u64, bool),
    // Result tab run
    PitOpened { run_id: u64, result: Result<String, String> },
    PageLoaded {
        run_id: u64,
        result: Result<es::Page, String>,
        append: bool,
    },
    ResultScrolled {
        run_id: u64,
        offset_y: f32,
        viewport_h: f32,
        content_h: f32,
    },
    RetryPage(u64),
    // Misc
    DismissStatus,
    Ignore,
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
            id_seq: 0,
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn next_id(&mut self) -> u64 {
        self.id_seq += 1;
        self.id_seq
    }

    fn active_file(&self) -> Option<usize> {
        self.active_tab
            .and_then(|t| self.open_tabs.get(t))
            .and_then(Tab::file)
    }

    fn connection(&self, id: &str) -> Option<&Connection> {
        self.config.connections.iter().find(|c| c.id == id)
    }

    /// Builds an [`es::Endpoint`] for a Connection, or `None` if its secret
    /// isn't available this session (keyring missing, not yet re-entered).
    fn endpoint_for(&self, conn: &Connection) -> Option<es::Endpoint> {
        let auth = match &conn.auth {
            Auth::None => es::AuthValue::None,
            Auth::Basic { username } => es::AuthValue::Basic {
                username: username.clone(),
                password: secrets::get(&conn.id)?,
            },
            Auth::ApiKey => es::AuthValue::ApiKey {
                key: secrets::get(&conn.id)?,
            },
        };
        Some(es::Endpoint {
            url: conn.url.clone(),
            auth,
            skip_tls_verify: conn.skip_tls_verify,
        })
    }

    fn result_mut(&mut self, run_id: u64) -> Option<&mut ResultTab> {
        self.open_tabs.iter_mut().find_map(|t| match t {
            Tab::Result(rt) if rt.run_id == run_id => Some(rt.as_mut()),
            _ => None,
        })
    }

    fn form_mut(&mut self, form_id: u64) -> Option<&mut SearchForm> {
        self.open_tabs.iter_mut().find_map(|t| match t {
            Tab::SearchForm(f) if f.form_id == form_id => Some(f.as_mut()),
            _ => None,
        })
    }

    fn active_form_mut(&mut self) -> Option<&mut SearchForm> {
        match self.active_tab.and_then(|t| self.open_tabs.get_mut(t)) {
            Some(Tab::SearchForm(f)) => Some(f.as_mut()),
            _ => None,
        }
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
            Message::CloseTab(tab) => return self.close_tab(tab),
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
                        f.secret = prompt.value.clone();
                    }
                    match prompt.then {
                        PendingAction::TestConnection => {
                            return self.start_connection_test();
                        }
                        PendingAction::RunSearch { run_id } => {
                            return self.start_run(run_id);
                        }
                    }
                }
            }
            Message::SecretPromptCancel => {
                if let Some(prompt) = self.secret_prompt.take() {
                    if let PendingAction::RunSearch { run_id } = prompt.then {
                        if let Some(rt) = self.result_mut(run_id) {
                            rt.state = RunState::Error(
                                "Connection secret required to run this search"
                                    .to_string(),
                            );
                        }
                    }
                }
            }

            Message::NewSearch(conn_id) => return self.open_search_form(conn_id),
            Message::OpenSavedSearch { connection, search } => {
                return self.open_result_tab(connection, search, None, None);
            }
            Message::SearchTargetsLoaded { form_id, result } => {
                if let Some(f) = self.form_mut(form_id) {
                    f.targets_loading = false;
                    if let Ok(mut options) = result {
                        options.sort();
                        f.target_options = options;
                    }
                }
            }
            Message::SearchName(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.name = v;
                    f.error = None;
                }
            }
            Message::SearchTargetInput(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.target = v;
                    f.error = None;
                }
            }
            Message::SearchTargetPicked(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.target = v;
                    f.error = None;
                }
                return self.load_form_fields();
            }
            Message::SearchQuery(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.query_string = v;
                }
            }
            Message::SearchTimeframeMode(mode) => {
                if let Some(f) = self.active_form_mut() {
                    f.mode = mode;
                }
            }
            Message::SearchRelAmount(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.rel_amount = v;
                }
            }
            Message::SearchRelUnit(u) => {
                if let Some(f) = self.active_form_mut() {
                    f.rel_unit = u;
                }
            }
            Message::SearchAbsFrom(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.abs_from = v;
                }
            }
            Message::SearchAbsTo(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.abs_to = v;
                }
            }
            Message::SearchTimestampField(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.timestamp_field = v;
                }
            }
            Message::SearchFieldsLoaded { form_id, result } => {
                if let Some(f) = self.form_mut(form_id) {
                    f.fields = match result {
                        Ok(caps) => Fields::Ready(caps),
                        Err(_) => Fields::Failed,
                    };
                }
            }
            Message::SearchColumnDraft(v) => {
                if let Some(f) = self.active_form_mut() {
                    f.column_draft = v;
                }
            }
            Message::SearchColumnAdd => {
                if let Some(f) = self.active_form_mut() {
                    let draft = f.column_draft.clone();
                    f.add_column(&draft);
                    f.error = None;
                }
            }
            Message::SearchColumnAddField(field) => {
                if let Some(f) = self.active_form_mut() {
                    f.add_column(&field);
                    f.error = None;
                }
            }
            Message::SearchColumnRemove(i) => {
                if let Some(f) = self.active_form_mut() {
                    f.remove_column(i);
                }
            }
            Message::SearchColumnMove(i, delta) => {
                if let Some(f) = self.active_form_mut() {
                    f.move_column(i, delta);
                }
            }
            Message::SearchSortField(field) => {
                if let Some(f) = self.active_form_mut() {
                    f.sort_field = field;
                }
            }
            Message::SearchSortDir(desc) => {
                if let Some(f) = self.active_form_mut() {
                    f.sort_desc = desc;
                }
            }
            Message::SearchSave => return self.save_search_form(),

            Message::ResultFieldsLoaded { run_id, result } => {
                if let Ok(caps) = result {
                    if let Some(rt) = self.result_mut(run_id) {
                        rt.all_fields = caps.all;
                        rt.sortable_fields = caps.sortable;
                    }
                }
            }
            Message::ResultColumnDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.column_draft = v;
                }
            }
            Message::ResultColumnAdd(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    let draft = rt.column_draft.clone();
                    rt.add_column(&draft);
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultColumnAddField(run_id, field) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.add_column(&field);
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultColumnRemove(run_id, i) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.remove_column(i);
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultColumnMove(run_id, i, delta) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.move_column(i, delta);
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultSortField(run_id, field) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let changed = rt.sort_field != field;
                        rt.sort_field = field;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultSortDir(run_id, desc) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let changed = rt.sort_desc != desc;
                        rt.sort_desc = desc;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }

            Message::PitOpened { run_id, result } => {
                return self.on_pit_opened(run_id, result);
            }
            Message::PageLoaded {
                run_id,
                result,
                append,
            } => {
                if let Some(rt) = self.result_mut(run_id) {
                    apply_page(rt, result, append);
                }
            }
            Message::ResultScrolled {
                run_id,
                offset_y,
                viewport_h,
                content_h,
            } => {
                let wants_more = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.scroll_y = offset_y;
                        rt.viewport_h = viewport_h;
                        rt.wants_more(offset_y, viewport_h, content_h)
                    })
                    .unwrap_or(false);
                if wants_more {
                    return self.load_more(run_id);
                }
            }
            Message::RetryPage(run_id) => return self.load_more(run_id),

            Message::DismissStatus => self.status = None,
            Message::Ignore => {}
        }

        Task::none()
    }

    // --- Sample Logs / tabs ---------------------------------------------

    fn close_tab(&mut self, tab: usize) -> Task<Message> {
        if tab >= self.open_tabs.len() {
            return Task::none();
        }

        // Release any server-side PIT this tab was holding.
        let closing_pit = match &self.open_tabs[tab] {
            Tab::Result(rt) => rt
                .pit_id
                .clone()
                .map(|pit| (rt.connection_id.clone(), pit)),
            _ => None,
        };

        self.open_tabs.remove(tab);
        self.active_tab = match self.active_tab {
            _ if self.open_tabs.is_empty() => None,
            Some(active) if active > tab => Some(active - 1),
            Some(active) if active == tab => Some(tab.min(self.open_tabs.len() - 1)),
            other => other,
        };
        self.reload_content();

        if let Some((conn_id, pit)) = closing_pit {
            if let Some(conn) = self.connection(&conn_id) {
                if let Some(endpoint) = self.endpoint_for(conn) {
                    return Task::perform(
                        es::close_pit(endpoint, pit),
                        |_| Message::Ignore,
                    );
                }
            }
        }
        Task::none()
    }

    // --- Connection form ----------------------------------------------

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
                let id = form
                    .editing_id
                    .clone()
                    .unwrap_or_else(|| "pending".to_string());
                let name = non_empty(&form.name).unwrap_or("this connection").to_string();
                self.secret_prompt = Some(SecretPrompt {
                    connection_id: id,
                    connection_name: name,
                    value: String::new(),
                    then: PendingAction::TestConnection,
                });
                Task::none()
            }
        }
    }

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
        let id = form.editing_id.clone().unwrap_or_else(config::new_id);

        let connection = Connection {
            id: id.clone(),
            name: form.name.trim().to_string(),
            url: form.url.trim().to_string(),
            auth: form.auth(),
            skip_tls_verify: form.skip_tls_verify,
            searches: self
                .connection(&id)
                .map(|c| c.searches.clone())
                .unwrap_or_default(),
        };

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

    // --- Search form -------------------------------------------------

    fn open_search_form(&mut self, conn_id: String) -> Task<Message> {
        let form_id = self.next_id();
        let form = SearchForm::new(form_id, conn_id.clone());
        self.open_tabs.push(Tab::SearchForm(Box::new(form)));
        self.active_tab = Some(self.open_tabs.len() - 1);
        self.expanded.insert(conn_id.clone());

        let targets = match self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) {
            Some(endpoint) => Task::perform(
                es::list_targets(endpoint),
                move |result| Message::SearchTargetsLoaded { form_id, result },
            ),
            None => {
                if let Some(f) = self.form_mut(form_id) {
                    f.targets_loading = false;
                }
                Task::none()
            }
        };
        // `from_saved` forms open with a Target already set — load its fields.
        Task::batch([targets, self.load_form_fields()])
    }

    /// Fetches `_field_caps` for the active Search form's Target, if it has one.
    fn load_form_fields(&mut self) -> Task<Message> {
        let Some((form_id, conn_id, target)) = self.active_form_mut().map(|f| {
            (f.form_id, f.connection_id.clone(), f.target.trim().to_string())
        }) else {
            return Task::none();
        };
        if target.is_empty() {
            return Task::none();
        }
        if let Some(f) = self.form_mut(form_id) {
            f.fields = Fields::Loading;
        }
        let Some(endpoint) = self.connection(&conn_id).and_then(|c| self.endpoint_for(c))
        else {
            if let Some(f) = self.form_mut(form_id) {
                f.fields = Fields::Failed;
            }
            return Task::none();
        };
        Task::perform(es::field_caps(endpoint, target), move |result| {
            Message::SearchFieldsLoaded { form_id, result }
        })
    }

    /// Writes a Result Tab's live Column / sort choices back onto its Saved
    /// Search and persists the config.
    fn sync_saved_from_result(&mut self, run_id: u64) {
        let Some((conn_id, saved_id, columns, sort_field, sort_desc)) =
            self.result_mut(run_id).map(|rt| {
                (
                    rt.connection_id.clone(),
                    rt.saved_id.clone(),
                    rt.columns.clone(),
                    rt.sort_field.clone(),
                    rt.sort_desc,
                )
            })
        else {
            return;
        };
        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) {
            if let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id) {
                saved.columns = columns;
                saved.sort_field = sort_field;
                saved.sort_desc = sort_desc;
            }
        }
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
    }

    fn save_search_form(&mut self) -> Task<Message> {
        let Some(idx) = self.active_tab else {
            return Task::none();
        };
        let Some(Tab::SearchForm(form)) = self.open_tabs.get(idx) else {
            return Task::none();
        };

        let saved = match form.to_saved() {
            Ok(saved) => saved,
            Err(err) => {
                if let Some(f) = self.active_form_mut() {
                    f.error = Some(err);
                }
                return Task::none();
            }
        };
        let conn_id = form.connection_id.clone();

        let Some(conn) = self
            .config
            .connections
            .iter_mut()
            .find(|c| c.id == conn_id)
        else {
            return Task::none();
        };
        match conn.searches.iter_mut().find(|s| s.id == saved.id) {
            Some(existing) => *existing = saved.clone(),
            None => conn.searches.push(saved.clone()),
        }

        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
        self.expanded.insert(conn_id.clone());

        // Carry the form's already-fetched fields into the Result Tab.
        let caps = match self.open_tabs.get(idx) {
            Some(Tab::SearchForm(f)) => f.fields.caps().cloned(),
            _ => None,
        };
        self.open_result_tab(conn_id, saved.id, Some(idx), caps)
    }

    // --- Result tab / run -------------------------------------------

    /// Opens (or focuses) the Result Tab for a Saved Search and starts its run.
    /// `replace` is the index of a Search form tab to turn into this Result Tab.
    fn open_result_tab(
        &mut self,
        conn_id: String,
        saved_id: String,
        replace: Option<usize>,
        caps: Option<es::FieldCaps>,
    ) -> Task<Message> {
        if let Some(existing) = self.open_tabs.iter().position(|t| {
            matches!(t, Tab::Result(rt) if rt.saved_id == saved_id)
        }) {
            self.active_tab = Some(existing);
            if let Some(form_idx) = replace {
                if form_idx != existing {
                    self.open_tabs.remove(form_idx);
                }
            }
            self.active_tab = self.open_tabs.iter().position(|t| {
                matches!(t, Tab::Result(rt) if rt.saved_id == saved_id)
            });
            return Task::none();
        }

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let Some(saved) = conn.searches.iter().find(|s| s.id == saved_id).cloned()
        else {
            return Task::none();
        };

        let run_id = self.next_id();
        let (gte, lte) = saved.timeframe.bounds();
        let (all_fields, sortable_fields) = caps
            .map(|c| (c.all, c.sortable))
            .unwrap_or_default();
        let tab = ResultTab {
            run_id,
            connection_id: conn_id.clone(),
            saved_id,
            saved_name: saved.name.clone(),
            target: saved.target.clone(),
            query_string: saved.query_string.clone(),
            timestamp_field: saved.timestamp_field.clone(),
            columns: saved.columns.clone(),
            column_draft: String::new(),
            sort_field: saved.sort_field.clone(),
            sort_desc: saved.sort_desc,
            all_fields,
            sortable_fields,
            gte,
            lte,
            pit_id: None,
            hits: Vec::new(),
            state: RunState::Loading,
            paging: Paging::Idle,
            scroll_y: 0.0,
            viewport_h: 600.0,
            utc: self.config.utc_timestamps,
        };
        let need_fields = tab.all_fields.is_empty();
        let target = tab.target.clone();

        match replace {
            Some(i) if i < self.open_tabs.len() => {
                self.open_tabs[i] = Tab::Result(Box::new(tab));
                self.active_tab = Some(i);
            }
            _ => {
                self.open_tabs.push(Tab::Result(Box::new(tab)));
                self.active_tab = Some(self.open_tabs.len() - 1);
            }
        }

        let fetch_fields: Task<Message> = if need_fields {
            match self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) {
                Some(endpoint) => Task::perform(
                    es::field_caps(endpoint, target),
                    move |result| Message::ResultFieldsLoaded { run_id, result },
                ),
                None => Task::none(),
            }
        } else {
            Task::none()
        };

        Task::batch([fetch_fields, self.start_run(run_id)])
    }

    /// Freshens the range, discards any prior PIT, opens a new one, and (on
    /// success) fetches the first Page.
    fn start_run(&mut self, run_id: u64) -> Task<Message> {
        let Some((conn_id, target, old_pit)) = self.result_mut(run_id).map(|rt| {
            rt.state = RunState::Loading;
            rt.hits.clear();
            rt.paging = Paging::Idle;
            rt.scroll_y = 0.0;
            let old_pit = rt.pit_id.take();
            (rt.connection_id.clone(), rt.target.clone(), old_pit)
        }) else {
            return Task::none();
        };

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let close_old: Task<Message> = match (&old_pit, self.endpoint_for(conn)) {
            (Some(pit), Some(endpoint)) => Task::perform(
                es::close_pit(endpoint, pit.clone()),
                |_| Message::Ignore,
            ),
            _ => Task::none(),
        };

        match self.endpoint_for(conn) {
            Some(endpoint) => Task::batch([
                close_old,
                Task::perform(es::open_pit(endpoint, target), move |result| {
                    Message::PitOpened { run_id, result }
                }),
            ]),
            None => {
                let name = conn.name.clone();
                self.secret_prompt = Some(SecretPrompt {
                    connection_id: conn_id,
                    connection_name: name,
                    value: String::new(),
                    then: PendingAction::RunSearch { run_id },
                });
                Task::none()
            }
        }
    }

    fn on_pit_opened(
        &mut self,
        run_id: u64,
        result: Result<String, String>,
    ) -> Task<Message> {
        let pit = match result {
            Ok(pit) => pit,
            Err(err) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.state = RunState::Error(err);
                }
                return Task::none();
            }
        };

        let Some((conn_id, params)) = self.result_mut(run_id).map(|rt| {
            rt.pit_id = Some(pit.clone());
            (
                rt.connection_id.clone(),
                es::SearchParams {
                    query_string: rt.query_string.clone(),
                    timestamp_field: rt.timestamp_field.clone(),
                    gte: rt.gte.clone(),
                    lte: rt.lte.clone(),
                    sort_field: rt.sort_field.clone(),
                    sort_desc: rt.sort_desc,
                    size: 1000,
                    search_after: None,
                },
            )
        }) else {
            return Task::none();
        };

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let Some(endpoint) = self.endpoint_for(conn) else {
            return Task::none();
        };
        Task::perform(es::search(endpoint, pit, params), move |result| {
            Message::PageLoaded {
                run_id,
                result,
                append: false,
            }
        })
    }

    /// Fetches the next Page for a Result Tab via `search_after` on its PIT.
    /// A no-op unless the tab is idle, under the cap, and has a cursor.
    fn load_more(&mut self, run_id: u64) -> Task<Message> {
        let Some((conn_id, pit, params)) =
            self.result_mut(run_id).and_then(|rt| {
                let pit = rt.pit_id.clone()?;
                let cursor = rt.next_cursor()?;
                let remaining = RETENTION_CAP.saturating_sub(rt.hits.len());
                if remaining == 0 {
                    rt.paging = Paging::Capped;
                    return None;
                }
                rt.paging = Paging::Loading;
                Some((
                    rt.connection_id.clone(),
                    pit,
                    es::SearchParams {
                        query_string: rt.query_string.clone(),
                        timestamp_field: rt.timestamp_field.clone(),
                        gte: rt.gte.clone(),
                        lte: rt.lte.clone(),
                        sort_field: rt.sort_field.clone(),
                        sort_desc: rt.sort_desc,
                        size: remaining.min(1000),
                        search_after: Some(cursor),
                    },
                ))
            })
        else {
            return Task::none();
        };

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let Some(endpoint) = self.endpoint_for(conn) else {
            return Task::none();
        };
        Task::perform(es::search(endpoint, pit, params), move |result| {
            Message::PageLoaded {
                run_id,
                result,
                append: true,
            }
        })
    }

    // --- View --------------------------------------------------------------

    fn view(&self) -> Element<'_, Message> {
        let body = row![
            self.file_picker(),
            rule::vertical(1.0),
            column![
                self.tab_bar(),
                rule::horizontal(1.0),
                self.main_area(),
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

    fn main_area(&self) -> Element<'_, Message> {
        match self.active_tab.and_then(|t| self.open_tabs.get(t)) {
            Some(Tab::SearchForm(form)) => self.search_form_view(form),
            Some(Tab::Result(tab)) => self.result_view(tab),
            _ => column![
                self.toolbar(),
                rule::horizontal(1.0),
                self.log_view(),
            ]
            .width(Fill)
            .height(Fill)
            .into(),
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
                    rows.push(self.connection_node(conn));
                }
            }
        }
        column(rows).spacing(1.0).width(Fill).into()
    }

    fn connection_node<'a>(&'a self, conn: &'a Connection) -> Element<'a, Message> {
        let open = self.expanded.contains(&conn.id);
        let marker = if open { "\u{25be}" } else { "\u{25b8}" };

        let header = row![
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
        .align_y(iced::Alignment::Center);

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
                    let active = matches!(
                        self.active_tab.and_then(|t| self.open_tabs.get(t)),
                        Some(Tab::Result(rt)) if rt.saved_id == saved.id
                    );
                    rows.push(
                        button(text(saved.name.clone()).size(13.0))
                            .on_press(Message::OpenSavedSearch {
                                connection: conn.id.clone(),
                                search: saved.id.clone(),
                            })
                            .width(Fill)
                            .padding(Padding::new(4.0).left(40.0).right(4.0))
                            .style(style::picker_row(active))
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
            let name = tab.title(&self.files);

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
                text("Open a file or Saved Search from the picker")
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

    // --- Search form view ----------------------------------------------

    fn search_form_view<'a>(&'a self, form: &'a SearchForm) -> Element<'a, Message> {
        let cancel_idx = self.active_tab.unwrap_or(0);
        let conn_name = self
            .connection(&form.connection_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();

        let mut col = column![
            text(if form.saved_id.is_some() {
                "Edit Search"
            } else {
                "New Search"
            })
            .size(16.0)
            .color(TEXT),
            text(format!("on {conn_name}")).size(12.0).color(TEXT_DIM),
            space().height(6.0),
            field_label("Name"),
            text_input("checkout-errors", &form.name)
                .on_input(Message::SearchName)
                .padding(6.0),
            field_label("Target — index, data stream, or pattern"),
            text_input("logs-*", &form.target)
                .on_input(Message::SearchTargetInput)
                .padding(6.0),
        ]
        .spacing(6.0)
        .max_width(560.0);

        if form.targets_loading {
            col = col.push(text("Loading indices\u{2026}").size(11.0).color(TEXT_DIM));
        } else {
            let matches = form.target_matches();
            if !matches.is_empty() {
                let mut opts = column![].spacing(1.0);
                for name in matches {
                    opts = opts.push(
                        button(text(name.clone()).size(12.0))
                            .on_press(Message::SearchTargetPicked(name.clone()))
                            .width(Fill)
                            .padding(Padding::new(3.0).left(8.0))
                            .style(style::picker_row(false)),
                    );
                }
                col = col.push(
                    container(opts)
                        .max_width(560.0)
                        .style(|_| style::panel(PANEL)),
                );
            }
        }

        col = col.push(field_label("Query string (Lucene) — empty matches all"));
        col = col.push(
            text_input("level:ERROR AND service:checkout", &form.query_string)
                .on_input(Message::SearchQuery)
                .padding(6.0),
        );

        col = col.push(field_label("Timeframe"));
        col = col.push(
            row![
                radio(
                    "Relative",
                    TimeframeMode::Relative,
                    Some(form.mode),
                    Message::SearchTimeframeMode,
                )
                .size(14.0),
                radio(
                    "Absolute",
                    TimeframeMode::Absolute,
                    Some(form.mode),
                    Message::SearchTimeframeMode,
                )
                .size(14.0),
            ]
            .spacing(16.0),
        );

        match form.mode {
            TimeframeMode::Relative => {
                let units = row(TimeUnit::ALL.iter().map(|&u| {
                    radio(u.label(), u, Some(form.rel_unit), Message::SearchRelUnit)
                        .size(14.0)
                        .into()
                }))
                .spacing(12.0);
                col = col.push(
                    row![
                        text("Last").size(13.0).color(TEXT),
                        text_input("15", &form.rel_amount)
                            .on_input(Message::SearchRelAmount)
                            .width(60.0)
                            .padding(6.0),
                        units,
                    ]
                    .spacing(10.0)
                    .align_y(iced::Alignment::Center),
                );
            }
            TimeframeMode::Absolute => {
                col = col.push(
                    row![
                        column![
                            field_label("From"),
                            text_input("2026-08-28T09:00:00", &form.abs_from)
                                .on_input(Message::SearchAbsFrom)
                                .padding(6.0),
                        ]
                        .spacing(4.0),
                        column![
                            field_label("To"),
                            text_input("2026-08-28T10:00:00", &form.abs_to)
                                .on_input(Message::SearchAbsTo)
                                .padding(6.0),
                        ]
                        .spacing(4.0),
                    ]
                    .spacing(10.0),
                );
            }
        }

        col = col.push(field_label("Timestamp field"));
        col = col.push(
            text_input("@timestamp", &form.timestamp_field)
                .on_input(Message::SearchTimestampField)
                .padding(6.0),
        );
        // --- Sort ---
        col = col.push(field_label("Sort field"));
        let sortable = form.fields.caps().map(|c| c.sortable.as_slice());
        let sort_ctl: Element<'_, Message> = match sortable {
            Some(options) if !options.is_empty() => pick_list(
                options,
                Some(form.sort_field.clone()),
                Message::SearchSortField,
            )
            .text_size(12.0)
            .padding(4.0)
            .width(Fill)
            .into(),
            _ => text_input("@timestamp", &form.sort_field)
                .on_input(Message::SearchSortField)
                .padding(6.0)
                .into(),
        };
        col = col.push(sort_ctl);
        col = col.push(
            row![
                radio("Descending", true, Some(form.sort_desc), Message::SearchSortDir)
                    .size(14.0),
                radio("Ascending", false, Some(form.sort_desc), Message::SearchSortDir)
                    .size(14.0),
            ]
            .spacing(16.0),
        );

        // --- Columns ---
        col = col.push(field_label("Columns"));
        for (i, name) in form.columns.iter().enumerate() {
            col = col.push(column_row(
                name,
                i,
                form.columns.len(),
                Message::SearchColumnMove,
                Message::SearchColumnRemove,
            ));
        }
        let all_fields = form.fields.caps().map(|c| c.all.as_slice());
        if let Some(options) = all_fields {
            if !options.is_empty() {
                col = col.push(
                    pick_list(options, None::<String>, Message::SearchColumnAddField)
                        .placeholder("Add a field\u{2026}")
                        .text_size(12.0)
                        .padding(4.0)
                        .width(Fill),
                );
            }
        }
        col = col.push(
            row![
                text_input("field.name", &form.column_draft)
                    .on_input(Message::SearchColumnDraft)
                    .on_submit(Message::SearchColumnAdd)
                    .padding(6.0),
                button(text("Add").size(13.0).color(TEXT))
                    .on_press(Message::SearchColumnAdd)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0),
        );

        if let Some(err) = &form.error {
            col = col.push(text(err.clone()).size(12.0).color(ERR_RED));
        }

        col = col.push(space().height(10.0));
        col = col.push(
            row![
                button(text("Save & Run").size(13.0).color(TEXT))
                    .on_press(Message::SearchSave)
                    .padding(Padding::new(6.0).left(16.0).right(16.0))
                    .style(style::picker_row(true)),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::CloseTab(cancel_idx))
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
            ]
            .spacing(8.0),
        );

        container(scrollable(col.padding(Padding::new(0.0).right(12.0))).height(Fill))
            .style(|_| style::panel(BG))
            .width(Fill)
            .height(Fill)
            .padding(16.0)
            .into()
    }

    // --- Result tab view ---------------------------------------------

    fn result_view<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let bar = container(
            row![
                text(tab.saved_name.clone()).size(12.0).color(TEXT),
                text(format!("\u{b7} {}", tab.target)).size(12.0).color(TEXT_DIM),
                space().width(Fill),
                meta(&format!("{} hits", tab.hits.len())),
            ]
            .spacing(12.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(6.0).left(12.0).right(12.0));

        let body: Element<'_, Message> = match &tab.state {
            RunState::Loading => centered("Running\u{2026}", TEXT_DIM),
            RunState::Empty => {
                centered("No hits for this query and timeframe", TEXT_DIM)
            }
            RunState::Error(err) => container(
                container(text(err.clone()).size(13.0).color(ERR_RED))
                    .style(|_| {
                        let mut s = style::panel(PANEL_ALT);
                        s.border = Border {
                            color: ERR_RED,
                            width: 1.0,
                            radius: 3.0.into(),
                        };
                        s
                    })
                    .padding(12.0)
                    .width(Fill),
            )
            .padding(12.0)
            .width(Fill)
            .into(),
            RunState::Loaded => self.hit_table(tab),
        };

        let mut layout = column![bar, rule::horizontal(1.0)]
            .width(Fill)
            .height(Fill);
        if matches!(tab.state, RunState::Loaded | RunState::Empty) {
            layout = layout.push(self.result_columns_bar(tab));
            layout = layout.push(rule::horizontal(1.0));
        }
        layout.push(body).into()
    }

    /// The live Column + sort editor strip above a Result Tab's table.
    fn result_columns_bar<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;

        let mut chips = row![text("Columns").size(11.0).color(TEXT_DIM)]
            .spacing(6.0)
            .align_y(iced::Alignment::Center);
        for (i, name) in tab.columns.iter().enumerate() {
            let mut left = button(text("\u{2039}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i > 0 {
                left = left.on_press(Message::ResultColumnMove(run_id, i, -1));
            }
            let mut right = button(text("\u{203a}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i + 1 < tab.columns.len() {
                right = right.on_press(Message::ResultColumnMove(run_id, i, 1));
            }
            chips = chips.push(
                container(
                    row![
                        text(name.clone()).size(11.0).color(TEXT),
                        left,
                        right,
                        button(text("\u{00d7}").size(10.0).color(TEXT_DIM))
                            .on_press(Message::ResultColumnRemove(run_id, i))
                            .padding(1.0)
                            .style(style::bare_button()),
                    ]
                    .spacing(3.0)
                    .align_y(iced::Alignment::Center),
                )
                .style(|_| style::panel(PANEL_ALT))
                .padding(Padding::new(2.0).left(6.0).right(4.0)),
            );
        }

        let add: Element<'_, Message> = if !tab.all_fields.is_empty() {
            pick_list(tab.all_fields.as_slice(), None::<String>, move |f| {
                Message::ResultColumnAddField(run_id, f)
            })
            .placeholder("+ field")
            .text_size(11.0)
            .padding(3.0)
            .into()
        } else {
            row![
                text_input("+ field", &tab.column_draft)
                    .on_input(move |v| Message::ResultColumnDraft(run_id, v))
                    .on_submit(Message::ResultColumnAdd(run_id))
                    .size(11.0)
                    .padding(3.0)
                    .width(120.0),
                button(text("Add").size(11.0).color(TEXT))
                    .on_press(Message::ResultColumnAdd(run_id))
                    .padding(Padding::new(2.0).left(8.0).right(8.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(4.0)
            .into()
        };
        chips = chips.push(add);

        let sort_ctl: Element<'_, Message> = if !tab.sortable_fields.is_empty() {
            pick_list(
                tab.sortable_fields.as_slice(),
                Some(tab.sort_field.clone()),
                move |f| Message::ResultSortField(run_id, f),
            )
            .text_size(11.0)
            .padding(3.0)
            .into()
        } else {
            text_input("@timestamp", &tab.sort_field)
                .on_input(move |v| Message::ResultSortField(run_id, v))
                .size(11.0)
                .padding(3.0)
                .width(150.0)
                .into()
        };
        let dir = button(
            text(if tab.sort_desc { "desc \u{25be}" } else { "asc \u{25b4}" })
                .size(11.0)
                .color(TEXT),
        )
        .on_press(Message::ResultSortDir(run_id, !tab.sort_desc))
        .padding(Padding::new(2.0).left(8.0).right(8.0))
        .style(style::bare_button());

        container(
            row![
                scrollable(chips).horizontal().width(Fill),
                text("Sort").size(11.0).color(TEXT_DIM),
                sort_ctl,
                dir,
            ]
            .spacing(8.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(4.0).left(12.0).right(12.0))
        .into()
    }

    fn hit_table<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let header = row(tab.columns.iter().map(|col| -> Element<'_, Message> {
            container(text(col.clone()).size(12.0).color(TEXT_DIM))
                .width(col_width(col, &tab.timestamp_field))
                .padding(Padding::new(4.0).left(6.0))
                .into()
        }))
        .spacing(8.0);

        // Only build widgets for the slice around the viewport; pad the rest
        // with spacers so the scrollbar still spans every loaded Hit.
        let (start, end) = tab.row_window();
        let mut body: Vec<Element<'_, Message>> = Vec::with_capacity(end - start + 2);
        if start > 0 {
            body.push(space().height(start as f32 * ROW_H).into());
        }
        for hit in &tab.hits[start..end] {
            body.push(
                container(
                    row(tab.columns.iter().map(|col| -> Element<'_, Message> {
                        let value = results::cell(
                            &hit.source,
                            col,
                            &tab.timestamp_field,
                            tab.utc,
                        );
                        container(
                            text(value)
                                .size(12.0)
                                .font(Font::MONOSPACE)
                                .wrapping(text::Wrapping::None),
                        )
                        .width(col_width(col, &tab.timestamp_field))
                        .padding(Padding::new(3.0).left(6.0))
                        .clip(true)
                        .into()
                    }))
                    .spacing(8.0),
                )
                .width(Fill)
                .height(Length::Fixed(ROW_H))
                .clip(true)
                .into(),
            );
        }
        let trailing = tab.hits.len().saturating_sub(end);
        if trailing > 0 {
            body.push(space().height(trailing as f32 * ROW_H).into());
        }

        let run_id = tab.run_id;
        let table = scrollable(column(body).width(Fill))
            .width(Fill)
            .height(Fill)
            .on_scroll(move |viewport| {
                let offset = viewport.absolute_offset();
                Message::ResultScrolled {
                    run_id,
                    offset_y: offset.y,
                    viewport_h: viewport.bounds().height,
                    content_h: viewport.content_bounds().height,
                }
            });

        let mut stacked = column![
            container(header)
                .style(|_| style::panel(PANEL_ALT))
                .width(Fill)
                .padding(Padding::new(2.0).left(6.0)),
            rule::horizontal(1.0),
            table,
        ]
        .width(Fill)
        .height(Fill);

        if let Some(footer) = self.paging_footer(tab) {
            stacked = stacked.push(rule::horizontal(1.0));
            stacked = stacked.push(footer);
        }

        stacked.into()
    }

    fn paging_footer<'a>(&self, tab: &'a ResultTab) -> Option<Element<'a, Message>> {
        let run_id = tab.run_id;
        let content: Element<'_, Message> = match &tab.paging {
            Paging::Idle | Paging::Exhausted => return None,
            Paging::Loading => {
                text("Loading more\u{2026}").size(12.0).color(TEXT_DIM).into()
            }
            Paging::Capped => text(format!(
                "Showing first {RETENTION_CAP} Hits — refine your search"
            ))
            .size(12.0)
            .color(TEXT_DIM)
            .into(),
            Paging::Failed(err) => row![
                text(format!("Failed to load more — {err}"))
                    .size(12.0)
                    .color(ERR_RED),
                button(text("Retry").size(12.0).color(TEXT))
                    .on_press(Message::RetryPage(run_id))
                    .padding(Padding::new(3.0).left(10.0).right(10.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(10.0)
            .align_y(iced::Alignment::Center)
            .into(),
        };

        Some(
            container(content)
                .style(|_| style::panel(PANEL_ALT))
                .width(Fill)
                .padding(Padding::new(5.0).left(12.0).right(12.0))
                .into(),
        )
    }

    // --- Modals ------------------------------------------------------

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
            fields.push(text(err.clone()).size(12.0).color(ERR_RED).into());
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

fn centered<'a>(label: &'a str, color: Color) -> Element<'a, Message> {
    container(text(label.to_string()).size(14.0).color(color))
        .center_x(Fill)
        .center_y(Fill)
        .width(Fill)
        .height(Fill)
        .into()
}

/// One editable Column row: name, reorder arrows, remove — used by the Search
/// form. `on_move(index, delta)` and `on_remove(index)` build the messages.
fn column_row<'a>(
    name: &'a str,
    index: usize,
    total: usize,
    on_move: impl Fn(usize, isize) -> Message,
    on_remove: impl Fn(usize) -> Message,
) -> Element<'a, Message> {
    let mut up = button(text("\u{2191}").size(11.0).color(TEXT_DIM))
        .padding(3.0)
        .style(style::bare_button());
    if index > 0 {
        up = up.on_press(on_move(index, -1));
    }
    let mut down = button(text("\u{2193}").size(11.0).color(TEXT_DIM))
        .padding(3.0)
        .style(style::bare_button());
    if index + 1 < total {
        down = down.on_press(on_move(index, 1));
    }
    row![
        text(name.to_string()).size(12.0).width(Fill),
        up,
        down,
        button(text("\u{00d7}").size(12.0).color(TEXT_DIM))
            .on_press(on_remove(index))
            .padding(3.0)
            .style(style::bare_button()),
    ]
    .spacing(4.0)
    .align_y(iced::Alignment::Center)
    .into()
}

fn col_width(col: &str, timestamp_field: &str) -> Length {
    if col == timestamp_field {
        Length::Fixed(210.0)
    } else {
        Length::Fill
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

fn test_result(state: &TestState) -> Element<'_, Message> {
    match state {
        TestState::Idle => space().width(0.0).into(),
        TestState::Running => text("Testing\u{2026}").size(12.0).color(TEXT_DIM).into(),
        TestState::Ok(msg) => text(format!("\u{2713} {msg}")).size(12.0).color(OK_GREEN).into(),
        TestState::Failed(err) => {
            text(format!("\u{2717} {err}")).size(12.0).color(ERR_RED).into()
        }
    }
}

/// Centres `content` in a panel card over a dimmed backdrop.
fn modal_card<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    let card = container(content).width(460.0).padding(20.0).style(|_| {
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

/// Folds a fetched Page into a Result Tab: replacing Hits on a first run,
/// appending on a scroll-driven load-more, and settling the paging state.
fn apply_page(rt: &mut ResultTab, result: Result<es::Page, String>, append: bool) {
    match result {
        Ok(page) => {
            if let Some(pit) = page.pit_id {
                rt.pit_id = Some(pit);
            }
            let got = page.hits.len();
            if append {
                rt.hits.extend(page.hits);
            } else {
                rt.hits = page.hits;
            }

            if !append {
                rt.state = if rt.hits.is_empty() {
                    RunState::Empty
                } else {
                    RunState::Loaded
                };
            }
            rt.paging = if rt.hits.len() >= RETENTION_CAP {
                Paging::Capped
            } else if got < 1000 {
                Paging::Exhausted
            } else {
                Paging::Idle
            };
        }
        Err(err) => {
            if append {
                // Leave already-loaded Hits untouched; offer a retry.
                rt.paging = Paging::Failed(err);
            } else {
                rt.state = RunState::Error(err);
            }
        }
    }
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
