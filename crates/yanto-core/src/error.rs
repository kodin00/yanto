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
    /// Set for `RateLimited`: seconds until the caller may retry (`Retry-After`).
    pub retry_after_secs: Option<u64>,
}

impl CoreError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
            retry_after_secs: None,
        }
    }

    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(
            ErrorKind::Validation(vec![FieldError {
                field: field.into(),
                message: message.clone(),
            }]),
            message,
        )
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unauthenticated, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Forbidden, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        let mut e = Self::new(ErrorKind::RateLimited, "too many attempts, try again later");
        e.retry_after_secs = Some(retry_after_secs);
        e
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
    fn rate_limited_carries_retry_after() {
        let e = CoreError::rate_limited(30);
        assert_eq!(e.kind, ErrorKind::RateLimited);
        assert_eq!(e.retry_after_secs, Some(30));
    }

    #[test]
    fn display_uses_message_not_source() {
        let err = CoreError::internal("db failed", std::io::Error::other("secret detail"));
        assert_eq!(err.to_string(), "db failed");
        assert!(StdError::source(&err).is_some());
    }
}
