// A release build is a GUI subsystem executable on Windows, so launching it
// from the Start menu opens the Log Lens window and nothing else. Without this
// a black console window appears behind the app, which is the loudest possible
// signal that it is not a real application. Debug builds keep their console, so
// `cargo run` still prints. See `attach_parent_console` for what the release
// build does when it *is* launched from a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod advance_cache;
mod config;
mod connection;
mod crashlog;
mod es;
mod icons;
mod line;
mod perf;
mod results;
mod results_view;
mod rules;
mod search;
mod secrets;
mod style;
mod tab;
mod update;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use iced::widget::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::widget::svg::Handle;
use iced::widget::{
    Id, button, checkbox, column, container, mouse_area, opaque, operation, pick_list, radio, row,
    rule, scrollable, space, stack, svg, text, text_editor, text_input,
};
use iced::window;
use iced::{Border, Color, Element, Fill, Length, Padding, Point, Size, Subscription, Task, Theme};

use config::{Auth, Config, Connection};
use config::{TimeUnit, TimeframeChoice, TimeframeMode};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use results::{Paging, ResultTab, RunState, TimeframeDraft, TotalHits};
use rules::{MatcherKind, RulesForm};
use search::{Fields, SearchForm};
use style::{ACCENT, BG, BORDER, PANEL, PANEL_ALT, TEXT, TEXT_DIM};
use tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, tracked in the `expanded`
/// set like a folder. The control char keeps it from colliding with a real
/// Connection name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

/// Menu bar geometry, shared by the bar and by every dropdown anchored under
/// it. A dropdown is a free-floating overlay layer over the whole window and
/// has no way to ask where its label ended up, so the two have to agree on one
/// set of numbers instead.
const MENU_BAR_PAD_LEFT: f32 = 8.0;
/// The width every Menu bar label occupies, whether or not its text fills it.
const MENU_LABEL_W: f32 = 46.0;
/// Gap between two Menu bar labels.
const MENU_LABEL_GAP: f32 = 12.0;
/// Distance from the top of the window to the underside of the Menu bar, which
/// is where a dropdown starts.
const MENU_BAR_H: f32 = 26.0;

/// The x offset of the `index`-th Menu bar label, for anchoring its dropdown.
fn menu_anchor_x(index: usize) -> f32 {
    MENU_BAR_PAD_LEFT + index as f32 * (MENU_LABEL_W + MENU_LABEL_GAP)
}

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
    // First thing, so a panic anywhere after it leaves a trace on disk rather
    // than only on a stderr that a release build does not have.
    crashlog::install_panic_hook();
    attach_parent_console();

    if handle_cli_flags() {
        return Ok(());
    }

    // Anything a previous run's Update left in the temp directory, cleared
    // before this run can create one of its own.
    update::clean_stale_downloads();
    register_for_restart();

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

