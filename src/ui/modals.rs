//! The forms: the Connection editor, the two halves of a Saved Search's
//! settings, the secret prompt, and the confirmation for a destructive action.
//!
//! All but one are modal cards over the main window. The exception is
//! [`search_form`], which is a tab body \u{2014} creating a Saved Search is not an
//! interruption of the work, it *is* the work, and it shares its fields with
//! the modal that edits one later.

use iced::widget::{
    button, checkbox, column, container, radio, row, scrollable, space, text, text_input,
};
use iced::{Element, Fill, Padding, Theme};

use crate::connection::{AuthKind, ConnectionForm, TestState};
use crate::search::SearchForm;
use crate::style::{self, BG, ERR_RED, OK_GREEN, PANEL, TEXT, TEXT_DIM};
use crate::ui::{field_label, modal_card};
use crate::{Confirm, Message, SecretPrompt};

pub(crate) fn connection_form<'a>(form: &'a ConnectionForm) -> Element<'a, Message> {
    let mut fields: Vec<Element<'a, Message>> = vec![
        text(form.title()).size(16.0).color(TEXT).into(),
        field_label("Name"),
        text_input("Production logs", &form.name)
            .on_input(Message::ConnFormName)
            .padding(6.0)
            .into(),
        field_label("URL"),
        text_input("https://localhost:9200", &form.url)
            .on_input(Message::ConnFormUrl)
            .padding(6.0)
            .into(),
        field_label("Authentication"),
        row(AuthKind::ALL.iter().map(|&kind| {
            radio(
                kind.label(),
                kind,
                Some(form.auth_kind),
                Message::ConnFormAuthKind,
            )
            .size(14.0)
            .into()
        }))
        .spacing(16.0)
        .into(),
    ];

    if form.auth_kind == AuthKind::Basic {
        fields.push(field_label("Username"));
        fields.push(
            text_input("elastic", &form.username)
                .on_input(Message::ConnFormUsername)
                .padding(6.0)
                .into(),
        );
    }
    if form.auth_kind.needs_secret() {
        let secret_label = if form.auth_kind == AuthKind::Basic {
            "Password"
        } else {
            "API key"
        };
        fields.push(field_label(secret_label));
        let placeholder = if form.editing_id.is_some() {
            "(unchanged)"
        } else {
            ""
        };
        fields.push(
            text_input(placeholder, &form.secret)
                .on_input(Message::ConnFormSecret)
                .secure(true)
                .padding(6.0)
                .into(),
        );
    }

    fields.push(
        checkbox(form.skip_tls_verify)
            .label("Skip TLS certificate verification")
            .on_toggle(Message::ConnFormSkipTls)
            .size(14.0)
            .into(),
    );

    fields.push(space().height(4.0).into());
    fields.push(
        row![
            button(text("Test").size(13.0))
                .on_press(Message::ConnFormTest)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            test_result(&form.test),
        ]
        .spacing(12.0)
        .align_y(iced::Alignment::Center)
        .into(),
    );

    if let Some(err) = &form.error {
        fields.push(text(err.clone()).size(12.0).color(ERR_RED).into());
    }

    fields.push(space().height(8.0).into());
    fields.push(
        row![
            space().width(Fill),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::ConnFormCancel)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            button(text("Save").size(13.0).color(TEXT))
                .on_press(Message::ConnFormSave)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ]
        .spacing(8.0)
        .into(),
    );

    modal_card(column(fields).spacing(6.0).width(Fill).into())
}

pub(crate) fn secret_prompt<'a>(prompt: &'a SecretPrompt) -> Element<'a, Message> {
    let card = column![
        text("Secret required").size(16.0).color(TEXT),
        text(format!(
            "The keyring is unavailable. Enter the secret for {} for this session.",
            prompt.connection_name
        ))
        .size(12.0)
        .color(TEXT_DIM),
        space().height(4.0),
        text_input("", &prompt.value)
            .on_input(Message::SecretPromptValue)
            .on_submit(Message::SecretPromptSubmit)
            .secure(true)
            .padding(6.0),
        space().height(8.0),
        row![
            space().width(Fill),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::SecretPromptCancel)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            button(text("Continue").size(13.0).color(TEXT))
                .on_press(Message::SecretPromptSubmit)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ]
        .spacing(8.0),
    ]
    .spacing(6.0)
    .width(Fill);

    modal_card(card.into())
}

pub(crate) fn confirm<'a>(confirm: &'a Confirm) -> Element<'a, Message> {
    let (title, body, proceed) = match confirm {
        Confirm::DeleteConnection { name, .. } => (
            "Delete Connection",
            format!("Delete \"{name}\" and all of its Saved Searches? This cannot be undone."),
            "Delete",
        ),
    };

    let card = column![
        text(title).size(16.0).color(TEXT),
        text(body).size(12.0).color(TEXT_DIM),
        space().height(10.0),
        row![
            space().width(Fill),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::ConfirmCancel)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            button(text(proceed).size(13.0).color(TEXT))
                .on_press(Message::ConfirmProceed)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(|_theme: &Theme, status| {
                    let base = style::picker_row(true)(_theme, status);
                    button::Style {
                        background: Some(ERR_RED.into()),
                        ..base
                    }
                }),
        ]
        .spacing(8.0),
    ]
    .spacing(6.0)
    .width(Fill);

    modal_card(card.into())
}

