//! State for a Search form tab (compose / edit a Saved Search).

use crate::config::{self, SavedSearch, TimeUnit, Timeframe};

/// Which Timeframe kind the form's toggle has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeMode {
    Relative,
    Absolute,
}

/// The editable Search form.
pub struct SearchForm {
    /// Stable id so async target results find their form.
    pub form_id: u64,
    pub connection_id: String,
    /// `Some` when editing an existing Saved Search.
    pub saved_id: Option<String>,
    pub name: String,
    pub target: String,
    /// Typeahead options from `_cat/indices` + `_data_stream`.
    pub target_options: Vec<String>,
    /// Whether `_cat/indices` / `_data_stream` are still in flight.
    pub targets_loading: bool,
    pub query_string: String,
    pub mode: TimeframeMode,
    pub rel_amount: String,
    pub rel_unit: TimeUnit,
    pub abs_from: String,
    pub abs_to: String,
    pub timestamp_field: String,
    pub columns: Vec<String>,
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
            query_string: String::new(),
            mode: TimeframeMode::Relative,
            rel_amount: "15".to_string(),
            rel_unit: TimeUnit::Minutes,
            abs_from: String::new(),
            abs_to: String::new(),
            timestamp_field: config::default_timestamp_field(),
            columns: config::default_columns(),
            error: None,
        }
    }

    /// A form pre-filled from an existing Saved Search.
    #[allow(dead_code)] // wired up when tree items become editable (#7)
    pub fn from_saved(form_id: u64, connection_id: String, saved: &SavedSearch) -> Self {
        let mut form = Self::new(form_id, connection_id);
        form.saved_id = Some(saved.id.clone());
        form.name = saved.name.clone();
        form.target = saved.target.clone();
        form.query_string = saved.query_string.clone();
        form.timestamp_field = saved.timestamp_field.clone();
        form.columns = saved.columns.clone();
        match &saved.timeframe {
            Timeframe::Relative { amount, unit } => {
                form.mode = TimeframeMode::Relative;
                form.rel_amount = amount.to_string();
                form.rel_unit = *unit;
            }
            Timeframe::Absolute { from, to } => {
                form.mode = TimeframeMode::Absolute;
                form.abs_from = from.clone();
                form.abs_to = to.clone();
            }
        }
        form
    }

    /// Typeahead matches for the current Target text (case-insensitive
    /// substring), capped for display.
    pub fn target_matches(&self) -> Vec<&String> {
        let needle = self.target.trim().to_lowercase();
        self.target_options
            .iter()
            .filter(|opt| {
                needle.is_empty()
                    || (opt.to_lowercase().contains(&needle) && *opt != &self.target)
            })
            .take(8)
            .collect()
    }

    /// The Timeframe the form currently describes.
    pub fn timeframe(&self) -> Timeframe {
        match self.mode {
            TimeframeMode::Relative => Timeframe::Relative {
                amount: self.rel_amount.trim().parse().unwrap_or(15),
                unit: self.rel_unit,
            },
            TimeframeMode::Absolute => Timeframe::Absolute {
                from: self.abs_from.trim().to_string(),
                to: self.abs_to.trim().to_string(),
            },
        }
    }

    /// Validates the form and builds the Saved Search it describes.
    pub fn to_saved(&self) -> Result<SavedSearch, String> {
        if self.name.trim().is_empty() {
            return Err("Name is required".to_string());
        }
        if self.target.trim().is_empty() {
            return Err("Target is required".to_string());
        }
        let timestamp_field = if self.timestamp_field.trim().is_empty() {
            config::default_timestamp_field()
        } else {
            self.timestamp_field.trim().to_string()
        };
        Ok(SavedSearch {
            id: self
                .saved_id
                .clone()
                .unwrap_or_else(config::new_id),
            name: self.name.trim().to_string(),
            target: self.target.trim().to_string(),
            query_string: self.query_string.clone(),
            timeframe: self.timeframe(),
            timestamp_field,
            columns: self.columns.clone(),
        })
    }
}
