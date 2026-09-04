//! What a Result Tab does with the events its own surfaces raise.
//!
//! Every [`Msg`] here is answerable by one tab and nothing else: it moves that
//! tab's drafts, opens or closes one of its popovers, or edits its Live
//! Search. None of them needs a Connection, a client, or another tab.
//! `LogLens::update` therefore routes the lot through a single arm and does
//! one thing with what comes back \u{2014} [`Edited`] says whether the change is
//! worth writing to the config, and whether it invalidates the Run.
//!
//! What is deliberately *not* here is everything that genuinely needs the app
//! around it: probing a Target against the cluster, paging a Run, and the
//! pointer drags, which outlive the widget they started on and so are tracked
//! on `LogLens` rather than on any one tab.

use iced::widget::text_editor;

use crate::config::{TimeUnit, TimeframeChoice, TimeframeMode};
use crate::es;
use crate::line::LayoutMode;
use crate::results::ResultTab;
use crate::search::Edited;

/// An event raised by one Result Tab's own surfaces \u{2014} its Search bar, its
/// options strip, its table headers, its popovers, its Hit detail panel.
///
/// Carried by `Message::Result(run_id, ..)`, which is what pairs it back up
/// with the tab that raised it; nothing in here names a tab itself.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The async Target listing for the suggestion dropdown landed.
    TargetsLoaded(Vec<String>),
    /// A keystroke in the Search bar's Target input; also opens the dropdown.
    TargetDraft(String),
    /// Toggle the Target suggestion dropdown (the caret button next to the
    /// field).
    TargetPanelToggle,
    /// Dismiss the Target suggestion dropdown without committing.
    TargetPanelDismiss,
    /// Clear the failed-Target-switch notice.
    DismissTargetError,

    QueryDraft(String),
    QuerySubmit,

    /// A timeframe dropdown pick: a preset applies immediately, `Custom` opens
    /// the popover.
    TimeframeChoice(TimeframeChoice),
    TfMode(TimeframeMode),
    TfRelAmount(String),
    TfRelUnit(TimeUnit),
    TfAbsFrom(String),
    TfAbsTo(String),
    /// Apply the "Custom\u{2026}" popover's draft timeframe and re-run.
    TfApply,
    /// Dismiss the popover without changing the timeframe.
    TfCancel,

    /// The `_field_caps` listing behind the Column and Sort pickers landed.
    FieldsLoaded(Result<es::FieldCaps, es::Error>),

    ColumnDraft(String),
    ColumnAdd,
    ColumnAddField(String),
    ColumnRemove(usize),
    ColumnMove(usize, isize),
    /// Toggle a column header's "\u{22ee}" settings menu (by column index).
    HeaderMenu(usize),
    /// Close any open column header settings menu.
    HeaderMenuDismiss,

    /// Toggle the options strip's "Sort fields" popover.
    SortPanel,
    /// Close the "Sort fields" popover (click outside).
    SortPanelDismiss,
    /// Set a field's sort direction, adding it to the sort order if new.
    SortSet(String, bool),
    /// Drop a field from the sort order.
    SortRemove(String),
    /// Reorder the sort key at the given position by `delta` places.
    SortMove(usize, isize),
    /// Clear the whole sort order.
    SortClear,

    /// Switch between Table and raw text mode.
    LayoutMode(LayoutMode),
    /// Toggle line wrapping (variable row heights).
    Wrap,
    /// Expand / collapse one wrapped Hit past the global row cap.
    HitExpand(usize),

    /// A keystroke in the raw text template input.
    TemplateDraft(String),
    /// Commit the raw text template draft (Enter).
    TemplateSubmit,
    /// Open the raw-text "Format" modal.
    OpenFormat,
    /// Commit the template draft and close the "Format" modal.
    CloseFormat,
    /// Discard the template draft and close the "Format" modal.
    FormatCancel,

    /// Open, swap, or close the Hit detail panel.
    HitClicked(usize),
    /// A key or pointer action inside the Hit detail panel's editor.
    DetailEdit(text_editor::Action),
}

