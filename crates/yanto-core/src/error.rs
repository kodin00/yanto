//! Core error type shared by every adapter (docs/rewrite/01 section 9).

use std::error::Error as StdError;
use std::fmt;

/// One invalid input field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

/// Coarse classification adapters map to HTTP status codes and UI states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    Validation(Vec<FieldError>),
    Unauthenticated,
    Forbidden,
    NotFound,
    Conflict,
    /// A Host Daemon is down or Docker is unreachable.
    Unavailable,
    RateLimited,
    Internal,
}

/// Error returned by the service layer. `message` is safe to show to callers;
/// `source` keeps the chain for logs only.
#[derive(Debug)]
pub struct CoreError {
    pub kind: ErrorKind,
    pub message: String,
    pub source: Option<Box<dyn StdError + Send + Sync>>,
}

impl CoreError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn internal(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(ErrorKind::Internal, message).with_source(source)
    }

    pub fn unavailable(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(ErrorKind::Unavailable, message).with_source(source)
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for CoreError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_message_not_source() {
        let err = CoreError::internal("db failed", std::io::Error::other("secret detail"));
        assert_eq!(err.to_string(), "db failed");
        assert!(StdError::source(&err).is_some());
    }
}
