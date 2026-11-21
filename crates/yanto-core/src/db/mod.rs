//! SQLite pools and embedded migrations (docs/rewrite/01 section 6.4).
//!
//! One writer pool with a single connection plus a read-only pool, so writes
//! never contend with each other and readers never block on the writer (WAL).

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::CoreError;

pub mod repo;

/// A write transaction on the single writer connection (`BEGIN IMMEDIATE`).
pub type WriteTx = sqlx::Transaction<'static, sqlx::Sqlite>;

const READER_CONNECTIONS: u32 = 4;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Migrations embedded at compile time from `crates/yanto-core/migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone)]
pub struct Db {
    writer: SqlitePool,
    reader: SqlitePool,
}

fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .pragma("temp_store", "MEMORY")
        .pragma("trusted_schema", "OFF")
}

impl Db {
    /// Open (creating if needed) the database at `path`, apply pending
    /// migrations, and open the reader pool.
    pub async fn open(path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            ensure_private_dir(parent)?;
        }
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options(path).create_if_missing(true))
            .await
            .map_err(|e| CoreError::unavailable("cannot open database", e))?;
        MIGRATOR
            .run(&writer)
            .await
            .map_err(|e| CoreError::internal("database migration failed", e))?;
        let reader = SqlitePoolOptions::new()
            .max_connections(READER_CONNECTIONS)
            .connect_with(options(path).read_only(true))
            .await
            .map_err(|e| CoreError::unavailable("cannot open database for reading", e))?;
        Ok(Self { writer, reader })
    }

    pub fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    pub fn reader(&self) -> &SqlitePool {
        &self.reader
    }

    /// Start a write transaction with `BEGIN IMMEDIATE` so a read-then-write
    /// never fails with a lock upgrade error (docs/rewrite/01 section 6.4).
    pub async fn begin_write(&self) -> Result<WriteTx, CoreError> {
        self.writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(repo::db_err)
    }

    /// Cheap liveness probe used by `/healthz`.
    pub async fn ping(&self) -> Result<(), CoreError> {
        sqlx::query("SELECT 1")
            .execute(&self.reader)
            .await
            .map(|_| ())
            .map_err(|e| CoreError::unavailable("database unreachable", e))
    }

    pub async fn close(&self) {
        self.reader.close().await;
        self.writer.close().await;
    }
}

/// Create `dir` (and parents) if missing; a newly created leaf is `0700` on Unix.
fn ensure_private_dir(dir: &Path) -> Result<(), CoreError> {
    let existed = dir.exists();
    std::fs::create_dir_all(dir)
        .map_err(|e| CoreError::internal("cannot create data directory", e))?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| CoreError::internal("cannot restrict data directory permissions", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("data").join("yanto.db"))
            .await
            .unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn migrations_apply_on_fresh_db() {
        let (_dir, db) = temp_db().await;
        let applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(applied, MIGRATOR.migrations.len() as i64);
        let table: String =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE name = 'app_settings'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(table, "app_settings");
    }

    #[tokio::test]
    async fn reopening_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yanto.db");
        Db::open(&path).await.unwrap().close().await;
        let db = Db::open(&path).await.unwrap();
        db.ping().await.unwrap();
    }

    #[tokio::test]
    async fn pragmas_are_set() {
        let (_dir, db) = temp_db().await;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(db.writer())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.writer())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(db.writer())
            .await
            .unwrap();
        assert_eq!(busy, 5000);
    }

    #[tokio::test]
    async fn app_settings_enforces_json_and_reader_is_read_only() {
        let (_dir, db) = temp_db().await;
        let bad = sqlx::query(
            "INSERT INTO app_settings (key, value, updated_at) VALUES ('k', 'not json', 0)",
        )
        .execute(db.writer())
        .await;
        assert!(bad.is_err());
        sqlx::query("INSERT INTO app_settings (key, value, updated_at) VALUES ('k', '2', 0)")
            .execute(db.writer())
            .await
            .unwrap();
        let v: String = sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'k'")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(v, "2");
        let write = sqlx::query("DELETE FROM app_settings")
            .execute(db.reader())
            .await;
        assert!(write.is_err());
    }

    async fn exec(db: &Db, sql: &'static str) -> Result<(), sqlx::Error> {
        sqlx::query(sql).execute(db.writer()).await.map(|_| ())
    }

    const OWNER: &str = "INSERT INTO users (id, username, role, status, password_hash, created_at, updated_at) VALUES ('usr_o', 'owner', 'owner', 'active', 'h', 0, 0)";

    #[tokio::test]
    async fn identity_schema_enforces_single_immutable_owner() {
        let (_dir, db) = temp_db().await;
        exec(&db, OWNER).await.unwrap();
        // A second Owner, a duplicate username (NOCASE), and an active user without a password all fail.
        assert!(exec(&db, "INSERT INTO users (id, username, role, status, password_hash, created_at, updated_at) VALUES ('usr_2', 'o2', 'owner', 'active', 'h', 0, 0)").await.is_err());
        assert!(exec(&db, "INSERT INTO users (id, username, role, status, created_at, updated_at) VALUES ('usr_3', 'OWNER', 'member', 'invited', 0, 0)").await.is_err());
        assert!(exec(&db, "INSERT INTO users (id, username, role, status, created_at, updated_at) VALUES ('usr_4', 'm', 'member', 'active', 0, 0)").await.is_err());
        // The Owner cannot be deleted, disabled, or demoted.
        assert!(
            exec(&db, "DELETE FROM users WHERE id = 'usr_o'")
                .await
                .is_err()
        );
        assert!(
            exec(
                &db,
                "UPDATE users SET status = 'disabled' WHERE id = 'usr_o'"
            )
            .await
            .is_err()
        );
        assert!(
            exec(&db, "UPDATE users SET role = 'member' WHERE id = 'usr_o'")
                .await
                .is_err()
        );
        // The Owner has no access rows.
        assert!(exec(&db, "INSERT INTO user_project_access (user_id, project_id, created_at, updated_at) VALUES ('usr_o', 'prj_1', 0, 0)").await.is_err());
    }

    #[tokio::test]
    async fn access_permissions_are_checked_and_cascade() {
        let (_dir, db) = temp_db().await;
        exec(&db, OWNER).await.unwrap();
        exec(&db, "INSERT INTO users (id, username, role, status, created_at, updated_at) VALUES ('usr_m', 'm', 'member', 'invited', 0, 0)").await.unwrap();
        exec(&db, "INSERT INTO user_project_access (user_id, project_id, created_at, updated_at) VALUES ('usr_m', 'prj_1', 0, 0)").await.unwrap();
        exec(&db, "INSERT INTO user_project_permissions (user_id, project_id, permission) VALUES ('usr_m', 'prj_1', 'deploy')").await.unwrap();
        assert!(exec(&db, "INSERT INTO user_project_permissions (user_id, project_id, permission) VALUES ('usr_m', 'prj_1', 'tasks')").await.is_err());
        assert!(exec(&db, "INSERT INTO user_project_permissions (user_id, project_id, permission) VALUES ('usr_m', 'prj_2', 'deploy')").await.is_err(), "permission needs an access row");
        exec(&db, "DELETE FROM users WHERE id = 'usr_m'")
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_project_permissions")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
