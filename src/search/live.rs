//! A Saved Search's content while it is being edited, and the rules about
//! editing it.
//!
//! [`Live`] holds exactly what [`SavedSearch`] holds apart from its identity,
//! and every edit answers the two questions its caller would otherwise have to
//! answer itself: does this have to be written back to the config, and does it
//! invalidate the Run? That answer is [`Edited`], and getting it right is the
//! whole point of this module — the Search bar edits a Saved Search from a
//! dozen different controls, and before this the rule was written out once per
//! control.

use crate::config::{SavedSearch, SortKey, Timeframe};
use crate::es;
use crate::line::{Layout, LayoutMode};

/// What an edit to a [`Live`] Search obliges its caller to do next.
///
/// Combine several edits with `|` when one action makes more than one change:
/// the obligations add up, they never cancel out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edited {
    /// The Saved Search changed and must be written back to the config.
    pub persist: bool,
    /// The change alters which Hits the cluster would return, so the Run is
    /// stale and a new one must start.
    pub rerun: bool,
}

impl Edited {
    /// Nothing changed. No write, no Run.
    pub const NONE: Edited = Edited {
        persist: false,
        rerun: false,
    };

    /// A change the cluster does not care about — a Column, the Layout mode,
    /// Wrap, the template. Columns are a projection over a Hit's `_source`, so
    /// the loaded Hits still answer; only the render changes.
    pub const DISPLAY: Edited = Edited {
        persist: true,
        rerun: false,
    };

    /// A change to what the cluster is asked for — Target, query string,
    /// Timeframe, sort. The sort belongs here because it is also the total
    /// order `search_after` pages along (ADR 0002), so re-sorting is a new Run
    /// rather than a re-render.
    pub const QUERY: Edited = Edited {
        persist: true,
        rerun: true,
    };

    /// `edit` when `changed`, otherwise [`Edited::NONE`].
    fn when(changed: bool, edit: Edited) -> Edited {
        if changed { edit } else { Edited::NONE }
    }
}

impl std::ops::BitOr for Edited {
    type Output = Edited;

    fn bitor(self, other: Edited) -> Edited {
        Edited {
            persist: self.persist | other.persist,
            rerun: self.rerun | other.rerun,
        }
    }
}

/// The Saved Search a Result Tab is currently showing: the runtime copy the
/// Search bar edits, written back to the persisted one after every change.
///
/// Its identity — which Saved Search, on which Connection, in which tab —
/// stays on the Result Tab, as does everything the user is only part-way
/// through typing. This is the committed content and nothing else.
#[derive(Debug, Clone)]
pub struct Live {
    pub name: String,
    pub target: String,
    /// Empty matches everything.
    pub query_string: String,
    pub timestamp_field: String,
    pub columns: Vec<String>,
    /// Highest priority first. Empty falls back to `timestamp_field`
    /// descending — `es` applies that default.
    pub sort: Vec<SortKey>,
    pub timeframe: Timeframe,
    pub mode: LayoutMode,
    pub wrap: bool,
    /// Raw text mode's template. Empty until [`Live::resolve_template`] fills
    /// it in from the Target's field list.
    pub template: String,
}

impl Live {
    pub fn from_saved(saved: &SavedSearch) -> Live {
        Live {
            name: saved.name.clone(),
            target: saved.target.clone(),
            query_string: saved.query_string.clone(),
            timestamp_field: saved.timestamp_field.clone(),
            columns: saved.columns.clone(),
            sort: saved.sort.clone(),
            timeframe: saved.timeframe.clone(),
            mode: saved.mode,
            wrap: saved.wrap,
            template: saved.template.clone(),
        }
    }

    /// Writes back the eight fields the Search bar owns.
    ///
    /// `id`, `name` and `timestamp_field` are deliberately left alone: they are
    /// the Search settings, editable only from their own modal. A Result Tab
    /// carries a copy of the latter two to run and title itself with, never to
    /// write.
    pub fn write_back(&self, saved: &mut SavedSearch) {
        saved.target = self.target.clone();
        saved.query_string = self.query_string.clone();
        saved.timeframe = self.timeframe.clone();
        saved.columns = self.columns.clone();
        saved.sort = self.sort.clone();
        saved.mode = self.mode;
        saved.wrap = self.wrap;
        saved.template = self.template.clone();
    }

    /// What to ask the cluster for, over the range frozen at the start of the
    /// Run.
    pub fn query(&self, gte: &str, lte: &str) -> es::Query {
        es::Query {
            target: self.target.clone(),
            query_string: self.query_string.clone(),
            timestamp_field: self.timestamp_field.clone(),
            gte: gte.to_string(),
            lte: lte.to_string(),
            sort: self
                .sort
                .iter()
                .map(|key| (key.field.clone(), key.desc))
                .collect(),
        }
    }

