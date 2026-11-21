//! Backups: domain types, the Yanto-DB archive and its restore (docs/rewrite/04 section 7).
//!
//! The Yanto-DB backup is a SQLite online snapshot (`VACUUM INTO`) bundled with `keys/app.key`
//! into one `.tar.zst` (D22). It lives entirely in the data dir, so it is implemented here; the
//! Project kinds (volumes, Postgres, MySQL) go through `ports::ProjectBackupPort`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

use crate::crypto::hex;
use crate::db::Db;
use crate::error::{CoreError, ErrorKind};

pub const DB_ENTRY: &str = "yanto.db";
pub const KEY_ENTRY: &str = "app.key";
const KEY_LEN: u64 = 32;
pub const ZSTD_LEVEL: i32 = 3;
/// Refuse to start a backup below this much free space, or below 2x the previous one (7.1).
pub const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_KEEP_LAST: u32 = 7;
pub const FAILED_RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    ProjectVolumes,
    ProjectPostgres,
    ProjectMysql,
    YantoSqlite,
}

impl BackupKind {
    pub const ALL: [BackupKind; 4] = [
        BackupKind::ProjectVolumes,
        BackupKind::ProjectPostgres,
        BackupKind::ProjectMysql,
        BackupKind::YantoSqlite,
    ];

    /// The value stored in `backups.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            BackupKind::ProjectVolumes => "project_volumes",
            BackupKind::ProjectPostgres => "project_postgres",
            BackupKind::ProjectMysql => "project_mysql",
            BackupKind::YantoSqlite => "yanto_sqlite",
        }
    }

    /// Accepts the stored value or the CLI/UI alias (`volumes`, `postgres`, `mysql`, `yanto`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "project_volumes" | "volumes" => BackupKind::ProjectVolumes,
            "project_postgres" | "postgres" => BackupKind::ProjectPostgres,
            "project_mysql" | "mysql" => BackupKind::ProjectMysql,
            "yanto_sqlite" | "yanto" => BackupKind::YantoSqlite,
            _ => return None,
        })
    }

    pub fn is_project(self) -> bool {
        self != BackupKind::YantoSqlite
    }

    /// File name suffix after the timestamp, e.g. `-yanto.tar.zst`.
    pub fn file_suffix(self) -> &'static str {
        match self {
            BackupKind::ProjectVolumes => "-volumes.tar.zst",
            BackupKind::ProjectPostgres => "-postgres.sql.zst",
            BackupKind::ProjectMysql => "-mysql.sql.zst",
            BackupKind::YantoSqlite => "-yanto.tar.zst",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Api,
    Cron,
    PreRestore,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Manual => "manual",
            Trigger::Api => "api",
            Trigger::Cron => "cron",
            Trigger::PreRestore => "pre_restore",
        }
    }
}

/// `20260131T140500Z` from unix milliseconds (UTC), used in file names.
pub fn timestamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

pub fn yanto_dir(data_dir: &Path) -> PathBuf {
    backups_dir(data_dir).join("yanto")
}

fn io_err(what: &'static str) -> impl Fn(std::io::Error) -> CoreError {
    move |e| CoreError::internal(what, e)
}

/// Create `dir` (and parents); the leaf and any new parent under `backups/` is `0700`.
pub fn ensure_private_dir(dir: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(dir).map_err(io_err("cannot create backup directory"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(io_err("cannot restrict backup directory"))?;
    }
    Ok(())
}

/// Create a new file readable only by the owner.
pub fn create_private(path: &Path) -> Result<std::fs::File, CoreError> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path).map_err(io_err("cannot create backup file"))
}

/// Resolve `path` and require it to live under `<data_dir>/backups` (path-traversal guard, 02 4.4).
pub fn within_backups(data_dir: &Path, path: &Path) -> Result<PathBuf, CoreError> {
    let root = std::fs::canonicalize(backups_dir(data_dir))
        .map_err(|_| CoreError::not_found("backup file is missing"))?;
    let real =
        std::fs::canonicalize(path).map_err(|_| CoreError::not_found("backup file is missing"))?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(CoreError::new(
            ErrorKind::Internal,
            "backup path escapes the backups directory",
        ))
    }
}

/// Writer that hashes and counts what passes through.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Result of writing an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub size_bytes: u64,
    pub sha256: String,
}

