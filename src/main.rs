mod config;
mod connection;
mod es;
mod icons;
mod results;
mod search;
mod secrets;
mod style;
mod tab;

use std::collections::{HashMap, HashSet};

use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, radio, row, rule, scrollable,
    space, stack, svg, text, text_editor, text_input,
};
use iced::widget::svg::Handle;
use iced::{Border, Color, Element, Fill, Font, Length, Padding, Point, Subscription, Task, Theme};

use config::{Auth, Config, Connection};
use config::{TimeUnit, TimeframeChoice, TimeframeMode};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use results::{Paging, RETENTION_CAP, ROW_H, ResultTab, RunState, TimeframeDraft};
use search::{Fields, SearchForm};
use style::{ACCENT, BG, BORDER, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, tracked in the `expanded`
/// set like a folder. The control char keeps it from colliding with a real
/// Connection name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

const OK_GREEN: Color = Color::from_rgb8(0x6c, 0xc0, 0x7a);
const ERR_RED: Color = Color::from_rgb8(0xe0, 0x6c, 0x6c);

pub fn main() -> iced::Result {
    iced::application(LogLens::new, LogLens::update, LogLens::view)
        .title("Log Lens")
        .theme(LogLens::theme)
        .subscription(LogLens::subscription)
        .window_size(iced::Size::new(1180.0, 760.0))
        .run()
}

// --- State -----------------------------------------------------------------

struct LogLens {
    config: Config,
    /// Open tabs, in tab order.
    open_tabs: Vec<Tab>,
    /// Index into `open_tabs`.
    active_tab: Option<usize>,
    expanded: HashSet<String>,
    /// The Connection form, when adding or editing one.
    connection_form: Option<ConnectionForm>,
    /// The Search settings modal, when editing an existing Saved Search's
    /// name / Target / timestamp field.
    search_settings: Option<SearchForm>,
    /// A prompt for a secret the keyring can't give us this session.
    secret_prompt: Option<SecretPrompt>,
    /// Transient status line (config save failures, keyring notices).
    status: Option<String>,
    /// Source of stable ids for Search forms and Result Tabs.
    id_seq: u64,
    /// Active drag of a Hit detail panel's top edge.
    detail_drag: Option<DetailDrag>,
    /// Active drag-resize of a Result Tab's table Column.
    column_drag: Option<ColumnDrag>,
    /// Column index whose header resize grip the pointer is currently over,
    /// so the hairline shows only on hover.
    grip_hover: Option<usize>,
    /// Column index whose header the pointer is currently over, so the
    /// "\u{22ee}" settings affordance shows only on hover.
    header_hover: Option<usize>,
    /// The tree row whose Edit / Delete menu is open.
    tree_menu: Option<TreeMenu>,
    /// Cursor position over the sidebar, tracked so the right-click menu can
    /// open as a floating dropdown at the pointer.
    sidebar_cursor: Point,
    /// Where the open `tree_menu` dropdown is anchored (sidebar coordinates).
    tree_menu_at: Point,
    /// A pending destructive action awaiting confirmation.
    confirm: Option<Confirm>,
}

/// Which tree row's management menu is currently open.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeMenu {
    Connection(String),
    Search { connection: String, search: String },
}

/// A destructive action shown in a confirmation modal before it runs.
#[derive(Debug, Clone)]
enum Confirm {
    DeleteConnection { id: String, name: String },
}

/// In-progress resize of a Result Tab's Hit detail panel.
struct DetailDrag {
    run_id: u64,
    /// Cursor y at the previous move event, for computing the delta.
    last_y: Option<f32>,
}

/// In-progress drag-resize of a Result Tab's table Column, by its header's
/// right edge.
struct ColumnDrag {
    run_id: u64,
    index: usize,
    /// Cursor x at the previous move event, for computing the delta.
    last_x: Option<f32>,
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
    // Tabs
    SelectTab(usize),
    CloseTab(usize),
    ToggleFolder(String),
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
    // Search settings (create form tab + edit modal)
    NewSearch(String),
    OpenSavedSearch {
        connection: String,
        search: String,
    },
    SearchTargetsLoaded {
        form_id: u64,
        result: Result<Vec<String>, String>,
    },
    SearchName(String),
    SearchTargetInput(String),
    SearchTargetPicked(String),
    SearchTimestampField(String),
    SearchFieldsLoaded {
        form_id: u64,
        result: Result<es::FieldCaps, String>,
    },
    /// Save & Run the new-Saved-Search form tab.
    SearchSave,
    /// Save the Search settings modal (re-runs an open Result Tab for it).
    SearchSettingsSave,
    /// Dismiss the Search settings modal without saving.
    SearchSettingsCancel,
    // Result tab: live query string, timeframe, columns + sort
    ResultQueryDraft(u64, String),
    ResultQuerySubmit(u64),
    /// A timeframe dropdown pick: a preset applies immediately, `Custom` opens
    /// the popover.
    ResultTimeframeChoice(u64, TimeframeChoice),
    ResultTfMode(u64, TimeframeMode),
    ResultTfRelAmount(u64, String),
    ResultTfRelUnit(u64, TimeUnit),
    ResultTfAbsFrom(u64, String),
    ResultTfAbsTo(u64, String),
    /// Apply the "Custom\u{2026}" popover's draft timeframe and re-run.
    ResultTfApply(u64),
    /// Dismiss the popover without changing the timeframe.
    ResultTfCancel(u64),
    ResultFieldsLoaded {
        run_id: u64,
        result: Result<es::FieldCaps, String>,
    },
    ResultColumnDraft(u64, String),
    ResultColumnAdd(u64),
    ResultColumnAddField(u64, String),
    ResultColumnRemove(u64, usize),
    ResultColumnMove(u64, usize, isize),
    /// Drag-resize of a table Column by its header's right edge.
    ColumnDragStart(u64, usize),
    ColumnDragTo(f32),
    ColumnDragEnd,
    /// Pointer entered (`Some`) or left (`None`) a header resize grip.
    GripHover(Option<usize>),
    /// Pointer entered (`Some`) or left (`None`) a table Column header.
    HeaderHover(Option<usize>),
    /// Toggle a column header's "\u{22ee}" settings menu (by column index).
    ResultHeaderMenu(u64, usize),
    /// Close any open column header settings menu.
    ResultHeaderMenuDismiss(u64),
    /// Toggle the Search bar's "Sort fields" popover.
    ResultSortPanel(u64),
    /// Set a field's sort direction, adding it to the sort order if new.
    ResultSortSet(u64, String, bool),
    /// Drop a field from the sort order.
    ResultSortRemove(u64, String),
    /// Reorder the sort key at the given position by `delta` places.
    ResultSortMove(u64, usize, isize),
    /// Clear the whole sort order.
    ResultSortClear(u64),
    // Hit detail panel
    HitClicked(u64, usize),
    CloseHitDetail,
    DetailEdit(u64, text_editor::Action),
    DetailDragStart(u64),
    DetailDragTo(f32),
    DetailDragEnd,
    // Result tab run
    PitOpened {
        run_id: u64,
        result: Result<String, String>,
    },
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
    /// Discard the active Result Tab's PIT and re-run it from the Search bar.
    RefreshResult(u64),
    // Tree item management
    /// Pointer moved over the sidebar; tracked to anchor the right-click menu.
    SidebarCursor(Point),
    /// Right-click on a tree row toggles its Edit / Delete dropdown.
    TreeMenuToggle(TreeMenu),
    /// Click outside an open tree menu closes it.
    TreeMenuDismiss,
    EditConnection(String),
    RequestDeleteConnection(String),
    EditSearch {
        connection: String,
        search: String,
    },
    DeleteSearch {
        connection: String,
        search: String,
    },
    ConfirmProceed,
    ConfirmCancel,
    // Misc
    DismissStatus,
    Ignore,
}

