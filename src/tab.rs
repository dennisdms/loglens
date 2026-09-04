//! The main area's open tabs.

use crate::results::ResultTab;
use crate::search::SearchForm;

/// One open tab in the main area.
pub enum Tab {
    /// A Saved Search being composed or edited.
    SearchForm(Box<SearchForm>),
    /// The Hits from a run of one Saved Search.
    Result(Box<ResultTab>),
}

impl Tab {
    /// The label shown on the tab strip.
    pub fn title(&self) -> String {
        match self {
            Tab::SearchForm(form) => {
                if form.name.trim().is_empty() {
                    "New Search".to_string()
                } else {
                    form.name.clone()
                }
            }
            Tab::Result(tab) => tab.search.name.clone(),
        }
    }
}
