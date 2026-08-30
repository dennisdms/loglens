mod config;
mod connection;
mod es;
mod icons;
mod line;
mod results;
mod rules;
mod search;
mod secrets;
mod style;
mod tab;

use std::collections::{HashMap, HashSet};

use iced::widget::scrollable::RelativeOffset;
use iced::widget::svg::Handle;
use iced::widget::{
    Id, button, checkbox, column, container, mouse_area, opaque, operation, pick_list, radio, row,
    rule, scrollable, space, stack, svg, text, text_editor, text_input, tooltip,
};
use iced::window;
use iced::{
    Border, Color, Element, Fill, Font, Length, Padding, Point, Size, Subscription, Task, Theme,
};

use config::{Auth, Config, Connection};
use config::{TimeUnit, TimeframeChoice, TimeframeMode};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use results::{Paging, ROW_H, ResultTab, RunState, TimeframeDraft, TotalHits};
use rules::{MatcherKind, RulesForm};
use search::{Fields, SearchForm};
use style::{ACCENT, BG, BORDER, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, tracked in the `expanded`
/// set like a folder. The control char keeps it from colliding with a real
/// Connection name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

const OK_GREEN: Color = Color::from_rgb8(0x6c, 0xc0, 0x7a);
const ERR_RED: Color = Color::from_rgb8(0xe0, 0x6c, 0x6c);
const WARN_AMBER: Color = Color::from_rgb8(0xd6, 0xa5, 0x4c);

/// The display name, as `--version` and `--help` print it. The binary is
/// `loglens`; this is what a human is shown.
const APP_NAME: &str = "Log Lens";

/// This build's version: the crate version plus the short commit hash it was
/// built from, e.g. `0.1.0 (a1b2c3d)`. Several builds share a version number,
/// so the hash is what makes the first bug report against "0.1.0" say *which*
/// 0.1.0. The hash is `unknown` when built from a source archive with no
/// repository present — see `build.rs`, which stamps it in.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("LOGLENS_GIT_SHA"),
    ")"
);

pub fn main() -> iced::Result {
    if handle_cli_flags() {
        return Ok(());
    }

    // A daemon rather than a plain application: Settings opens in its own OS
    // window, and only `daemon`'s `view` / `title` are handed the `window::Id`
    // needed to render each window differently.
    iced::daemon(LogLens::new, LogLens::update, LogLens::view)
        .title(LogLens::title)
        .theme(LogLens::theme)
        .subscription(LogLens::subscription)
        .run()
}

/// Answers `--version` / `--help` and reports whether the process should stop
/// there, before any window exists.
///
/// This is not a command-line interface and is not growing into one — anything
/// unrecognised is ignored and the app starts as normal, so a desktop launcher
/// passing its own arguments never breaks. Two flags do not justify an
/// argument-parsing dependency.
///
/// It runs ahead of `iced` deliberately: a GUI cannot be launched on a headless
/// CI runner, but a binary that prints the version it claims to be and exits
/// cleanly has proved it loaded, linked and ran on that platform. That is the
/// release workflow's smoke test over every Artifact.
fn handle_cli_flags() -> bool {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("{APP_NAME} {VERSION}");
                return true;
            }
            "--help" | "-h" => {
                println!("{APP_NAME} {VERSION} — a desktop IDE for browsing logs");
                println!("Usage: loglens [-V | --version] [-h | --help]");
                return true;
            }
            _ => {}
        }
    }

    false
}

/// The X11 `WM_CLASS` / Wayland `app_id` for every Log Lens window. Desktop
/// environments key alt-tab grouping, the taskbar label, and the `.desktop`
/// match off this, so it stays constant across windows. It is the
/// freedesktop-conventional reverse-DNS id, and the installed desktop entry
/// (`io.github.dennisdms.LogLens.desktop`) and icon
/// (`…/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png`) are named
/// after it exactly — that filename match is how GNOME binds a running window
/// to its launcher icon in the dock and alt-tab. The display name stays
/// "Log Lens"; it comes from the desktop entry's `Name=`, not from here. Only
/// `PlatformSpecific` on Linux carries the field; macOS and Windows take the
/// name from the bundle / executable instead.
#[cfg(target_os = "linux")]
const APP_ID: &str = "io.github.dennisdms.LogLens";

/// Settings shared by every Log Lens window: the app icon and (on Linux) the
/// desktop application id. Per-window fields (size, resizability) are set by
/// the callers.
fn base_window_settings() -> window::Settings {
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut platform_specific = window::settings::PlatformSpecific::default();
    #[cfg(target_os = "linux")]
    {
        platform_specific.application_id = APP_ID.to_string();
    }

    window::Settings {
        icon: icons::app_icon(),
        platform_specific,
        ..window::Settings::default()
    }
}

/// Window settings for the main Log Lens window, opened once at boot.
fn main_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1180.0, 760.0),
        ..base_window_settings()
    }
}

/// Window settings for the Settings window.
fn settings_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(460.0, 340.0),
        resizable: true,
        ..base_window_settings()
    }
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
    /// The Highlight rules modal, when open.
    rules_form: Option<RulesForm>,
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
    /// Frame counter for the Hit-count spinner, advanced while any Result Tab
    /// is still counting its total Hits.
    spinner_frame: usize,
    /// The main window's id, opened at boot. Closing it exits the app.
    main_window: window::Id,
    /// The Settings window's id while it is open.
    settings_window: Option<window::Id>,
    /// Whether the Menu bar's "File" dropdown is showing.
    file_menu_open: bool,
    /// Editable copy of `config.es`, backing the Settings window's fields.
    settings_draft: SettingsDraft,
}

/// Draft text for the Settings window's Elasticsearch page. Committed back into
/// `Config.es` (parsed and clamped) on Save.
#[derive(Debug, Clone, Default)]
struct SettingsDraft {
    max_results: String,
    fetch_size: String,
    error: Option<String>,
}

impl SettingsDraft {
    fn from_settings(es: config::EsSettings) -> Self {
        Self {
            max_results: es.max_results.to_string(),
            fetch_size: es.fetch_size.to_string(),
            error: None,
        }
    }
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
    /// The async `list_targets` for a Result Tab's suggestion dropdown landed.
    ResultTargetsLoaded {
        run_id: u64,
        result: Result<Vec<String>, String>,
    },
    /// A keystroke in the Search bar's Target input; also opens the dropdown.
    ResultTargetDraft(u64, String),
    /// Toggle the Target suggestion dropdown (the caret button next to the
    /// field).
    ResultTargetPanelToggle(u64),
    /// Dismiss the Target suggestion dropdown without committing.
    ResultTargetPanelDismiss(u64),
    /// Pick a suggestion from the Target dropdown (commits + re-runs).
    ResultTargetPicked(u64, String),
    /// Commit the Target draft (Enter): re-run against the new Target, or
    /// revert to the committed value when the draft is blank / unchanged.
    ResultTargetSubmit(u64),
    /// The `_field_caps` check for a Target the user is switching to landed.
    /// `Ok` re-points the tab and re-runs; `Err` (e.g. the index does not
    /// exist) reverts the input and reports the failure in the status bar.
    ResultTargetProbed {
        run_id: u64,
        candidate: String,
        result: Result<es::FieldCaps, String>,
    },
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
    /// Toggle the options strip's "Sort fields" popover.
    ResultSortPanel(u64),
    /// Close the "Sort fields" popover (click outside).
    ResultSortPanelDismiss(u64),
    /// Set a field's sort direction, adding it to the sort order if new.
    ResultSortSet(u64, String, bool),
    /// Drop a field from the sort order.
    ResultSortRemove(u64, String),
    /// Reorder the sort key at the given position by `delta` places.
    ResultSortMove(u64, usize, isize),
    /// Clear the whole sort order.
    ResultSortClear(u64),
    // Result tab: Layout mode + raw text template
    /// Switch a Result Tab between Table and raw text mode.
    ResultLayoutMode(u64, line::LayoutMode),
    /// A keystroke in the raw text template input.
    ResultTemplateDraft(u64, String),
    /// Commit the raw text template draft (Enter).
    ResultTemplateSubmit(u64),
    /// Open the raw-text "Format" modal for a Result Tab.
    OpenFormat(u64),
    /// Commit the template draft and close the "Format" modal.
    CloseFormat(u64),
    /// Discard the template draft and close the "Format" modal.
    FormatCancel(u64),
    // Highlight rules modal
    OpenRulesForm,
    RulesFormCancel,
    RulesFormSave,
    RulesEditRule(usize),
    RulesDeleteRule(usize),
    RulesToggleRule(usize),
    RulesMoveRule(usize, isize),
    RulesDraftName(String),
    RulesDraftKind(MatcherKind),
    RulesDraftPath(String),
    RulesDraftOp(line::Op),
    RulesDraftValue(String),
    RulesDraftPattern(String),
    RulesDraftFg(String),
    RulesDraftBg(String),
    RulesDraftCommit,
    RulesDraftReset,
    // Hit detail panel
    HitClicked(u64, usize),
    CloseHitDetail,
    DetailEdit(u64, text_editor::Action),
    DetailDragStart(u64),
    DetailDragTo(f32),
    DetailDragEnd,
    // Result tab run
    /// The async `_count` for a run's total Hits landed.
    TotalHitsLoaded {
        run_id: u64,
        generation: u64,
        result: Result<u64, String>,
    },
    /// Advance the Hit-count spinner one frame.
    SpinnerTick,
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
    /// Re-run the active Result Tab from the Search bar.
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
    // Menu bar + Settings window
    /// Toggle the Menu bar's "File" dropdown.
    FileMenuToggle,
    /// Close the "File" dropdown (click outside).
    FileMenuDismiss,
    /// Open (or focus) the Settings window.
    OpenSettings,
    SettingsMaxResults(String),
    SettingsFetchSize(String),
    /// Validate + persist the Settings draft, then close the window.
    SettingsSave,
    /// Close the Settings window without saving.
    SettingsClose,
    /// An OS window was closed. Exits the app when it is the main window.
    WindowClosed(window::Id),
    // Misc
    DismissStatus,
    /// Clear the active Result Tab's failed-Target-switch notice.
    DismissTargetError(u64),
    /// Deliberately does nothing — used to let a modal backdrop swallow scroll
    /// events so they never reach the widgets behind it.
    Ignore,
}

