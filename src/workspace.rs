//! The open document set: the Connections and Saved Searches on disk, the tabs
//! showing them, and which one is in front.
//!
//! These five pieces of state are never touched apart. Closing a tab shifts
//! `active_tab`; deleting a Connection closes its tabs and forgets that it was
//! expanded; saving a Saved Search writes `config` and then opens a tab for it.
//! Held as separate fields on `LogLens` they were five things any of its
//! seventy-odd `update` arms could reach; held here they are one thing with a
//! stated set of moves.
//!
//! What makes that worth doing is what these moves *don't* need. None of them
//! wants an [`es::Client`](crate::es), a `Task`, or a window id — so unlike
//! almost everything else `LogLens` does, they can be run in a test. The tab
//! arithmetic below (which tab comes forward when the one in front closes,
//! which tabs a deleted Connection takes with it) is exactly the kind that is
//! invisible until it is wrong, and it is checked at the bottom of this file.
//!
//! Deliberately left on `LogLens`: the memoized clients, the keyring, the
//! modals and drags, and every `Task`. A Workspace is what the app is showing,
//! not what it is doing.

use std::collections::HashSet;

use crate::config::{self, Config, Connection, SavedSearch};
use crate::results::ResultTab;
use crate::search::SearchForm;
use crate::tab::Tab;

/// Pseudo tree-node name for the Elasticsearch root, tracked in the `expanded`
/// set like a folder. The control char keeps it from colliding with a real
/// Connection name. Expanded at boot.
pub const ES_ROOT: &str = "\u{1}Elasticsearch";

pub struct Workspace {
    pub config: Config,
    /// Open tabs, in tab order.
    pub open_tabs: Vec<Tab>,
    /// Index into `open_tabs`.
    pub active_tab: Option<usize>,
    /// Sidebar nodes currently expanded, by Connection id (plus [`ES_ROOT`]).
    pub expanded: HashSet<String>,
    /// Source of stable ids for Search forms and Result Tabs.
    id_seq: u64,
}