    /// How to draw a Hit: the persisted Layout plus the two runtime values
    /// (the timestamp field and the UTC preference) that are not part of it.
    pub fn layout(&self, utc: bool) -> Layout {
        Layout {
            mode: self.mode,
            columns: self.columns.clone(),
            template: self.template.clone(),
            timestamp_field: self.timestamp_field.clone(),
            utc,
        }
    }

    /// This field's position in the sort order, if it is sorted on.
    pub fn sort_index(&self, field: &str) -> Option<usize> {
        self.sort.iter().position(|key| key.field == field)
    }

    // --- Query edits -------------------------------------------------------

    pub fn set_target(&mut self, target: String) -> Edited {
        if self.target == target {
            return Edited::NONE;
        }
        self.target = target;
        Edited::QUERY
    }

    pub fn set_query_string(&mut self, query_string: String) -> Edited {
        if self.query_string == query_string {
            return Edited::NONE;
        }
        self.query_string = query_string;
        Edited::QUERY
    }

    pub fn set_timeframe(&mut self, timeframe: Timeframe) -> Edited {
        if self.timeframe == timeframe {
            return Edited::NONE;
        }
        self.timeframe = timeframe;
        Edited::QUERY
    }

    /// Sets `field`'s sort direction, appending it to the order if it is not
    /// already sorted on.
    pub fn set_sort_dir(&mut self, field: &str, desc: bool) -> Edited {
        match self.sort.iter_mut().find(|key| key.field == field) {
            Some(key) if key.desc == desc => Edited::NONE,
            Some(key) => {
                key.desc = desc;
                Edited::QUERY
            }
            None => {
                self.sort.push(SortKey::new(field, desc));
                Edited::QUERY
            }
        }
    }

    /// Drops `field` from the sort order.
    pub fn remove_sort(&mut self, field: &str) -> Edited {
        let before = self.sort.len();
        self.sort.retain(|key| key.field != field);
        Edited::when(self.sort.len() != before, Edited::QUERY)
    }

    /// Moves the sort key at `index` by `delta` places.
    pub fn move_sort(&mut self, index: usize, delta: isize) -> Edited {
        Edited::when(swap_by(&mut self.sort, index, delta), Edited::QUERY)
    }

    /// Clears the sort order (Hits fall back to the timestamp field,
    /// descending).
    pub fn clear_sort(&mut self) -> Edited {
        let had = !self.sort.is_empty();
        self.sort.clear();
        Edited::when(had, Edited::QUERY)
    }

    // --- Display edits -----------------------------------------------------

    /// Appends `field` as a Column. Blank names and Columns already shown are
    /// ignored.
    pub fn add_column(&mut self, field: &str) -> Edited {
        let field = field.trim();
        if field.is_empty() || self.columns.iter().any(|col| col == field) {
            return Edited::NONE;
        }
        self.columns.push(field.to_string());
        Edited::DISPLAY
    }

    pub fn remove_column(&mut self, index: usize) -> Edited {
        if index >= self.columns.len() {
            return Edited::NONE;
        }
        self.columns.remove(index);
        Edited::DISPLAY
    }

    pub fn move_column(&mut self, index: usize, delta: isize) -> Edited {
        Edited::when(swap_by(&mut self.columns, index, delta), Edited::DISPLAY)
    }

    pub fn set_mode(&mut self, mode: LayoutMode) -> Edited {
        if self.mode == mode {
            return Edited::NONE;
        }
        self.mode = mode;
        Edited::DISPLAY
    }

    pub fn toggle_wrap(&mut self) -> Edited {
        self.wrap = !self.wrap;
        Edited::DISPLAY
    }

    /// Commits a template. An emptied one is left empty so
    /// [`Live::resolve_template`] can fill the default back in.
    pub fn set_template(&mut self, template: String) -> Edited {
        let template = template.trim().to_string();
        if self.template == template {
            return Edited::NONE;
        }
        self.template = template;
        Edited::DISPLAY
    }

    /// Fills the template in from the Target's field list the first time it is
    /// needed. Does nothing once one is set, or before the field list lands.
    pub fn resolve_template(&mut self, all_fields: &[String]) -> Edited {
        if !self.template.is_empty() || all_fields.is_empty() {
            return Edited::NONE;
        }
        self.template = Layout::default_template(all_fields);
        Edited::DISPLAY
    }
}

