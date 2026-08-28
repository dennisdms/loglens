//! State for the Connection form (add / edit a Connection).

use crate::config::{Auth, Connection};
use crate::es;
use crate::secrets;

/// Which auth scheme the form's radio group has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthKind {
    #[default]
    None,
    Basic,
    ApiKey,
}

impl AuthKind {
    pub const ALL: [AuthKind; 3] = [AuthKind::None, AuthKind::Basic, AuthKind::ApiKey];

    pub fn label(self) -> &'static str {
        match self {
            AuthKind::None => "None",
            AuthKind::Basic => "Basic",
            AuthKind::ApiKey => "API key",
        }
    }

    /// Whether this scheme needs a secret (password / key).
    pub fn needs_secret(self) -> bool {
        matches!(self, AuthKind::Basic | AuthKind::ApiKey)
    }
}

/// Outcome of the last **Test** press.
#[derive(Debug, Clone, Default)]
pub enum TestState {
    #[default]
    Idle,
    Running,
    Ok(String),
    Failed(String),
}

/// The editable Connection form.
#[derive(Debug, Clone, Default)]
pub struct ConnectionForm {
    /// `Some` when editing an existing Connection, `None` when adding one.
    pub editing_id: Option<String>,
    pub name: String,
    pub url: String,
    pub auth_kind: AuthKind,
    pub username: String,
    /// Password or API key. Left blank when editing keeps the stored secret.
    pub secret: String,
    pub skip_tls_verify: bool,
    pub test: TestState,
    /// Set when Save is blocked (missing name / URL).
    pub error: Option<String>,
}

impl ConnectionForm {
    pub fn adding() -> Self {
        Self::default()
    }

    /// A form pre-filled from an existing Connection.
    #[allow(dead_code)] // wired up when tree items become editable (#7)
    pub fn editing(connection: &Connection) -> Self {
        let (auth_kind, username) = match &connection.auth {
            Auth::None => (AuthKind::None, String::new()),
            Auth::Basic { username } => (AuthKind::Basic, username.clone()),
            Auth::ApiKey => (AuthKind::ApiKey, String::new()),
        };
        Self {
            editing_id: Some(connection.id.clone()),
            name: connection.name.clone(),
            url: connection.url.clone(),
            auth_kind,
            username,
            secret: String::new(),
            skip_tls_verify: connection.skip_tls_verify,
            test: TestState::Idle,
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.editing_id.is_some() {
            "Edit Connection"
        } else {
            "Add Connection"
        }
    }

    /// The `Auth` value (no secret) this form describes.
    pub fn auth(&self) -> Auth {
        match self.auth_kind {
            AuthKind::None => Auth::None,
            AuthKind::Basic => Auth::Basic {
                username: self.username.trim().to_string(),
            },
            AuthKind::ApiKey => Auth::ApiKey,
        }
    }

    /// Resolves the secret to use right now: the freshly typed one, else the
    /// stored secret for the Connection being edited. `None` means "need to ask".
    pub fn resolved_secret(&self) -> Option<String> {
        if !self.secret.is_empty() {
            return Some(self.secret.clone());
        }
        self.editing_id.as_deref().and_then(secrets::get)
    }

    /// Builds an [`es::Endpoint`] for a Test press, or reports what's missing.
    pub fn endpoint(&self) -> Result<es::Endpoint, EndpointError> {
        if self.url.trim().is_empty() {
            return Err(EndpointError::MissingUrl);
        }
        let auth = match self.auth_kind {
            AuthKind::None => es::AuthValue::None,
            AuthKind::Basic => es::AuthValue::Basic {
                username: self.username.trim().to_string(),
                password: self.resolved_secret().ok_or(EndpointError::MissingSecret)?,
            },
            AuthKind::ApiKey => es::AuthValue::ApiKey {
                key: self.resolved_secret().ok_or(EndpointError::MissingSecret)?,
            },
        };
        Ok(es::Endpoint {
            url: self.url.trim().to_string(),
            auth,
            skip_tls_verify: self.skip_tls_verify,
        })
    }
}

/// Why an [`es::Endpoint`] could not be built from the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointError {
    MissingUrl,
    /// The keyring is unavailable and no secret has been entered this session.
    MissingSecret,
}
