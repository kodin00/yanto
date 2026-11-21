//! SQLite online copy used by `self-update` (D31) via `VACUUM INTO`.

use std::path::Path;

use anyhow::Context;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

/// Copy `db` into a new file `dest` (must not exist), mode 0600.
pub fn backup(db: &Path, dest: &Path) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let opts = SqliteConnectOptions::new().filename(db).read_only(true);
        let mut conn = SqliteConnection::connect_with(&opts)
            .await
            .with_context(|| format!("opening {}", db.display()))?;
        // `VACUUM INTO` takes a string literal; escape single quotes.
        let target = dest.to_string_lossy().replace('\'', "''");
        sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{target}'")))
            .execute(&mut conn)
            .await
            .context("VACUUM INTO failed")?;
        conn.close().await?;
        anyhow::Ok(())
    })?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// `YYYYMMDDTHHMMSSZ` for a unix timestamp (civil-from-days, UTC).
pub fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    utc_stamp(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_known_values() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_700_000_000), "20231114T221320Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
    }

    #[test]
    fn backup_copies_a_migrated_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("yanto.db");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let d = yanto_core::Db::open(&db).await.unwrap();
            d.close().await;
        });
        let dest = dir.path().join("backups/yanto/pre.db");
        backup(&db, &dest).unwrap();
        assert!(dest.metadata().unwrap().len() > 0);
        rt.block_on(async {
            let d = yanto_core::Db::open(&dest).await.unwrap();
            d.ping().await.unwrap();
            d.close().await;
        });
    }
}
