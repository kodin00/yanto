//! Service-layer tests. Shared helpers live in [`crate::testing`].

mod audit;
mod auth;
mod authorized_keys;
mod backups;
mod containers;
mod cron;
mod matrix;
mod system;
mod tokens;
mod users;

use crate::ErrorKind;
use crate::error::CoreError;

pub(crate) fn kind<T>(r: Result<T, CoreError>) -> ErrorKind {
    r.err().expect("expected an error").kind
}

/// Count audit rows with `action` (optionally also `outcome`).
pub(crate) async fn audit_count(
    core: &crate::testing::TestCore,
    action: &str,
    outcome: &str,
) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = ? AND outcome = ?")
        .bind(action)
        .bind(outcome)
        .fetch_one(core.db.reader())
        .await
        .unwrap()
}