impl Workspace {
    /// A Workspace showing nothing, over the Connections a config carries.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            open_tabs: Vec::new(),
            active_tab: None,
            expanded: HashSet::from([ES_ROOT.to_string()]),
            id_seq: 0,
        }
    }

    /// The next stable id for a Search form or Result Tab. Never repeats within
    /// a session, which is what lets an async result find the tab that asked
    /// for it even after the tabs either side of it have closed.
    pub fn next_id(&mut self) -> u64 {
        self.id_seq += 1;
        self.id_seq
    }

    /// Writes the config to disk, handing back the reason it could not be.
    ///
    /// Every mutation below leaves this to the caller rather than persisting
    /// itself: a save is what the *user* asked for at a particular moment, and
    /// one of them ([`LogLens::record_update_check`]) deliberately says nothing
    /// when it fails. Folding the write into the edits would hide that
    /// difference.
    ///
    /// [`LogLens::record_update_check`]: crate::LogLens
    pub fn save(&self) -> Result<(), String> {
        config::save(&self.config)
    }

    // --- Connections and Saved Searches ---------------------------------

    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.config.connections.iter().find(|c| c.id == id)
    }

    pub fn connection_mut(&mut self, id: &str) -> Option<&mut Connection> {
        self.config.connections.iter_mut().find(|c| c.id == id)
    }

    /// A Connection's display name, for the forms that show which cluster they
    /// are editing against. Empty when the Connection has been deleted out from
    /// under an open form — the name is a subtitle, not the subject, so a blank
    /// one is better there than an error.
    pub fn conn_name(&self, id: &str) -> &str {
        self.connection(id).map_or("", |c| c.name.as_str())
    }

    pub fn saved(&self, conn_id: &str, search_id: &str) -> Option<&SavedSearch> {
        self.connection(conn_id)?
            .searches
            .iter()
            .find(|s| s.id == search_id)
    }

    pub fn saved_mut(&mut self, conn_id: &str, search_id: &str) -> Option<&mut SavedSearch> {
        self.connection_mut(conn_id)?
            .searches
            .iter_mut()
            .find(|s| s.id == search_id)
    }

    /// Opens a collapsed sidebar node, or collapses an open one.
    pub fn toggle_folder(&mut self, name: String) {
        if !self.expanded.remove(&name) {
            self.expanded.insert(name);
        }
    }

    /// Makes sure a sidebar node is showing, so something just added under it
    /// is visible without a click.
    pub fn expand(&mut self, name: &str) {
        self.expanded.insert(name.to_string());
    }

    /// Adds a Connection, or replaces the one with the same id in place so it
    /// keeps its position in the sidebar.
    pub fn upsert_connection(&mut self, connection: Connection) {
        match self.connection_mut(&connection.id) {
            Some(existing) => *existing = connection,
            None => self.config.connections.push(connection),
        }
    }

    /// Adds a Saved Search to a Connection, or replaces the one with the same
    /// id. No-op if the Connection is gone.
    pub fn upsert_search(&mut self, conn_id: &str, saved: SavedSearch) {
        let Some(conn) = self.connection_mut(conn_id) else {
            return;
        };
        match conn.searches.iter_mut().find(|s| s.id == saved.id) {
            Some(existing) => *existing = saved,
            None => conn.searches.push(saved),
        }
    }

    /// Forgets a Saved Search and closes everything showing it — its Result Tab
    /// and the form tab that created it.
    ///
    /// Does not touch the Search settings modal, which is not a tab; the caller
    /// closes that if it was editing this Saved Search.
    pub fn delete_search(&mut self, conn_id: &str, search_id: &str) {
        self.close_tabs(|tab| match tab {
            Tab::Result(rt) => rt.saved_id == search_id,
            Tab::SearchForm(f) => f.saved_id.as_deref() == Some(search_id),
        });
        if let Some(conn) = self.connection_mut(conn_id) {
            conn.searches.retain(|s| s.id != search_id);
        }
    }

    /// Forgets a Connection, everything saved under it, and every tab showing
    /// any of it.
    ///
    /// The keyring entry and the memoized client are the caller's to drop —
    /// they are not part of the document set.
    pub fn delete_connection(&mut self, conn_id: &str) {
        self.close_connection_tabs(conn_id);
        self.config.connections.retain(|c| c.id != conn_id);
        self.expanded.remove(conn_id);
    }

    // --- Tabs -----------------------------------------------------------

    /// The active tab, when it is a Result Tab.
    ///
    /// Most of the chrome is a function of exactly this: the Search bar, the
    /// options strip, their three overlays, the info bar's Hit-count readout,
    /// and the Format modal all read one `ResultTab` and nothing else. Written
    /// once here so those surfaces can be free functions over a `&ResultTab`.
    pub fn active_result(&self) -> Option<&ResultTab> {
        match self.active_tab.and_then(|t| self.open_tabs.get(t))? {
            Tab::Result(tab) => Some(tab),
            Tab::SearchForm(_) => None,
        }
    }

    pub fn active_result_mut(&mut self) -> Option<&mut ResultTab> {
        match self.active_tab.and_then(|t| self.open_tabs.get_mut(t))? {
            Tab::Result(tab) => Some(tab),
            Tab::SearchForm(_) => None,
        }
    }

    /// Every open Result Tab, in tab order — for the handful of things that
    /// are true of all of them at once: whether any is still counting, and
    /// pushing changed fetch limits onto each.
    pub fn results(&self) -> impl Iterator<Item = &ResultTab> {
        self.open_tabs.iter().filter_map(|t| match t {
            Tab::Result(rt) => Some(rt.as_ref()),
            Tab::SearchForm(_) => None,
        })
    }

    pub fn results_mut(&mut self) -> impl Iterator<Item = &mut ResultTab> {
        self.open_tabs.iter_mut().filter_map(|t| match t {
            Tab::Result(rt) => Some(rt.as_mut()),
            Tab::SearchForm(_) => None,
        })
    }

    pub fn result_mut(&mut self, run_id: u64) -> Option<&mut ResultTab> {
        self.open_tabs.iter_mut().find_map(|t| match t {
            Tab::Result(rt) if rt.run_id == run_id => Some(rt.as_mut()),
            _ => None,
        })
    }

    /// Where the Result Tab for a Saved Search is, if one is open. At most one
    /// ever is — [`Self::place`]'s callers focus this instead of opening a
    /// second.
    pub fn result_tab_for(&self, saved_id: &str) -> Option<usize> {
        self.open_tabs
            .iter()
            .position(|t| matches!(t, Tab::Result(rt) if rt.saved_id == saved_id))
    }

    /// A Search *form* tab by its id. The Search settings modal shares the
    /// `SearchForm` type but is not a tab, so it is not found here; see
    /// `LogLens::form_mut`.
    pub fn form_tab_mut(&mut self, form_id: u64) -> Option<&mut SearchForm> {
        self.open_tabs.iter_mut().find_map(|t| match t {
            Tab::SearchForm(f) if f.form_id == form_id => Some(f.as_mut()),
            _ => None,
        })
    }

    pub fn active_form_mut(&mut self) -> Option<&mut SearchForm> {
        match self.active_tab.and_then(|t| self.open_tabs.get_mut(t)) {
            Some(Tab::SearchForm(f)) => Some(f.as_mut()),
            _ => None,
        }
    }

    /// Brings a tab to the front. Out-of-range indices are ignored rather than
    /// clearing the selection — they mean a stale click, not "select nothing".
    pub fn focus(&mut self, tab: usize) {
        if tab < self.open_tabs.len() {
            self.active_tab = Some(tab);
        }
    }

    /// Puts a tab in the slot `replace` names — the Search form it was created
    /// from — or on the end, and brings it to the front. Returns where it went.
    pub fn place(&mut self, tab: Tab, replace: Option<usize>) -> usize {
        let at = match replace {
            Some(i) if i < self.open_tabs.len() => {
                self.open_tabs[i] = tab;
                i
            }
            _ => {
                self.open_tabs.push(tab);
                self.open_tabs.len() - 1
            }
        };
        self.active_tab = Some(at);
        at
    }

    /// Closes one tab, keeping whichever tab was in front in front.
    ///
    /// Closing the active tab hands the front to its right-hand neighbour, or
    /// to the new last tab when it had none.
    pub fn close_tab(&mut self, tab: usize) {
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

    /// Closes every tab — Result or Search form — belonging to a Connection.
    pub fn close_connection_tabs(&mut self, conn_id: &str) {
        self.close_tabs(|tab| match tab {
            Tab::Result(rt) => rt.connection_id == conn_id,
            Tab::SearchForm(f) => f.connection_id == conn_id,
        });
    }

    /// Closes every tab matching `doomed`, one at a time, so each close gets
    /// the same front-tab treatment a user closing them by hand would.
    fn close_tabs(&mut self, doomed: impl Fn(&Tab) -> bool) {
        while let Some(pos) = self.open_tabs.iter().position(&doomed) {
            self.close_tab(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Auth, EsSettings, SortKey, TimeUnit, Timeframe};
    use crate::line::LayoutMode;

    fn saved(id: &str, name: &str) -> SavedSearch {
        SavedSearch {
            id: id.to_string(),
            name: name.to_string(),
            target: "logs-nginx".to_string(),
            query_string: String::new(),
            timeframe: Timeframe::Relative {
                amount: 15,
                unit: TimeUnit::Minutes,
            },
            timestamp_field: "@timestamp".to_string(),
            columns: vec!["@timestamp".to_string(), "message".to_string()],
            sort: vec![SortKey::new("@timestamp", true)],
            mode: LayoutMode::Table,
            template: String::new(),
            wrap: false,
        }
    }

    fn conn(id: &str, searches: Vec<SavedSearch>) -> Connection {
        Connection {
            id: id.to_string(),
            name: format!("Cluster {id}"),
            url: "http://localhost:9200".to_string(),
            auth: Auth::None,
            skip_tls_verify: false,
            searches,
        }
    }

    /// Two Connections, one with two Saved Searches and one with a third, and
    /// nothing open.
    fn workspace() -> Workspace {
        Workspace::new(Config {
            connections: vec![
                conn("c1", vec![saved("s1", "Nginx"), saved("s2", "App")]),
                conn("c2", vec![saved("s3", "Audit")]),
            ],
            ..Config::default()
        })
    }

    fn result_tab(ws: &mut Workspace, conn_id: &str, search_id: &str) -> u64 {
        let saved = ws.saved(conn_id, search_id).cloned().expect("saved search");
        let run_id = ws.next_id();
        let tab = ResultTab::new(
            run_id,
            conn_id.to_string(),
            &saved,
            None,
            EsSettings::default(),
            false,
        );
        ws.place(Tab::Result(Box::new(tab)), None);
        run_id
    }

    fn search_form(ws: &mut Workspace, conn_id: &str) -> u64 {
        let form_id = ws.next_id();
        ws.place(
            Tab::SearchForm(Box::new(SearchForm::new(form_id, conn_id.to_string()))),
            None,
        );
        form_id
    }

    /// Titles of the open tabs, which is what the tab strip shows and so the
    /// clearest way to say what survived a close.
    fn titles(ws: &Workspace) -> Vec<String> {
        ws.open_tabs.iter().map(Tab::title).collect()
    }

    // --- Ids ------------------------------------------------------------

    /// Ids address tabs across the async gap: a `_search` that comes back after
    /// the tab that asked for it closed must not find whatever took its place.
    #[test]
    fn ids_are_never_handed_out_twice() {
        let mut ws = workspace();
        let first = ws.next_id();
        let run_id = result_tab(&mut ws, "c1", "s1");
        ws.close_tab(0);
        let after = ws.next_id();

        assert_ne!(first, run_id);
        assert_ne!(run_id, after);
        assert!(after > run_id, "ids only ever go up");
    }

    // --- Closing tabs ---------------------------------------------------

    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_same_tab_in_front() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");
        result_tab(&mut ws, "c2", "s3");
        ws.focus(2);

        ws.close_tab(0);

        assert_eq!(titles(&ws), ["App", "Audit"]);
        assert_eq!(
            ws.active_result().map(|rt| rt.search.name.as_str()),
            Some("Audit"),
            "the tab in front is still the same tab, at its new index"
        );
    }

    #[test]
    fn closing_a_tab_right_of_the_active_one_leaves_the_index_alone() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");
        ws.focus(0);

        ws.close_tab(1);

        assert_eq!(ws.active_tab, Some(0));
    }

    #[test]
    fn closing_the_active_tab_hands_the_front_to_its_right_hand_neighbour() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");
        result_tab(&mut ws, "c2", "s3");
        ws.focus(1);

        ws.close_tab(1);

        assert_eq!(titles(&ws), ["Nginx", "Audit"]);
        assert_eq!(
            ws.active_result().map(|rt| rt.search.name.as_str()),
            Some("Audit"),
        );
    }

    /// The right-hand neighbour rule has nothing to fall back on at the end of
    /// the strip, so the tab before it comes forward instead.
    #[test]
    fn closing_the_active_last_tab_falls_back_to_the_new_last() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");
        ws.focus(1);

        ws.close_tab(1);

        assert_eq!(ws.active_tab, Some(0));
    }

    #[test]
    fn closing_the_only_tab_leaves_nothing_in_front() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");

        ws.close_tab(0);

        assert!(ws.open_tabs.is_empty());
        assert_eq!(ws.active_tab, None);
    }

    /// A close that arrives for a tab that is already gone. Ignoring it is not
    /// the same as ignoring the selection: the tab in front must stay in front.
    #[test]
    fn closing_a_tab_that_is_not_there_changes_nothing() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        ws.focus(0);

        ws.close_tab(7);

        assert_eq!(titles(&ws), ["Nginx"]);
        assert_eq!(ws.active_tab, Some(0));
    }

    // --- Deleting --------------------------------------------------------

    #[test]
    fn deleting_a_connection_takes_its_tabs_and_leaves_the_others() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c2", "s3");
        search_form(&mut ws, "c1");
        ws.expand("c1");

        ws.delete_connection("c1");

        assert_eq!(titles(&ws), ["Audit"]);
        assert!(ws.connection("c1").is_none());
        assert!(ws.connection("c2").is_some());
        assert!(
            !ws.expanded.contains("c1"),
            "a Connection that is gone is not an expanded folder"
        );
    }

    #[test]
    fn deleting_a_saved_search_closes_its_result_tab_and_keeps_its_siblings() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");

        ws.delete_search("c1", "s1");

        assert_eq!(titles(&ws), ["App"]);
        assert!(ws.saved("c1", "s1").is_none());
        assert!(ws.saved("c1", "s2").is_some());
    }

    /// The form tab that created a Saved Search keeps pointing at it by
    /// `saved_id`, so it has to go with it.
    #[test]
    fn deleting_a_saved_search_closes_the_form_tab_that_was_editing_it() {
        let mut ws = workspace();
        let form_id = search_form(&mut ws, "c1");
        ws.form_tab_mut(form_id).expect("form").saved_id = Some("s1".to_string());
        result_tab(&mut ws, "c1", "s2");

        ws.delete_search("c1", "s1");

        assert_eq!(titles(&ws), ["App"]);
    }

    /// Deleting a Connection out from under an open form is allowed; the form
    /// showing its name just stops naming it.
    #[test]
    fn a_deleted_connection_has_a_blank_name_rather_than_no_answer() {
        let mut ws = workspace();
        assert_eq!(ws.conn_name("c1"), "Cluster c1");

        ws.delete_connection("c1");

        assert_eq!(ws.conn_name("c1"), "");
    }

    // --- Placing ---------------------------------------------------------

    #[test]
    fn a_saved_search_has_at_most_one_result_tab() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        result_tab(&mut ws, "c1", "s2");

        assert_eq!(ws.result_tab_for("s1"), Some(0));
        assert_eq!(ws.result_tab_for("s2"), Some(1));
        assert_eq!(ws.result_tab_for("s3"), None);
    }

    /// Saving a Search form turns that tab into the Result Tab rather than
    /// leaving a spent form behind it.
    #[test]
    fn a_result_tab_placed_over_a_form_takes_its_slot() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        let form = ws.open_tabs.len();
        search_form(&mut ws, "c1");
        result_tab(&mut ws, "c2", "s3");

        let saved = ws.saved("c1", "s2").cloned().expect("saved search");
        let run_id = ws.next_id();
        let tab = ResultTab::new(
            run_id,
            "c1".to_string(),
            &saved,
            None,
            EsSettings::default(),
            false,
        );
        let at = ws.place(Tab::Result(Box::new(tab)), Some(form));

        assert_eq!(at, form);
        assert_eq!(titles(&ws), ["Nginx", "App", "Audit"]);
        assert_eq!(ws.active_tab, Some(form), "the new tab comes forward");
    }

    /// `replace` is an index into a tab strip that may have shrunk since it was
    /// taken — the form's neighbour closed while its `_field_caps` was in
    /// flight. One past the end is the near miss that matters: it appends
    /// rather than indexing off the end.
    #[test]
    fn a_replace_index_one_past_the_end_appends() {
        let mut ws = workspace();
        result_tab(&mut ws, "c1", "s1");
        let form = ws.open_tabs.len();
        search_form(&mut ws, "c1");
        ws.close_tab(0);
        assert_eq!(form, ws.open_tabs.len(), "the slot is now one off the end");

        let saved = ws.saved("c1", "s2").cloned().expect("saved search");
        let run_id = ws.next_id();
        let tab = ResultTab::new(
            run_id,
            "c1".to_string(),
            &saved,
            None,
            EsSettings::default(),
            false,
        );
        let at = ws.place(Tab::Result(Box::new(tab)), Some(form));

        assert_eq!(at, 1);
        assert_eq!(titles(&ws), ["New Search", "App"]);
    }

    // --- Editing the config ----------------------------------------------

    #[test]
    fn saving_a_connection_that_exists_replaces_it_where_it_stands() {
        let mut ws = workspace();
        let mut edited = conn("c1", vec![saved("s1", "Nginx"), saved("s2", "App")]);
        edited.name = "Renamed".to_string();

        ws.upsert_connection(edited);

        assert_eq!(ws.config.connections.len(), 2);
        assert_eq!(ws.config.connections[0].name, "Renamed");
        assert_eq!(ws.config.connections[1].id, "c2");
    }

    #[test]
    fn saving_a_new_connection_adds_it_at_the_end() {
        let mut ws = workspace();

        ws.upsert_connection(conn("c3", Vec::new()));

        let ids: Vec<_> = ws
            .config
            .connections
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["c1", "c2", "c3"]);
    }

    #[test]
    fn saving_a_search_edits_the_one_with_that_id_rather_than_adding_a_second() {
        let mut ws = workspace();

        ws.upsert_search("c1", saved("s1", "Nginx 5xx"));

        let names: Vec<_> = ws
            .connection("c1")
            .expect("connection")
            .searches
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Nginx 5xx", "App"]);
    }

    /// A Saved Search can outlive the form that was writing it — the
    /// Connection may have been deleted from the sidebar meanwhile. It is
    /// dropped rather than resurrecting the Connection.
    #[test]
    fn saving_a_search_under_a_connection_that_is_gone_does_nothing() {
        let mut ws = workspace();
        ws.delete_connection("c1");

        ws.upsert_search("c1", saved("s9", "Orphan"));

        assert!(ws.connection("c1").is_none());
        assert_eq!(ws.config.connections.len(), 1);
    }

    // --- Sidebar ----------------------------------------------------------

    #[test]
    fn a_folder_toggles_open_and_shut_but_expanding_twice_is_still_open() {
        let mut ws = workspace();
        assert!(ws.expanded.contains(ES_ROOT), "the root starts open");

        ws.toggle_folder("c1".to_string());
        assert!(ws.expanded.contains("c1"));

        ws.expand("c1");
        assert!(ws.expanded.contains("c1"));

        ws.toggle_folder("c1".to_string());
        assert!(!ws.expanded.contains("c1"));
    }
}
