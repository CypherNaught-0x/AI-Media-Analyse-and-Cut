//! The error every Tauri command returns.

use crate::run_control::RUN_CANCELLED_MESSAGE;
use serde::Serialize;

/// What kind of failure an [`AppError`] is, so the frontend can react to it
/// (e.g. stay quiet on cancellation) without matching message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// The user cancelled the run (or a newer run superseded it).
    Cancelled,
    /// Anything else; `message` says what went wrong.
    Failed,
}

/// A command failure as the frontend receives it: `{ kind, message }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Cancelled,
            message: RUN_CANCELLED_MESSAGE.to_string(),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Failed,
            message: message.into(),
        }
    }
}

/// Internal helpers still report errors as strings; cancellation is the
/// well-known RUN_CANCELLED_MESSAGE, everything else is a failure.
impl From<String> for AppError {
    fn from(message: String) -> Self {
        if message == RUN_CANCELLED_MESSAGE {
            Self::cancelled()
        } else {
            Self::failed(message)
        }
    }
}

impl From<&str> for AppError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        // `{:#}` keeps the context chain ("Failed to load model: file not found").
        format!("{error:#}").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_messages_become_the_cancelled_kind() {
        assert_eq!(
            AppError::from(RUN_CANCELLED_MESSAGE.to_string()).kind,
            ErrorKind::Cancelled
        );
        assert_eq!(
            AppError::from(anyhow::anyhow!(RUN_CANCELLED_MESSAGE)).kind,
            ErrorKind::Cancelled
        );
        assert_eq!(AppError::from("disk full").kind, ErrorKind::Failed);
    }

    #[test]
    fn anyhow_context_is_kept_in_the_message() {
        let error = anyhow::anyhow!("file not found").context("Failed to load model");
        assert_eq!(
            AppError::from(error).message,
            "Failed to load model: file not found"
        );
    }

    #[test]
    fn serializes_as_kind_and_message() {
        let json = serde_json::to_value(AppError::cancelled()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "cancelled", "message": RUN_CANCELLED_MESSAGE })
        );
    }
}