/// Tar + zstd the SQLite copy and `app.key` into `dest` (a new `0600` file). Blocking.
/// The key bytes only travel file to file; they are never logged or returned.
pub fn write_yanto_archive(
    db_copy: &Path,
    key_file: &Path,
    dest: &Path,
    mtime_secs: u64,
) -> Result<Written, CoreError> {
    let out = create_private(dest)?;
    let hw = HashingWriter {
        inner: std::io::BufWriter::new(out),
        hasher: Sha256::new(),
        written: 0,
    };
    let enc = zstd::Encoder::new(hw, ZSTD_LEVEL).map_err(io_err("cannot start compression"))?;
    let mut tar = tar::Builder::new(enc);
    for (name, path) in [(DB_ENTRY, db_copy), (KEY_ENTRY, key_file)] {
        let mut f = std::fs::File::open(path).map_err(|e| {
            if name == KEY_ENTRY {
                CoreError::internal(
                    "app.key is missing or unreadable; cannot build a restorable backup",
                    e,
                )
            } else {
                CoreError::internal("cannot read the database snapshot", e)
            }
        })?;
        let len = f
            .metadata()
            .map_err(io_err("cannot stat backup input"))?
            .len();
        let mut h = tar::Header::new_gnu();
        h.set_size(len);
        h.set_mode(0o600);
        h.set_mtime(mtime_secs);
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, name, &mut f)
            .map_err(io_err("cannot write backup archive"))?;
    }
    let enc = tar.into_inner().map_err(io_err("cannot finish archive"))?;
    let mut hw = enc.finish().map_err(io_err("cannot finish compression"))?;
    hw.flush().map_err(io_err("cannot flush backup archive"))?;
    let (size, digest) = (hw.written, hw.hasher.finalize());
    let file = hw
        .inner
        .into_inner()
        .map_err(|e| CoreError::internal("cannot flush backup archive", e.into_error()))?;
    file.sync_all()
        .map_err(io_err("cannot sync backup archive"))?;
    Ok(Written {
        size_bytes: size,
        sha256: hex(&digest),
    })
}

/// SHA-256 of a file, streaming.
pub fn sha256_file(path: &Path) -> Result<String, CoreError> {
    let mut f = std::fs::File::open(path).map_err(io_err("cannot read backup file"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(io_err("cannot read backup file"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Path of the `sha256sum`-format sidecar next to an archive.
pub fn sidecar_path(archive: &Path) -> PathBuf {
    let mut s = archive.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

pub fn write_sidecar(archive: &Path, sha256: &str) -> Result<(), CoreError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut f = create_private(&sidecar_path(archive))?;
    f.write_all(format!("{sha256}  {name}\n").as_bytes())
        .map_err(io_err("cannot write checksum file"))
}

/// Contents extracted from a Yanto archive.
#[derive(Debug)]
pub struct Extracted {
    pub db: PathBuf,
    pub key: PathBuf,
}

/// Extract exactly `yanto.db` and `app.key` into `into` (an existing private dir). Any other
/// entry, a duplicate, a non-regular file or a key of the wrong size is refused.
pub fn extract_yanto_archive(archive: &Path, into: &Path) -> Result<Extracted, CoreError> {
    let bad = |m: &str| CoreError::validation("archive", m.to_owned());
    let f = std::fs::File::open(archive).map_err(io_err("cannot open the archive"))?;
    let dec = zstd::Decoder::new(f).map_err(|_| bad("not a zstd archive"))?;
    let mut tar = tar::Archive::new(dec);
    let (mut got_db, mut got_key) = (false, false);
    for entry in tar.entries().map_err(|_| bad("not a tar archive"))? {
        let mut entry = entry.map_err(|_| bad("corrupt archive"))?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(bad("archive contains a non-regular entry"));
        }
        let name = entry
            .path()
            .map_err(|_| bad("corrupt entry name"))?
            .to_string_lossy()
            .into_owned();
        let (seen, limit) = match name.as_str() {
            DB_ENTRY => (&mut got_db, u64::MAX),
            KEY_ENTRY => (&mut got_key, KEY_LEN),
            _ => return Err(bad("archive contains an unexpected entry")),
        };
        if std::mem::replace(seen, true) {
            return Err(bad("archive contains a duplicate entry"));
        }
        if entry.header().size().map_err(|_| bad("corrupt entry"))? > limit {
            return Err(bad("archive entry has an invalid size"));
        }
        let mut out = create_private(&into.join(&name))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|_| bad("cannot extract (corrupt archive?)"))?;
        out.sync_all()
            .map_err(io_err("cannot sync extracted file"))?;
    }
    if !(got_db && got_key) {
        return Err(bad("archive must contain yanto.db and app.key"));
    }
    let key = into.join(KEY_ENTRY);
    if std::fs::metadata(&key)
        .map_err(io_err("cannot stat key"))?
        .len()
        != KEY_LEN
    {
        return Err(bad("archive entry has an invalid size"));
    }
    Ok(Extracted {
        db: into.join(DB_ENTRY),
        key,
    })
}

/// `VACUUM INTO` a consistent copy of the live database at `dest` (must not exist). Runs on a
/// reader connection so writers are not blocked (OQ-18).
pub async fn snapshot_db(db: &Db, dest: &Path) -> Result<(), CoreError> {
    sqlx::query("VACUUM INTO ?")
        .bind(dest.to_string_lossy().into_owned())
        .execute(db.reader())
        .await
        .map_err(|e| CoreError::internal("database snapshot failed", e))?;
    Ok(())
}

/// `PRAGMA integrity_check` on a database file; errors unless it answers `ok`.
pub async fn integrity_check(path: &Path) -> Result<(), CoreError> {
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .map_err(|e| CoreError::internal("cannot open the database copy", e))?;
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await
        .map_err(|e| CoreError::internal("integrity check failed to run", e))?;
    let _ = conn.close().await;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::Internal,
            format!("integrity check failed: {}", rows.join("; ")),
        ))
    }
}