/// The new-Saved-Search form tab: only the structural fields. Query string,
/// timeframe, Columns and sort get defaults and are tuned from the Search
/// bar once the Result Tab opens.
pub(crate) fn search_form<'a>(
    form: &'a SearchForm,
    conn_name: &str,
    cancel_tab: usize,
) -> Element<'a, Message> {
    let mut col = column![
        text("New Search").size(16.0).color(TEXT),
        text(format!("on {conn_name}")).size(12.0).color(TEXT_DIM),
        text(
            "Query string, timeframe, Columns and sort are tuned from the \
             Search bar once this opens."
        )
        .size(11.0)
        .color(TEXT_DIM),
        space().height(6.0),
    ]
    .spacing(6.0)
    .max_width(560.0);

    for field in search_settings_fields(form, true) {
        col = col.push(field);
    }

    if let Some(err) = &form.error {
        col = col.push(text(err.clone()).size(12.0).color(ERR_RED));
    }

    col = col.push(space().height(10.0));
    col = col.push(
        row![
            button(text("Save & Run").size(13.0).color(TEXT))
                .on_press(Message::SearchSave)
                .padding(Padding::new(6.0).left(16.0).right(16.0))
                .style(style::picker_row(true)),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::CloseTab(cancel_tab))
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
        ]
        .spacing(8.0),
    );

    container(scrollable(col.padding(Padding::new(0.0).right(12.0))).height(Fill))
        .style(|_| style::panel(BG))
        .width(Fill)
        .height(Fill)
        .padding(16.0)
        .into()
}

/// The Search settings modal: the same three fields as the create form,
/// shown over the current tab rather than as a tab of its own. Saving it
/// re-runs an open Result Tab for the Saved Search.
pub(crate) fn search_settings<'a>(form: &'a SearchForm, conn_name: &str) -> Element<'a, Message> {
    let mut card = column![
        text("Search settings").size(16.0).color(TEXT),
        text(format!("on {conn_name}")).size(12.0).color(TEXT_DIM),
        space().height(2.0),
    ]
    .spacing(6.0)
    .width(Fill);

    for field in search_settings_fields(form, false) {
        card = card.push(field);
    }

    if let Some(err) = &form.error {
        card = card.push(text(err.clone()).size(12.0).color(ERR_RED));
    }

    card = card.push(space().height(8.0));
    card = card.push(
        row![
            space().width(Fill),
            button(text("Cancel").size(13.0).color(TEXT_DIM))
                .on_press(Message::SearchSettingsCancel)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::bare_button()),
            button(text("Save").size(13.0).color(TEXT))
                .on_press(Message::SearchSettingsSave)
                .padding(Padding::new(6.0).left(14.0).right(14.0))
                .style(style::picker_row(true)),
        ]
        .spacing(8.0),
    );

    modal_card(card.into())
}

/// The structural fields shared by the new-Saved-Search form and the Search
/// settings modal: name, timestamp field, and — only when `include_target`
/// — the Target (with typeahead). The edit modal omits the Target; it is
/// re-pointed from the Search bar instead.
fn search_settings_fields<'a>(
    form: &'a SearchForm,
    include_target: bool,
) -> Vec<Element<'a, Message>> {
    let mut fields: Vec<Element<'a, Message>> = vec![
        field_label("Name"),
        text_input("checkout-errors", &form.name)
            .on_input(Message::SearchName)
            .padding(6.0)
            .into(),
    ];

    if include_target {
        fields.push(field_label("Target — index, data stream, or pattern"));
        fields.push(
            text_input("logs-*", &form.target)
                .on_input(Message::SearchTargetInput)
                .padding(6.0)
                .into(),
        );
        if form.targets_loading {
            fields.push(
                text("Loading indices\u{2026}")
                    .size(11.0)
                    .color(TEXT_DIM)
                    .into(),
            );
        } else {
            let matches = form.target_matches();
            if !matches.is_empty() {
                let mut opts = column![].spacing(1.0);
                for name in matches {
                    opts = opts.push(
                        button(text(name.clone()).size(12.0))
                            .on_press(Message::SearchTargetPicked(name.clone()))
                            .width(Fill)
                            .padding(Padding::new(3.0).left(8.0))
                            .style(style::picker_row(false)),
                    );
                }
                fields.push(container(opts).style(|_| style::panel(PANEL)).into());
            }
        }
    }

    fields.push(field_label("Timestamp field"));
    fields.push(
        text_input("@timestamp", &form.timestamp_field)
            .on_input(Message::SearchTimestampField)
            .padding(6.0)
            .into(),
    );
    fields
}

fn test_result(state: &TestState) -> Element<'_, Message> {
    match state {
        TestState::Idle => space().width(0.0).into(),
        TestState::Running => text("Testing\u{2026}").size(12.0).color(TEXT_DIM).into(),
        TestState::Ok(msg) => text(format!("\u{2713} {msg}"))
            .size(12.0)
            .color(OK_GREEN)
            .into(),
        TestState::Failed(err) => text(format!("\u{2717} {err}"))
            .size(12.0)
            .color(ERR_RED)
            .into(),
    }
}
