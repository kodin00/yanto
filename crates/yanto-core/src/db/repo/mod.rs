//! SQLite repositories. Functions take `&mut SqliteConnection` so the same code
//! runs on a reader connection or inside a [`crate::db::WriteTx`]. One file per
//! table group; add a `pub mod` line here for new ones.
//!
//! Repositories contain SQL and row mapping only. No authorization, no audit:
//! those belong to the service layer.

pub mod access;
pub mod account_tokens;
pub mod api_tokens;
pub mod audit;
pub mod deploy_keys;
pub mod deployments;
pub mod projects;
pub mod registry_credentials;
pub mod sessions;
pub mod settings;
pub mod users;

use crate::error::CoreError;

/// Map an sqlx error to an opaque internal error (details stay in the source chain).
pub fn db_err(e: sqlx::Error) -> CoreError {
    CoreError::internal("database error", e)
}

/// `true` when `e` is a UNIQUE/PRIMARY KEY constraint violation.
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|d| d.is_unique_violation())
}