impl LogLens {
    fn new() -> (Self, Task<Message>) {
        let mut expanded = HashSet::new();
        expanded.insert(ES_ROOT.to_string());

        let config = config::load();
        let (main_window, open_main) = window::open(main_window_settings());

        let app = Self {
            settings_draft: SettingsDraft::from_settings(config.es),
            config,
            open_tabs: Vec::new(),
            active_tab: None,
            expanded,
            connection_form: None,
            search_settings: None,
            rules_form: None,
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
            spinner_frame: 0,
            main_window,
            settings_window: None,
            file_menu_open: false,
        };
        (app, open_main.discard())
    }

    fn theme(&self, _window: window::Id) -> Theme {
        Theme::Dark
    }

    fn title(&self, window: window::Id) -> String {
        if Some(window) == self.settings_window {
            "Settings — Log Lens".to_string()
        } else {
            "Log Lens".to_string()
        }
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

        // Closing the main window exits the app; closing Settings just clears
        // its handle (a daemon keeps running with no windows open).
        let closes = window::close_events().map(Message::WindowClosed);

        // Tick the Hit-count spinner only while a run is still counting.
        let counting = self
            .open_tabs
            .iter()
            .any(|t| matches!(t, Tab::Result(rt) if matches!(rt.total_hits, TotalHits::Loading)));
        let spinner = if counting {
            iced::time::every(std::time::Duration::from_millis(90)).map(|_| Message::SpinnerTick)
        } else {
            Subscription::none()
        };

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
            return Subscription::batch([escape, closes, spinner, drag]);
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
            return Subscription::batch([escape, closes, spinner, drag]);
        }

