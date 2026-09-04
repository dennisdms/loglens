//! A Saved Search at runtime.
//!
//! It has two editing surfaces, and they own different halves of it. [`Live`]
//! is the Search bar's: the Target, query string, Timeframe, Columns, sort and
//! Layout of the Saved Search a Result Tab is showing, edited continuously and
//! written back after every change. [`SearchForm`] is the Search settings': the
//! name and timestamp field, edited in a modal and committed on Save.
//!
//! The split is why [`Live::write_back`] writes eight of a Saved Search's
//! eleven fields and leaves the rest to the form.

mod form;
mod live;

pub use form::{Fields, SearchForm};
pub use live::{Edited, Live};