/// Reattaches a Windows release build to the terminal it was launched from, if
/// it was launched from one.
///
/// **Do not delete this as dead code.** It is what makes `handle_cli_flags`
/// above visible on Windows at all. A release build is a GUI subsystem
/// executable (see the attribute at the top of this file), and such a process
/// starts with no console attached and no valid standard handles, so
/// `loglens.exe --version` typed at a `cmd.exe` or PowerShell prompt would
/// print into the void. That output is the release workflow's smoke test over
/// every Artifact, and the only way a user can ask a build which version it is.
///
/// `AttachConsole(ATTACH_PARENT_PROCESS)` borrows the parent's console when
/// there is one and points the standard streams at it. When there is no parent
/// console — launched from the Start menu, from Explorer, from a shortcut — the
/// attach fails, nothing is printed, and nothing is shown, which is exactly
/// what is wanted.
///
/// Deliberately *not* `AllocConsole`: that would create the very console window
/// the GUI subsystem attribute exists to remove.
///
/// Only the standard handles that are *invalid* are repointed at the console.
/// A GUI subsystem process launched with a redirection — `loglens.exe
/// --version > out.txt`, or a CI step capturing the output — inherits a valid
/// handle for the redirected stream, and overwriting that with `CONOUT$` would
/// send the output to the console and leave the file empty. Which handles
/// arrive valid is exactly the question being asked, so the check is per
/// handle: redirect stdout alone and stderr still lands on the console.
///
/// A debug build is a console subsystem executable and already has its own
/// console, so the attach fails there and this leaves its streams untouched.
///
/// Every other platform's process simply keeps the streams it was started with,
/// so there is nothing to do there.
#[cfg(windows)]
fn attach_parent_console() {
    use std::os::windows::io::IntoRawHandle;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // SAFETY: a plain FFI call taking one constant. A zero return means there
    // was no parent console to attach to, which is not an error here.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }

    // Attaching does not by itself repoint the process's standard handles at
    // the console, so `println!` would still be writing to the invalid handles
    // the process started with. Open the console's own pseudo-files and install
    // them; `std::io::stdout` resolves the handle on every write, so this takes
    // effect immediately.
    for (name, id) in [
        ("CONIN$", STD_INPUT_HANDLE),
        ("CONOUT$", STD_OUTPUT_HANDLE),
        ("CONOUT$", STD_ERROR_HANDLE),
    ] {
        // SAFETY: a plain FFI call taking one constant. A process that was
        // handed this stream — by a redirection, a pipe, or a parent that set
        // `STARTF_USESTDHANDLES` — gets a valid handle back and keeps it.
        // One with nothing on the stream gets null or `INVALID_HANDLE_VALUE`.
        let existing = unsafe { GetStdHandle(id) };
        if !existing.is_null() && existing != INVALID_HANDLE_VALUE {
            continue;
        }

        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
        else {
            continue;
        };
        // Leaked on purpose: the handle has to stay open for the rest of the
        // process's life, and the OS reclaims it at exit.
        let handle = file.into_raw_handle();
        // SAFETY: `handle` is a live console handle that nothing else owns now.
        unsafe { SetStdHandle(id, handle.cast()) };
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// Tells Windows how to start Log Lens again after the Restart Manager has
/// closed it.
///
/// This is the other half of the Windows Update path. `update::apply` spawns
/// the Release's installer with `/SILENT`, and `CloseApplications=yes` /
/// `RestartApplications=yes` in `packaging/windows/loglens.iss` have Setup ask
/// the Restart Manager to close Log Lens — Windows will not overwrite a
/// running `.exe` — and to bring it back once the new files are in place.
///
/// The Restart Manager restarts a process it closed by running the command
/// line that process registered here. Registering is what makes the relaunch
/// something Log Lens has asked for and can rely on, rather than something the
/// Restart Manager may or may not manage on its own; "the app closed itself
/// during an update and never came back" is the one outcome the silent path
/// must not have.
///
/// A null command line means "restart me with no arguments", which is exactly
/// how the Start-menu shortcut starts it. Flags of 0 mean "restart for any
/// reason", which includes the patching this exists for.
///
/// Best effort: the `HRESULT` is deliberately dropped. There is nothing a user
/// could do about a failure to register, and the cost of one is a relaunch
/// they can do by hand.
#[cfg(windows)]
fn register_for_restart() {
    use windows_sys::Win32::System::Recovery::RegisterApplicationRestart;

    // SAFETY: a plain FFI call. A null command line is documented as "restart
    // with no command-line arguments"; nothing here is borrowed or freed.
    let _ = unsafe { RegisterApplicationRestart(std::ptr::null(), 0) };
}

/// Only Windows has a Restart Manager. Linux Updates hand over to the new
/// binary themselves (`update::restart`).
#[cfg(not(windows))]
fn register_for_restart() {}

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
    /// Whether the Menu bar's "Help" dropdown is showing.
    help_menu_open: bool,
    /// The newer Release an Update check turned up, until the user dismisses
    /// the banner showing it.
    ///
    /// Session state on purpose. Dismissing means "not now", not "never tell
    /// me again": a dismissal written to the config file would have to be
    /// invalidated against a version to avoid hiding every future Release too,
    /// and that rule buys nothing over simply asking again at the next check.
    new_release: Option<update::Release>,
    /// Whether an Update check is in flight, so `Check for updates\u{2026}` can
    /// say so and go inert while it is.
    checking_for_updates: bool,
    /// How this copy of Log Lens was installed, and therefore whether it may
    /// replace itself.
    ///
    /// Decided once, at startup, and never asked again. It reads the Install
    /// flavour marker off disk, which the banner (redrawn every frame) has no
    /// business doing \u{2014} and on Linux an Update unlinks the running binary
    /// partway through, after which its own path can no longer be read back.
    flavour: update::Flavour,
    /// How far an Update the user started from the banner has got. `None`
    /// until they press it.
    updating: Option<Updating>,
    /// Whether the About dialog is open.
    about_open: bool,
    /// Editable copy of `config.es`, backing the Settings window's fields.
    settings_draft: SettingsDraft,
    /// The scripted-scroll performance harness, when `LOGLENS_PERF_SCROLL=1`.
    /// Drives a fixed scroll over one Saved Search, then prints timings and
    /// exits. See `src/perf.rs`.
    perf: Option<PerfHarness>,
}

/// Drives a deterministic scroll over a Result Tab so scroll-performance
/// numbers are comparable run to run. Active only under `LOGLENS_PERF_SCROLL=1`
/// (see [`perf`]); otherwise this is `None` and every hook below is skipped.
struct PerfHarness {
    /// Seconds the scroll should take from top to bottom.
    secs: f32,
    /// Wall time of the previous [`Message::PerfTick`], to measure the
    /// realized frame interval under scroll load.
    last_tick: Option<std::time::Instant>,
    /// `None` until the target tab's first Page lands and the scroll starts.
    run: Option<PerfRun>,
}

struct PerfRun {
    run_id: u64,
    /// When the scripted scroll began.
    start: std::time::Instant,
    /// Set once the scroll has reached the bottom and the timings are printed,
    /// so the ticker subscription stops and no second exit is issued.
    done: bool,
}

/// How far the Update the user started from the banner has got.
///
/// An Update is only ever started by pressing a button, so unlike an Update
/// *check* there is no silent half to it: every state here is on screen, and a
/// failure stays on screen with a way to reach the releases page. The
/// check-time silent/loud split lives in `update::outcome` and stops there.
#[derive(Debug, Clone)]
enum Updating {
    /// Downloading and verifying, or waiting on an installer that is now
    /// running. The banner says which and the Update button goes away.
    Busy(&'static str),
    /// It failed, and the reason is shown until the banner is dismissed.
    Failed(String),
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
    /// One frame of the scripted-scroll performance harness (see `PerfHarness`).
    PerfTick,
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
    // Help menu + Update check
    /// Toggle the Menu bar's "Help" dropdown.
    HelpMenuToggle,
    /// Close the "Help" dropdown (click outside).
    HelpMenuDismiss,
    /// `Help > Check for updates\u{2026}`: an Update check the user asked for,
    /// which runs regardless of when the last one did.
    CheckForUpdates,
    /// An Update check finished. The [`update::Trigger`] rides along with the
    /// result because it, and not the result, decides whether a failure is
    /// allowed to be seen \u{2014} see [`update::outcome`].
    UpdateCheckDone {
        trigger: update::Trigger,
        result: Result<Option<update::Release>, String>,
    },
    /// Hide the Update banner for the rest of the session.
    DismissUpdateBanner,
    /// The banner's Update button: download this Release's Artifact for this
    /// platform, verify it against the Release's `SHA256SUMS`, and run it.
    /// Only ever reachable on an installer-managed copy.
    ApplyUpdate,
    /// An Update got as far as a background task can take it, or failed
    /// trying.
    UpdateApplied(Result<update::Applied, String>),
    /// Open the Release's page: the only route a Portable copy is offered, and
    /// the way out of a failed Update.
    OpenReleasesPage,
    /// Open the About dialog from the "Help" dropdown.
    OpenAbout,
    /// Close the About dialog.
    CloseAbout,
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

        // The startup Update check, which runs at most once a day. The cadence
        // is decided here rather than inside `update::check`, so that the
        // manual path can simply not ask.
        let due = update::is_due(config.last_update_check, chrono::Utc::now());

        let perf = perf::scroll_harness().then(|| PerfHarness {
            secs: perf::scroll_secs(),
            last_tick: None,
            run: None,
        });

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
            help_menu_open: false,
            new_release: None,
            checking_for_updates: due,
            flavour: update::flavour(),
            updating: None,
            about_open: false,
            perf,
        };

        let startup_check = if due {
            update_check_task(update::Trigger::Background)
        } else {
            Task::none()
        };

        // Under the scroll-perf harness, open the target Saved Search straight
        // away — the rest of the run drives itself from there (see
        // `Message::PerfTick` and the `PageLoaded` hook).
        let perf_open = if app.perf.is_some() {
            match perf_open_search(&app.config) {
                Some(msg) => Task::done(msg),
                None => {
                    eprintln!(
                        "LOGLENS_PERF_SCROLL: no Saved Search to open — configure one, \
                         or point LOGLENS_PERF_SEARCH at an existing id/name"
                    );
                    Task::none()
                }
            }
        } else {
            Task::none()
        };

        (
            app,
            Task::batch([open_main.discard(), startup_check, perf_open]),
        )
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

        // Drive the scripted scroll while a perf run is underway.
        // `window::frames()` yields once per frame the app actually renders —
        // and subscribing to it keeps those redraws coming — so one
        // `PerfTick` maps to one real frame, and the interval between them is
        // the realized frame time under scroll load.
        let perf_tick = match &self.perf {
            Some(p) if p.run.as_ref().is_some_and(|r| !r.done) => {
                iced::window::frames().map(|_| Message::PerfTick)
            }
            _ => Subscription::none(),
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
            return Subscription::batch([escape, closes, spinner, perf_tick, drag]);
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
            return Subscription::batch([escape, closes, spinner, perf_tick, drag]);
        }

        Subscription::batch([escape, closes, spinner, perf_tick])
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
        let _span = perf::span("update");
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

                // Scroll-perf harness: the target tab's first Page is in, so
                // arm the scripted scroll. In fixture mode there is no async
                // `_count`, so settle the total here or the Hit-count spinner
                // subscription would tick through the whole measured run.
                if !append && ok && self.perf.as_ref().is_some_and(|p| p.run.is_none()) {
                    if let Some(rt) = self.result_mut(run_id) {
                        if perf::fixture_path().is_some() {
                            rt.total_hits = TotalHits::Known(rt.hits.len() as u64);
                        }
                        if let Some(mode) = perf::force_mode() {
                            rt.mode = mode;
                            rt.resolve_template();
                        }
                    }
                    if let Some(perf) = self.perf.as_mut() {
                        perf.run = Some(PerfRun {
                            run_id,
                            start: std::time::Instant::now(),
                            done: false,
                        });
                        perf.last_tick = None;
                    }
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
            Message::PerfTick => {
                let now = std::time::Instant::now();
                let (run_id, progress, finished) = {
                    let Some(perf) = self.perf.as_mut() else {
                        return Task::none();
                    };
                    if let Some(prev) = perf.last_tick.replace(now) {
                        perf::record("perf.frame_interval", (now - prev).as_secs_f32() * 1_000.0);
                    }
                    let secs = perf.secs;
                    let Some(run) = perf.run.as_mut() else {
                        return Task::none();
                    };
                    if run.done {
                        return Task::none();
                    }
                    let progress = run.start.elapsed().as_secs_f32() / secs;
                    if progress >= 1.0 {
                        run.done = true;
                    }
                    (run.run_id, progress.min(1.0), progress >= 1.0)
                };
                if finished {
                    perf::dump();
                    return iced::exit();
                }
                let Some(rt) = self.result_mut(run_id) else {
                    perf::dump();
                    return iced::exit();
                };
                let content_h = rt.hits.len() as f32 * results::ROW_H;
                let target_y = progress * (content_h - rt.viewport_h).max(0.0);
                // Set `scroll_y` directly — it is what `row_window()` reads, so
                // this is what actually shifts the rendered slice — then move
                // the real scrollable to match for a faithful draw. Done in one
                // `update` so there is exactly one `view` per frame, not the
                // two a follow-up `ResultScrolled` message would cause. (The
                // harness deliberately does not page: it measures a fixed
                // loaded set.)
                rt.scroll_y = target_y;
                let scroll_id = rt.scroll_id.clone();
                return operation::scroll_to(
                    scroll_id,
                    AbsoluteOffset {
                        x: 0.0,
                        y: target_y,
                    },
                );
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
            Message::HelpMenuToggle => self.help_menu_open = !self.help_menu_open,
            Message::HelpMenuDismiss => self.help_menu_open = false,
            Message::CheckForUpdates => {
                self.help_menu_open = false;
                // A second check while one is already in flight would spend
                // another of the hour's 60 unauthenticated GitHub requests to
                // answer a question already being asked.
                if self.checking_for_updates {
                    return Task::none();
                }
                self.checking_for_updates = true;
                self.status = Some("Checking for updates\u{2026}".to_string());
                return update_check_task(update::Trigger::Manual);
            }
            Message::UpdateCheckDone { trigger, result } => {
                self.checking_for_updates = false;
                self.record_update_check();
                match update::outcome(trigger, result) {
                    update::Outcome::Found(release) => {
                        // Clears the manual path's "Checking\u{2026}" line; the
                        // banner is the answer now.
                        self.status = None;
                        // A newer Release than the one an earlier Update failed
                        // on: that failure is about a Release nobody is being
                        // offered any more.
                        self.updating = None;
                        self.new_release = Some(release);
                    }
                    update::Outcome::UpToDate => {
                        self.status = Some(format!(
                            "Log Lens {} is the latest version.",
                            update::RUNNING_VERSION
                        ));
                    }
                    update::Outcome::Failed(err) => {
                        self.status = Some(format!("Could not check for updates: {err}"));
                    }
                    // A background check that found nothing, or failed. Nobody
                    // asked, so nothing is said \u{2014} and nothing needs
                    // clearing either, since only the manual path ever put a
                    // "Checking\u{2026}" line up.
                    update::Outcome::Silent => {}
                }
            }
            Message::DismissUpdateBanner => {
                self.new_release = None;
                self.updating = None;
            }
            Message::ApplyUpdate => {
                // A Portable copy is shown no Update button at all; this is the
                // same rule stated where it is enforced rather than only where
                // it is drawn.
                let Some(exe) = self.flavour.installed_exe().map(Path::to_path_buf) else {
                    return Task::none();
                };
                let Some(release) = self.new_release.clone() else {
                    return Task::none();
                };
                if matches!(self.updating, Some(Updating::Busy(_))) {
                    return Task::none();
                }

                // Persist before starting: from here on this process ends
                // without warning. On Windows the Restart Manager closes Log
                // Lens, forcefully if it does not go quietly; on Linux the
                // hand-over replaces the process image outright. Neither gives
                // the app a chance to write anything on the way out.
                //
                // It writes what is already on disk — every save the user asked
                // for has happened at the moment they asked for it, and this
                // touches nothing else: an Update never goes near Connections,
                // Saved Searches, settings or the keyring. Its own failure is
                // swallowed, because nobody asked for this save and it must not
                // take the place of the Update they are waiting on.
                let _ = config::save(&self.config);

                self.updating = Some(Updating::Busy("Downloading\u{2026}"));
                return Task::perform(update::apply(release, exe), Message::UpdateApplied);
            }
            Message::UpdateApplied(result) => match result {
                // Linux: the new version is installed and this process hands
                // over to it. `restart` replaces the process image, so it only
                // returns when the hand-over itself failed.
                Ok(update::Applied::HandOver(exe)) => {
                    self.updating = Some(Updating::Failed(update::restart(&exe)));
                }
                // Windows: the installer is running and the Restart Manager
                // will close and reopen the app. Nothing left to do but say so
                // until it does.
                Ok(update::Applied::InstallerRunning) => {
                    self.updating = Some(Updating::Busy(
                        "Installing\u{2026} Log Lens will close and reopen.",
                    ));
                }
                // Always shown, whatever went wrong: the user pressed a button
                // and is owed an answer, and the banner keeps a route to the
                // releases page beside it.
                Err(err) => self.updating = Some(Updating::Failed(err)),
            },
            Message::OpenReleasesPage => {
                if let Some(release) = &self.new_release
                    && let Err(err) = update::open_in_browser(&release.html_url)
                {
                    // No browser to hand: the address itself is the fallback.
                    self.status = Some(format!("{err} \u{2014} open {}", release.html_url));
                }
            }
            Message::OpenAbout => {
                self.help_menu_open = false;
                self.about_open = true;
            }
            Message::CloseAbout => self.about_open = false,

            Message::WindowClosed(id) => {
                if id == self.main_window {
                    // Flush whatever the perf instrumentation collected this
                    // session (a plain `LOGLENS_PERF=1` run with no scripted
                    // scroll reaches its dump only here).
                    perf::dump();
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

    // --- Update check ------------------------------------------------------

    /// Stamps "a check just ran" into the config, whether the check succeeded
    /// or not.
    ///
    /// Recording a failure matters as much as recording a hit. A timestamp
    /// written only on success would leave anyone permanently behind a proxy
    /// that blocks api.github.com checking again on every single launch \u{2014}
    /// the population a failing check is least entitled to bother, and the one
    /// that would burn a shared office IP's 60-an-hour GitHub budget fastest.
    ///
    /// A failed write is swallowed rather than shown. This timestamp is
    /// scheduling bookkeeping nobody asked to persist, and a config-file error
    /// raised on startup by a check the user did not request is the exact
    /// interruption the failure policy exists to prevent. Every save the user
    /// *did* ask for still reports its own failures; the only cost here is one
    /// redundant check tomorrow.
    fn record_update_check(&mut self) {
        self.config.last_update_check = Some(chrono::Utc::now());
        let _ = config::save(&self.config);
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
        // Scroll-perf harness: load Hits from a saved `_search` response on
        // disk instead of hitting a cluster, so a run is byte-identical every
        // time and needs no Elasticsearch. See `src/perf.rs`.
        if let Some(path) = perf::fixture_path() {
            if let Some(rt) = self.result_mut(run_id) {
                rt.refreshing = false;
                rt.state = RunState::Loading;
                rt.hits.clear();
                rt.paging = Paging::Idle;
                rt.scroll_y = 0.0;
                rt.total_hits = TotalHits::Loading;
            }
            return Task::perform(
                async move {
                    std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|body| es::parse_page(&body))
                },
                move |result| Message::PageLoaded {
                    run_id,
                    result,
                    append: false,
                },
            );
        }

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
        let _span = perf::span("view");
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

        // Built as a Vec rather than a `column!` so the Update banner and the
        // rule beneath it appear together or not at all.
        let mut chrome: Vec<Element<'_, Message>> =
            vec![self.menu_bar(), rule::horizontal(1.0).into()];
        if let Some(banner) = self.update_banner() {
            chrome.push(banner);
            chrome.push(rule::horizontal(1.0).into());
        }
        chrome.push(container(body).width(Fill).height(Fill).into());
        chrome.push(self.status_bar());
        chrome.push(rule::horizontal(1.0).into());
        chrome.push(self.info_bar());

        let base: Element<'_, Message> = container(column(chrome))
            .style(|_| style::panel(BG))
            .width(Fill)
            .height(Fill)
            .into();

        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if let Some(menu) = self.file_menu_overlay() {
            layers.push(menu);
        }
        if let Some(menu) = self.help_menu_overlay() {
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
            let prepared = line::Prepared::from_rules(&self.config.rules);
            layers.push(results_view::format_modal(tab, &prepared));
        }
        if let Some(prompt) = &self.secret_prompt {
            layers.push(self.secret_prompt_modal(prompt));
        }
        if let Some(confirm) = &self.confirm {
            layers.push(self.confirm_modal(confirm));
        }
        if self.about_open {
            layers.push(self.about_modal());
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
            Some(Tab::Result(tab)) => {
                let prepared = line::Prepared::from_rules(&self.config.rules);
                results_view::result_view(
                    tab,
                    &prepared,
                    self.header_hover,
                    self.grip_hover,
                    self.column_drag.as_ref(),
                )
            }
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

    /// The always-present Menu bar across the top of the window. `File` and
    /// `Help` open dropdowns (see [`Self::file_menu_overlay`] and
    /// [`Self::help_menu_overlay`]); `View` is still inert.
    ///
    /// Every label occupies the same fixed cell width. A dropdown is a
    /// free-floating overlay layer stacked over the whole window, so nothing
    /// tells it where the label it hangs under actually landed; uniform cells
    /// turn the anchor into [`menu_anchor_x`] arithmetic instead of a number
    /// measured off a screenshot and re-measured whenever a label is renamed.
    fn menu_bar(&self) -> Element<'_, Message> {
        container(
            row![
                menu_bar_label("File", self.file_menu_open, Message::FileMenuToggle),
                // Inert, so it is rendered as dimmed text in a cell of the same
                // width rather than as a button that does nothing when pressed.
                container(text("View").size(13.0).color(TEXT_DIM))
                    .width(Length::Fixed(MENU_LABEL_W))
                    .center_x(Fill),
                menu_bar_label("Help", self.help_menu_open, Message::HelpMenuToggle),
            ]
            .spacing(MENU_LABEL_GAP)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| style::panel(PANEL_ALT))
        .width(Fill)
        .padding(Padding::new(4.0).left(MENU_BAR_PAD_LEFT).right(12.0))
        .into()
    }

    /// The floating "Help" dropdown, anchored under its Menu bar label.
    fn help_menu_overlay(&self) -> Option<Element<'_, Message>> {
        if !self.help_menu_open {
            return None;
        }

        // While a check is in flight the item says so and stops responding.
        // The unauthenticated GitHub API allows 60 requests an hour per IP,
        // shared by everyone behind an office NAT, and a menu item that looks
        // like it did nothing invites exactly the repeated clicking that spends
        // them.
        let checking = self.checking_for_updates;
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

        let block = container(
            column![
                check,
                button(text("About").size(12.0).color(TEXT))
                    .on_press(Message::OpenAbout)
                    .width(Fill)
                    .padding(Padding::new(4.0).left(10.0).right(10.0))
                    .style(style::picker_row(false)),
            ]
            .spacing(1.0),
        )
        .width(178.0)
        .padding(3.0)
        .style(|_| style::menu_popup());

        let anchored = container(block)
            .width(Fill)
            .height(Fill)
            // Index 2: the bar reads File, View, Help.
            .padding(Padding::new(0.0).left(menu_anchor_x(2)).top(MENU_BAR_H));

        Some(
            mouse_area(anchored)
                .on_press(Message::HelpMenuDismiss)
                .on_right_press(Message::HelpMenuDismiss)
                .into(),
        )
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
    fn update_banner(&self) -> Option<Element<'_, Message>> {
        let release = self.new_release.as_ref()?;

        let mut left = column![
            text(format!("{APP_NAME} {} is available.", release.version))
                .size(13.0)
                .color(Color::WHITE),
        ]
        .spacing(4.0);

        let notes = release.notes.trim();
        if !notes.is_empty() {
            // GitHub's generated notes are markdown of no fixed length, shown
            // as the plain text they are. Bounded and scrollable so a long
            // changelog cannot push the tab strip off the bottom of the window.
            left = left.push(
                container(scrollable(text(notes.to_string()).size(12.0).color(
                    Color {
                        a: 0.85,
                        ..Color::WHITE
                    },
                )))
                .max_height(72.0),
            );
        }

        let portable = self.flavour.installed_exe().is_none();
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
        if let Some(Updating::Failed(err)) = &self.updating {
            left = left.push(
                text(format!("Update failed: {err}"))
                    .size(12.0)
                    .color(ERR_RED),
            );
        }

        // The Update button belongs to installer-managed copies only, and goes
        // away while one is running so it cannot be pressed twice.
        let mut trailing: Vec<Element<'_, Message>> = Vec::new();
        if let Some(Updating::Busy(step)) = &self.updating {
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
            let again = matches!(self.updating, Some(Updating::Failed(_)));
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
        if portable || matches!(self.updating, Some(Updating::Failed(_))) {
            trailing.push(
                button(text("Releases page").size(12.0).color(TEXT))
                    .on_press(Message::OpenReleasesPage)
                    .padding(Padding::new(4.0).left(12.0).right(12.0))
                    .style(style::icon_button(false))
                    .into(),
            );
        }

        Some(
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
            .padding(Padding::new(8.0).left(12.0).right(8.0))
            .into(),
        )
    }

    /// The About dialog: what this build is, where it came from, and where it
    /// leaves a trace when it crashes.
    ///
    /// An overlay modal in the main window rather than a second OS window like
    /// Settings. Settings earns a window of its own because it is an editor
    /// people leave open beside the app while they work; About is read once and
    /// dismissed, and giving four lines of text its own taskbar button and
    /// alt-tab entry costs more than it returns.
    fn about_modal(&self) -> Element<'_, Message> {
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
            .padding(Padding::new(0.0).left(menu_anchor_x(0)).top(MENU_BAR_H));

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
        Some(results_view::result_sort_bar(
            tab,
            self.rules_form.is_some(),
        ))
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

        let card = container(results_view::timeframe_popover(tab)).width(Length::Fixed(CARD_W));
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

    // --- Result tab view -----------------------------------------------
    //
    // The Hit table, raw text mode, Hit detail panel, header menu, Sort
    // fields / Custom timeframe popover content, and the Format modal all
    // live in `results_view.rs` now — they only ever needed a `ResultTab`
    // plus a few transient hover fields, never the rest of `LogLens`.

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

        let card = container(results_view::sort_fields_popover(tab)).width(Length::Fixed(CARD_W));
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

/// One Menu bar label that opens a dropdown, in a cell of the shared width so
/// [`menu_anchor_x`] can place that dropdown underneath it.
fn menu_bar_label<'a>(label: &'a str, open: bool, toggle: Message) -> Element<'a, Message> {
    button(text(label).size(13.0).color(TEXT).width(Fill).center())
        .on_press(toggle)
        .width(Length::Fixed(MENU_LABEL_W))
        .padding(Padding::new(2.0))
        .style(style::picker_row(open))
        .into()
}

/// Runs one Update check, tagging the result with why it ran so that
/// [`update::outcome`] can decide what may be shown.
fn update_check_task(trigger: update::Trigger) -> Task<Message> {
    Task::perform(update::check(), move |result| Message::UpdateCheckDone {
        trigger,
        result,
    })
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

/// The `OpenSavedSearch` message the scroll-perf harness opens on boot: the
/// Saved Search whose id or name matches `LOGLENS_PERF_SEARCH`, or the first
/// one configured when that is unset. `None` if the config has no searches.
fn perf_open_search(config: &Config) -> Option<Message> {
    let want = perf::target_search();
    for conn in &config.connections {
        for search in &conn.searches {
            let matched = match &want {
                Some(w) => &search.id == w || &search.name == w,
                None => true,
            };
            if matched {
                return Some(Message::OpenSavedSearch {
                    connection: conn.id.clone(),
                    search: search.id.clone(),
                });
            }
        }
    }
    None
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
