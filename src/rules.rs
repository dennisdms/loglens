//! Working state for the Highlight rules modal: a detached copy of
//! `Config.rules` plus the transient sub-form for the one rule being added or
//! edited. Committed to `Config.rules` on Save, discarded on Cancel.

use crate::line::{Matcher, Op, Rule, Style, parse_hex};

/// Which matcher kind the sub-form's toggle has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherKind {
    Field,
    Text,
}

impl std::fmt::Display for MatcherKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MatcherKind::Field => "Field",
            MatcherKind::Text => "Text",
        })
    }
}

pub struct RulesForm {
    /// Working copy of `Config.rules`; only written back on Save.
    pub rules: Vec<Rule>,
    /// Index into `rules` of the rule the sub-form is editing; `None` while
    /// composing a brand-new rule.
    pub editing: Option<usize>,
    pub draft_name: String,
    pub draft_kind: MatcherKind,
    pub draft_path: String,
    pub draft_op: Op,
    pub draft_value: String,
    pub draft_pattern: String,
    /// Hex colour text (`#rrggbb`); empty means "no colour".
    pub draft_fg: String,
    pub draft_bg: String,
    pub error: Option<String>,
}

impl RulesForm {
    pub fn new(rules: Vec<Rule>) -> Self {
        let mut form = Self {
            rules,
            editing: None,
            draft_name: String::new(),
            draft_kind: MatcherKind::Field,
            draft_path: String::new(),
            draft_op: Op::Eq,
            draft_value: String::new(),
            draft_pattern: String::new(),
            draft_fg: String::new(),
            draft_bg: String::new(),
            error: None,
        };
        form.reset_draft();
        form
    }

    /// Clears the sub-form back to "adding a new rule".
    pub fn reset_draft(&mut self) {
        self.editing = None;
        self.draft_name.clear();
        self.draft_kind = MatcherKind::Field;
        self.draft_path.clear();
        self.draft_op = Op::Eq;
        self.draft_value.clear();
        self.draft_pattern.clear();
        self.draft_fg.clear();
        self.draft_bg.clear();
        self.error = None;
    }

    /// Loads rule `index` into the sub-form for editing.
    pub fn load(&mut self, index: usize) {
        let Some(rule) = self.rules.get(index) else {
            return;
        };
        self.editing = Some(index);
        self.draft_name = rule.name.clone();
        match &rule.matcher {
            Matcher::Field { path, op, value } => {
                self.draft_kind = MatcherKind::Field;
                self.draft_path = path.clone();
                self.draft_op = *op;
                self.draft_value = value.clone();
                self.draft_pattern.clear();
            }
            Matcher::Text { pattern } => {
                self.draft_kind = MatcherKind::Text;
                self.draft_pattern = pattern.clone();
                self.draft_path.clear();
                self.draft_value.clear();
            }
        }
        self.draft_fg = hex_or_empty(rule.style.fg);
        self.draft_bg = hex_or_empty(rule.style.bg);
        self.error = None;
    }

    /// Builds a [`Rule`] from the sub-form, or an error message.
    fn draft_to_rule(&self) -> Result<Rule, String> {
        let name = self.draft_name.trim().to_string();
        if name.is_empty() {
            return Err("Rule name is required".to_string());
        }
        let matcher = match self.draft_kind {
            MatcherKind::Field => {
                let path = self.draft_path.trim().to_string();
                if path.is_empty() {
                    return Err("Field path is required".to_string());
                }
                Matcher::Field {
                    path,
                    op: self.draft_op,
                    value: self.draft_value.trim().to_string(),
                }
            }
            MatcherKind::Text => {
                let pattern = self.draft_pattern.clone();
                if pattern.trim().is_empty() {
                    return Err("Text pattern is required".to_string());
                }
                Matcher::Text { pattern }
            }
        };
        let fg = parse_colour(&self.draft_fg)?;
        let bg = parse_colour(&self.draft_bg)?;
        let enabled = self
            .editing
            .and_then(|i| self.rules.get(i))
            .map(|r| r.enabled)
            .unwrap_or(true);
        Ok(Rule {
            name,
            enabled,
            matcher,
            style: Style { fg, bg },
        })
    }

    /// Commits the sub-form: replaces the edited rule or appends a new one.
    /// Returns whether it succeeded (a failure leaves `error` set).
    pub fn commit_draft(&mut self) -> bool {
        match self.draft_to_rule() {
            Ok(rule) => {
                match self.editing {
                    Some(i) if i < self.rules.len() => self.rules[i] = rule,
                    _ => self.rules.push(rule),
                }
                self.reset_draft();
                true
            }
            Err(err) => {
                self.error = Some(err);
                false
            }
        }
    }

    pub fn delete(&mut self, index: usize) {
        if index < self.rules.len() {
            self.rules.remove(index);
        }
        if self.editing == Some(index) {
            self.reset_draft();
        }
    }

    pub fn toggle(&mut self, index: usize) {
        if let Some(rule) = self.rules.get_mut(index) {
            rule.enabled = !rule.enabled;
        }
    }

    pub fn move_rule(&mut self, index: usize, delta: isize) {
        let target = index as isize + delta;
        if index < self.rules.len() && target >= 0 && (target as usize) < self.rules.len() {
            self.rules.swap(index, target as usize);
        }
    }
}

fn hex_or_empty(color: Option<iced::Color>) -> String {
    match color {
        Some(c) => {
            let [r, g, b, _] = c.into_rgba8();
            format!("#{r:02x}{g:02x}{b:02x}")
        }
        None => String::new(),
    }
}

/// Empty text → no colour; a valid `#rrggbb` → that colour; anything else is
/// an error so a typo can't silently drop styling.
fn parse_colour(text: &str) -> Result<Option<iced::Color>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    parse_hex(text)
        .map(Some)
        .ok_or_else(|| format!("Not a #rrggbb colour: {text}"))
}
