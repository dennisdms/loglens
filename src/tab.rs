//! The main area's open tabs.
//!
//! Tabs used to be a bare `Vec<usize>` of file indices. They are now a list of
//! typed [`Tab`] values so that Search forms and Result Tabs can sit alongside
//! Sample Log files as the Elasticsearch feature lands.

/// One open tab in the main area.
pub enum Tab {
    /// A Sample Log file, identified by its index into `LogLens::files`.
    File { file: usize },
    // Room for the Elasticsearch feature:
    // SearchForm { .. } — a Saved Search being composed
    // Result { .. }     — the Hits from a run
}

impl Tab {
    /// The Sample Log file this tab shows, if it is a file tab.
    pub fn file(&self) -> Option<usize> {
        match self {
            Tab::File { file } => Some(*file),
        }
    }
}