        Subscription::batch([escape, closes, spinner])
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
            Message::CloseTab(tab) => self.close_tab(tab),
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
                if let Some(prompt) = self.secret_prompt.take()
                    && let PendingAction::RunSearch { run_id } = prompt.then
                    && let Some(rt) = self.result_mut(run_id)
                {
                    rt.refreshing = false;
                    rt.state = RunState::Error(
                        "Connection secret required to run this search".to_string(),
                    );
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

            Message::ResultTargetsLoaded { run_id, result } => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.targets_loading = false;
                    if let Ok(mut options) = result {
                        options.sort();
                        rt.target_options = options;
                    }
                }
            }
            Message::ResultTargetDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_draft = v;
                    rt.target_panel_open = true;
                    rt.target_error = None;
                }
            }
            Message::ResultTargetPanelToggle(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_panel_open = !rt.target_panel_open;
                    if rt.target_panel_open {
                        rt.tf.open = false;
                    } else {
                        rt.target_draft = rt.target.clone();
                    }
                }
            }
            Message::ResultTargetPanelDismiss(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_panel_open = false;
                    rt.target_draft = rt.target.clone();
                }
            }
            Message::ResultTargetPicked(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_draft = v;
                }
                return self.commit_target(run_id);
            }
            Message::ResultTargetSubmit(run_id) => return self.commit_target(run_id),
            Message::ResultTargetProbed {
                run_id,
                candidate,
                result,
            } => return self.on_target_probed(run_id, candidate, result),

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
                    let resolved = self.result_mut(run_id).map(|rt| {
                        rt.all_fields = caps.all;
                        rt.sortable_fields = caps.sortable;
                        rt.resolve_template()
                    });
                    if resolved == Some(true) {
                        self.sync_saved_from_result(run_id);
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
            Message::ResultSortPanelDismiss(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.sort_panel_open = false;
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

            Message::ResultLayoutMode(run_id, mode) => {
                let changed = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let changed = rt.mode != mode;
                        rt.mode = mode;
                        rt.resolve_template();
                        changed
                    })
                    .unwrap_or(false);
                // Switching display mode never needs a new Elasticsearch query —
                // only a re-render, which happens automatically. Persist so the
                // choice survives a restart.
                if changed {
                    self.sync_saved_from_result(run_id);
                }
            }
            Message::ResultTemplateDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template_draft = v;
                }
            }
            Message::ResultTemplateSubmit(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template = rt.template_draft.trim().to_string();
                    // An emptied template falls back to the computed default.
                    rt.resolve_template();
                    rt.template_draft = rt.template.clone();
                }
                self.sync_saved_from_result(run_id);
            }
            Message::OpenFormat(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.format_open = true;
                }
            }
            Message::CloseFormat(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template = rt.template_draft.trim().to_string();
                    rt.resolve_template();
                    rt.template_draft = rt.template.clone();
                    rt.format_open = false;
                }
                self.sync_saved_from_result(run_id);
            }
            Message::FormatCancel(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template_draft = rt.template.clone();
                    rt.format_open = false;
                }
            }

            Message::OpenRulesForm => {
                self.rules_form = Some(RulesForm::new(self.config.rules.clone()));
            }
            Message::RulesFormCancel => self.rules_form = None,
            Message::RulesFormSave => {
                if let Some(form) = self.rules_form.take() {
                    self.config.rules = form.rules;
                    if let Err(err) = config::save(&self.config) {
                        self.status = Some(format!("Could not save config: {err}"));
                    }
                }
            }
            Message::RulesEditRule(i) => {
                if let Some(form) = &mut self.rules_form {
                    form.load(i);
                }
            }
            Message::RulesDeleteRule(i) => {
                if let Some(form) = &mut self.rules_form {
                    form.delete(i);
                }
            }
            Message::RulesToggleRule(i) => {
                if let Some(form) = &mut self.rules_form {
                    form.toggle(i);
                }
            }
            Message::RulesMoveRule(i, delta) => {
                if let Some(form) = &mut self.rules_form {
                    form.move_rule(i, delta);
                }
            }
            Message::RulesDraftName(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_name = v;
                }
            }
            Message::RulesDraftKind(kind) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_kind = kind;
                }
            }
            Message::RulesDraftPath(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_path = v;
                }
            }
            Message::RulesDraftOp(op) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_op = op;
                }
            }
            Message::RulesDraftValue(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_value = v;
                }
            }
            Message::RulesDraftPattern(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_pattern = v;
                }
            }
            Message::RulesDraftFg(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_fg = v;
                }
            }
            Message::RulesDraftBg(v) => {
                if let Some(form) = &mut self.rules_form {
                    form.draft_bg = v;
                }
            }
            Message::RulesDraftCommit => {
                if let Some(form) = &mut self.rules_form {
                    form.commit_draft();
                }
            }
            Message::RulesDraftReset => {
                if let Some(form) = &mut self.rules_form {
                    form.reset_draft();
                }
            }

            Message::TotalHitsLoaded {
                run_id,
                generation,
                result,
            } => {
                if let Some(rt) = self.result_mut(run_id)
                    && rt.total_generation == generation
                {
                    rt.total_hits = match result {
                        Ok(total) => TotalHits::Known(total),
                        Err(_) => TotalHits::Failed,
                    };
                }
            }
            Message::SpinnerTick => self.spinner_frame = self.spinner_frame.wrapping_add(1),
            Message::PageLoaded {
                run_id,
                result,
                append,
            } => {
                let ok = result.is_ok();
                if let Some(rt) = self.result_mut(run_id) {
                    apply_page(rt, result, append);
                }
                // A completed first Page (initial run or refresh) starts at the
                // top: the table `scrollable` stays mounted across a refresh, so
                // snap it back explicitly rather than relying on a remount.
                if !append
                    && ok
                    && let Some(rt) = self.result_mut(run_id)
                {
                    rt.scroll_y = 0.0;
                    return operation::snap_to(rt.scroll_id.clone(), RelativeOffset::START);
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
                    rt.format_open = false;
                    rt.tf.open = false;
                    if rt.target_panel_open {
                        rt.target_panel_open = false;
                        rt.target_draft = rt.target.clone();
                    }
                }
            }
            Message::DetailEdit(run_id, action) => {
                if !action.is_edit()
                    && let Some(rt) = self.result_mut(run_id)
                {
                    rt.detail_content.perform(action);
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
                    if delta != 0.0
                        && let Some(rt) = self.result_mut(run_id)
                    {
                        rt.detail_height = (rt.detail_height + delta)
                            .clamp(results::DETAIL_MIN_H, results::DETAIL_MAX_H);
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
                    if delta != 0.0
                        && let Some(rt) = self.result_mut(run_id)
                    {
                        rt.resize_column(index, delta);
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
                self.delete_search(connection, search);
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

            Message::FileMenuToggle => self.file_menu_open = !self.file_menu_open,
            Message::FileMenuDismiss => self.file_menu_open = false,
            Message::OpenSettings => {
                self.file_menu_open = false;
                if let Some(id) = self.settings_window {
                    return window::gain_focus(id);
                }
                self.settings_draft = SettingsDraft::from_settings(self.config.es);
                let (id, open) = window::open(settings_window_settings());
                self.settings_window = Some(id);
                return open.discard();
            }
            Message::SettingsMaxResults(v) => {
                self.settings_draft.max_results = v;
                self.settings_draft.error = None;
            }
            Message::SettingsFetchSize(v) => {
                self.settings_draft.fetch_size = v;
                self.settings_draft.error = None;
            }
            Message::SettingsSave => return self.save_settings(),
            Message::SettingsClose => {
                if let Some(id) = self.settings_window.take() {
                    return window::close(id);
                }
            }
            Message::WindowClosed(id) => {
                if id == self.main_window {
                    return iced::exit();
                }
                if Some(id) == self.settings_window {
                    self.settings_window = None;
                }
            }

            Message::DismissStatus => self.status = None,
            Message::DismissTargetError(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_error = None;
                }
            }
            // A scroll (or stray press) on a modal backdrop routes here so the
            // `mouse_area` in `modal_card_sized` can capture the event and keep
            // it from reaching the Result Tab behind the modal.
            Message::Ignore => {}
        }

        Task::none()
    }

    // --- Settings window ---------------------------------------------------

    /// Parses and clamps the Settings draft, writes it into `Config.es`,
    /// persists, pushes the new limits onto every open Result Tab, and closes
    /// the Settings window. A malformed field leaves everything untouched and
    /// shows an inline error.
    fn save_settings(&mut self) -> Task<Message> {
        let parse = |raw: &str, label: &str| -> Result<usize, String> {
            raw.trim()
                .replace([',', '_'], "")
                .parse::<usize>()
                .map_err(|_| format!("{label} must be a whole number"))
                .and_then(|n| {
                    if n == 0 {
                        Err(format!("{label} must be at least 1"))
                    } else {
                        Ok(n)
                    }
                })
        };

        let max_results = match parse(&self.settings_draft.max_results, "Max Results") {
            Ok(n) => n,
            Err(err) => {
                self.settings_draft.error = Some(err);
                return Task::none();
            }
        };
        let fetch_size = match parse(&self.settings_draft.fetch_size, "Fetch size") {
            Ok(n) => n,
            Err(err) => {
                self.settings_draft.error = Some(err);
                return Task::none();
            }
        };

        let es = config::EsSettings {
            max_results,
            fetch_size,
        }
        .normalized();
        self.config.es = es;
        self.settings_draft = SettingsDraft::from_settings(es);

        for tab in &mut self.open_tabs {
            if let Tab::Result(rt) = tab {
                rt.max_results = es.max_results;
                rt.fetch_size = es.fetch_size;
            }
        }

        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }

        match self.settings_window.take() {
            Some(id) => window::close(id),
            None => Task::none(),
        }
    }

    // --- Tabs ----------------------------------------------------------

    fn close_tab(&mut self, tab: usize) {
        if tab >= self.open_tabs.len() {
            return;
        }

        self.open_tabs.remove(tab);
        self.active_tab = match self.active_tab {
            _ if self.open_tabs.is_empty() => None,
            Some(active) if active > tab => Some(active - 1),
            Some(active) if active == tab => Some(tab.min(self.open_tabs.len() - 1)),
            other => other,
        };
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
        let mut form = SearchForm::from_saved(form_id, conn_id.clone(), &saved);
        // The edit modal has no Target field, so it never needs the index list
        // or a `_field_caps` prewarm.
        form.targets_loading = false;
        self.search_settings = Some(form);
        Task::none()
    }

    fn delete_search(&mut self, conn_id: String, search_id: String) {
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
            self.close_tab(pos);
        }

        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id) {
            conn.searches.retain(|s| s.id != search_id);
        }
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
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
        self.close_connection_tabs(&conn_id);

        self.config.connections.retain(|c| c.id != conn_id);
        secrets::delete(&conn_id);
        self.expanded.remove(&conn_id);
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
        Task::none()
    }

    /// Closes every tab — Result or Search form — belonging to a Connection.
    fn close_connection_tabs(&mut self, conn_id: &str) {
        while let Some(pos) = self.open_tabs.iter().position(|t| match t {
            Tab::Result(rt) => rt.connection_id == conn_id,
            Tab::SearchForm(f) => f.connection_id == conn_id,
        }) {
            self.close_tab(pos);
        }
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

    /// Writes a Result Tab's live Target / query string / timeframe / Column /
    /// sort choices back onto its Saved Search and persists the config.
    fn sync_saved_from_result(&mut self, run_id: u64) {
        let Some((
            conn_id,
            saved_id,
            target,
            query_string,
            timeframe,
            columns,
            sort,
            mode,
            template,
        )) = self.result_mut(run_id).map(|rt| {
            (
                rt.connection_id.clone(),
                rt.saved_id.clone(),
                rt.target.clone(),
                rt.query_string.clone(),
                rt.timeframe.clone(),
                rt.columns.clone(),
                rt.sort.clone(),
                rt.mode,
                rt.template.clone(),
            )
        })
        else {
            return;
        };
        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id)
            && let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id)
        {
            saved.target = target;
            saved.query_string = query_string;
            saved.timeframe = timeframe;
            saved.columns = columns;
            saved.sort = sort;
            saved.mode = mode;
            saved.template = template;
        }
        if let Err(err) = config::save(&self.config) {
            self.status = Some(format!("Could not save config: {err}"));
        }
    }

    /// Starts committing the Search bar's Target draft. A blank or unchanged
    /// draft just closes the dropdown (reverting a blank one). A real change is
    /// only *probed* here — a `_field_caps` call against the candidate — so the
    /// current results are left untouched until it comes back. The re-point and
    /// re-run happen in [`Self::on_target_probed`] on success; a failure (e.g.
    /// the index does not exist) surfaces in the status bar instead.
    fn commit_target(&mut self, run_id: u64) -> Task<Message> {
        let Some(rt) = self.result_mut(run_id) else {
            return Task::none();
        };
        rt.target_panel_open = false;
        let draft = rt.target_draft.trim().to_string();
        if draft.is_empty() || draft == rt.target {
            rt.target_draft = rt.target.clone();
            rt.target_probe = None;
            rt.target_error = None;
            return Task::none();
        }
        rt.target_draft = draft.clone();
        rt.target_probe = Some(draft.clone());
        let conn_id = rt.connection_id.clone();

        match self.connection(&conn_id).and_then(|c| self.endpoint_for(c)) {
            Some(endpoint) => {
                let candidate = draft.clone();
                Task::perform(es::field_caps(endpoint, draft), move |result| {
                    Message::ResultTargetProbed {
                        run_id,
                        candidate,
                        result,
                    }
                })
            }
            None => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_probe = None;
                    rt.target_draft = rt.target.clone();
                }
                Task::none()
            }
        }
    }

    /// Handles the `_field_caps` probe for a Target the user is switching to.
    /// On success the tab re-points to the candidate (carrying the fresh field
    /// list), persists, and re-runs. On failure the input reverts and the
    /// error goes to the info bar (`target_error`), leaving the current
    /// results in place.
    fn on_target_probed(
        &mut self,
        run_id: u64,
        candidate: String,
        result: Result<es::FieldCaps, String>,
    ) -> Task<Message> {
        let Some(rt) = self.result_mut(run_id) else {
            return Task::none();
        };
        // A newer probe (or a plain re-open) has superseded this one.
        if rt.target_probe.as_deref() != Some(candidate.as_str()) {
            return Task::none();
        }
        rt.target_probe = None;

        let caps = match result {
            Ok(caps) => caps,
            Err(err) => {
                rt.target_draft = rt.target.clone();
                rt.target_error = Some(format!("Target \u{201c}{candidate}\u{201d}: {err}"));
                return Task::none();
            }
        };

        rt.target = candidate.clone();
        rt.target_draft = candidate;
        rt.target_error = None;
        rt.all_fields = caps.all;
        rt.sortable_fields = caps.sortable;
        rt.resolve_template();
        self.sync_saved_from_result(run_id);
        self.start_run(run_id)
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
        let timestamp_field = form.resolved_timestamp_field();

        // The modal edits Name and Timestamp field only; the Target is
        // re-pointed from the Search bar.
        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id)
            && let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id)
        {
            saved.name = name;
            saved.timestamp_field = timestamp_field;
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
            self.open_result_tab(conn_id, saved_id, None, None, true)
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
            if let Some(form_idx) = replace
                && form_idx != existing
            {
                self.open_tabs.remove(form_idx);
            }
            let existing = self
                .open_tabs
                .iter()
                .position(|t| matches!(t, Tab::Result(rt) if rt.saved_id == saved_id));
            self.active_tab = existing;

            // A saved edit refreshes the open Result Tab's parameters and
            // re-runs it; a plain re-open just focuses it.
            if rerun_existing
                && let (Some(idx), Some(saved)) = (
                    existing,
                    self.connection(&conn_id)
                        .and_then(|c| c.searches.iter().find(|s| s.id == saved_id))
                        .cloned(),
                )
            {
                let (gte, lte) = saved.timeframe.bounds();
                let target = saved.target.clone();
                let run_id = match self.open_tabs.get_mut(idx) {
                    Some(Tab::Result(rt)) => {
                        let target_changed = rt.target != saved.target;
                        rt.saved_name = saved.name.clone();
                        rt.target = saved.target.clone();
                        rt.target_draft = saved.target.clone();
                        rt.target_probe = None;
                        rt.target_error = None;
                        rt.target_panel_open = false;
                        rt.query_string = saved.query_string.clone();
                        rt.query_draft = saved.query_string.clone();
                        rt.timestamp_field = saved.timestamp_field.clone();
                        rt.columns = saved.columns.clone();
                        rt.sort = saved.sort.clone();
                        rt.mode = saved.mode;
                        rt.template = saved.template.clone();
                        rt.template_draft = saved.template.clone();
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
        let mut tab = ResultTab {
            run_id,
            connection_id: conn_id.clone(),
            saved_id,
            saved_name: saved.name.clone(),
            target: saved.target.clone(),
            target_draft: saved.target.clone(),
            target_probe: None,
            target_error: None,
            target_options: Vec::new(),
            targets_loading: true,
            target_panel_open: false,
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
            hits: Vec::new(),
            state: RunState::Loading,
            refreshing: false,
            scroll_id: Id::unique(),
            paging: Paging::Idle,
            total_hits: TotalHits::Loading,
            total_generation: 0,
            scroll_y: 0.0,
            viewport_h: 600.0,
            selected_hit: None,
            detail_content: text_editor::Content::new(),
            detail_height: results::DETAIL_DEFAULT_H,
            utc: self.config.utc_timestamps,
            max_results: self.config.es.max_results,
            fetch_size: self.config.es.fetch_size,
            mode: saved.mode,
            template: saved.template.clone(),
            template_draft: saved.template.clone(),
            format_open: false,
        };
        // Resolve the raw text template up front if the field list is already
        // known; otherwise it is resolved lazily when `ResultFieldsLoaded`
        // lands (see that handler).
        tab.resolve_template();
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

        let endpoint = self.connection(&conn_id).and_then(|c| self.endpoint_for(c));

        let fetch_fields: Task<Message> = match (&endpoint, need_fields) {
            (Some(endpoint), true) => {
                Task::perform(es::field_caps(endpoint.clone(), target), move |result| {
                    Message::ResultFieldsLoaded { run_id, result }
                })
            }
            _ => Task::none(),
        };

        // Populate the Search bar's Target suggestion dropdown for this tab.
        let fetch_targets: Task<Message> = match &endpoint {
            Some(endpoint) => Task::perform(es::list_targets(endpoint.clone()), move |result| {
                Message::ResultTargetsLoaded { run_id, result }
            }),
            None => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.targets_loading = false;
                }
                Task::none()
            }
        };

        Task::batch([fetch_fields, fetch_targets, self.start_run(run_id)])
    }

    /// Freshens the range, then fetches the first Page (and the `_count`).
    fn start_run(&mut self, run_id: u64) -> Task<Message> {
        let Some((conn_id, target, generation, count_params, search_params)) =
            self.result_mut(run_id).map(|rt| {
                // If this tab already had a table up, keep the strips pinned and
                // the previous rows on screen while the re-run is in flight, so
                // nothing flickers. The old Hits are swapped out wholesale when
                // the new first Page lands (see `PageLoaded`).
                rt.refreshing = matches!(rt.state, RunState::Loaded | RunState::Empty);
                rt.state = RunState::Loading;
                rt.selected_hit = None;
                if !rt.refreshing {
                    rt.hits.clear();
                    rt.paging = Paging::Idle;
                    rt.scroll_y = 0.0;
                }
                // Re-resolve the range so a relative window re-anchors to "now".
                let (gte, lte) = rt.timeframe.bounds();
                rt.gte = gte;
                rt.lte = lte;
                rt.total_hits = TotalHits::Loading;
                rt.total_generation += 1;
                let count_params = es::CountParams {
                    query_string: rt.query_string.clone(),
                    timestamp_field: rt.timestamp_field.clone(),
                    gte: rt.gte.clone(),
                    lte: rt.lte.clone(),
                };
                let search_params = es::SearchParams {
                    query_string: rt.query_string.clone(),
                    timestamp_field: rt.timestamp_field.clone(),
                    gte: rt.gte.clone(),
                    lte: rt.lte.clone(),
                    sort: rt.effective_sort(),
                    size: rt.fetch_size.min(rt.max_results),
                    search_after: None,
                };
                (
                    rt.connection_id.clone(),
                    rt.target.clone(),
                    rt.total_generation,
                    count_params,
                    search_params,
                )
            })
        else {
            return Task::none();
        };

        let Some(conn) = self.connection(&conn_id) else {
            return Task::none();
        };

        match self.endpoint_for(conn) {
            Some(endpoint) => Task::batch([
                Task::perform(
                    es::count(endpoint.clone(), target.clone(), count_params),
                    move |result| Message::TotalHitsLoaded {
                        run_id,
                        generation,
                        result,
                    },
                ),
                Task::perform(es::search(endpoint, target, search_params), move |result| {
                    Message::PageLoaded {
                        run_id,
                        result,
                        append: false,
                    }
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

    /// Fetches the next Page for a Result Tab via `search_after`.
    /// A no-op unless the tab is idle, under the cap, and has a cursor.
    fn load_more(&mut self, run_id: u64) -> Task<Message> {
        let Some((conn_id, target, params)) = self.result_mut(run_id).and_then(|rt| {
            let cursor = rt.next_cursor()?;
            let remaining = rt.max_results.saturating_sub(rt.hits.len());
            if remaining == 0 {
                rt.paging = Paging::Capped;
                return None;
            }
            rt.paging = Paging::Loading;
            Some((
                rt.connection_id.clone(),
                rt.target.clone(),
                es::SearchParams {
                    query_string: rt.query_string.clone(),
                    timestamp_field: rt.timestamp_field.clone(),
                    gte: rt.gte.clone(),
                    lte: rt.lte.clone(),
                    sort: rt.effective_sort(),
                    size: remaining.min(rt.fetch_size),
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
        Task::perform(es::search(endpoint, target, params), move |result| {
            Message::PageLoaded {
                run_id,
                result,
                append: true,
            }
        })
    }

    // --- View --------------------------------------------------------------

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if Some(window) == self.settings_window {
            return self.settings_view();
        }
        self.main_view()
    }

    fn main_view(&self) -> Element<'_, Message> {
        // Right column, top to bottom: an optional Search bar, then an optional
        // options strip, then the tab strip sitting directly above the main
        // area. The two optional strips only appear while a Result Tab is
        // active.
        let mut right: Vec<Element<'_, Message>> = Vec::new();
        if let Some(search_bar) = self.search_bar() {
            right.push(search_bar);
            right.push(rule::horizontal(1.0).into());
        }
        if let Some(options_bar) = self.options_bar() {
            right.push(options_bar);
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
            rule::horizontal(1.0),
            self.info_bar(),
        ])
        .style(|_| style::panel(BG))
        .width(Fill)
        .height(Fill)
        .into();

        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if let Some(menu) = self.file_menu_overlay() {
            layers.push(menu);
        }
        if let Some(menu) = self.tree_menu_overlay() {
            layers.push(menu);
        }
        if let Some(popover) = self.sort_fields_popover_overlay() {
            layers.push(popover);
        }
        if let Some(popover) = self.timeframe_popover_overlay() {
            layers.push(popover);
        }
        if let Some(dropdown) = self.target_suggestions_overlay() {
            layers.push(dropdown);
        }
        if let Some(form) = &self.connection_form {
            layers.push(self.connection_form_modal(form));
        }
        if let Some(form) = &self.search_settings {
            layers.push(self.search_settings_modal(form));
        }
        if let Some(form) = &self.rules_form {
            layers.push(self.rules_form_modal(form));
        }
        if let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t))
            && tab.format_open
            && tab.mode == line::LayoutMode::RawText
        {
            layers.push(self.format_modal(tab));
        }
        if let Some(prompt) = &self.secret_prompt {
            layers.push(self.secret_prompt_modal(prompt));
        }
        if let Some(confirm) = &self.confirm {
            layers.push(self.confirm_modal(confirm));
        }

        // Always wrap in a `stack`, even with no overlays: collapsing to the
        // bare `base` when the last overlay closes (or expanding away from it
        // when the first opens) swaps the root widget's type, which discards
        // the whole widget tree's state — including `text_input` focus. The
        // Target dropdown opens *while* the user is typing in the Search bar,
        // so that reset would drop focus after every keystroke.
        stack(layers).width(Fill).height(Fill).into()
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

    /// A persistent info bar across the very bottom of the window, carrying
    /// summary details for the active tab: the loaded-Hit count for a Result
    /// Tab on the left, and a failed Target switch (a red outlined pill) on
    /// the right.
    fn info_bar(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        if let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) {
            items.push(self.hit_count_readout(tab));
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
    fn hit_count_readout<'a>(&self, tab: &'a ResultTab) -> Element<'a, Message> {
        let loaded = thousands(tab.hits.len() as u64);
        match tab.total_hits {
            TotalHits::Loading => row![
                meta(&format!("Loaded {loaded} of")),
                text(spinner_frame(self.spinner_frame))
                    .size(12.0)
                    .color(TEXT_DIM),
                meta("hits"),
            ]
            .spacing(5.0)
            .align_y(iced::Alignment::Center)
            .into(),
            TotalHits::Known(total) => {
                meta(&format!("Loaded {loaded} of {} hits", thousands(total)))
            }
            TotalHits::Failed => meta(&format!("Loaded {loaded} hits")),
        }
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

        let x = self.tree_menu_at.x.clamp(2.0, 240.0 - 136.0);
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

    /// The always-present Menu bar across the top of the window. `File` opens a
    /// dropdown (see [`Self::file_menu_overlay`]); `View` is still inert.
    fn menu_bar(&self) -> Element<'_, Message> {
        container(
            row![
                button(text("File").size(13.0).color(TEXT))
                    .on_press(Message::FileMenuToggle)
                    .padding(Padding::new(2.0).left(6.0).right(6.0))
                    .style(style::picker_row(self.file_menu_open)),
                text("View").size(13.0).color(TEXT_DIM),
            ]
            .spacing(12.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL_ALT))
        .width(Fill)
        .padding(Padding::new(4.0).left(8.0).right(12.0))
        .into()
    }

    /// The floating "File" dropdown, anchored under its Menu bar label.
    fn file_menu_overlay(&self) -> Option<Element<'_, Message>> {
        if !self.file_menu_open {
            return None;
        }
        let block = container(
            button(text("Settings").size(12.0).color(TEXT))
                .on_press(Message::OpenSettings)
                .width(Fill)
                .padding(Padding::new(4.0).left(10.0).right(10.0))
                .style(style::picker_row(false)),
        )
        .width(150.0)
        .padding(3.0)
        .style(|_| style::menu_popup());

        let anchored = container(block)
            .width(Fill)
            .height(Fill)
            .padding(Padding::new(0.0).left(8.0).top(26.0));

        Some(
            mouse_area(anchored)
                .on_press(Message::FileMenuDismiss)
                .on_right_press(Message::FileMenuDismiss)
                .into(),
        )
    }

    /// The Settings window body: a single Elasticsearch page with the two fetch
    /// limits. Rendered whenever `view` is asked for the Settings window.
    fn settings_view(&self) -> Element<'_, Message> {
        let draft = &self.settings_draft;

        let max_results = column![
            field_label("Max Results"),
            text("Stop fetching once a tab has loaded this many documents.")
                .size(11.0)
                .color(TEXT_DIM),
            text_input("", &draft.max_results)
                .on_input(Message::SettingsMaxResults)
                .on_submit(Message::SettingsSave)
                .padding(6.0)
                .width(Length::Fixed(140.0)),
        ]
        .spacing(4.0);

        let fetch_size = column![
            field_label("Fetch size"),
            text("Documents per request while paging (max 10,000).")
                .size(11.0)
                .color(TEXT_DIM),
            text_input("", &draft.fetch_size)
                .on_input(Message::SettingsFetchSize)
                .on_submit(Message::SettingsSave)
                .padding(6.0)
                .width(Length::Fixed(140.0)),
        ]
        .spacing(4.0);

        let mut col = column![
            text("Elasticsearch").size(16.0).color(TEXT),
            text(
                "How many log documents Log Lens pulls from a cluster, and in \
                 what size batches."
            )
            .size(11.0)
            .color(TEXT_DIM),
            space().height(4.0),
            max_results,
            fetch_size,
        ]
        .spacing(10.0);

        if let Some(err) = &draft.error {
            col = col.push(text(err.clone()).size(12.0).color(ERR_RED));
        }

        col = col.push(space().height(6.0));
        col = col.push(
            row![
                button(text("Save").size(13.0).color(TEXT))
                    .on_press(Message::SettingsSave)
                    .padding(Padding::new(6.0).left(16.0).right(16.0))
                    .style(style::picker_row(true)),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::SettingsClose)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
            ]
            .spacing(8.0),
        );

        container(scrollable(col).height(Fill))
            .style(|_| style::panel(BG))
            .width(Fill)
            .height(Fill)
            .padding(20.0)
            .into()
    }

    /// The options strip shown below the Search bar while a Result Tab is
    /// active, directly above the tab strip: the live Column + sort controls
    /// moved out of the Result Tab. Hidden for Search form tabs and when no tab
    /// is open.
    fn options_bar(&self) -> Option<Element<'_, Message>> {
        let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) else {
            return None;
        };
        if !tab.strips_visible() {
            return None;
        }

        // The "Sort fields" popover is *not* pushed inline here — it floats as a
        // stack layer (`sort_fields_popover_overlay`) so opening it never reflows
        // the strips or table below.
        Some(self.result_sort_bar(tab))
    }

    /// The Search bar shown at the top of the right column, above the options
    /// strip and tab strip, while a Result Tab is active: the index/datastream
    /// target, query string, timeframe and a Refresh control, in that order.
    /// The loaded-Hit count lives in the bottom info bar. Hidden for Search
    /// form tabs and when no tab is open.
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

        // The Target is edited inline here, Kibana-style: typing opens a
        // suggestion dropdown (`target_suggestions_overlay`) and the caret
        // button toggles it; a pick or Enter re-points the tab. Free text
        // (patterns like `logs-*`) commits on Enter without appearing in the
        // list.
        let target_ctl = row![
            text_input("index or data stream", &tab.target_draft)
                .on_input(move |v| Message::ResultTargetDraft(run_id, v))
                .on_submit(Message::ResultTargetSubmit(run_id))
                .size(12.0)
                .padding(4.0)
                .width(Length::Fixed(160.0)),
            button(text("\u{25be}").size(9.0).color(TEXT_DIM))
                .on_press(Message::ResultTargetPanelToggle(run_id))
                .padding(Padding::new(4.0).left(6.0).right(6.0))
                .style(style::picker_row(tab.target_panel_open)),
        ]
        .spacing(2.0)
        .align_y(iced::Alignment::Center);

        let row1 = container(
            row![
                target_ctl,
                text_input("Lucene Query", &tab.query_draft)
                    .on_input(move |v| Message::ResultQueryDraft(run_id, v))
                    .on_submit(Message::ResultQuerySubmit(run_id))
                    .size(12.0)
                    .padding(4.0)
                    .width(Fill),
                timeframe_ctl,
                button(
                    svg(Handle::clone(&icons::REFRESH))
                        .width(Length::Fixed(16.0))
                        .height(Length::Fixed(16.0))
                        .style(|_theme, _status| svg::Style { color: Some(TEXT) }),
                )
                .on_press(Message::RefreshResult(run_id))
                .padding(Padding::new(5.0).left(9.0).right(9.0))
                .style(style::icon_button(false)),
            ]
            .spacing(12.0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL))
        .width(Fill)
        .padding(Padding::new(6.0).left(12.0).right(12.0));

        // The raw-text template is edited in the "Format" modal (opened from the
        // options strip), not here — the Search bar stays a single row.
        Some(row1.into())
    }

    /// The floating "Custom\u{2026}" timeframe editor, anchored under the Search
    /// bar's timeframe control as a stack layer so it never reflows the strips
    /// or main area below it (the options strip sits below the Search bar now).
    /// Mirrors the sidebar right-click menu: a click anywhere outside dismisses
    /// it.
    fn timeframe_popover_overlay(&self) -> Option<Element<'_, Message>> {
        let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) else {
            return None;
        };
        if !tab.tf.open {
            return None;
        }
        let run_id = tab.run_id;

        const CARD_W: f32 = 480.0;
        // Distance from the top of the window down to just below the Search bar
        // row, matching the right column's layout in `view`: the Menu bar, then
        // the Search bar row (the options strip sits below it). Each figure
        // includes its trailing 1px rule.
        let top = 25.0 + 40.0;

        let card = container(self.timeframe_popover(tab)).width(Length::Fixed(CARD_W));
        let anchored = container(column![
            space().height(top),
            row![space().width(Fill), card, space().width(12.0)],
        ])
        .width(Fill)
        .height(Fill);

        Some(
            mouse_area(anchored)
                .on_press(Message::ResultTfCancel(run_id))
                .into(),
        )
    }

    /// The Search bar's Target suggestion dropdown, floated as a stack layer
    /// under the Target input so it never reflows the strips or table below.
    /// Anchored with the same top offset as the timeframe popover; a click
    /// anywhere outside dismisses it.
    fn target_suggestions_overlay(&self) -> Option<Element<'_, Message>> {
        let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) else {
            return None;
        };
        if !tab.target_panel_open {
            return None;
        }
        let run_id = tab.run_id;

        const CARD_W: f32 = 240.0;
        // Matches `timeframe_popover_overlay`: Menu bar, then the Search bar row.
        let top = 25.0 + 40.0;

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
        // Left edge of the Target input: sidebar (240) + its rule (1) + the
        // Search bar row's left padding (12).
        let anchored = container(column![
            space().height(top),
            row![space().width(253.0), card, space().width(Fill)],
        ])
        .width(Fill)
        .height(Fill);

        Some(
            mouse_area(anchored)
                .on_press(Message::ResultTargetPanelDismiss(run_id))
                .into(),
        )
    }

    /// The "Custom\u{2026}" timeframe popover card: a relative or absolute window
    /// editor, applied or dismissed. Floated by [`Self::timeframe_popover_overlay`].
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

    /// The structural fields shared by the new-Saved-Search form and the Search
    /// settings modal: name, timestamp field, and — only when `include_target`
    /// — the Target (with typeahead). The edit modal omits the Target; it is
    /// re-pointed from the Search bar instead.
    fn search_settings_fields<'a>(
        &'a self,
        form: &'a SearchForm,
        include_target: bool,
    ) -> Vec<Element<'a, Message>> {
        let mut fields: Vec<Element<'a, Message>> = vec![
            field_label("Name"),
            text_input("checkout-errors", &form.name)
                .on_input(Message::SearchName)
                .padding(6.0)
                .into(),
        ];

        if include_target {
            fields.push(field_label("Target — index, data stream, or pattern"));
            fields.push(
                text_input("logs-*", &form.target)
                    .on_input(Message::SearchTargetInput)
                    .padding(6.0)
                    .into(),
            );
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

        for field in self.search_settings_fields(form, true) {
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

        for field in self.search_settings_fields(form, false) {
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

    /// The Highlight rules modal: a reorderable list of rules over a sub-form
    /// for adding or editing one. Save writes the working copy onto
    /// `Config.rules`; Cancel discards it.
    fn rules_form_modal<'a>(&'a self, form: &'a RulesForm) -> Element<'a, Message> {
        let last = form.rules.len().saturating_sub(1);

        let mut list = column![].spacing(4.0);
        if form.rules.is_empty() {
            list = list.push(text("No rules yet.").size(11.0).color(TEXT_DIM));
        }
        for (i, rule) in form.rules.iter().enumerate() {
            let summary = match &rule.matcher {
                line::Matcher::Field { path, op, value } => format!("{path} {op} {value}"),
                line::Matcher::Text { pattern } => format!("text \u{201c}{pattern}\u{201d}"),
            };
            let mut up = button(text("\u{25b4}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i > 0 {
                up = up.on_press(Message::RulesMoveRule(i, -1));
            }
            let mut down = button(text("\u{25be}").size(10.0).color(TEXT_DIM))
                .padding(1.0)
                .style(style::bare_button());
            if i < last {
                down = down.on_press(Message::RulesMoveRule(i, 1));
            }
            list = list.push(
                row![
                    checkbox(rule.enabled)
                        .on_toggle(move |_| Message::RulesToggleRule(i))
                        .size(13.0),
                    column![
                        text(rule.name.clone()).size(12.0).color(TEXT),
                        text(summary).size(10.0).color(TEXT_DIM),
                    ]
                    .spacing(1.0),
                    space().width(Fill),
                    swatch(rule.style.fg),
                    swatch(rule.style.bg),
                    button(text("Edit").size(11.0).color(ACCENT))
                        .on_press(Message::RulesEditRule(i))
                        .padding(2.0)
                        .style(style::bare_button()),
                    button(text("\u{00d7}").size(12.0).color(TEXT_DIM))
                        .on_press(Message::RulesDeleteRule(i))
                        .padding(2.0)
                        .style(style::bare_button()),
                    column![up, down].spacing(0.0),
                ]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
            );
        }

        let kind_btn = |label: &'static str, kind: MatcherKind| {
            button(text(label).size(11.0).color(TEXT))
                .on_press(Message::RulesDraftKind(kind))
                .padding(Padding::new(3.0).left(10.0).right(10.0))
                .style(style::picker_row(form.draft_kind == kind))
        };

        let matcher_fields: Element<'a, Message> = match form.draft_kind {
            MatcherKind::Field => row![
                text_input("field.path", &form.draft_path)
                    .on_input(Message::RulesDraftPath)
                    .size(12.0)
                    .padding(4.0)
                    .width(Fill),
                pick_list(
                    &line::Op::ALL[..],
                    Some(form.draft_op),
                    Message::RulesDraftOp
                )
                .text_size(12.0)
                .padding(4.0),
                text_input("value", &form.draft_value)
                    .on_input(Message::RulesDraftValue)
                    .size(12.0)
                    .padding(4.0)
                    .width(Length::Fixed(120.0)),
            ]
            .spacing(6.0)
            .align_y(iced::Alignment::Center)
            .into(),
            MatcherKind::Text => text_input("substring to highlight", &form.draft_pattern)
                .on_input(Message::RulesDraftPattern)
                .size(12.0)
                .padding(4.0)
                .width(Fill)
                .into(),
        };

        let colour_field = |title: &'static str, val: &'a str, msg: fn(String) -> Message| {
            column![
                text(title).size(10.0).color(TEXT_DIM),
                row![
                    text_input("#rrggbb", val)
                        .on_input(msg)
                        .size(12.0)
                        .padding(4.0)
                        .width(Length::Fixed(110.0)),
                    swatch(line::parse_hex(val.trim())),
                ]
                .spacing(6.0)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(2.0)
        };

        let sub_form = column![
            text(if form.editing.is_some() {
                "Edit rule"
            } else {
                "Add rule"
            })
            .size(12.0)
            .color(TEXT_DIM),
            text_input("Rule name", &form.draft_name)
                .on_input(Message::RulesDraftName)
                .size(12.0)
                .padding(4.0)
                .width(Fill),
            row![
                kind_btn("Field", MatcherKind::Field),
                kind_btn("Text", MatcherKind::Text),
            ]
            .spacing(1.0),
            matcher_fields,
            row![
                colour_field("Foreground", &form.draft_fg, Message::RulesDraftFg),
                colour_field("Background", &form.draft_bg, Message::RulesDraftBg),
            ]
            .spacing(12.0),
            row![
                button(
                    text(if form.editing.is_some() {
                        "Update"
                    } else {
                        "Add"
                    })
                    .size(12.0)
                    .color(TEXT)
                )
                .on_press(Message::RulesDraftCommit)
                .padding(Padding::new(4.0).left(12.0).right(12.0))
                .style(style::picker_row(true)),
                button(text("Clear").size(12.0).color(TEXT_DIM))
                    .on_press(Message::RulesDraftReset)
                    .padding(4.0)
                    .style(style::bare_button()),
            ]
            .spacing(8.0),
        ]
        .spacing(6.0);

        let mut card = column![
            text("Highlight rules").size(16.0).color(TEXT),
            text(
                "Applied to every Result Tab, in order. The first matching field \
                 rule colours the whole line; text rules layer on top."
            )
            .size(11.0)
            .color(TEXT_DIM),
            space().height(2.0),
            list,
            rule::horizontal(1.0),
            sub_form,
        ]
        .spacing(8.0)
        .width(Fill);

        if let Some(err) = &form.error {
            card = card.push(text(err.clone()).size(12.0).color(ERR_RED));
        }
        card = card.push(space().height(4.0));
        card = card.push(
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::RulesFormCancel)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text("Save").size(13.0).color(TEXT))
                    .on_press(Message::RulesFormSave)
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0),
        );

        modal_card(card.into())
    }

    /// The raw-text "Format" modal. Three sections, top to bottom: the fields
    /// available in the current search (reference only), the `%{...}` template
    /// itself, and a live preview of the first Hits that re-renders as the
    /// template is edited. Reached from the options strip's "Format" button
    /// while a Result Tab is in Text mode.
    fn format_modal<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;

        let mut card = column![
            text("Format").size(16.0).color(TEXT),
            text(
                "Template for Text mode. \u{201c}%{field.path}\u{201d} inserts a \
                 field; \u{201c}%{_source}\u{201d} inserts the whole document as \
                 JSON."
            )
            .size(11.0)
            .color(TEXT_DIM),
        ]
        .spacing(8.0)
        .width(Fill);

        // 1. Fields available in this search — reference only, no click behaviour.
        card = card.push(rule::horizontal(1.0));
        card = card.push(text("Fields").size(12.0).color(TEXT_DIM));
        let fields: Element<'_, Message> = if tab.all_fields.is_empty() {
            text("Field list not loaded yet.")
                .size(11.0)
                .color(TEXT_DIM)
                .into()
        } else {
            let mut list = column![].spacing(1.0);
            for f in &tab.all_fields {
                list = list.push(
                    text(f.clone())
                        .size(11.0)
                        .font(Font::MONOSPACE)
                        .color(TEXT_DIM),
                );
            }
            list = list.push(
                text("_source \u{2014} the whole document as JSON")
                    .size(11.0)
                    .color(TEXT_DIM),
            );
            scrollable(list)
                .height(Length::Fixed(140.0))
                .width(Fill)
                .into()
        };
        card = card.push(fields);

        // 2. Format — the template string, plus a non-blocking unknown-field warning.
        card = card.push(rule::horizontal(1.0));
        card = card.push(text("Format").size(12.0).color(TEXT_DIM));
        card = card.push(
            text_input("%{field.path} template", &tab.template_draft)
                .on_input(move |v| Message::ResultTemplateDraft(run_id, v))
                .on_submit(Message::ResultTemplateSubmit(run_id))
                .size(12.0)
                .padding(4.0)
                .font(Font::MONOSPACE)
                .width(Fill),
        );
        let unknown = line::unknown_template_fields(&tab.template_draft, &tab.all_fields);
        if !unknown.is_empty() {
            let list = unknown
                .iter()
                .map(|f| format!("\u{201c}{f}\u{201d}"))
                .collect::<Vec<_>>()
                .join(", ");
            card = card.push(
                text(format!("Unknown field: {list}"))
                    .size(11.0)
                    .color(WARN_AMBER),
            );
        }

        // 3. Display — a live preview of the first Hits under the current draft.
        card = card.push(rule::horizontal(1.0));
        card = card.push(text("Display").size(12.0).color(TEXT_DIM));
        let template = {
            let draft = tab.template_draft.trim();
            if draft.is_empty() {
                tab.template.clone()
            } else {
                draft.to_string()
            }
        };
        let preview_layout = line::Layout {
            mode: line::LayoutMode::RawText,
            columns: Vec::new(),
            template,
            timestamp_field: tab.timestamp_field.clone(),
            utc: tab.utc,
        };
        let prepared = line::Prepared::from_rules(&self.config.rules);
        let preview: Element<'_, Message> = if tab.hits.is_empty() {
            text("Run a search to preview.")
                .size(11.0)
                .color(TEXT_DIM)
                .into()
        } else {
            let mut lines = column![];
            for hit in tab.hits.iter().take(10) {
                let rendered = line::render(hit, &preview_layout, &prepared);
                let content: Element<'_, Message> = match rendered.parts.first() {
                    Some(part) => part_widget(part),
                    None => text("").size(12.0).font(Font::MONOSPACE).into(),
                };
                lines = lines.push(container(content).height(Length::Fixed(results::ROW_H)));
            }
            scrollable(lines)
                .width(Fill)
                .height(Length::Fixed(results::ROW_H * 10.0 + 6.0))
                .direction(scrollable::Direction::Both {
                    vertical: scrollable::Scrollbar::default(),
                    horizontal: scrollable::Scrollbar::default(),
                })
                .into()
        };
        card = card.push(
            container(preview)
                .width(Fill)
                .padding(6.0)
                .style(|_| style::panel(BG)),
        );

        card = card.push(space().height(4.0));
        card = card.push(
            row![
                space().width(Fill),
                button(text("Cancel").size(13.0).color(TEXT_DIM))
                    .on_press(Message::FormatCancel(run_id))
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::bare_button()),
                button(text("Done").size(13.0).color(TEXT))
                    .on_press(Message::CloseFormat(run_id))
                    .padding(Padding::new(6.0).left(14.0).right(14.0))
                    .style(style::picker_row(true)),
            ]
            .spacing(8.0),
        );

        modal_card_sized(card.into(), 640.0)
    }

    // --- Result tab view ---------------------------------------------

    fn result_view<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let hits_view = |this: &'a Self| -> Element<'a, Message> {
            match tab.mode {
                line::LayoutMode::Table => this.hit_table(tab),
                line::LayoutMode::RawText => this.raw_text_view(tab),
            }
        };
        let body: Element<'_, Message> = match &tab.state {
            // A refresh over an already-populated tab keeps the old table on
            // screen until the new first Page lands, so nothing flashes.
            _ if tab.table_visible() => hits_view(self),
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
            RunState::Loaded => hits_view(self),
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

    /// The live options strip above a Result Tab's table, left-aligned:
    /// "Sort fields", "Highlight rules", then the Layout options group — the
    /// Table/Text mode toggle, joined by "Format" while in Text mode, wrapped
    /// in one bordered surface so they read as a unit. (Column add / remove /
    /// reorder live in each header's "\u{22ee}" menu.)
    fn result_sort_bar<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;

        // The icon sits in a box as tall as the sort button's size-14 digit, so
        // every button in this strip comes out the same height.
        let icon_box = |handle: &Handle, color: Color| {
            container(
                svg(Handle::clone(handle))
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .style(move |_theme, _status| svg::Style { color: Some(color) }),
            )
            .center_y(Length::Fixed(18.0))
        };

        // A dropdown-style tooltip bubble shared by every button in the strip.
        let tip = |label: &'a str| {
            container(text(label).size(11.0).color(TEXT))
                .padding(Padding::new(4.0).left(6.0).right(6.0))
                .style(|_| style::menu_popup())
        };

        let sort_btn = button(
            row![
                icon_box(&icons::SORT_FIELDS, TEXT),
                text(format!("{}", tab.sort.len())).size(14.0).color(TEXT),
            ]
            .spacing(5.0)
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::ResultSortPanel(run_id))
        .padding(Padding::new(5.0).left(9.0).right(9.0))
        .style(style::icon_button(tab.sort_panel_open));
        let sort_ctl = tooltip(sort_btn, tip("Sort fields"), tooltip::Position::Bottom).gap(4.0);

        // Highlight rules apply in both modes, so this button is always shown;
        // it opens the global rules modal (formerly reached from the Menu bar).
        let rules_btn = button(icon_box(&icons::HIGHLIGHT_RULES, TEXT))
            .on_press(Message::OpenRulesForm)
            .padding(Padding::new(5.0).left(9.0).right(9.0))
            .style(style::icon_button(self.rules_form.is_some()));
        let rules_ctl =
            tooltip(rules_btn, tip("Highlight rules"), tooltip::Position::Bottom).gap(4.0);

        // Layout options group: the Table/Text mode toggle, joined by the
        // "Format" button while in Text mode — Format edits the raw-text
        // template, so it is meaningless (and hidden) in Table mode. The shared
        // bordered surface ties the two together as one control.
        let is_raw = tab.mode == line::LayoutMode::RawText;
        let (mode_icon, mode_label, next_mode) = if is_raw {
            (&icons::RAW_TEXT, "Text", line::LayoutMode::Table)
        } else {
            (&icons::TABLE, "Table", line::LayoutMode::RawText)
        };
        let mode_btn = button(icon_box(mode_icon, TEXT))
            .on_press(Message::ResultLayoutMode(run_id, next_mode))
            .padding(Padding::new(5.0).left(9.0).right(9.0))
            .style(style::icon_button(false));
        let mut group =
            row![tooltip(mode_btn, tip(mode_label), tooltip::Position::Bottom).gap(4.0)]
                .spacing(6.0)
                .align_y(iced::Alignment::Center);

        if is_raw {
            let format_btn = button(icon_box(&icons::FORMAT, TEXT))
                .on_press(Message::OpenFormat(run_id))
                .padding(Padding::new(5.0).left(9.0).right(9.0))
                .style(style::icon_button(tab.format_open));
            group =
                group.push(tooltip(format_btn, tip("Format"), tooltip::Position::Bottom).gap(4.0));
        }

        let layout_group = container(group)
            .padding(3.0)
            .style(|_| style::options_group());

        let controls = row![sort_ctl, rules_ctl, layout_group, space().width(Fill)]
            .spacing(8.0)
            .align_y(iced::Alignment::Center);

        container(controls)
            .style(|_| style::panel(PANEL))
            .width(Fill)
            .padding(Padding::new(4.0).left(12.0).right(12.0))
            .into()
    }

    /// The floating "Sort fields" editor, anchored under the options strip's
    /// "Sort fields" button as a stack layer so it never reflows the strips or
    /// main area below it. A click anywhere outside dismisses it.
    fn sort_fields_popover_overlay(&self) -> Option<Element<'_, Message>> {
        let Some(Tab::Result(tab)) = self.active_tab.and_then(|t| self.open_tabs.get(t)) else {
            return None;
        };
        if !tab.sort_panel_open {
            return None;
        }
        if !tab.strips_visible() {
            return None;
        }
        let run_id = tab.run_id;

        const CARD_W: f32 = 460.0;
        // Just below the options strip: the Menu bar (25) + its rule, then the
        // Search bar row (40), then the options strip row (29) including its
        // trailing 1px rule.
        let top = 25.0 + 40.0 + 29.0;
        // Left edge of the "Sort fields" button: sidebar (240) + its rule (1) +
        // the options strip row's left padding (12).
        let left = 253.0;

        let card = container(self.sort_fields_popover(tab)).width(Length::Fixed(CARD_W));
        let anchored = container(column![
            space().height(top),
            row![space().width(left), card, space().width(Fill)],
        ])
        .width(Fill)
        .height(Fill);

        Some(
            mouse_area(anchored)
                .on_press(Message::ResultSortPanelDismiss(run_id))
                .into(),
        )
    }

    /// The "Sort fields" popover: one row per sort key (remove, direction
    /// toggle, reorder) plus a picker to add a field and a "Clear sorting"
    /// action. Floated by [`Self::sort_fields_popover_overlay`].
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
        let layout = tab.layout();
        let prepared = line::Prepared::from_rules(&self.config.rules);
        let mut body: Vec<Element<'_, Message>> = Vec::with_capacity(end - start + 2);
        if start > 0 {
            body.push(space().height(start as f32 * ROW_H).into());
        }
        for (offset, hit) in tab.hits[start..end].iter().enumerate() {
            let index = start + offset;
            let selected = tab.selected_hit == Some(index);
            let rendered = line::render(hit, &layout, &prepared);
            let cells = container(
                row(tab
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| -> Element<'_, Message> {
                        let width = if i == last {
                            Length::Fill
                        } else {
                            Length::Fixed(tab.col_width(col))
                        };
                        container(part_widget(&rendered.parts[i]))
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
            .id(tab.scroll_id.clone())
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

    /// Raw text mode's answer to [`Self::hit_table`]: one fixed-height,
    /// non-wrapping row per Hit rendering the template's single Part, with no
    /// headers and horizontal overflow reachable by scrolling. Row clicks open
    /// the same Hit-detail panel as Table mode.
    fn raw_text_view<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message> {
        let run_id = tab.run_id;
        let layout = tab.layout();
        let prepared = line::Prepared::from_rules(&self.config.rules);

        let (start, end) = tab.row_window();
        let mut body: Vec<Element<'_, Message>> = Vec::with_capacity(end - start + 2);
        if start > 0 {
            body.push(space().height(start as f32 * ROW_H).into());
        }
        for (offset, hit) in tab.hits[start..end].iter().enumerate() {
            let index = start + offset;
            let selected = tab.selected_hit == Some(index);
            let rendered = line::render(hit, &layout, &prepared);
            let content: Element<'_, Message> = match rendered.parts.first() {
                Some(part) => part_widget(part),
                None => text("").size(12.0).font(Font::MONOSPACE).into(),
            };

            let row_el = container(content)
                .height(Length::Fixed(ROW_H))
                .padding(Padding::new(3.0).left(6.0))
                .style(move |_| {
                    if selected {
                        style::panel(ACCENT)
                    } else {
                        container::Style::default()
                    }
                });

            body.push(
                mouse_area(row_el)
                    .on_press(Message::HitClicked(run_id, index))
                    .into(),
            );
        }
        let trailing = tab.hits.len().saturating_sub(end);
        if trailing > 0 {
            body.push(space().height(trailing as f32 * ROW_H).into());
        }

        let view = scrollable(column(body))
            .id(tab.scroll_id.clone())
            .width(Fill)
            .height(Fill)
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::default(),
                horizontal: scrollable::Scrollbar::default(),
            })
            .on_scroll(move |viewport| {
                let offset = viewport.absolute_offset();
                Message::ResultScrolled {
                    run_id,
                    offset_y: offset.y,
                    viewport_h: viewport.bounds().height,
                    content_h: viewport.content_bounds().height,
                }
            });

        let mut stacked = column![view].width(Fill).height(Fill);
        if let Some(footer) = self.paging_footer(tab) {
            stacked = stacked.push(rule::horizontal(1.0));
            stacked = stacked.push(footer);
        }
        stacked.into()
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
        let entry =
            |handle: &'static std::sync::LazyLock<Handle>, label: &str, msg: Option<Message>| {
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
            Paging::Capped => {
                let cap = tab.max_results as u64;
                let msg = match tab.total_hits {
                    TotalHits::Known(total) if total > cap => format!(
                        "Showing first {} of {} Hits — refine your search",
                        thousands(cap),
                        thousands(total),
                    ),
                    _ => format!("Showing first {cap} Hits — refine your search"),
                };
                text(msg).size(12.0).color(TEXT_DIM).into()
            }
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

/// One `Line` Part as a widget. When no Highlight rule touched the Part it is
/// a plain `text` — cheap `Auto`/`Basic` shaping. Only a Part a rule actually
/// split or recoloured goes through `rich_text`, which forces `Advanced`
/// shaping on the whole run (measurably slower on long log lines).
fn part_widget<'a>(part: &line::Part) -> Element<'a, Message> {
    match part.plain() {
        Some(s) => text(s.to_string())
            .size(12.0)
            .font(Font::MONOSPACE)
            .wrapping(text::Wrapping::None)
            .into(),
        None => iced::widget::rich_text(
            part.segments
                .iter()
                .map(line::Segment::to_span)
                .collect::<Vec<_>>(),
        )
        .size(12.0)
        .font(Font::MONOSPACE)
        .wrapping(text::Wrapping::None)
        .into(),
    }
}

/// A small colour chip for the Highlight rules modal: the colour itself, or
/// `PANEL_ALT` when unset / unparsed.
fn swatch<'a>(color: Option<Color>) -> Element<'a, Message> {
    let fill = color.unwrap_or(PANEL_ALT);
    container(space().width(14.0).height(14.0))
        .style(move |_| style::panel(fill))
        .into()
}

/// Centres `content` in a panel card over a dimmed backdrop.
fn modal_card<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    modal_card_sized(content, 460.0)
}

