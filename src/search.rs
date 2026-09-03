//! State for the Search settings surface: the structural fields of a Saved
//! Search (name, Target, timestamp field) that live outside the Search bar.
//! Shown as a form tab when creating a Saved Search and as a modal when
//! editing an existing one.

use crate::config::{self, SavedSearch, SortKey, Timeframe};
use crate::es::FieldCaps;

/// Where the form's `_field_caps` lookup stands. Prewarmed so the Result Tab it
/// opens can skip the fetch.
pub enum Fields {
    /// No Target chosen yet, or lookup not started.
    Idle,
    Loading,
    Ready(FieldCaps),
    Failed,
}

impl Fields {
    pub fn caps(&self) -> Option<&FieldCaps> {
        match self {
            Fields::Ready(caps) => Some(caps),
            _ => None,
        }
    }
}

/// The editable Search settings: a Saved Search's name, Target and timestamp
/// field. Its query string, timeframe, Columns and sort are not set here — a
/// new Saved Search gets defaults for them, tuned afterwards from the Search
/// bar.
pub struct SearchForm {
    /// Stable id so async target results find their form.
    pub form_id: u64,
    pub connection_id: String,
    /// `Some` when editing an existing Saved Search (the modal).
    pub saved_id: Option<String>,
    pub name: String,
    pub target: String,
    /// Typeahead options from `_cat/indices` + `_data_stream`.
    pub target_options: Vec<String>,
    /// Whether `_cat/indices` / `_data_stream` are still in flight.
    pub targets_loading: bool,
    pub timestamp_field: String,
    /// `_field_caps` for the current Target, prewarmed for the Result Tab.
    pub fields: Fields,
    pub error: Option<String>,
}

impl SearchForm {
    pub fn new(form_id: u64, connection_id: String) -> Self {
        Self {
            form_id,
            connection_id,
            saved_id: None,
            name: String::new(),
            target: String::new(),
            target_options: Vec::new(),
            targets_loading: true,
            timestamp_field: config::default_timestamp_field(),
            fields: Fields::Idle,
            error: None,
        }
    }

    /// A form pre-filled from an existing Saved Search, for the edit modal.
    pub fn from_saved(form_id: u64, connection_id: String, saved: &SavedSearch) -> Self {
        let mut form = Self::new(form_id, connection_id);
        form.saved_id = Some(saved.id.clone());
        form.name = saved.name.clone();
        form.target = saved.target.clone();
        form.timestamp_field = saved.timestamp_field.clone();
        form
    }

    /// Typeahead matches for the current Target text (case-insensitive
    /// substring), capped for display.
    pub fn target_matches(&self) -> Vec<&String> {
        let needle = self.target.trim().to_lowercase();
        self.target_options
            .iter()
            .filter(|opt| {
                needle.is_empty() || (opt.to_lowercase().contains(&needle) && *opt != &self.target)
            })
            .take(8)
            .collect()
    }

    /// Checks name and Target are filled in.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Name is required".to_string());
        }
        if self.target.trim().is_empty() {
            return Err("Target is required".to_string());
        }
        Ok(())
    }

    /// The timestamp field to persist, falling back to the default when blank.
    pub fn resolved_timestamp_field(&self) -> String {
        let field = self.timestamp_field.trim();
        if field.is_empty() {
            config::default_timestamp_field()
        } else {
            field.to_string()
        }
    }

    /// Validates the form and builds a fresh Saved Search from it, giving the
    /// query string, timeframe, Columns and sort their defaults — the user
    /// tunes those from the Search bar once the Result Tab opens.
    pub fn to_saved(&self) -> Result<SavedSearch, String> {
        self.validate()?;
        let timestamp_field = self.resolved_timestamp_field();
        Ok(SavedSearch {
            id: self.saved_id.clone().unwrap_or_else(config::new_id),
            name: self.name.trim().to_string(),
            target: self.target.trim().to_string(),
            query_string: String::new(),
            timeframe: Timeframe::default(),
            sort: vec![SortKey::new(timestamp_field.clone(), true)],
            timestamp_field,
            columns: config::default_columns(),
            mode: crate::line::LayoutMode::default(),
            template: String::new(),
            wrap: false,
        })
    }
}
