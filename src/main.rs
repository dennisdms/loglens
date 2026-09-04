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
mod search;
mod secrets;
mod style;
mod tab;
mod ui;
mod update;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use iced::widget::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, column, container, operation, row, rule, stack, text_editor};
use iced::window;
use iced::{Element, Fill, Point, Size, Subscription, Task, Theme};

use config::{Auth, Config, Connection};
use config::{TimeUnit, TimeframeChoice, TimeframeMode};
use connection::{AuthKind, ConnectionForm, EndpointError, TestState};
use results::{Paging, ResultTab, RunState, TimeframeDraft, TotalHits};
use search::{Fields, SearchForm};
use style::{BG, TEXT_DIM};
use tab::Tab;
use ui::centered;
use ui::chrome::Chrome;

/// Pseudo tree-node name for the Elasticsearch root, tracked in the `expanded`
/// set like a folder. The control char keeps it from colliding with a real
/// Connection name.
const ES_ROOT: &str = "\u{1}Elasticsearch";

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
    /// One connected [`es::Client`] per Connection, by Connection id, built on
    /// first use. Dropped by [`LogLens::forget_client`] when what it was built
    /// from changes.
    clients: HashMap<String, es::Client>,
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
    /// Empty string = no cap; otherwise the parsed row cap.
    wrap_row_cap: String,
    error: Option<String>,
}

