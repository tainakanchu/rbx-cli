// SPDX-License-Identifier: GPL-2.0-or-later
//! The one error type commands return, carrying its protocol code.

use crate::protocol::ErrorCode;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

pub type CliResult<T> = Result<T, CliError>;

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "Cancelled.")
    }

    pub fn io(context: &str, error: &std::io::Error) -> Self {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::NotFound
        } else {
            ErrorCode::Io
        };
        Self::new(code, format!("{context}: {error}"))
    }
}

impl From<rbl_export::ExportError> for CliError {
    fn from(error: rbl_export::ExportError) -> Self {
        use rbl_export::ExportError as E;
        let code = match &error {
            E::Cancelled => ErrorCode::Cancelled,
            E::NotADirectory(_) => ErrorCode::NotFound,
            E::DeviceGone => ErrorCode::DeviceGone,
            E::Conflict(_) => ErrorCode::Conflict,
            E::Empty => ErrorCode::InvalidRequest,
            E::Io(e) if e.kind() == std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            E::Io(_) => ErrorCode::Io,
            E::OneLibrary(_) => ErrorCode::Internal,
        };
        Self::new(code, error.to_string())
    }
}