impl LogLens {
    fn new() -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(ES_ROOT.to_string());

        Self {
            config: config::load(),
            open_tabs: Vec::new(),
            active_tab: None,
            expanded,
            connection_form: None,
            search_settings: None,
            secret_prompt: None,
            status: None,
            id_seq: 0,
            detail_drag: None,
            column_drag: None,
            grip_hover: None,
            header_hover: None,
            tree_menu: None,
            sidebar_cursor: Point::ORIGIN,
            tree_menu_at: Point::ORIGIN,
            confirm: None,
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        use iced::event::Event;
        use iced::keyboard;

        let escape = iced::event::listen_with(|event, _status, _id| match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::CloseHitDetail),
            _ => None,
        });

        if self.detail_drag.is_some() {
            let drag = iced::event::listen_with(|event, _status, _id| match event {
                Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::DetailDragTo(position.y))
                }
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::DetailDragEnd)
                }
                _ => None,
            });
            return Subscription::batch([escape, drag]);
        }

        if self.column_drag.is_some() {
            let drag = iced::event::listen_with(|event, _status, _id| match event {
                Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::ColumnDragTo(position.x))
                }
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::ColumnDragEnd)
                }
                _ => None,
            });
            return Subscription::batch([escape, drag]);
        }

        escape
    }

    fn next_id(&mut self) -> u64 {
        self.id_seq += 1;
        self.id_seq
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
        if self
            .search_settings
            .as_ref()
            .is_some_and(|f| f.form_id == form_id)
        {
            return self.search_settings.as_mut();
        }
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

    /// The Search form currently taking field edits: the Search settings modal
    /// while it is open, otherwise the active new-Saved-Search form tab.
    fn editing_search_form_mut(&mut self) -> Option<&mut SearchForm> {
        if self.search_settings.is_some() {
            return self.search_settings.as_mut();
        }
        self.active_form_mut()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectTab(tab) => {
                if tab < self.open_tabs.len() {
                    self.active_tab = Some(tab);
                }
            }
            Message::CloseTab(tab) => return self.close_tab(tab),
            Message::ToggleFolder(name) => {
                if !self.expanded.remove(&name) {
                    self.expanded.insert(name);
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
                                "Connection secret required to run this search".to_string(),
                            );
                        }
                    }
                }
            }

            Message::NewSearch(conn_id) => return self.open_search_form(conn_id),
            Message::OpenSavedSearch { connection, search } => {
                return self.open_result_tab(connection, search, None, None, false);
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
                if let Some(f) = self.editing_search_form_mut() {
                    f.name = v;
                    f.error = None;
                }
            }
            Message::SearchTargetInput(v) => {
                if let Some(f) = self.editing_search_form_mut() {
                    f.target = v;
                    f.error = None;
                }
            }
            Message::SearchTargetPicked(v) => {
                if let Some(f) = self.editing_search_form_mut() {
                    f.target = v;
                    f.error = None;
                }
                return self.load_form_fields();
            }
            Message::SearchTimestampField(v) => {
                if let Some(f) = self.editing_search_form_mut() {
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
            Message::SearchSave => return self.save_search_form(),
            Message::SearchSettingsSave => return self.save_search_settings(),
            Message::SearchSettingsCancel => self.search_settings = None,

            Message::ResultQueryDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.query_draft = v;
                }
            }
            Message::ResultQuerySubmit(run_id) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let changed = rt.query_string != rt.query_draft;
                        rt.query_string = rt.query_draft.clone();
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultTimeframeChoice(run_id, choice) => match choice.to_timeframe() {
                Some(timeframe) => {
                    let changed = self
                        .result_mut(run_id)
                        .map(|rt| {
                            rt.tf.open = false;
                            let changed = rt.timeframe != timeframe;
                            rt.timeframe = timeframe;
                            changed
                        })
                        .unwrap_or(false);
                    if changed {
                        self.sync_saved_from_result(run_id);
                        return self.start_run(run_id);
                    }
                }
                None => {
                    if let Some(rt) = self.result_mut(run_id) {
                        let current = rt.timeframe.clone();
                        rt.tf.seed(&current);
                    }
                }
            },
            Message::ResultTfMode(run_id, mode) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.mode = mode;
                }
            }
            Message::ResultTfRelAmount(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.rel_amount = v;
                }
            }
            Message::ResultTfRelUnit(run_id, unit) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.rel_unit = unit;
                }
            }
            Message::ResultTfAbsFrom(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.abs_from = v;
                }
            }
            Message::ResultTfAbsTo(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.abs_to = v;
                }
            }
            Message::ResultTfApply(run_id) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let timeframe = rt.tf.to_timeframe();
                        rt.tf.open = false;
                        let changed = rt.timeframe != timeframe;
                        rt.timeframe = timeframe;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultTfCancel(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.open = false;
                }
            }
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
                    rt.header_menu = None;
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultColumnMove(run_id, i, delta) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.move_column(i, delta);
                    rt.header_menu = None;
                }
                self.sync_saved_from_result(run_id);
            }
            Message::ResultHeaderMenu(run_id, index) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.header_menu = if rt.header_menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                }
            }
            Message::ResultHeaderMenuDismiss(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.header_menu = None;
                }
            }
            Message::ResultSortPanel(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.sort_panel_open = !rt.sort_panel_open;
                }
            }
            Message::ResultSortSet(run_id, field, desc) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.set_sort_dir(&field, desc)
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultSortRemove(run_id, field) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.remove_sort(&field)
                    })
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultSortMove(run_id, index, delta) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| rt.move_sort(index, delta))
                    .unwrap_or(false);
                if changed {
                    self.sync_saved_from_result(run_id);
                    return self.start_run(run_id);
                }
            }
            Message::ResultSortClear(run_id) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| rt.clear_sort())
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
            Message::RefreshResult(run_id) => return self.start_run(run_id),

            Message::HitClicked(run_id, index) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.toggle_detail(index);
                }
            }
            Message::CloseHitDetail => {
                self.tree_menu = None;
                if let Some(Tab::Result(rt)) =
                    self.active_tab.and_then(|t| self.open_tabs.get_mut(t))
                {
                    rt.selected_hit = None;
                    rt.header_menu = None;
                    rt.sort_panel_open = false;
                }
            }
            Message::DetailEdit(run_id, action) => {
                if !action.is_edit() {
                    if let Some(rt) = self.result_mut(run_id) {
                        rt.detail_content.perform(action);
                    }
                }
            }
            Message::DetailDragStart(run_id) => {
                self.detail_drag = Some(DetailDrag {
                    run_id,
                    last_y: None,
                });
            }
            Message::DetailDragTo(y) => {
                if let Some(drag) = self.detail_drag.as_mut() {
                    let run_id = drag.run_id;
                    let delta = drag.last_y.map_or(0.0, |prev| prev - y);
                    drag.last_y = Some(y);
                    if delta != 0.0 {
                        if let Some(rt) = self.result_mut(run_id) {
                            rt.detail_height = (rt.detail_height + delta)
                                .clamp(results::DETAIL_MIN_H, results::DETAIL_MAX_H);
                        }
                    }
                }
            }
            Message::DetailDragEnd => self.detail_drag = None,

            Message::ColumnDragStart(run_id, index) => {
                self.column_drag = Some(ColumnDrag {
                    run_id,
                    index,
                    last_x: None,
                });
            }
            Message::ColumnDragTo(x) => {
                if let Some(drag) = self.column_drag.as_mut() {
                    let (run_id, index) = (drag.run_id, drag.index);
                    let delta = drag.last_x.map_or(0.0, |prev| x - prev);
                    drag.last_x = Some(x);
                    if delta != 0.0 {
                        if let Some(rt) = self.result_mut(run_id) {
                            rt.resize_column(index, delta);
                        }
                    }
                }
            }
            Message::ColumnDragEnd => self.column_drag = None,
            Message::GripHover(v) => self.grip_hover = v,
            Message::HeaderHover(v) => self.header_hover = v,

            Message::SidebarCursor(pos) => self.sidebar_cursor = pos,
            Message::TreeMenuToggle(target) => {
                self.tree_menu = if self.tree_menu.as_ref() == Some(&target) {
                    None
                } else {
                    self.tree_menu_at = self.sidebar_cursor;
                    Some(target)
                };
            }
            Message::TreeMenuDismiss => self.tree_menu = None,
            Message::EditConnection(id) => {
                self.tree_menu = None;
                if let Some(conn) = self.connection(&id) {
                    self.connection_form = Some(ConnectionForm::editing(conn));
                }
            }
            Message::RequestDeleteConnection(id) => {
                self.tree_menu = None;
                if let Some(conn) = self.connection(&id) {
                    self.confirm = Some(Confirm::DeleteConnection {
                        id: conn.id.clone(),
                        name: conn.name.clone(),
                    });
                }
            }
            Message::EditSearch { connection, search } => {
                self.tree_menu = None;
                return self.open_search_settings(connection, search);
            }
            Message::DeleteSearch { connection, search } => {
                self.tree_menu = None;
                return self.delete_search(connection, search);
            }
            Message::ConfirmProceed => {
                let confirm = self.confirm.take();
                match confirm {
                    Some(Confirm::DeleteConnection { id, .. }) => {
                        return self.delete_connection(id);
                    }
                    None => {}
                }
            }
            Message::ConfirmCancel => self.confirm = None,

            Message::DismissStatus => self.status = None,
            Message::Ignore => {}
        }

        Task::none()
    }

    // --- Tabs ----------------------------------------------------------

    fn close_tab(&mut self, tab: usize) -> Task<Message> {
        if tab >= self.open_tabs.len() {
            return Task::none();
        }

        // Release any server-side PIT this tab was holding.
        let closing_pit = match &self.open_tabs[tab] {
            Tab::Result(rt) => rt.pit_id.clone().map(|pit| (rt.connection_id.clone(), pit)),
            _ => None,
        };

        self.open_tabs.remove(tab);
        self.active_tab = match self.active_tab {
            _ if self.open_tabs.is_empty() => None,
            Some(active) if active > tab => Some(active - 1),
            Some(active) if active == tab => Some(tab.min(self.open_tabs.len() - 1)),
            other => other,
        };

        if let Some((conn_id, pit)) = closing_pit {
            if let Some(conn) = self.connection(&conn_id) {
                if let Some(endpoint) = self.endpoint_for(conn) {
                    return Task::perform(es::close_pit(endpoint, pit), |_| Message::Ignore);
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
                let name = non_empty(&form.name)
                    .unwrap_or("this connection")
                    .to_string();
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
                self.status =
                    Some("Keyring unavailable — secret kept for this session only.".to_string());
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
            Some(endpoint) => Task::perform(es::list_targets(endpoint), move |result| {
                Message::SearchTargetsLoaded { form_id, result }
            }),
            None => {
                if let Some(f) = self.form_mut(form_id) {
                    f.targets_loading = false;
                }
                Task::none()
            }
        };
        // A fresh form has no Target yet, so `load_form_fields` is a no-op
        // here; it fires once the user picks one.
        Task::batch([targets, self.load_form_fields()])
    }

    /// Opens the Search settings modal pre-filled from an existing Saved Search.
    fn open_search_settings(&mut self, conn_id: String, search_id: String) -> Task<Message> {
        let Some(saved) = self
            .connection(&conn_id)
            .and_then(|c| c.searches.iter().find(|s| s.id == search_id))
            .cloned()
        else {
            return Task::none();
        };

        let form_id = self.next_id();
        self.search_settings = Some(SearchForm::from_saved(form_id, conn_id.clone(), &saved));

        let targets = match self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) {
            Some(endpoint) => Task::perform(es::list_targets(endpoint), move |result| {
                Message::SearchTargetsLoaded { form_id, result }
            }),
            None => {
                if let Some(f) = self.form_mut(form_id) {
                    f.targets_loading = false;
                }
                Task::none()
            }
        };
        Task::batch([targets, self.load_form_fields()])
    }

    fn delete_search(&mut self, conn_id: String, search_id: String) -> Task<Message> {
        let mut tasks: Vec<Task<Message>> = Vec::new();
        // Close the Search settings modal if it targets this search.
        if self
            .search_settings
            .as_ref()
            .and_then(|f| f.saved_id.as_deref())
            == Some(search_id.as_str())
        {
            self.search_settings = None;
        }
        // Close an open Result Tab or form for this search.
        while let Some(pos) = self.open_tabs.iter().position(|t| match t {
            Tab::Result(rt) => rt.saved_id == search_id,
            Tab::SearchForm(f) => f.saved_id.as_deref() == Some(search_id.as_str()),
        }) {
            tasks.push(self.close_tab(pos));
        }

        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) {
            conn.searches.retain(|s| s.id != search_id);
        }
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
        Task::batch(tasks)
    }

    fn delete_connection(&mut self, conn_id: String) -> Task<Message> {
        if self
            .search_settings
            .as_ref()
            .map(|f| f.connection_id.as_str())
            == Some(conn_id.as_str())
        {
            self.search_settings = None;
        }
        let close = self.close_connection_tabs(&conn_id);

        self.config.connections.retain(|c| c.id != conn_id);
        secrets::delete(&conn_id);
        self.expanded.remove(&conn_id);
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
        close
    }

    /// Closes every tab — Result or Search form — belonging to a Connection,
    /// releasing any PITs.
    fn close_connection_tabs(&mut self, conn_id: &str) -> Task<Message> {
        let mut tasks: Vec<Task<Message>> = Vec::new();
        while let Some(pos) = self.open_tabs.iter().position(|t| match t {
            Tab::Result(rt) => rt.connection_id == conn_id,
            Tab::SearchForm(f) => f.connection_id == conn_id,
        }) {
            tasks.push(self.close_tab(pos));
        }
        Task::batch(tasks)
    }

    /// Fetches `_field_caps` for the Search form's Target, if it has one.
    fn load_form_fields(&mut self) -> Task<Message> {
        let Some((form_id, conn_id, target)) = self.editing_search_form_mut().map(|f| {
            (
                f.form_id,
                f.connection_id.clone(),
                f.target.trim().to_string(),
            )
        }) else {
            return Task::none();
        };
        if target.is_empty() {
            return Task::none();
        }
        if let Some(f) = self.form_mut(form_id) {
            f.fields = Fields::Loading;
        }
        let Some(endpoint) = self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) else {
            if let Some(f) = self.form_mut(form_id) {
                f.fields = Fields::Failed;
            }
            return Task::none();
        };
        Task::perform(es::field_caps(endpoint, target), move |result| {
            Message::SearchFieldsLoaded { form_id, result }
        })
    }

    /// Writes a Result Tab's live query string / timeframe / Column / sort
    /// choices back onto its Saved Search and persists the config.
    fn sync_saved_from_result(&mut self, run_id: u64) {
        let Some((conn_id, saved_id, query_string, timeframe, columns, sort)) =
            self.result_mut(run_id).map(|rt| {
                (
                    rt.connection_id.clone(),
                    rt.saved_id.clone(),
                    rt.query_string.clone(),
                    rt.timeframe.clone(),
                    rt.columns.clone(),
                    rt.sort.clone(),
                )
            })
        else {
            return;
        };
        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) {
            if let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id) {
                saved.query_string = query_string;
                saved.timeframe = timeframe;
                saved.columns = columns;
                saved.sort = sort;
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

        let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) else {
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
        let (caps, editing) = match self.open_tabs.get(idx) {
            Some(Tab::SearchForm(f)) => (f.fields.caps().cloned(), f.saved_id.is_some()),
            _ => (None, false),
        };
        self.open_result_tab(conn_id, saved.id, Some(idx), caps, editing)
    }

    /// Persists the Search settings modal's three fields onto the Saved Search
    /// and, if a Result Tab for it is open, re-runs that tab.
    fn save_search_settings(&mut self) -> Task<Message> {
        let Some(form) = &self.search_settings else {
            return Task::none();
        };
        if let Err(err) = form.validate() {
            if let Some(f) = &mut self.search_settings {
                f.error = Some(err);
            }
            return Task::none();
        }

        let form = self.search_settings.take().unwrap();
        let Some(saved_id) = form.saved_id.clone() else {
            return Task::none();
        };
        let conn_id = form.connection_id.clone();
        let name = form.name.trim().to_string();
        let target = form.target.trim().to_string();
        let timestamp_field = form.resolved_timestamp_field();

        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) {
            if let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id) {
                saved.name = name;
                saved.target = target;
                saved.timestamp_field = timestamp_field;
            }
        }
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }

        // Re-run an open Result Tab for this Saved Search; do nothing if none
        // is open (editing settings never opens a tab).
        let has_tab = self
            .open_tabs
            .iter()
            .any(|t| matches!(t, Tab::Result(rt) if rt.saved_id == saved_id));
        if has_tab {
            let caps = form.fields.caps().cloned();
            self.open_result_tab(conn_id, saved_id, None, caps, true)
        } else {
            Task::none()
        }
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
        rerun_existing: bool,
    ) -> Task<Message> {
        if let Some(existing) = self
            .open_tabs
            .iter()
            .position(|t| matches!(t, Tab::Result(rt) if rt.saved_id == saved_id))
        {
            if let Some(form_idx) = replace {
                if form_idx != existing {
                    self.open_tabs.remove(form_idx);
                }
            }
            let existing = self
                .open_tabs
                .iter()
                .position(|t| matches!(t, Tab::Result(rt) if rt.saved_id == saved_id));
            self.active_tab = existing;

            // A saved edit refreshes the open Result Tab's parameters and
            // re-runs it; a plain re-open just focuses it.
            if rerun_existing {
                if let (Some(idx), Some(saved)) = (
                    existing,
                    self.connection(&conn_id)
                        .and_then(|c| c.searches.iter().find(|s| s.id == saved_id))
                        .cloned(),
                ) {
                    let (gte, lte) = saved.timeframe.bounds();
                    let target = saved.target.clone();
                    let run_id = match self.open_tabs.get_mut(idx) {
                        Some(Tab::Result(rt)) => {
                            let target_changed = rt.target != saved.target;
                            rt.saved_name = saved.name.clone();
                            rt.target = saved.target.clone();
                            rt.query_string = saved.query_string.clone();
                            rt.query_draft = saved.query_string.clone();
                            rt.timestamp_field = saved.timestamp_field.clone();
                            rt.columns = saved.columns.clone();
                            rt.sort = saved.sort.clone();
                            rt.timeframe = saved.timeframe.clone();
                            rt.tf = TimeframeDraft::from_timeframe(&saved.timeframe);
                            rt.gte = gte;
                            rt.lte = lte;
                            match caps {
                                Some(caps) => {
                                    rt.all_fields = caps.all;
                                    rt.sortable_fields = caps.sortable;
                                }
                                None if target_changed => {
                                    rt.all_fields.clear();
                                    rt.sortable_fields.clear();
                                }
                                None => {}
                            }
                            rt.run_id
                        }
                        _ => return Task::none(),
                    };
                    // Refetch field caps if the new Target left us without any.
                    let refetch: Task<Message> = if self
                        .result_mut(run_id)
                        .is_some_and(|rt| rt.all_fields.is_empty())
                    {
                        match self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) {
                            Some(endpoint) => {
                                Task::perform(es::field_caps(endpoint, target), move |result| {
                                    Message::ResultFieldsLoaded { run_id, result }
                                })
                            }
                            None => Task::none(),
                        }
                    } else {
                        Task::none()
                    };
                    return Task::batch([refetch, self.start_run(run_id)]);
                }
            }
            return Task::none();
        }

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let Some(saved) = conn.searches.iter().find(|s| s.id == saved_id).cloned() else {
            return Task::none();
        };

        let run_id = self.next_id();
        let (gte, lte) = saved.timeframe.bounds();
        let (all_fields, sortable_fields) = caps.map(|c| (c.all, c.sortable)).unwrap_or_default();
        let tab = ResultTab {
            run_id,
            connection_id: conn_id.clone(),
            saved_id,
            saved_name: saved.name.clone(),
            target: saved.target.clone(),
            query_string: saved.query_string.clone(),
            query_draft: saved.query_string.clone(),
            timestamp_field: saved.timestamp_field.clone(),
            columns: saved.columns.clone(),
            column_draft: String::new(),
            col_widths: HashMap::new(),
            sort: saved.sort.clone(),
            sort_panel_open: false,
            header_menu: None,
            all_fields,
            sortable_fields,
            timeframe: saved.timeframe.clone(),
            tf: TimeframeDraft::from_timeframe(&saved.timeframe),
            gte,
            lte,
            pit_id: None,
            hits: Vec::new(),
            state: RunState::Loading,
            paging: Paging::Idle,
            scroll_y: 0.0,
            viewport_h: 600.0,
            selected_hit: None,
            detail_content: text_editor::Content::new(),
            detail_height: results::DETAIL_DEFAULT_H,
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
                Some(endpoint) => Task::perform(es::field_caps(endpoint, target), move |result| {
                    Message::ResultFieldsLoaded { run_id, result }
                }),
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
            rt.selected_hit = None;
            // Re-resolve the range so a relative window re-anchors to "now".
            let (gte, lte) = rt.timeframe.bounds();
            rt.gte = gte;
            rt.lte = lte;
            let old_pit = rt.pit_id.take();
            (rt.connection_id.clone(), rt.target.clone(), old_pit)
        }) else {
            return Task::none();
        };

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };
        let close_old: Task<Message> = match (&old_pit, self.endpoint_for(conn)) {
            (Some(pit), Some(endpoint)) => {
                Task::perform(es::close_pit(endpoint, pit.clone()), |_| Message::Ignore)
            }
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

    fn on_pit_opened(&mut self, run_id: u64, result: Result<String, String>) -> Task<Message> {
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
                    sort: rt.effective_sort(),
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
        let Some((conn_id, pit, params)) = self.result_mut(run_id).and_then(|rt| {
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
                    sort: rt.effective_sort(),
                    size: remaining.min(1000),
                    search_after: Some(cursor),
                },
            ))
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
                append: true,
            }
        })
    }

    // --- View --------------------------------------------------------------

    fn view(&self) -> Element<'_, Message> {
        // Right column: an optional Search bar sits above the tab strip while a
        // Result Tab is active.
        let mut right: Vec<Element<'_, Message>> = Vec::new();
        if let Some(search_bar) = self.search_bar() {
            right.push(search_bar);
            right.push(rule::horizontal(1.0).into());
        }
        right.push(self.tab_bar());
        right.push(rule::horizontal(1.0).into());
        right.push(self.main_area());

        let body = row![
            self.sidebar(),
            rule::vertical(1.0),
            column(right).width(Fill),
        ]
        .height(Fill);

        let base: Element<'_, Message> = container(column![
            self.menu_bar(),
            rule::horizontal(1.0),
            container(body).width(Fill).height(Fill),
            self.status_bar(),
        ])
        .style(|_| style::panel(BG))
        .width(Fill)
        .height(Fill)
        .into();

        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if let Some(menu) = self.tree_menu_overlay() {
            layers.push(menu);
        }
        if let Some(form) = &self.connection_form {
            layers.push(self.connection_form_modal(form));
        }
        if let Some(form) = &self.search_settings {
            layers.push(self.search_settings_modal(form));
        }
        if let Some(prompt) = &self.secret_prompt {
            layers.push(self.secret_prompt_modal(prompt));
        }
        if let Some(confirm) = &self.confirm {
            layers.push(self.confirm_modal(confirm));
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
            None => centered("Open a Saved Search from the sidebar", TEXT_DIM),
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

    fn sidebar(&self) -> Element<'_, Message> {
        let panel =
            container(scrollable(column![self.es_section()].spacing(1.0).width(Fill)).height(Fill))
                .style(|_| style::panel(PANEL))
                .width(240.0)
                .height(Fill)
                .padding(6.0);

        mouse_area(panel).on_move(Message::SidebarCursor).into()
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
                    let active = matches!(
                        self.active_tab.and_then(|t| self.open_tabs.get(t)),
                        Some(Tab::Result(rt)) if rt.saved_id == saved.id
                    );
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

    /// The floating right-click dropdown for the open `tree_menu`, anchored at
    /// the pointer as a stack layer so it never reflows the tree.
    fn tree_menu_overlay(&self) -> Option<Element<'_, Message>> {
        let (edit, delete) = match self.tree_menu.as_ref()? {
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

        let x = self.tree_menu_at.x.min(240.0 - 136.0).max(2.0);
        let y = self.tree_menu_at.y.max(2.0);
        let anchored = container(tree_menu_block(edit, delete))
            .width(Fill)
            .height(Fill)
            .padding(Padding::new(0.0).left(x).top(y));

        Some(
            mouse_area(anchored)
                .on_press(Message::TreeMenuDismiss)
                .on_right_press(Message::TreeMenuDismiss)
                .into(),
        )
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        if self.open_tabs.is_empty() {
            return container(space().height(34.0))
                .style(|_| style::panel(PANEL_ALT))
                .width(Fill)
                .into();
        }

        let tabs = self
            .open_tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| -> Element<'_, Message> {
                let active = self.active_tab == Some(i);
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

    /// The always-present Menu bar across the top of the window. `File` and
    /// `View` are visible but inert until dropdowns land.
    fn menu_bar(&self) -> Element<'_, Message> {
        container(
            row![
                text("File").size(13.0).color(TEXT_DIM),
                text("View").size(13.0).color(TEXT_DIM),
            ]
            .spacing(18.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL_ALT))
        .width(Fill)
        .padding(Padding::new(4.0).left(12.0).right(12.0))
        .into()
    }

    /// The Search bar shown above the tab strip while a Result Tab is active:
    /// row 1 carries the query string, timeframe, Target, a Refresh control and
    /// the loaded-Hit count; row 2 is the live Column + sort strip moved out of
    /// the Result Tab. Hidden for Search form tabs and when no tab is open.
    fn search_bar(&self) -> Option<Element<'_, Message>> {
        let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) else {
            return None;
        };
        let run_id = tab.run_id;

        let selected = tab
            .timeframe
            .matches_preset()
            .unwrap_or(TimeframeChoice::Custom);
        let timeframe_ctl = pick_list(&TimeframeChoice::ALL[..], Some(selected), move |choice| {
            Message::ResultTimeframeChoice(run_id, choice)
        })
        .text_size(12.0)
        .padding(4.0);

        let row1 = container(
            row![
                text_input("Lucene Query", &tab.query_draft)
                    .on_input(move |v| Message::ResultQueryDraft(run_id, v))
                    .on_submit(Message::ResultQuerySubmit(run_id))
                    .size(12.0)
                    .padding(4.0)
                    .width(Fill),
                timeframe_ctl,
                text(format!("\u{b7} {}", tab.target))
                    .size(12.0)
                    .color(TEXT_DIM),
                button(text("Refresh").size(12.0).color(TEXT_DIM))
                    .on_press(Message::RefreshResult(run_id))
                    .padding(Padding::new(2.0).left(8.0).right(8.0))
                    .style(style::bare_button()),
                meta(&format!("{} hits", tab.hits.len())),
            ]
            .spacing(12.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(6.0).left(12.0).right(12.0));

        let mut col: Vec<Element<'_, Message>> = vec![row1.into()];
        if tab.tf.open {
            col.push(self.timeframe_popover(tab));
        }
        if matches!(tab.state, RunState::Loaded | RunState::Empty) {
            col.push(rule::horizontal(1.0).into());
            col.push(self.result_sort_bar(tab));
            if tab.sort_panel_open {
                col.push(rule::horizontal(1.0).into());
                col.push(self.sort_fields_popover(tab));
            }
        }
        Some(column(col).width(Fill).into())
    }

    /// The "Custom\u{2026}" timeframe popover: a relative or absolute window
    /// editor that drops out of the Search bar until applied or dismissed.
    fn timeframe_popover<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let tf = &tab.tf;

        let modes = row![
            radio(
                "Relative",
                TimeframeMode::Relative,
                Some(tf.mode),
                move |m| Message::ResultTfMode(run_id, m),
            )
            .size(14.0),
            radio(
                "Absolute",
                TimeframeMode::Absolute,
                Some(tf.mode),
                move |m| Message::ResultTfMode(run_id, m),
            )
            .size(14.0),
        ]
        .spacing(16.0);

        let detail: Element<'_, Message> = match tf.mode {
            TimeframeMode::Relative => {
                let units = row(TimeUnit::ALL.iter().map(move |&u| {
                    radio(u.label(), u, Some(tf.rel_unit), move |u| {
                        Message::ResultTfRelUnit(run_id, u)
                    })
                    .size(14.0)
                    .into()
                }))
                .spacing(12.0);
                row![
                    text("Last").size(13.0).color(TEXT),
                    text_input("15", &tf.rel_amount)
                        .on_input(move |v| Message::ResultTfRelAmount(run_id, v))
                        .on_submit(Message::ResultTfApply(run_id))
                        .width(60.0)
                        .padding(6.0),
                    units,
                ]
                .spacing(10.0)
                .align_y(iced::Alignment::Center)
                .into()
            }
            TimeframeMode::Absolute => row![
                column![
                    field_label("From"),
                    text_input("2026-08-28T09:00:00", &tf.abs_from)
                        .on_input(move |v| Message::ResultTfAbsFrom(run_id, v))
                        .padding(6.0),
                ]
                .spacing(4.0),
                column![
                    field_label("To"),
                    text_input("2026-08-28T10:00:00", &tf.abs_to)
                        .on_input(move |v| Message::ResultTfAbsTo(run_id, v))
                        .padding(6.0),
                ]
                .spacing(4.0),
            ]
            .spacing(10.0)
            .into(),
        };

        let actions = row![
            space().width(Fill),
            button(text("Cancel").size(12.0).color(TEXT_DIM))
                .on_press(Message::ResultTfCancel(run_id))
                .padding(Padding::new(4.0).left(12.0).right(12.0))
                .style(style::bare_button()),
            button(text("Apply").size(12.0).color(TEXT))
                .on_press(Message::ResultTfApply(run_id))
                .padding(Padding::new(4.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ]
        .spacing(8.0);

        container(column![modes, detail, space().height(2.0), actions].spacing(8.0))
            .style(|_| {
                let mut s = style::panel(PANEL);
                s.border = Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 4.0.into(),
                };
                s
            })
            .width(Fill)
            .padding(Padding::new(10.0).left(12.0).right(12.0))
            .into()
    }

    // --- Search settings (create form + edit modal) ------------------

    /// The three structural fields shared by the new-Saved-Search form and the
    /// Search settings modal: name, Target (with typeahead), timestamp field.
    fn search_settings_fields<'a>(&'a self, form: &'a SearchForm) -> Vec<Element<'a, Message>> {
        let mut fields: Vec<Element<'a, Message>> = vec![
            field_label("Name"),
            text_input("checkout-errors", &form.name)
                .on_input(Message::SearchName)
                .padding(6.0)
                .into(),
            field_label("Target — index, data stream, or pattern"),
            text_input("logs-*", &form.target)
                .on_input(Message::SearchTargetInput)
                .padding(6.0)
                .into(),
        ];

        if form.targets_loading {
            fields.push(
                text("Loading indices\u{2026}")
                    .size(11.0)
                    .color(TEXT_DIM)
                    .into(),
            );
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
                fields.push(container(opts).style(|_| style::panel(PANEL)).into());
            }
        }

        fields.push(field_label("Timestamp field"));
        fields.push(
            text_input("@timestamp", &form.timestamp_field)
                .on_input(Message::SearchTimestampField)
                .padding(6.0)
                .into(),
        );
        fields
    }

    /// The new-Saved-Search form tab: only the structural fields. Query string,
    /// timeframe, Columns and sort get defaults and are tuned from the Search
    /// bar once the Result Tab opens.
    fn search_form_view<'a>(&'a self, form: &'a SearchForm) -> Element<'a, Message> {
        let cancel_idx = self.active_tab.unwrap_or(0);
        let conn_name = self
            .connection(&form.connection_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();

        let mut col = column![
            text("New Search").size(16.0).color(TEXT),
            text(format!("on {conn_name}")).size(12.0).color(TEXT_DIM),
            text(
                "Query string, timeframe, Columns and sort are tuned from the \
                 Search bar once this opens."
            )
            .size(11.0)
            .color(TEXT_DIM),
            space().height(6.0),
        ]
        .spacing(6.0)
        .max_width(560.0);

        for field in self.search_settings_fields(form) {
            col = col.push(field);
        }

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

    /// The Search settings modal: the same three fields as the create form,
    /// shown over the current tab rather than as a tab of its own. Saving it
    /// re-runs an open Result Tab for the Saved Search.
    fn search_settings_modal<'a>(&'a self, form: &'a SearchForm) -> Element<'a, Message> {
        let conn_name = self
            .connection(&form.connection_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();

        let mut card = column![
            text("Search settings").size(16.0).color(TEXT),
            text(format!("on {conn_name}")).size(12.0).color(TEXT_DIM),
            space().height(2.0),
        ]
        .spacing(6.0)
        .width(Fill);

        for field in self.search_settings_fields(form) {
            card = card.push(field);
        }

        if let Some(err) = &form.error {
            card = card.push(text(err.clone()).size(12.0).color(ERR_RED));
        }

        card = card.push(space().height(8.0));
        card = card.push(
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::SearchSettingsCancel)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text("Save").size(13.0).color(TEXT))
                    .on_press(Message::SearchSettingsSave)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0),
        );

        modal_card(card.into())
    }

    // --- Result tab view ---------------------------------------------

    fn result_view<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let body: Element<'_, Message> = match &tab.state {
            RunState::Loading => centered("Running\u{2026}", TEXT_DIM),
            RunState::Empty => centered("No hits for this query and timeframe", TEXT_DIM),
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

        let mut layout = column![body].width(Fill).height(Fill);
        if tab.selected_hit.is_some() && matches!(tab.state, RunState::Loaded) {
            layout = layout.push(self.hit_detail(tab));
        }
        layout.into()
    }

    /// The bottom panel showing the selected Hit's full `_source`, resizable by
    /// its top edge and dismissed with Esc or a second click on the row.
    fn hit_detail<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let index = tab.selected_hit.unwrap_or(0);

        let grip = mouse_area(
            container(space().height(6.0))
                .width(Fill)
                .style(|_| style::panel(BORDER)),
        )
        .on_press(Message::DetailDragStart(run_id));

        let header = row![
            text(format!("Hit {} \u{b7} _source", index + 1))
                .size(11.0)
                .color(TEXT_DIM),
            space().width(Fill),
            button(text("Close (Esc)").size(11.0).color(TEXT_DIM))
                .on_press(Message::CloseHitDetail)
                .padding(2.0)
                .style(style::bare_button()),
        ]
        .align_y(iced::Alignment::Center);

        let editor = text_editor(&tab.detail_content)
            .on_action(move |action| Message::DetailEdit(run_id, action))
            .font(Font::MONOSPACE)
            .size(12.0)
            .height(Fill)
            .padding(Padding::new(4.0).left(8.0))
            .style(style::editor);

        column![
            grip,
            container(column![header, editor].spacing(4.0))
                .width(Fill)
                .height(Length::Fixed(tab.detail_height))
                .style(|_| style::panel(BG))
                .padding(Padding::new(6.0).left(10.0).right(10.0)),
        ]
        .width(Fill)
        .into()
    }

    /// The live controls above a Result Tab's table: a "Sort fields" button
    /// that opens the multi-field sort popover. (Column add / remove / reorder
    /// live in each header's "\u{22ee}" menu.)
    fn result_sort_bar<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;

        let sort_summary = if tab.sort.is_empty() {
            "Sort fields".to_string()
        } else {
            format!("Sort fields  {}", tab.sort.len())
        };
        let sort_btn = button(
            row![
                text("\u{2195}").size(11.0).color(TEXT_DIM),
                text(sort_summary).size(11.0).color(TEXT),
                text("\u{25be}").size(9.0).color(TEXT_DIM),
            ]
            .spacing(4.0)
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::ResultSortPanel(run_id))
        .padding(Padding::new(3.0).left(8.0).right(8.0))
        .style(style::picker_row(tab.sort_panel_open));

        container(
            row![space().width(Fill), sort_btn]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(4.0).left(12.0).right(12.0))
        .into()
    }

    /// The "Sort fields" popover: one row per sort key (remove, direction
    /// toggle, reorder) plus a picker to add a field and a "Clear sorting"
    /// action. Drops out of the Search bar until dismissed.
    fn sort_fields_popover<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let last = tab.sort.len().saturating_sub(1);

        let mut rows = column![].spacing(4.0);
        for (i, key) in tab.sort.iter().enumerate() {
            let field = key.field.clone();

            let remove = button(text("\u{00d7}").size(12.0).color(TEXT_DIM))
                .on_press(Message::ResultSortRemove(run_id, field.clone()))
                .padding(2.0)
                .style(style::bare_button());

            let name = container(text(key.field.clone()).size(12.0).color(TEXT))
                .width(Length::Fixed(220.0))
                .clip(true);

            let is_time = key.field == tab.timestamp_field;
            let (asc_label, desc_label) = if is_time {
                ("Old\u{2013}New", "New\u{2013}Old")
            } else {
                ("A\u{2013}Z", "Z\u{2013}A")
            };
            let asc = button(text(asc_label).size(11.0).color(TEXT))
                .on_press(Message::ResultSortSet(run_id, field.clone(), false))
                .padding(Padding::new(3.0).left(10.0).right(10.0))
                .style(style::picker_row(!key.desc));
            let desc = button(text(desc_label).size(11.0).color(TEXT))
                .on_press(Message::ResultSortSet(run_id, field.clone(), true))
                .padding(Padding::new(3.0).left(10.0).right(10.0))
                .style(style::picker_row(key.desc));

            let mut up = button(text("\u{25b4}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i > 0 {
                up = up.on_press(Message::ResultSortMove(run_id, i, -1));
            }
            let mut down = button(text("\u{25be}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i < last {
                down = down.on_press(Message::ResultSortMove(run_id, i, 1));
            }

            rows = rows.push(
                row![
                    remove,
                    name,
                    row![asc, desc].spacing(1.0),
                    space().width(Fill),
                    column![up, down].spacing(0.0),
                ]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
            );
        }
        if tab.sort.is_empty() {
            rows = rows.push(
                text(
                    "No sort fields \u{2014} Hits fall back to the timestamp field, newest first.",
                )
                .size(11.0)
                .color(TEXT_DIM),
            );
        }

        let pool = if !tab.sortable_fields.is_empty() {
            &tab.sortable_fields
        } else {
            &tab.all_fields
        };
        let available: Vec<String> = pool
            .iter()
            .filter(|f| tab.sort_index(f).is_none())
            .cloned()
            .collect();
        let picker: Element<'_, Message> = if available.is_empty() {
            text("Pick fields to sort by")
                .size(11.0)
                .color(TEXT_DIM)
                .into()
        } else {
            pick_list(available, None::<String>, move |f| {
                Message::ResultSortSet(run_id, f, true)
            })
            .placeholder("Pick fields to sort by")
            .text_size(11.0)
            .padding(3.0)
            .into()
        };

        let mut footer = row![picker, space().width(Fill)]
            .spacing(8.0)
            .align_y(iced::Alignment::Center);
        if !tab.sort.is_empty() {
            footer = footer.push(
                button(text("Clear sorting").size(11.0).color(ACCENT))
                    .on_press(Message::ResultSortClear(run_id))
                    .padding(Padding::new(3.0).left(8.0).right(8.0))
                    .style(style::bare_button()),
            );
        }

        container(
            column![rows, rule::horizontal(1.0), footer]
                .spacing(8.0)
                .width(Fill),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(8.0).left(12.0).right(12.0))
        .into()
    }

    fn hit_table<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let last = tab.columns.len().saturating_sub(1);
        let multi_sort = tab.sort.len() > 1;
        let header = row(tab
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| -> Element<'_, Message> {
                let mut label_row = row![text(col.clone()).size(12.0).color(TEXT_DIM)]
                    .spacing(3.0)
                    .align_y(iced::Alignment::Center);
                if let Some(rank) = tab.sort_index(col) {
                    let arrow = if tab.sort[rank].desc {
                        "\u{25be}"
                    } else {
                        "\u{25b4}"
                    };
                    label_row = label_row.push(text(arrow).size(10.0).color(TEXT));
                    if multi_sort {
                        label_row =
                            label_row.push(text(format!("{}", rank + 1)).size(9.0).color(TEXT_DIM));
                    }
                }
                let label = container(label_row)
                    .width(Fill)
                    .clip(true)
                    .padding(Padding::new(4.0).left(6.0));

                // A "\u{22ee}" affordance that opens this Column's settings menu
                // (add / remove / reorder / sort). Like the resize grip, it only shows while
                // the pointer is over the header (or the menu is open); a
                // fixed-width slot keeps the header from reflowing on hover.
                let show_dots =
                    self.header_hover == Some(i) || tab.header_menu == Some(i);
                let dots: Element<'_, Message> = if show_dots {
                    button(text("\u{22ee}").size(12.0).color(TEXT_DIM))
                        .on_press(Message::ResultHeaderMenu(run_id, i))
                        .padding(Padding::new(0.0).left(2.0).right(2.0))
                        .style(style::bare_button())
                        .into()
                } else {
                    space().width(12.0).into()
                };
                let dots = container(dots).width(Length::Fixed(14.0));

                // The last Column flexes to fill the pane, so it has no edge to
                // drag; every other Column gets a right-edge resize grip.
                let inner: Element<'_, Message> = if i == last {
                    container(row![label, dots].align_y(iced::Alignment::Center))
                        .width(Fill)
                        .into()
                } else {
                    // The hairline shows while the pointer is anywhere over this
                    // Column's header (or its own grip, or it is being dragged) —
                    // and only for that Column.
                    let lit = self.grip_hover == Some(i)
                        || self.header_hover == Some(i)
                        || matches!(&self.column_drag, Some(d) if d.run_id == run_id && d.index == i);
                    let line = container(space().width(2.0).height(14.0)).style(move |_| {
                        style::panel(if lit { TEXT_DIM } else { Color::TRANSPARENT })
                    });
                    let grip =
                        mouse_area(container(line).padding(Padding::new(0.0).left(4.0).right(4.0)))
                            .interaction(iced::mouse::Interaction::ResizingColumn)
                            .on_enter(Message::GripHover(Some(i)))
                            .on_exit(Message::GripHover(None))
                            .on_press(Message::ColumnDragStart(run_id, i));

                    container(row![label, dots, grip].align_y(iced::Alignment::Center))
                        .width(Length::Fixed(tab.col_width(col)))
                        .into()
                };

                mouse_area(inner)
                    .on_enter(Message::HeaderHover(Some(i)))
                    .on_exit(Message::HeaderHover(None))
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
        for (offset, hit) in tab.hits[start..end].iter().enumerate() {
            let index = start + offset;
            let selected = tab.selected_hit == Some(index);
            let cells = container(
                row(tab
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| -> Element<'_, Message> {
                        let value = results::cell(&hit.source, col, &tab.timestamp_field, tab.utc);
                        let width = if i == last {
                            Length::Fill
                        } else {
                            Length::Fixed(tab.col_width(col))
                        };
                        container(
                            text(value)
                                .size(12.0)
                                .font(Font::MONOSPACE)
                                .wrapping(text::Wrapping::None),
                        )
                        .width(width)
                        .padding(Padding::new(3.0).left(6.0))
                        .clip(true)
                        .into()
                    }))
                .spacing(8.0),
            )
            .width(Fill)
            .height(Length::Fixed(ROW_H))
            .clip(true)
            .style(move |_| {
                if selected {
                    style::panel(ACCENT)
                } else {
                    container::Style::default()
                }
            });

            body.push(
                mouse_area(cells)
                    .on_press(Message::HitClicked(run_id, index))
                    .into(),
            );
        }
        let trailing = tab.hits.len().saturating_sub(end);
        if trailing > 0 {
            body.push(space().height(trailing as f32 * ROW_H).into());
        }

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

        let Some(menu_col) = tab.header_menu.filter(|i| *i < tab.columns.len()) else {
            return stacked.into();
        };
        stack(vec![
            stacked.into(),
            self.header_menu_overlay(tab, menu_col),
        ])
        .into()
    }

    /// The floating "\u{22ee}" column-settings menu, dropped under the header of
    /// column `index` as a stack layer so it never reflows the table.
    fn header_menu_overlay<'a>(&self, tab: &'a ResultTab, index: usize) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let field = tab.columns[index].clone();
        let last = tab.columns.len().saturating_sub(1);
        let sorted = tab.sort_index(&field).is_some();

        const MENU_W: f32 = 200.0;

        // A 13px symbolic icon, recoloured to `tint` via the svg colour filter.
        let glyph = |handle: &'static std::sync::LazyLock<Handle>, tint: Color| {
            svg(Handle::clone(handle))
                .width(Length::Fixed(13.0))
                .height(Length::Fixed(13.0))
                .style(move |_theme, _status| svg::Style { color: Some(tint) })
        };

        // One "icon + label" menu row. `msg == None` renders it greyed and
        // inert — used when a move would run off the end of the Column list.
        let entry = |handle: &'static std::sync::LazyLock<Handle>,
                     label: &str,
                     msg: Option<Message>| {
            let (fg, tint) = match msg {
                Some(_) => (TEXT, TEXT_DIM),
                None => (TEXT_DIM, BORDER),
            };
            let mut b = button(
                row![
                    glyph(handle, tint),
                    text(label.to_string()).size(12.0).color(fg),
                ]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
            )
            .width(Fill)
            .padding(Padding::new(4.0).left(8.0).right(8.0))
            .style(style::picker_row(false));
            if let Some(msg) = msg {
                b = b.on_press(msg);
            }
            b
        };

        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(
            entry(
                &icons::ARROW_LEFT,
                "Move column left",
                (index > 0).then_some(Message::ResultColumnMove(run_id, index, -1)),
            )
            .into(),
        );
        items.push(
            entry(
                &icons::ARROW_RIGHT,
                "Move column right",
                (index < last).then_some(Message::ResultColumnMove(run_id, index, 1)),
            )
            .into(),
        );
        items.push(
            entry(
                &icons::TRASH,
                "Remove column",
                Some(Message::ResultColumnRemove(run_id, index)),
            )
            .into(),
        );

        // Fields not already shown as Columns; picked from the menu to add one.
        let available: Vec<String> = tab
            .all_fields
            .iter()
            .filter(|f| !tab.columns.iter().any(|c| c == *f))
            .cloned()
            .collect();
        let add_ctl: Element<'_, Message> = if !available.is_empty() {
            pick_list(available, None::<String>, move |f| {
                Message::ResultColumnAddField(run_id, f)
            })
            .placeholder("Add column\u{2026}")
            .text_size(12.0)
            .padding(Padding::new(4.0).left(6.0).right(6.0))
            .width(Fill)
            .into()
        } else {
            row![
                text_input("Add column\u{2026}", &tab.column_draft)
                    .on_input(move |v| Message::ResultColumnDraft(run_id, v))
                    .on_submit(Message::ResultColumnAdd(run_id))
                    .size(12.0)
                    .padding(4.0)
                    .width(Fill),
                button(text("+").size(12.0).color(TEXT))
                    .on_press(Message::ResultColumnAdd(run_id))
                    .padding(Padding::new(4.0).left(8.0).right(8.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(4.0)
            .into()
        };
        items.push(
            container(
                row![glyph(&icons::PLUS, TEXT_DIM), add_ctl]
                    .spacing(8.0)
                    .align_y(iced::Alignment::Center),
            )
            .padding(Padding::new(2.0).left(8.0).right(8.0))
            .into(),
        );

        items.push(rule::horizontal(1.0).into());
        items.push(
            entry(
                &icons::SORT_ASCENDING,
                "Sort ascending",
                Some(Message::ResultSortSet(run_id, field.clone(), false)),
            )
            .into(),
        );
        items.push(
            entry(
                &icons::SORT_DESCENDING,
                "Sort descending",
                Some(Message::ResultSortSet(run_id, field.clone(), true)),
            )
            .into(),
        );
        if sorted {
            items.push(
                entry(
                    &icons::SORT_REMOVE,
                    "Remove from sort",
                    Some(Message::ResultSortRemove(run_id, field.clone())),
                )
                .into(),
            );
        }
        let card = container(column(items).spacing(1.0).width(Length::Fixed(MENU_W)))
            .style(|_| style::menu_popup())
            .padding(3.0);

        // Header geometry: 6px container pad + each fixed Column's width + 8px
        // row spacing between Columns. Anchor the card's right edge near the
        // Column's right edge, then clamp into the pane.
        let anchored: Element<'_, Message> = if index == last {
            row![
                space().width(Fill),
                container(card).padding(Padding::new(0.0).right(6.0)),
            ]
            .into()
        } else {
            let mut right_edge = 6.0;
            for i in 0..=index {
                right_edge += tab.col_width(&tab.columns[i]) + 8.0;
            }
            let left = (right_edge - MENU_W).max(6.0);
            container(card).padding(Padding::new(0.0).left(left)).into()
        };

        mouse_area(
            container(column![space().height(26.0), anchored])
                .width(Fill)
                .height(Fill),
        )
        .on_press(Message::ResultHeaderMenuDismiss(run_id))
        .on_right_press(Message::ResultHeaderMenuDismiss(run_id))
        .into()
    }

    fn paging_footer<'a>(&self, tab: &'a ResultTab) -> Option<Element<'a, Message>> {
        let run_id = tab.run_id;
        let content: Element<'_, Message> = match &tab.paging {
            Paging::Idle | Paging::Exhausted => return None,
            Paging::Loading => text("Loading more\u{2026}")
                .size(12.0)
                .color(TEXT_DIM)
                .into(),
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

    fn connection_form_modal<'a>(&'a self, form: &'a ConnectionForm) -> Element<'a, Message> {
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

    fn secret_prompt_modal<'a>(&'a self, prompt: &'a SecretPrompt) -> Element<'a, Message> {
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

    fn confirm_modal<'a>(&'a self, confirm: &'a Confirm) -> Element<'a, Message> {
        let (title, body, proceed) = match confirm {
            Confirm::DeleteConnection { name, .. } => (
                "Delete Connection",
                format!("Delete \"{name}\" and all of its Saved Searches? This cannot be undone."),
                "Delete",
            ),
        };

        let card = column![
            text(title).size(16.0).color(TEXT),
            text(body).size(12.0).color(TEXT_DIM),
            space().height(10.0),
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::ConfirmCancel)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text(proceed).size(13.0).color(TEXT))
                    .on_press(Message::ConfirmProceed)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(|_theme: &Theme, status| {
                        let base = style::picker_row(true)(_theme, status);
                        button::Style {
                            background: Some(ERR_RED.into()),
                            ..base
                        }
                    }),
            ]
            .spacing(8.0),
        ]
        .spacing(6.0)
        .width(Fill);

        modal_card(card.into())
    }
}

// --- Small view helpers ----------------------------------------------------

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

/// The floating Edit / Delete dropdown opened by right-clicking a tree row.
fn tree_menu_block<'a>(edit: Message, delete: Message) -> Element<'a, Message> {
    container(
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
    )
    .width(130.0)
    .padding(3.0)
    .style(|_| style::menu_popup())
    .into()
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

fn test_result(state: &TestState) -> Element<'_, Message> {
    match state {
        TestState::Idle => space().width(0.0).into(),
        TestState::Running => text("Testing\u{2026}").size(12.0).color(TEXT_DIM).into(),
        TestState::Ok(msg) => text(format!("\u{2713} {msg}"))
            .size(12.0)
            .color(OK_GREEN)
            .into(),
        TestState::Failed(err) => text(format!("\u{2717} {err}"))
            .size(12.0)
            .color(ERR_RED)
            .into(),
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
            background: Some(
                Color {
                    a: 0.6,
                    ..Color::BLACK
                }
                .into(),
            ),
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