/// How strictly [`restore_yanto`] checks the archive's checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    /// A `.sha256` sidecar must exist and match.
    Required,
    /// Verify when a sidecar exists, otherwise continue (explicit operator choice).
    IfPresent,
}

#[derive(Debug)]
pub struct RestoreReport {
    pub previous_db: Option<PathBuf>,
    pub previous_key: Option<PathBuf>,
    pub key_replaced: bool,
}

/// Restore the Yanto DB from an archive (04 7.6). The service must be stopped: the caller checks.
/// Steps: verify checksum, extract to a private temp dir, `integrity_check` the copy, move the
/// current DB aside as `yanto.db.pre-restore-<ts>`, install the restored DB (and `app.key` when it
/// differs, keeping the old one as `app.key.pre-restore-<ts>`), re-check, clean up.
pub async fn restore_yanto(
    data_dir: &Path,
    archive: &Path,
    now_ms: i64,
    verify: Verify,
) -> Result<RestoreReport, CoreError> {
    let sidecar = sidecar_path(archive);
    match (std::fs::read_to_string(&sidecar), verify) {
        (Ok(text), _) => {
            let expected = text
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            let actual = sha256_file(archive)?;
            if !crate::crypto::ct_eq(&expected, &actual) {
                return Err(CoreError::validation(
                    "archive",
                    "checksum mismatch: the archive is corrupted or was modified; nothing was changed",
                ));
            }
        }
        (Err(_), Verify::Required) => {
            return Err(CoreError::validation(
                "archive",
                "no .sha256 file next to the archive; only Yanto-created backups can be restored (or pass --allow-unverified)",
            ));
        }
        (Err(_), Verify::IfPresent) => {}
    }

    let tmp_root = data_dir.join("tmp");
    ensure_private_dir(&tmp_root)?;
    let tmp = tmp_root.join(format!("restore-{}", crate::crypto::random_hex::<6>()));
    ensure_private_dir(&tmp)?;
    let result = restore_from_tmp(data_dir, archive, now_ms, &tmp).await;
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

async fn restore_from_tmp(
    data_dir: &Path,
    archive: &Path,
    now_ms: i64,
    tmp: &Path,
) -> Result<RestoreReport, CoreError> {
    let extracted = extract_yanto_archive(archive, tmp)?;
    integrity_check(&extracted.db).await?;
    // Close out any leftover WAL files the check created next to the copy.
    for ext in ["-wal", "-shm"] {
        let mut p = extracted.db.as_os_str().to_owned();
        p.push(ext);
        let _ = std::fs::remove_file(p);
    }

    let ts = timestamp(now_ms);
    let db_path = data_dir.join(DB_ENTRY);
    let mut previous_db = None;
    if db_path.exists() {
        let aside = data_dir.join(format!("{DB_ENTRY}.pre-restore-{ts}"));
        std::fs::rename(&db_path, &aside)
            .map_err(io_err("cannot move the current database aside"))?;
        for ext in ["-wal", "-shm"] {
            let mut from = db_path.as_os_str().to_owned();
            from.push(ext);
            let mut to = aside.as_os_str().to_owned();
            to.push(ext);
            let _ = std::fs::rename(from, to);
        }
        previous_db = Some(aside);
    }
    std::fs::rename(&extracted.db, &db_path)
        .map_err(io_err("cannot install the restored database"))?;

    let key_path = crate::secrets::key_path(data_dir);
    let new_key = std::fs::read(&extracted.key).map_err(io_err("cannot read restored key"))?;
    let (mut previous_key, mut key_replaced) = (None, false);
    match std::fs::read(&key_path) {
        Ok(old) if old == new_key => {}
        existing => {
            ensure_private_dir(key_path.parent().expect("key path has a parent"))?;
            if existing.is_ok() {
                let aside = key_path.with_file_name(format!("{KEY_ENTRY}.pre-restore-{ts}"));
                std::fs::rename(&key_path, &aside)
                    .map_err(io_err("cannot move the current key aside"))?;
                previous_key = Some(aside);
            }
            std::fs::rename(&extracted.key, &key_path)
                .map_err(io_err("cannot install the restored key"))?;
            key_replaced = true;
        }
    }
    integrity_check(&db_path).await?;
    Ok(RestoreReport {
        previous_db,
        previous_key,
        key_replaced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_aliases() {
        for k in BackupKind::ALL {
            assert_eq!(BackupKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(BackupKind::parse("yanto"), Some(BackupKind::YantoSqlite));
        assert_eq!(
            BackupKind::parse("volumes"),
            Some(BackupKind::ProjectVolumes)
        );
        assert!(BackupKind::parse("r2").is_none());
        assert!(!BackupKind::YantoSqlite.is_project());
        assert!(BackupKind::ProjectPostgres.is_project());
        assert!(BackupKind::YantoSqlite.file_suffix().ends_with(".tar.zst"));
        assert!(BackupKind::ProjectMysql.file_suffix().ends_with(".sql.zst"));
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "19700101T000000Z");
        assert_eq!(timestamp(1_767_225_600_000), "20260101T000000Z");
        assert_eq!(timestamp(1_709_210_096_000), "20240229T123456Z");
    }

    #[test]
    fn archive_roundtrip_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let key = dir.path().join("key");
        std::fs::write(&db, b"not really sqlite").unwrap();
        std::fs::write(&key, [7u8; 32]).unwrap();
        let dest = dir.path().join("a.tar.zst");
        let w = write_yanto_archive(&db, &key, &dest, 1).unwrap();
        assert_eq!(w.size_bytes, std::fs::metadata(&dest).unwrap().len());
        assert_eq!(w.sha256, sha256_file(&dest).unwrap());
        let out = dir.path().join("out");
        ensure_private_dir(&out).unwrap();
        let x = extract_yanto_archive(&dest, &out).unwrap();
        assert_eq!(std::fs::read(x.db).unwrap(), b"not really sqlite");
        assert_eq!(std::fs::read(x.key).unwrap(), vec![7u8; 32]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    fn evil_archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let dest = dir.join("evil.tar.zst");
        let enc = zstd::Encoder::new(std::fs::File::create(&dest).unwrap(), 1).unwrap();
        let mut tar = tar::Builder::new(enc);
        for (name, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut h, name, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        dest
    }

    #[test]
    fn extraction_refuses_unexpected_entries() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        ensure_private_dir(&out).unwrap();
        let key = [1u8; 32];
        for entries in [
            vec![
                ("yanto.db", &b"x"[..]),
                ("app.key", &key[..]),
                ("extra", &b"y"[..]),
            ],
            vec![("yanto.db", &b"x"[..])],
            vec![("yanto.db", &b"x"[..]), ("app.key", &b"short"[..])],
            vec![
                ("yanto.db", &b"x"[..]),
                ("yanto.db", &b"x"[..]),
                ("app.key", &key[..]),
            ],
        ] {
            let a = evil_archive(dir.path(), &entries);
            assert!(extract_yanto_archive(&a, &out).is_err(), "{entries:?}");
            let _ = std::fs::remove_file(out.join("yanto.db"));
            let _ = std::fs::remove_file(out.join("app.key"));
        }
        let not_zstd = dir.path().join("plain");
        std::fs::write(&not_zstd, b"hello").unwrap();
        assert!(extract_yanto_archive(&not_zstd, &out).is_err());
    }

    #[test]
    fn within_backups_blocks_escapes() {
        let dir = tempfile::tempdir().unwrap();
        ensure_private_dir(&yanto_dir(dir.path())).unwrap();
        let inside = yanto_dir(dir.path()).join("a");
        std::fs::write(&inside, b"x").unwrap();
        assert!(within_backups(dir.path(), &inside).is_ok());
        let outside = dir.path().join("secret");
        std::fs::write(&outside, b"x").unwrap();
        assert!(within_backups(dir.path(), &outside).is_err());
        let dotdot = yanto_dir(dir.path()).join("../../secret");
        assert!(within_backups(dir.path(), &dotdot).is_err());
        assert!(within_backups(dir.path(), &yanto_dir(dir.path()).join("nope")).is_err());
    }
}