impl ResultTab {
    /// Applies one of this tab's own events to it.
    ///
    /// The returned [`Edited`] is the whole of what the caller has left to do:
    /// `Edited::NONE` for the arms that move nothing but view state, and
    /// whatever the [`Live`](crate::search::Live) edit reported for the ones
    /// that change the Saved Search.
    pub fn update(&mut self, msg: Msg) -> Edited {
        match msg {
            Msg::TargetsLoaded(targets) => {
                self.targets_loading = false;
                self.target_options = targets;
                Edited::NONE
            }
            Msg::TargetDraft(v) => {
                self.target_draft = v;
                self.target_panel_open = true;
                self.target_error = None;
                Edited::NONE
            }
            Msg::TargetPanelToggle => {
                self.target_panel_open = !self.target_panel_open;
                if self.target_panel_open {
                    self.tf.open = false;
                } else {
                    self.target_draft = self.search.target.clone();
                }
                Edited::NONE
            }
            Msg::TargetPanelDismiss => {
                self.target_panel_open = false;
                self.target_draft = self.search.target.clone();
                Edited::NONE
            }
            Msg::DismissTargetError => {
                self.target_error = None;
                Edited::NONE
            }

            Msg::QueryDraft(v) => {
                self.query_draft = v;
                Edited::NONE
            }
            Msg::QuerySubmit => {
                let draft = self.query_draft.clone();
                self.search.set_query_string(draft)
            }

            Msg::TimeframeChoice(choice) => match choice.to_timeframe() {
                Some(timeframe) => {
                    self.tf.open = false;
                    self.search.set_timeframe(timeframe)
                }
                // "Custom\u{2026}": no timeframe of its own, it opens the popover
                // on the current one instead.
                None => {
                    let current = self.search.timeframe.clone();
                    self.tf.seed(&current);
                    Edited::NONE
                }
            },
            Msg::TfMode(mode) => {
                self.tf.mode = mode;
                Edited::NONE
            }
            Msg::TfRelAmount(v) => {
                self.tf.rel_amount = v;
                Edited::NONE
            }
            Msg::TfRelUnit(unit) => {
                self.tf.rel_unit = unit;
                Edited::NONE
            }
            Msg::TfAbsFrom(v) => {
                self.tf.abs_from = v;
                Edited::NONE
            }
            Msg::TfAbsTo(v) => {
                self.tf.abs_to = v;
                Edited::NONE
            }
            Msg::TfApply => {
                let timeframe = self.tf.to_timeframe();
                self.tf.open = false;
                self.search.set_timeframe(timeframe)
            }
            Msg::TfCancel => {
                self.tf.open = false;
                Edited::NONE
            }

            // A failed field listing is not worth interrupting anyone over:
            // the Column and Sort pickers simply stay on what they already
            // have, and the next run asks again.
            Msg::FieldsLoaded(Err(_)) => Edited::NONE,
            Msg::FieldsLoaded(Ok(caps)) => {
                self.all_fields = caps.all;
                self.sortable_fields = caps.sortable;
                self.resolve_template()
            }

            Msg::ColumnDraft(v) => {
                self.column_draft = v;
                Edited::NONE
            }
            Msg::ColumnAdd => {
                let draft = self.column_draft.clone();
                self.add_column_from_draft(&draft)
            }
            Msg::ColumnAddField(field) => self.add_column_from_draft(&field),
            Msg::ColumnRemove(i) => {
                self.header_menu = None;
                self.search.remove_column(i)
            }
            Msg::ColumnMove(i, delta) => {
                self.header_menu = None;
                self.search.move_column(i, delta)
            }
            Msg::HeaderMenu(index) => {
                self.header_menu = if self.header_menu == Some(index) {
                    None
                } else {
                    Some(index)
                };
                Edited::NONE
            }
            Msg::HeaderMenuDismiss => {
                self.header_menu = None;
                Edited::NONE
            }

            Msg::SortPanel => {
                self.sort_panel_open = !self.sort_panel_open;
                Edited::NONE
            }
            Msg::SortPanelDismiss => {
                self.sort_panel_open = false;
                Edited::NONE
            }
            Msg::SortSet(field, desc) => {
                self.header_menu = None;
                self.search.set_sort_dir(&field, desc)
            }
            Msg::SortRemove(field) => {
                self.header_menu = None;
                self.search.remove_sort(&field)
            }
            Msg::SortMove(index, delta) => self.search.move_sort(index, delta),
            Msg::SortClear => self.search.clear_sort(),

            // Entering raw text mode for the first time also resolves the
            // template, which is a second edit to persist — hence the `|`.
            Msg::LayoutMode(mode) => self.search.set_mode(mode) | self.resolve_template(),
            // The next `prepare_heights` sees the `WrapCtx` change and
            // rebuilds the row-height model; a one-off render pass measures
            // every line the first time wrap turns on.
            Msg::Wrap => self.search.toggle_wrap(),
            Msg::HitExpand(index) => {
                self.line_cache.get_mut().toggle_expand(index);
                Edited::NONE
            }

            Msg::TemplateDraft(v) => {
                self.template_draft = v;
                Edited::NONE
            }
            Msg::TemplateSubmit => self.commit_template(),
            Msg::OpenFormat => {
                self.format_open = true;
                Edited::NONE
            }
            Msg::CloseFormat => {
                self.format_open = false;
                self.commit_template()
            }
            Msg::FormatCancel => {
                self.template_draft = self.search.template.clone();
                self.format_open = false;
                Edited::NONE
            }

            Msg::HitClicked(index) => {
                self.toggle_detail(index);
                Edited::NONE
            }
            Msg::DetailEdit(action) => {
                // The panel is a read-only view of one Hit's `_source`, so
                // only the actions that move the cursor or the selection are
                // let through.
                if !action.is_edit() {
                    self.detail_content.perform(action);
                }
                Edited::NONE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::config::{EsSettings, SavedSearch, SortKey, Timeframe};
    use crate::line::LayoutMode;

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

    fn tab() -> ResultTab {
        ResultTab::new(
            1,
            "c1".to_string(),
            &saved(),
            None,
            EsSettings::default(),
            false,
        )
    }

    /// The claim this module's doc makes: a message that only moves a draft or
    /// opens a popover is never worth a config write or a new Run. Worth
    /// pinning, because the cost of getting it wrong is a cluster round trip
    /// per keystroke.
    #[test]
    fn view_only_messages_neither_persist_nor_rerun() {
        let view_only = [
            Msg::TargetsLoaded(vec!["logs-other".to_string()]),
            Msg::TargetDraft("logs-o".to_string()),
            Msg::TargetPanelToggle,
            Msg::TargetPanelDismiss,
            Msg::DismissTargetError,
            Msg::QueryDraft("status:404".to_string()),
            Msg::TfMode(TimeframeMode::Absolute),
            Msg::TfRelAmount("30".to_string()),
            Msg::TfRelUnit(TimeUnit::Hours),
            Msg::TfAbsFrom("2026-09-01".to_string()),
            Msg::TfAbsTo("2026-09-02".to_string()),
            Msg::TfCancel,
            Msg::ColumnDraft("host".to_string()),
            Msg::HeaderMenu(0),
            Msg::HeaderMenuDismiss,
            Msg::SortPanel,
            Msg::SortPanelDismiss,
            Msg::HitExpand(0),
            Msg::TemplateDraft("%{host}".to_string()),
            Msg::OpenFormat,
            Msg::FormatCancel,
            Msg::HitClicked(0),
        ];
        let mut tab = tab();
        for msg in view_only {
            let label = format!("{msg:?}");
            assert_eq!(tab.update(msg), Edited::NONE, "{label}");
        }
    }

    #[test]
    fn a_query_draft_commits_only_on_submit() {
        let mut tab = tab();
        assert_eq!(
            tab.update(Msg::QueryDraft("status:404".to_string())),
            Edited::NONE
        );
        assert_eq!(tab.search.query_string, "status:500");

        assert_eq!(tab.update(Msg::QuerySubmit), Edited::QUERY);
        assert_eq!(tab.search.query_string, "status:404");
    }

    #[test]
    fn resubmitting_an_unchanged_query_starts_no_run() {
        let mut tab = tab();
        assert_eq!(tab.update(Msg::QuerySubmit), Edited::NONE);
    }

    #[test]
    fn dismissing_the_target_dropdown_puts_the_committed_target_back() {
        let mut tab = tab();
        tab.update(Msg::TargetDraft("logs-typo".to_string()));
        assert!(tab.target_panel_open);

        tab.update(Msg::TargetPanelDismiss);
        assert!(!tab.target_panel_open);
        assert_eq!(tab.target_draft, "logs-nginx");
    }

    #[test]
    fn editing_the_target_clears_the_last_failed_switch() {
        let mut tab = tab();
        tab.target_error = Some("no such index".to_string());
        tab.update(Msg::TargetDraft("logs-o".to_string()));
        assert_eq!(tab.target_error, None);
    }

    /// Both are stack layers over the same strip, so at most one may be up.
    #[test]
    fn opening_the_target_dropdown_closes_the_timeframe_popover() {
        let mut tab = tab();
        tab.tf.open = true;
        tab.update(Msg::TargetPanelToggle);
        assert!(tab.target_panel_open);
        assert!(!tab.tf.open);
    }

    #[test]
    fn a_timeframe_preset_applies_at_once_but_custom_only_opens_the_popover() {
        let mut tab = tab();
        assert_eq!(
            tab.update(Msg::TimeframeChoice(TimeframeChoice::Preset {
                amount: 1,
                unit: TimeUnit::Hours
            })),
            Edited::QUERY
        );
        assert!(!tab.tf.open);

        assert_eq!(
            tab.update(Msg::TimeframeChoice(TimeframeChoice::Custom)),
            Edited::NONE
        );
        assert!(tab.tf.open);
        // Seeded from what is committed now, not from the saved value.
        assert_eq!(tab.tf.rel_amount, "1");
        assert_eq!(tab.tf.rel_unit, TimeUnit::Hours);
    }

    #[test]
    fn applying_the_timeframe_popover_closes_it_and_reruns() {
        let mut tab = tab();
        tab.update(Msg::TimeframeChoice(TimeframeChoice::Custom));
        tab.update(Msg::TfRelAmount("45".to_string()));
        assert_eq!(tab.update(Msg::TfApply), Edited::QUERY);
        assert!(!tab.tf.open);
        assert_eq!(
            tab.search.timeframe,
            Timeframe::Relative {
                amount: 45,
                unit: TimeUnit::Minutes
            }
        );
    }

    #[test]
    fn adding_a_column_clears_the_draft_even_when_it_is_rejected() {
        let mut tab = tab();
        tab.update(Msg::ColumnDraft("host".to_string()));
        assert_eq!(tab.update(Msg::ColumnAdd), Edited::DISPLAY);
        assert!(tab.search.columns.contains(&"host".to_string()));
        assert!(tab.column_draft.is_empty());

        // A duplicate is no edit at all, but the input still empties — a
        // rejected name must not sit there looking pending.
        tab.update(Msg::ColumnDraft("host".to_string()));
        assert_eq!(tab.update(Msg::ColumnAdd), Edited::NONE);
        assert!(tab.column_draft.is_empty());
    }

    #[test]
    fn a_column_edit_from_the_header_menu_closes_it() {
        let mut tab = tab();
        tab.update(Msg::HeaderMenu(1));
        assert_eq!(tab.header_menu, Some(1));
        tab.update(Msg::ColumnRemove(1));
        assert_eq!(tab.header_menu, None);
        assert_eq!(tab.search.columns, vec!["@timestamp".to_string()]);
    }

    /// Sort is also the total order `search_after` pages along (ADR 0002), so
    /// re-sorting is a new Run rather than a re-render.
    #[test]
    fn sort_changes_rerun_but_column_changes_do_not() {
        let mut tab = tab();
        assert_eq!(
            tab.update(Msg::SortSet("host".to_string(), false)),
            Edited::QUERY
        );
        assert_eq!(
            tab.update(Msg::SortRemove("host".to_string())),
            Edited::QUERY
        );
        assert_eq!(tab.update(Msg::ColumnMove(0, 1)), Edited::DISPLAY);
    }

    #[test]
    fn clearing_an_already_empty_sort_asks_for_nothing() {
        let mut tab = tab();
        assert_eq!(tab.update(Msg::SortClear), Edited::QUERY);
        assert_eq!(tab.update(Msg::SortClear), Edited::NONE);
    }

    #[test]
    fn the_format_modal_commits_on_close_and_reverts_on_cancel() {
        let mut tab = tab();
        tab.update(Msg::OpenFormat);
        tab.update(Msg::TemplateDraft("%{host}".to_string()));
        tab.update(Msg::FormatCancel);
        assert!(!tab.format_open);
        assert_eq!(tab.template_draft, "%{message}");
        assert_eq!(tab.search.template, "%{message}");

        tab.update(Msg::OpenFormat);
        tab.update(Msg::TemplateDraft("%{host}".to_string()));
        assert_eq!(tab.update(Msg::CloseFormat), Edited::DISPLAY);
        assert!(!tab.format_open);
        assert_eq!(tab.search.template, "%{host}");
    }

    /// A `_field_caps` failure is silent by design: the pickers keep whatever
    /// they had rather than emptying out under the user.
    #[test]
    fn a_failed_field_listing_leaves_the_pickers_alone() {
        let mut tab = tab();
        tab.all_fields = vec!["message".to_string()];
        let err = es::Error::NoSuchTarget("logs-nginx".to_string());
        assert_eq!(tab.update(Msg::FieldsLoaded(Err(err))), Edited::NONE);
        assert_eq!(tab.all_fields, vec!["message".to_string()]);
    }

    #[test]
    fn clicking_the_same_hit_twice_closes_the_detail_panel() {
        let mut tab = tab();
        tab.hits = vec![es::Hit::detached(json!({"message": "boom"}))];
        tab.update(Msg::HitClicked(0));
        assert_eq!(tab.selected_hit, Some(0));
        tab.update(Msg::HitClicked(0));
        assert_eq!(tab.selected_hit, None);
    }
}
