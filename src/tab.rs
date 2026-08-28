//! The main area's open tabs.

use crate::results::ResultTab;
use crate::search::SearchForm;

/// One open tab in the main area.
pub enum Tab {
    /// A Sample Log file, identified by its index into `LogLens::files`.
    File { file: usize },
    /// A Saved Search being composed or edited.
    SearchForm(Box<SearchForm>),
    /// The Hits from a run of one Saved Search.
    Result(Box<ResultTab>),
}

impl Tab {
    /// The Sample Log file this tab shows, if it is a file tab.
    pub fn file(&self) -> Option<usize> {
        match self {
            Tab::File { file } => Some(*file),
            _ => None,
        }
    }

    /// The label shown on the tab strip.
    pub fn title(&self, files: &[crate::sample::LogFile]) -> String {
        match self {
            Tab::File { file } => files[*file].name.clone(),
            Tab::SearchForm(form) => {
                if form.name.trim().is_empty() {
                    "New Search".to_string()
                } else {
                    form.name.clone()
                }
            }
            Tab::Result(tab) => tab.saved_name.clone(),
        }
    }
}