/// [`modal_card`] with an explicit card width — for modals that need more room
/// than the default (e.g. the Format modal's log-line preview).
fn modal_card_sized<'a>(content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
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

/// Folds a fetched Page into a Result Tab: replacing Hits on a first run,
/// appending on a scroll-driven load-more, and settling the paging state.
fn apply_page(rt: &mut ResultTab, result: Result<es::Page, String>, append: bool) {
    if !append {
        rt.refreshing = false;
    }
    match result {
        Ok(page) => {
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
            rt.paging = if rt.hits.len() >= rt.max_results {
                Paging::Capped
            } else if got < rt.fetch_size {
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

/// A solid-red alert pill for the info bar: a warning triangle, `msg`, and a
/// `\u{00d7}` button that clears the notice.
fn error_pill<'a>(run_id: u64, msg: &str) -> Element<'a, Message> {
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

/// One frame of the braille activity spinner, chosen by a monotonic counter.
fn spinner_frame(frame: usize) -> &'static str {
    const FRAMES: [&str; 10] = [
        "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
        "\u{2827}", "\u{2807}", "\u{280f}",
    ];
    FRAMES[frame % FRAMES.len()]
}

/// Groups an integer into thousands: `1234567` → `1,234,567`.
fn thousands(n: u64) -> String {
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
