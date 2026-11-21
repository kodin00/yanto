//! `users` table.

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::actor::Role;
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Invited,
    Active,
    Disabled,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UserStatus::Invited => "invited",
            UserStatus::Active => "active",
            UserStatus::Disabled => "disabled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "invited" => Some(Self::Invited),
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub role: String,
    pub status: String,
    pub password_hash: Option<String>,
    pub last_login_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl UserRow {
    pub fn role(&self) -> Role {
        Role::parse(&self.role).unwrap_or(Role::Member)
    }

    pub fn status(&self) -> UserStatus {
        UserStatus::parse(&self.status).unwrap_or(UserStatus::Disabled)
    }
}

const COLS: &str =
    "id, username, role, status, password_hash, last_login_at, created_at, updated_at";

pub async fn insert(
    conn: &mut SqliteConnection,
    id: &str,
    username: &str,
    role: Role,
    status: UserStatus,
    password_hash: Option<&str>,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO users (id, username, role, status, password_hash, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(username)
    .bind(role.as_str())
    .bind(status.as_str())
    .bind(password_hash)
    .bind(now)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("that username (or an Owner) already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<UserRow>, CoreError> {
    let sql = format!("SELECT {COLS} FROM users WHERE id = ?");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn by_username(
    conn: &mut SqliteConnection,
    username: &str,
) -> Result<Option<UserRow>, CoreError> {
    let sql = format!("SELECT {COLS} FROM users WHERE username = ?");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(username)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn owner(conn: &mut SqliteConnection) -> Result<Option<UserRow>, CoreError> {
    let sql = format!("SELECT {COLS} FROM users WHERE role = 'owner'");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<UserRow>, CoreError> {
    let sql =
        format!("SELECT {COLS} FROM users ORDER BY role = 'owner' DESC, username COLLATE NOCASE");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

/// Set the password and activate the account (invite/reset acceptance, password change).
pub async fn set_password_active(
    conn: &mut SqliteConnection,
    id: &str,
    hash: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE users SET password_hash = ?, status = 'active', updated_at = ? WHERE id = ?",
    )
    .bind(hash)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

/// Password reset: back to `invited` with no password (legacy behavior).
pub async fn clear_password_invited(
    conn: &mut SqliteConnection,
    id: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE users SET password_hash = NULL, status = 'invited', updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn set_status(
    conn: &mut SqliteConnection,
    id: &str,
    status: UserStatus,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE users SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn touch_login(conn: &mut SqliteConnection, id: &str, now: i64) -> Result<(), CoreError> {
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}