impl SettingsDraft {
    fn from_config(config: &config::Config) -> Self {
        Self {
            max_results: config.es.max_results.to_string(),
            fetch_size: config.es.fetch_size.to_string(),
            wrap_row_cap: config
                .wrap_row_cap
                .map(|n| n.to_string())
                .unwrap_or_default(),
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
    ConnFormTestDone(Result<es::ClusterInfo, es::Error>),
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
        targets: Vec<String>,
    },
    SearchName(String),
    SearchTargetInput(String),
    SearchTargetPicked(String),
    SearchTimestampField(String),
    SearchFieldsLoaded {
        form_id: u64,
        result: Result<es::FieldCaps, es::Error>,
    },
    /// Save & Run the new-Saved-Search form tab.
    SearchSave,
    /// Save the Search settings modal (re-runs an open Result Tab for it).
    SearchSettingsSave,
    /// Dismiss the Search settings modal without saving.
    SearchSettingsCancel,
    // Result tab: live query string, timeframe, columns + sort
    /// The async Target listing for a Result Tab's suggestion dropdown landed.
    ResultTargetsLoaded {
        run_id: u64,
        targets: Vec<String>,
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
        result: Result<es::FieldCaps, es::Error>,
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
        result: Result<es::FieldCaps, es::Error>,
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
    /// Toggle line wrapping (variable row heights) for a Result Tab.
    ResultWrap(u64),
    /// Expand / collapse one wrapped Hit past the global row cap.
    ResultHitExpand(u64, usize),
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
        result: Result<u64, es::Error>,
    },
    /// Advance the Hit-count spinner one frame.
    SpinnerTick,
    /// One frame of the scripted-scroll performance harness (see `PerfHarness`).
    PerfTick,
    /// A Page of a Result Tab's Run landed. Carries the Run back so the tab
    /// can ask it for the next one.
    PageLoaded {
        run_id: u64,
        generation: u64,
        advance: Box<es::Advance>,
        /// Whether this Page extends the tab's Hits or replaces them.
        append: bool,
    },
    ResultScrolled {
        run_id: u64,
        offset_y: f32,
        viewport_h: f32,
        content_h: f32,
        offset_x: f32,
        viewport_w: f32,
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
    SettingsWrapCap(String),
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
            settings_draft: SettingsDraft::from_config(&config),
            config,
            open_tabs: Vec::new(),
            active_tab: None,
            expanded,
            clients: HashMap::new(),
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

    /// A Connection's display name, for the forms that show which cluster they
    /// are editing against. Empty when the Connection has been deleted out from
    /// under an open form \u{2014} the name is a subtitle, not the subject, so a
    /// blank one is better there than an error.
    fn conn_name(&self, id: &str) -> &str {
        self.connection(id).map_or("", |c| c.name.as_str())
    }

    /// The [`es::Client`] for a Connection, connecting and memoizing on first
    /// use so every call to that cluster shares one connection pool. `None` if
    /// the Connection is gone, or its secret isn't available this session
    /// (keyring missing, not yet re-entered) — the caller prompts for it.
    ///
    /// Hands back a clone, which is cheap, so the borrow ends here.
    fn client_for(&mut self, conn_id: &str) -> Option<es::Client> {
        if let Some(client) = self.clients.get(conn_id) {
            return Some(client.clone());
        }
        let conn = self.connection(conn_id)?;
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
        let client = es::Client::connect(es::Endpoint {
            url: conn.url.clone(),
            auth,
            skip_tls_verify: conn.skip_tls_verify,
        })
        .ok()?;
        self.clients.insert(conn_id.to_string(), client.clone());
        Some(client)
    }

    /// Drops the memoized Client for a Connection, so the next call rebuilds it.
    /// Called whenever what it was built from — URL, auth, TLS setting, secret
    /// — may have changed.
    fn forget_client(&mut self, conn_id: &str) {
        self.clients.remove(conn_id);
    }

    /// The active tab, when it is a Result Tab.
    ///
    /// Most of the chrome is a function of exactly this: the Search bar, the
    /// options strip, their three overlays, the info bar's Hit-count readout,
    /// and the Format modal all read one `ResultTab` and nothing else in
    /// `LogLens`. Written once here so those surfaces can be free functions
    /// over a `&ResultTab` instead of methods reaching back through `&self`.
    fn active_result(&self) -> Option<&ResultTab> {
        match self.active_tab.and_then(|t| self.open_tabs.get(t))? {
            Tab::Result(tab) => Some(tab),
            Tab::SearchForm(_) => None,
        }
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
                        Err(err) => TestState::Failed(err.to_string()),
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
                    self.forget_client(&prompt.connection_id);
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
            Message::SearchTargetsLoaded { form_id, targets } => {
                if let Some(f) = self.form_mut(form_id) {
                    f.targets_loading = false;
                    f.target_options = targets;
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

            Message::ResultTargetsLoaded { run_id, targets } => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.targets_loading = false;
                    rt.target_options = targets;
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
                        rt.target_draft = rt.search.target.clone();
                    }
                }
            }
            Message::ResultTargetPanelDismiss(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.target_panel_open = false;
                    rt.target_draft = rt.search.target.clone();
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
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let draft = rt.query_draft.clone();
                        rt.search.set_query_string(draft)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultTimeframeChoice(run_id, choice) => match choice.to_timeframe() {
                Some(timeframe) => {
                    let edited = self
                        .result_mut(run_id)
                        .map(|rt| {
                            rt.tf.open = false;
                            rt.search.set_timeframe(timeframe)
                        })
                        .unwrap_or_default();
                    return self.apply_edit(run_id, edited);
                }
                None => {
                    if let Some(rt) = self.result_mut(run_id) {
                        let current = rt.search.timeframe.clone();
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
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let timeframe = rt.tf.to_timeframe();
                        rt.tf.open = false;
                        rt.search.set_timeframe(timeframe)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultTfCancel(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.tf.open = false;
                }
            }
            Message::ResultFieldsLoaded { run_id, result } => {
                if let Ok(caps) = result {
                    let edited = self
                        .result_mut(run_id)
                        .map(|rt| {
                            rt.all_fields = caps.all;
                            rt.sortable_fields = caps.sortable;
                            rt.resolve_template()
                        })
                        .unwrap_or_default();
                    return self.apply_edit(run_id, edited);
                }
            }
            Message::ResultColumnDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.column_draft = v;
                }
            }
            Message::ResultColumnAdd(run_id) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        let draft = rt.column_draft.clone();
                        rt.add_column_from_draft(&draft)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultColumnAddField(run_id, field) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.add_column_from_draft(&field))
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultColumnRemove(run_id, i) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.search.remove_column(i)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultColumnMove(run_id, i, delta) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.search.move_column(i, delta)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
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
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.search.set_sort_dir(&field, desc)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultSortRemove(run_id, field) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.header_menu = None;
                        rt.search.remove_sort(&field)
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultSortMove(run_id, index, delta) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.search.move_sort(index, delta))
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultSortClear(run_id) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.search.clear_sort())
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }

            Message::ResultLayoutMode(run_id, mode) => {
                // Entering raw text mode for the first time also resolves the
                // template, which is a second edit to persist — hence the `|`.
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.search.set_mode(mode) | rt.resolve_template())
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultWrap(run_id) => {
                // The next `prepare_heights` sees the `WrapCtx` change and
                // rebuilds the row-height model; a one-off render pass measures
                // every line the first time wrap turns on.
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.search.toggle_wrap())
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::ResultHitExpand(run_id, index) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.line_cache.get_mut().toggle_expand(index);
                }
            }
            Message::ResultTemplateDraft(run_id, v) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template_draft = v;
                }
            }
            Message::ResultTemplateSubmit(run_id) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| rt.commit_template())
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::OpenFormat(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.format_open = true;
                }
            }
            Message::CloseFormat(run_id) => {
                let edited = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.format_open = false;
                        rt.commit_template()
                    })
                    .unwrap_or_default();
                return self.apply_edit(run_id, edited);
            }
            Message::FormatCancel(run_id) => {
                if let Some(rt) = self.result_mut(run_id) {
                    rt.template_draft = rt.search.template.clone();
                    rt.format_open = false;
                }
            }

            Message::TotalHitsLoaded {
                run_id,
                generation,
                result,
            } => {
                if let Some(rt) = self.result_mut(run_id)
                    && rt.generation == generation
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
                generation,
                advance,
                append,
            } => {
                let ok = !matches!(advance.state, es::State::Failed(_));
                match self.result_mut(run_id) {
                    // A Page from a superseded run would append Hits from a
                    // Query the tab has moved on from.
                    Some(rt) if rt.generation == generation => apply_page(rt, *advance, append),
                    _ => return Task::none(),
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
                            rt.search.mode = mode;
                            rt.resolve_template();
                        }
                        if perf::force_wrap() {
                            rt.search.wrap = true;
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
                offset_x,
                viewport_w,
            } => {
                let wants_more = self
                    .result_mut(run_id)
                    .map(|rt| {
                        rt.scroll_y = offset_y;
                        rt.viewport_h = viewport_h;
                        rt.scroll_x = offset_x;
                        rt.viewport_w = viewport_w;
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
                // The row-height model knows the real content height (it may
                // be variable-height wrapped rows); fall back to a flat grid
                // only before the first `view` has primed it.
                let content_h = {
                    let h = rt.line_cache.borrow().content_height();
                    if h > 0.0 {
                        h
                    } else {
                        rt.hits.len() as f32 * results::ROW_H
                    }
                };
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
                        rt.target_draft = rt.search.target.clone();
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
                self.settings_draft = SettingsDraft::from_config(&self.config);
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
            Message::SettingsWrapCap(v) => {
                self.settings_draft.wrap_row_cap = v;
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

        // Wrap row cap: blank / "none" / "0" means no cap.
        let wrap_cap_raw = self.settings_draft.wrap_row_cap.trim().to_ascii_lowercase();
        let wrap_row_cap =
            if wrap_cap_raw.is_empty() || wrap_cap_raw == "none" || wrap_cap_raw == "0" {
                None
            } else {
                match wrap_cap_raw.replace([',', '_'], "").parse::<usize>() {
                    Ok(n) => Some(n.max(1)),
                    Err(_) => {
                        self.settings_draft.error = Some(
                            "Wrap row cap must be a whole number (or blank for none)".to_string(),
                        );
                        return Task::none();
                    }
                }
            };

        let es = config::EsSettings {
            max_results,
            fetch_size,
        }
        .normalized();
        self.config.es = es;
        self.config.wrap_row_cap = wrap_row_cap;
        self.settings_draft = SettingsDraft::from_config(&self.config);

        for tab in &mut self.open_tabs {
            if let Tab::Result(rt) = tab {
                rt.max_results = es.max_results;
                rt.fetch_size = es.fetch_size;
                let limits = rt.limits();
                if let Some(run) = &mut rt.run {
                    run.relimit(limits);
                }
                // A changed `wrap_row_cap` reaches the row-height model
                // through `WrapCtx` — the next `prepare_heights` re-keys and
                // rebuilds it, no explicit reset needed.
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
                // The form may be describing a Connection that isn't saved yet,
                // so this Client is one-off rather than memoized.
                match es::Client::connect(endpoint) {
                    Ok(client) => Task::perform(
                        async move { client.ping().await },
                        Message::ConnFormTestDone,
                    ),
                    Err(err) => {
                        form.test = TestState::Failed(err.to_string());
                        Task::none()
                    }
                }
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
        self.forget_client(&id);

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

        let targets = match self.client_for(&conn_id) {
            Some(client) => Task::perform(async move { client.targets().await }, move |targets| {
                Message::SearchTargetsLoaded { form_id, targets }
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
        self.forget_client(&conn_id);
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
        let Some(client) = self.client_for(&conn_id) else {
            if let Some(f) = self.form_mut(form_id) {
                f.fields = Fields::Failed;
            }
            return Task::none();
        };
        Task::perform(async move { client.fields(&target).await }, move |result| {
            Message::SearchFieldsLoaded { form_id, result }
        })
    }

    /// Carries out what an edit to a Result Tab's Live Search obliged: writing
    /// the Saved Search back to the config, and starting a fresh Run when the
    /// change is one the cluster cares about.
    fn apply_edit(&mut self, run_id: u64, edited: search::Edited) -> Task<Message> {
        if edited.persist {
            self.sync_saved_from_result(run_id);
        }
        if edited.rerun {
            self.start_run(run_id)
        } else {
            Task::none()
        }
    }

    /// Writes a Result Tab's Live Search back onto its Saved Search and
    /// persists the config.
    fn sync_saved_from_result(&mut self, run_id: u64) {
        let Some((conn_id, saved_id, live)) = self.result_mut(run_id).map(|rt| {
            (
                rt.connection_id.clone(),
                rt.saved_id.clone(),
                rt.search.clone(),
            )
        }) else {
            return;
        };
        if let Some(conn) = self.config.connections.iter_mut().find(|c| c.id == conn_id)
            && let Some(saved) = conn.searches.iter_mut().find(|s| s.id == saved_id)
        {
            live.write_back(saved);
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
        if draft.is_empty() || draft == rt.search.target {
            rt.target_draft = rt.search.target.clone();
            rt.target_probe = None;
            rt.target_error = None;
            return Task::none();
        }
        rt.target_draft = draft.clone();
        rt.target_probe = Some(draft.clone());
        let conn_id = rt.connection_id.clone();

        match self.client_for(&conn_id) {
            Some(client) => {
                let candidate = draft.clone();
                Task::perform(async move { client.fields(&draft).await }, move |result| {
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
                    rt.target_draft = rt.search.target.clone();
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
        result: Result<es::FieldCaps, es::Error>,
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
            Err(es::Error::NoSuchTarget(_)) => {
                rt.target_draft = rt.search.target.clone();
                rt.target_error =
                    Some(format!("Target \u{201c}{candidate}\u{201d} does not exist"));
                return Task::none();
            }
            Err(err) => {
                rt.target_draft = rt.search.target.clone();
                rt.target_error = Some(format!("Target \u{201c}{candidate}\u{201d}: {err}"));
                return Task::none();
            }
        };

        rt.target_draft = candidate.clone();
        rt.target_error = None;
        rt.all_fields = caps.all;
        rt.sortable_fields = caps.sortable;
        let edited = rt.search.set_target(candidate) | rt.resolve_template();
        self.apply_edit(run_id, edited)
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
                        let target_changed = rt.search.target != saved.target;
                        rt.adopt(search::Live::from_saved(&saved));
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
                    match self.client_for(&conn_id) {
                        Some(client) => Task::perform(
                            async move { client.fields(&target).await },
                            move |result| Message::ResultFieldsLoaded { run_id, result },
                        ),
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
            search: search::Live::from_saved(&saved),
            target_draft: saved.target.clone(),
            target_probe: None,
            target_error: None,
            target_options: Vec::new(),
            targets_loading: true,
            target_panel_open: false,
            query_draft: saved.query_string.clone(),
            column_draft: String::new(),
            col_widths: HashMap::new(),
            sort_panel_open: false,
            header_menu: None,
            all_fields,
            sortable_fields,
            tf: TimeframeDraft::from_timeframe(&saved.timeframe),
            gte,
            lte,
            hits: Vec::new(),
            state: RunState::Loading,
            refreshing: false,
            scroll_id: Id::unique(),
            paging: Paging::Idle,
            total_hits: TotalHits::Loading,
            generation: 0,
            scroll_y: 0.0,
            viewport_h: 600.0,
            scroll_x: 0.0,
            viewport_w: 1200.0,
            selected_hit: None,
            detail_content: text_editor::Content::new(),
            detail_height: results::DETAIL_DEFAULT_H,
            utc: self.config.utc_timestamps,
            max_results: self.config.es.max_results,
            fetch_size: self.config.es.fetch_size,
            template_draft: saved.template.clone(),
            format_open: false,
            line_cache: Default::default(),
            run: None,
        };
        // Resolve the raw text template up front if the field list is already
        // known; otherwise it is resolved lazily when `ResultFieldsLoaded`
        // lands (see that handler).
        tab.resolve_template();
        let need_fields = tab.all_fields.is_empty();
        let target = tab.search.target.clone();

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

        let client = self.client_for(&conn_id);

        let fetch_fields: Task<Message> = match (&client, need_fields) {
            (Some(client), true) => {
                let client = client.clone();
                Task::perform(async move { client.fields(&target).await }, move |result| {
                    Message::ResultFieldsLoaded { run_id, result }
                })
            }
            _ => Task::none(),
        };

        // Populate the Search bar's Target suggestion dropdown for this tab.
        let fetch_targets: Task<Message> = match client {
            Some(client) => Task::perform(async move { client.targets().await }, move |targets| {
                Message::ResultTargetsLoaded { run_id, targets }
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

    /// Freshens the range, then starts a new Run: its first Page, and beside
    /// it the `_count` of everything the Query matches.
    fn start_run(&mut self, run_id: u64) -> Task<Message> {
        let Some((conn_id, generation, query, limits)) = self.result_mut(run_id).map(|rt| {
            // If this tab already had a table up, keep the strips pinned and
            // the previous rows on screen while the re-run is in flight, so
            // nothing flickers. The old Hits are swapped out wholesale when
            // the new first Page lands (see `PageLoaded`).
            rt.refreshing = matches!(rt.state, RunState::Loaded | RunState::Empty);
            rt.state = RunState::Loading;
            rt.selected_hit = None;
            // The previous Run is dead the moment a new one starts; the tab
            // gets the new one when its first Page lands.
            rt.run = None;
            if !rt.refreshing {
                rt.hits.clear();
                rt.reset_line_cache();
                rt.paging = Paging::Idle;
                rt.scroll_y = 0.0;
            }
            // Re-resolve the range so a relative window re-anchors to "now".
            let (gte, lte) = rt.search.timeframe.bounds();
            rt.gte = gte;
            rt.lte = lte;
            rt.total_hits = TotalHits::Loading;
            rt.generation += 1;
            (
                rt.connection_id.clone(),
                rt.generation,
                rt.query(),
                rt.limits(),
            )
        }) else {
            return Task::none();
        };

        // Scroll-perf harness: take the Hits from a saved `_search` response on
        // disk instead of from a cluster, so a run is byte-identical every time
        // and needs no Elasticsearch. See `src/perf.rs`.
        if let Some(path) = perf::fixture_path() {
            let run = es::Run::fixture(path, perf::hits_repeat());
            return first_page(run_id, generation, run);
        }

        let Some(client) = self.client_for(&conn_id) else {
            let Some(conn) = self.connection(&conn_id) else {
                return Task::none();
            };
            let name = conn.name.clone();
            self.secret_prompt = Some(SecretPrompt {
                connection_id: conn_id,
                connection_name: name,
                value: String::new(),
                then: PendingAction::RunSearch { run_id },
            });
            return Task::none();
        };

        let total = {
            let (client, query) = (client.clone(), query.clone());
            Task::perform(async move { client.total(&query).await }, move |result| {
                Message::TotalHitsLoaded {
                    run_id,
                    generation,
                    result,
                }
            })
        };
        Task::batch([
            total,
            first_page(run_id, generation, es::Run::live(client, query, limits)),
        ])
    }

    /// Fetches the next Page of a Result Tab's Run. A no-op unless the tab has
    /// a Run to advance — which it has not while a Page is already in flight.
    fn load_more(&mut self, run_id: u64) -> Task<Message> {
        let Some((generation, run)) = self.result_mut(run_id).and_then(|rt| {
            let run = rt.run.take()?;
            rt.paging = Paging::Loading;
            Some((rt.generation, run))
        }) else {
            return Task::none();
        };

        Task::perform(run.next_page(), move |advance| Message::PageLoaded {
            run_id,
            generation,
            advance: Box::new(advance),
            append: true,
        })
    }

    // --- View --------------------------------------------------------------

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        let _span = perf::span("view");
        if Some(window) == self.settings_window {
            return ui::settings::view(&self.settings_draft);
        }
        self.main_view()
    }

    fn main_view(&self) -> Element<'_, Message> {
        // Right column, top to bottom: an optional Search bar, then an optional
        // options strip, then the tab strip sitting directly above the main
        // area. The two optional strips only appear while a Result Tab is
        // active.
        let mut right: Vec<Element<'_, Message>> = Vec::new();
        let active = self.active_result();
        let search_bar = active.map(ui::search_bar::search_bar);
        let options_bar = active.and_then(ui::search_bar::options_bar);
        let (has_search_bar, has_options_bar) = (search_bar.is_some(), options_bar.is_some());
        if let Some(search_bar) = search_bar {
            right.push(search_bar);
            right.push(rule::horizontal(1.0).into());
        }
        if let Some(options_bar) = options_bar {
            right.push(options_bar);
            right.push(rule::horizontal(1.0).into());
        }
        right.push(ui::menu::tab_bar(&self.open_tabs, self.active_tab));
        right.push(rule::horizontal(1.0).into());
        right.push(self.main_area());

        let body = row![
            ui::tree::sidebar(
                &self.config.connections,
                &self.expanded,
                active.map(|rt| rt.saved_id.as_str()),
            ),
            rule::vertical(1.0),
            column(right).width(Fill),
        ]
        .height(Fill);

        // Built as a Vec rather than a `column!` so the Update banner and the
        // rule beneath it appear together or not at all.
        let mut frame: Vec<Element<'_, Message>> = vec![
            ui::menu::bar(self.file_menu_open, self.help_menu_open),
            rule::horizontal(1.0).into(),
        ];
        let mut banner_h = None;
        if let Some((banner, height)) = ui::menu::update_banner(
            self.new_release.as_ref(),
            self.updating.as_ref(),
            &self.flavour,
        ) {
            frame.push(banner);
            frame.push(rule::horizontal(1.0).into());
            banner_h = Some(height);
        }
        frame.push(container(body).width(Fill).height(Fill).into());
        frame.push(ui::menu::status_bar(self.status.as_deref()));
        frame.push(rule::horizontal(1.0).into());
        frame.push(ui::menu::info_bar(active, self.spinner_frame));

        // Everything above is the fixed chrome; every overlay below is a layer
        // over the whole window that has to be told where that chrome ended.
        let metrics = Chrome::new(banner_h, has_search_bar, has_options_bar);

        let base: Element<'_, Message> = container(column(frame))
            .style(|_| style::panel(BG))
            .width(Fill)
            .height(Fill)
            .into();

        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if let Some(menu) = ui::menu::file_overlay(self.file_menu_open, &metrics) {
            layers.push(menu);
        }
        if let Some(menu) =
            ui::menu::help_overlay(self.help_menu_open, self.checking_for_updates, &metrics)
        {
            layers.push(menu);
        }
        if let Some(menu) = ui::tree::menu_overlay(self.tree_menu.as_ref(), self.tree_menu_at) {
            layers.push(menu);
        }
        if let Some(popover) = active.and_then(|tab| ui::search_bar::sort_overlay(tab, &metrics)) {
            layers.push(popover);
        }
        if let Some(popover) =
            active.and_then(|tab| ui::search_bar::timeframe_overlay(tab, &metrics))
        {
            layers.push(popover);
        }
        if let Some(dropdown) = active.and_then(|tab| ui::search_bar::target_overlay(tab, &metrics))
        {
            layers.push(dropdown);
        }
        if let Some(form) = &self.connection_form {
            layers.push(ui::modals::connection_form(form));
        }
        if let Some(form) = &self.search_settings {
            layers.push(ui::modals::search_settings(
                form,
                self.conn_name(&form.connection_id),
            ));
        }
        if let Some(tab) = active
            && tab.format_open
            && tab.search.mode == line::LayoutMode::RawText
        {
            layers.push(ui::results::format_modal(tab));
        }
        if let Some(prompt) = &self.secret_prompt {
            layers.push(ui::modals::secret_prompt(prompt));
        }
        if let Some(confirm) = &self.confirm {
            layers.push(ui::modals::confirm(confirm));
        }
        if self.about_open {
            layers.push(ui::menu::about_modal());
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
            Some(Tab::SearchForm(form)) => ui::modals::search_form(
                form,
                self.conn_name(&form.connection_id),
                self.active_tab.unwrap_or(0),
            ),
            Some(Tab::Result(tab)) => ui::results::result_view(
                tab,
                self.header_hover,
                self.grip_hover,
                self.column_drag.as_ref(),
                self.config.wrap_row_cap,
            ),
            None => centered("Open a Saved Search from the sidebar", TEXT_DIM),
        }
    }
}

// --- Small view helpers ----------------------------------------------------

/// Runs one Update check, tagging the result with why it ran so that
/// [`update::outcome`] can decide what may be shown.
fn update_check_task(trigger: update::Trigger) -> Task<Message> {
    Task::perform(update::check(), move |result| Message::UpdateCheckDone {
        trigger,
        result,
    })
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

/// Folds a fetched Page into a Result Tab: replacing Hits on a first run,
/// appending on a scroll-driven load-more, and settling the paging state.
/// Kicks off a Run's first Page.
fn first_page(run_id: u64, generation: u64, run: es::Run) -> Task<Message> {
    Task::perform(run.next_page(), move |advance| Message::PageLoaded {
        run_id,
        generation,
        advance: Box::new(advance),
        append: false,
    })
}

fn apply_page(rt: &mut ResultTab, advance: es::Advance, append: bool) {
    let es::Advance { run, hits, state } = advance;
    rt.run = Some(run);

    if let es::State::Failed(err) = state {
        if append {
            // Leave already-loaded Hits untouched; offer a retry.
            rt.paging = Paging::Failed(err);
        } else {
            rt.refreshing = false;
            rt.state = RunState::Error(err.to_string());
        }
        return;
    }

    if append {
        // Existing positions keep their Hit; new ones are absent from the
        // cache until first rendered — no reset needed.
        rt.hits.extend(hits);
    } else {
        rt.refreshing = false;
        rt.hits = hits;
        rt.reset_line_cache();
        rt.state = if rt.hits.is_empty() {
            RunState::Empty
        } else {
            RunState::Loaded
        };
    }

    rt.paging = match state {
        es::State::Exhausted => Paging::Exhausted,
        es::State::Capped => Paging::Capped,
        _ => Paging::Idle,
    };
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