/// Swaps the item at `index` with the one `delta` places away, if both are in
/// range. Returns whether anything moved.
fn swap_by<T>(items: &mut [T], index: usize, delta: isize) -> bool {
    let Some(target) = index.checked_add_signed(delta) else {
        return false;
    };
    if index >= items.len() || target >= items.len() {
        return false;
    }
    items.swap(index, target);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TimeUnit;

    fn saved() -> SavedSearch {
        SavedSearch {
            id: "s1".to_string(),
            name: "Nginx".to_string(),
            target: "logs-nginx".to_string(),
            query_string: "status:500".to_string(),
            timeframe: Timeframe::Relative {
                amount: 15,
                unit: TimeUnit::Minutes,
            },
            timestamp_field: "@timestamp".to_string(),
            columns: vec!["@timestamp".to_string(), "message".to_string()],
            sort: vec![SortKey::new("@timestamp", true)],
            mode: LayoutMode::Table,
            template: "%{message}".to_string(),
            wrap: false,
        }
    }

    fn live() -> Live {
        Live::from_saved(&saved())
    }

    #[test]
    fn from_saved_then_write_back_round_trips_every_search_bar_field() {
        let mut target = SavedSearch {
            target: String::new(),
            query_string: String::new(),
            timeframe: Timeframe::default(),
            columns: Vec::new(),
            sort: Vec::new(),
            mode: LayoutMode::RawText,
            template: String::new(),
            wrap: true,
            ..saved()
        };
        live().write_back(&mut target);

        let original = saved();
        assert_eq!(target.target, original.target);
        assert_eq!(target.query_string, original.query_string);
        assert_eq!(target.timeframe, original.timeframe);
        assert_eq!(target.columns, original.columns);
        assert_eq!(target.sort, original.sort);
        assert_eq!(target.mode, original.mode);
        assert_eq!(target.wrap, original.wrap);
        assert_eq!(target.template, original.template);
    }

    #[test]
    fn write_back_leaves_the_name_and_timestamp_field_alone() {
        let mut renamed = SavedSearch {
            name: "Renamed in the modal".to_string(),
            timestamp_field: "event.created".to_string(),
            ..saved()
        };
        live().write_back(&mut renamed);

        assert_eq!(renamed.id, "s1");
        assert_eq!(renamed.name, "Renamed in the modal");
        assert_eq!(renamed.timestamp_field, "event.created");
    }

    #[test]
    fn adding_a_blank_column_changes_nothing() {
        let mut live = live();
        assert_eq!(live.add_column("   "), Edited::NONE);
        assert_eq!(live.columns.len(), 2);
    }

    #[test]
    fn adding_a_column_that_is_already_there_changes_nothing() {
        let mut live = live();
        assert_eq!(live.add_column("message"), Edited::NONE);
        assert_eq!(live.columns.len(), 2);
    }

    #[test]
    fn adding_a_column_persists_without_rerunning() {
        let mut live = live();
        assert_eq!(live.add_column(" host.name "), Edited::DISPLAY);
        assert_eq!(live.columns, ["@timestamp", "message", "host.name"]);
    }

    #[test]
    fn removing_a_column_out_of_range_changes_nothing() {
        let mut live = live();
        assert_eq!(live.remove_column(2), Edited::NONE);
        assert_eq!(live.columns.len(), 2);
    }

    #[test]
    fn moving_a_column_past_either_end_changes_nothing() {
        let mut live = live();
        assert_eq!(live.move_column(0, -1), Edited::NONE);
        assert_eq!(live.move_column(1, 1), Edited::NONE);
        assert_eq!(live.columns, ["@timestamp", "message"]);
    }

    #[test]
    fn moving_a_column_within_range_persists_without_rerunning() {
        let mut live = live();
        assert_eq!(live.move_column(0, 1), Edited::DISPLAY);
        assert_eq!(live.columns, ["message", "@timestamp"]);
    }

    #[test]
    fn setting_a_sort_direction_that_is_already_set_changes_nothing() {
        let mut live = live();
        assert_eq!(live.set_sort_dir("@timestamp", true), Edited::NONE);
    }

    #[test]
    fn setting_a_new_sort_field_appends_it_and_asks_for_a_rerun() {
        let mut live = live();
        assert_eq!(live.set_sort_dir("status", false), Edited::QUERY);
        assert_eq!(live.sort_index("status"), Some(1));
        assert!(!live.sort[1].desc);
    }

    #[test]
    fn flipping_an_existing_sort_direction_asks_for_a_rerun() {
        let mut live = live();
        assert_eq!(live.set_sort_dir("@timestamp", false), Edited::QUERY);
        assert!(!live.sort[0].desc);
    }

    #[test]
    fn removing_a_sort_field_that_is_not_sorted_on_changes_nothing() {
        let mut live = live();
        assert_eq!(live.remove_sort("status"), Edited::NONE);
        assert_eq!(live.sort.len(), 1);
    }

    #[test]
    fn clearing_the_sort_order_twice_only_changes_it_once() {
        let mut live = live();
        assert_eq!(live.clear_sort(), Edited::QUERY);
        assert_eq!(live.clear_sort(), Edited::NONE);
    }

    #[test]
    fn committing_an_unchanged_query_string_changes_nothing() {
        let mut live = live();
        assert_eq!(
            live.set_query_string("status:500".to_string()),
            Edited::NONE
        );
    }

    #[test]
    fn committing_a_new_query_string_asks_for_a_rerun() {
        let mut live = live();
        assert_eq!(
            live.set_query_string("status:404".to_string()),
            Edited::QUERY
        );
        assert_eq!(live.query_string, "status:404");
    }

    #[test]
    fn re_pointing_the_target_asks_for_a_rerun() {
        let mut live = live();
        assert_eq!(live.set_target("logs-app".to_string()), Edited::QUERY);
        assert_eq!(live.set_target("logs-app".to_string()), Edited::NONE);
    }

    #[test]
    fn setting_the_timeframe_it_already_has_changes_nothing() {
        let mut live = live();
        let same = Timeframe::Relative {
            amount: 15,
            unit: TimeUnit::Minutes,
        };
        assert_eq!(live.set_timeframe(same), Edited::NONE);

        let wider = Timeframe::Relative {
            amount: 1,
            unit: TimeUnit::Hours,
        };
        assert_eq!(live.set_timeframe(wider), Edited::QUERY);
    }

    #[test]
    fn switching_layout_mode_persists_without_rerunning() {
        let mut live = live();
        assert_eq!(live.set_mode(LayoutMode::RawText), Edited::DISPLAY);
        assert_eq!(live.set_mode(LayoutMode::RawText), Edited::NONE);
    }

    #[test]
    fn toggling_wrap_persists_without_rerunning() {
        let mut live = live();
        assert_eq!(live.toggle_wrap(), Edited::DISPLAY);
        assert!(live.wrap);
    }

    #[test]
    fn resolving_a_template_that_is_already_set_changes_nothing() {
        let mut live = live();
        let fields = vec!["@timestamp".to_string(), "message".to_string()];
        assert_eq!(live.resolve_template(&fields), Edited::NONE);
        assert_eq!(live.template, "%{message}");
    }

    #[test]
    fn resolving_a_template_with_no_fields_yet_changes_nothing() {
        let mut live = Live {
            template: String::new(),
            ..live()
        };
        assert_eq!(live.resolve_template(&[]), Edited::NONE);
        assert!(live.template.is_empty());
    }

    #[test]
    fn resolving_an_empty_template_fills_it_in_once() {
        let mut live = Live {
            template: String::new(),
            ..live()
        };
        let fields = vec!["@timestamp".to_string(), "message".to_string()];
        assert_eq!(live.resolve_template(&fields), Edited::DISPLAY);
        assert!(!live.template.is_empty());
        assert_eq!(live.resolve_template(&fields), Edited::NONE);
    }

    #[test]
    fn an_emptied_template_falls_back_to_the_computed_default() {
        let mut live = live();
        assert_eq!(live.set_template("  ".to_string()), Edited::DISPLAY);
        assert!(live.template.is_empty());

        let fields = vec!["@timestamp".to_string(), "message".to_string()];
        assert_eq!(live.resolve_template(&fields), Edited::DISPLAY);
        assert!(!live.template.is_empty());
    }

    #[test]
    fn the_query_carries_the_frozen_range_and_the_sort_order() {
        let mut live = live();
        live.set_sort_dir("status", false);
        let query = live.query("now-15m", "now");

        assert_eq!(query.target, "logs-nginx");
        assert_eq!(query.query_string, "status:500");
        assert_eq!(query.timestamp_field, "@timestamp");
        assert_eq!(query.gte, "now-15m");
        assert_eq!(query.lte, "now");
        assert_eq!(
            query.sort,
            [
                ("@timestamp".to_string(), true),
                ("status".to_string(), false)
            ]
        );
    }

    #[test]
    fn the_layout_carries_the_timestamp_field_and_the_utc_preference() {
        let layout = live().layout(true);

        assert_eq!(layout.mode, LayoutMode::Table);
        assert_eq!(layout.columns, ["@timestamp", "message"]);
        assert_eq!(layout.template, "%{message}");
        assert_eq!(layout.timestamp_field, "@timestamp");
        assert!(layout.utc);
    }

    #[test]
    fn obligations_add_up_when_edits_combine() {
        assert_eq!(Edited::DISPLAY | Edited::NONE, Edited::DISPLAY);
        assert_eq!(Edited::DISPLAY | Edited::QUERY, Edited::QUERY);
        assert_eq!(Edited::NONE | Edited::NONE, Edited::NONE);
    }
}
