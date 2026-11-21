//! Project volume archives and Postgres/MySQL dumps through the `docker` CLI (docs/rewrite/04
//! section 7; the CLI is used like everywhere else in M2, see m2-deviations).
//!
//! * Volumes: a throwaway helper container (no network, volume mounted read-only, label
//!   `yanto.managed=backup-helper`) runs `tar` and Yanto compresses its stdout with zstd.
//! * Dumps: `docker exec` into the declared service's running container. Credentials are read
//!   from the container's own environment by the shell inside it, so no secret reaches Yanto or
//!   a process list on the Host; only non-secret names are passed as `-e YANTO_DB=...`.
//! * Restores stream the decompressed file into the same helper or into `psql`/`mysql`.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use yanto_core::CoreError;
use yanto_core::backup::{BackupKind, ZSTD_LEVEL};
use yanto_core::ports::{
    DbTarget, ProjectBackupPort, ProjectBackupRequest, ProjectRestoreRequest, RestoreMode,
};

const COMPOSE_PROJECT: &str = "com.docker.compose.project";
const COMPOSE_SERVICE: &str = "com.docker.compose.service";
const CHUNK: usize = 64 * 1024;
const STDERR_TAIL: usize = 4 * 1024;
/// ASSUMPTION (04 7.2): the helper only needs `tar` and `sh`.
pub const DEFAULT_HELPER_IMAGE: &str = "busybox:stable";

#[derive(Debug, Clone)]
pub struct DockerProjectBackups {
    docker: String,
    helper_image: String,
}

impl Default for DockerProjectBackups {
    fn default() -> Self {
        Self::new("docker", DEFAULT_HELPER_IMAGE)
    }
}

fn unavailable(what: &str, e: &std::io::Error) -> CoreError {
    CoreError::unavailable(
        format!("cannot run docker for {what}: {}", e.kind()),
        std::io::Error::new(e.kind(), e.to_string()),
    )
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> CoreError {
    move |e| CoreError::internal(what, e)
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

async fn read_tail<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Vec<u8> {
    let mut all = Vec::new();
    let mut buf = vec![0u8; 8192];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        if all.len() > 4 * STDERR_TAIL {
            all.drain(..all.len() - STDERR_TAIL);
        }
    }
    all
}

/// Runs `docker compose -p <slug> start` when dropped while armed, so a cancelled or panicking
/// backup never leaves the Project stopped (04 7.3).
struct StartGuard {
    docker: String,
    slug: String,
    armed: bool,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::process::Command::new(&self.docker)
                .args(["compose", "-p", &self.slug, "start"])
                .stdin(Stdio::null())
                .output();
        }
    }
}

impl DockerProjectBackups {
    pub fn new(docker: impl Into<String>, helper_image: impl Into<String>) -> Self {
        Self {
            docker: docker.into(),
            helper_image: helper_image.into(),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(&self.docker);
        c.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        c
    }

    async fn run(&self, args: &[&str]) -> Result<Out, CoreError> {
        let o = self
            .cmd(args)
            .output()
            .await
            .map_err(|e| unavailable("a backup", &e))?;
        Ok(Out {
            code: o.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: tail(&o.stderr),
        })
    }

    /// The volume must belong to the Compose project, so a Project's `backups` permission cannot
    /// reach another Project's data (and `docker run -v` would silently create a missing one).
    async fn check_volume(&self, slug: &str, volume: &str) -> Result<(), CoreError> {
        let o = self
            .run(&[
                "volume",
                "inspect",
                "--format",
                &format!("{{{{ index .Labels \"{COMPOSE_PROJECT}\" }}}}"),
                volume,
            ])
            .await?;
        if o.code != 0 {
            return Err(CoreError::validation(
                "target",
                format!("volume {volume} does not exist"),
            ));
        }
        if o.stdout.trim() != slug {
            return Err(CoreError::validation(
                "target",
                format!("volume {volume} does not belong to this Project"),
            ));
        }
        Ok(())
    }

    /// Pull the helper image once when it is not present (04 7.8).
    async fn ensure_helper(&self) -> Result<(), CoreError> {
        if self
            .run(&["image", "inspect", &self.helper_image])
            .await?
            .code
            == 0
        {
            return Ok(());
        }
        let o = self.run(&["pull", "--quiet", &self.helper_image]).await?;
        if o.code != 0 {
            return Err(CoreError::unavailable(
                format!(
                    "helper image {} is missing and could not be pulled: {}",
                    self.helper_image, o.stderr
                ),
                std::io::Error::other("pull failed"),
            ));
        }
        Ok(())
    }

    /// Id of the running container of a declared service.
    async fn running_container(&self, slug: &str, service: &str) -> Result<String, CoreError> {
        let o = self
            .run(&[
                "ps",
                "--no-trunc",
                "--quiet",
                "--filter",
                &format!("label={COMPOSE_PROJECT}={slug}"),
                "--filter",
                &format!("label={COMPOSE_SERVICE}={service}"),
                "--filter",
                "status=running",
            ])
            .await?;
        if o.code != 0 {
            return Err(CoreError::unavailable(
                format!("docker ps failed: {}", o.stderr),
                std::io::Error::other("docker ps"),
            ));
        }
        o.stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                CoreError::new(
                    yanto_core::ErrorKind::Unavailable,
                    format!("service {service} must be running"),
                )
            })
    }

    /// Run `cmd`, compress its stdout with zstd into the new `0600` file `dest`.
    async fn stream_to_zstd(
        &self,
        mut cmd: Command,
        dest: &Path,
        what: &str,
    ) -> Result<(), CoreError> {
        let out = yanto_core::backup::create_private(dest)?;
        let mut child = cmd.spawn().map_err(|e| unavailable(what, &e))?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let err_task = tokio::spawn(read_tail(stderr));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let writer = tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            let mut enc = zstd::Encoder::new(std::io::BufWriter::new(out), ZSTD_LEVEL)
                .map_err(io("cannot start compression"))?;
            while let Some(chunk) = rx.blocking_recv() {
                std::io::Write::write_all(&mut enc, &chunk)
                    .map_err(io("cannot write the backup"))?;
            }
            let mut w = enc.finish().map_err(io("cannot finish compression"))?;
            std::io::Write::flush(&mut w).map_err(io("cannot flush the backup"))?;
            let f = w
                .into_inner()
                .map_err(|e| CoreError::internal("cannot flush the backup", e.into_error()))?;
            f.sync_all().map_err(io("cannot sync the backup"))
        });
        let mut buf = vec![0u8; CHUNK];
        let mut read_err = None;
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break; // the writer failed; its error is reported below
                    }
                }
                Err(e) => {
                    read_err = Some(e);
                    break;
                }
            }
        }
        drop(tx);
        let written = writer
            .await
            .map_err(|e| CoreError::internal("backup writer failed", e))?;
        let status = child.wait().await.map_err(|e| unavailable(what, &e))?;
        let err_text = tail(&err_task.await.unwrap_or_default());
        written?;
        if let Some(e) = read_err {
            return Err(CoreError::internal("cannot read the backup stream", e));
        }
        if !status.success() {
            return Err(failure(what, status.code(), &err_text));
        }
        Ok(())
    }

    /// Run `cmd` with the decompressed contents of `source` on stdin.
    async fn feed_from_zstd(
        &self,
        mut cmd: Command,
        source: &Path,
        what: &str,
    ) -> Result<(), CoreError> {
        cmd.stdin(Stdio::piped());
        let file = std::fs::File::open(source).map_err(io("cannot open the backup file"))?;
        let mut child = cmd.spawn().map_err(|e| unavailable(what, &e))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let err_task = tokio::spawn(read_tail(stderr));
        let out_task = tokio::spawn(read_tail(stdout));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, CoreError>>(8);
        let reader = tokio::task::spawn_blocking(move || {
            let mut dec = match zstd::Decoder::new(file) {
                Ok(d) => d,
                Err(_) => {
                    let _ =
                        tx.blocking_send(Err(CoreError::validation("backup", "not a zstd file")));
                    return;
                }
            };
            let mut buf = vec![0u8; CHUNK];
            loop {
                match std::io::Read::read(&mut dec, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = tx.blocking_send(Err(CoreError::validation(
                            "backup",
                            "the backup file is corrupt",
                        )));
                        return;
                    }
                }
            }
        });
        let mut feed_err: Option<CoreError> = None;
        while let Some(chunk) = rx.recv().await {
            match chunk {
                Ok(c) => {
                    if stdin.write_all(&c).await.is_err() {
                        break; // the process exited early; its status explains why
                    }
                }
                Err(e) => {
                    feed_err = Some(e);
                    break;
                }
            }
        }
        drop(rx);
        drop(stdin);
        let _ = reader.await;
        let status = child.wait().await.map_err(|e| unavailable(what, &e))?;
        let err_text = tail(&err_task.await.unwrap_or_default());
        let _ = out_task.await;
        if let Some(e) = feed_err {
            return Err(e);
        }
        if !status.success() {
            return Err(failure(what, status.code(), &err_text));
        }
        Ok(())
    }

    fn volume_run(&self, volume: &str, mode: &str, interactive: bool, script: &[&str]) -> Command {
        let bind = format!("{volume}:/data:{mode}");
        let mut args = vec!["run", "--rm"];
        if interactive {
            args.push("-i");
        }
        args.extend([
            "--network",
            "none",
            "--label",
            "yanto.managed=backup-helper",
            "-v",
            &bind,
            &self.helper_image,
        ]);
        args.extend(script);
        self.cmd(&args)
    }

    async fn stop_project(&self, slug: &str) -> Result<StartGuard, CoreError> {
        let o = self.run(&["compose", "-p", slug, "stop"]).await?;
        if o.code != 0 {
            return Err(CoreError::unavailable(
                format!("cannot stop the Project: {}", o.stderr),
                std::io::Error::other("compose stop"),
            ));
        }
        Ok(StartGuard {
            docker: self.docker.clone(),
            slug: slug.to_owned(),
            armed: true,
        })
    }

    async fn start_project(&self, slug: &str) -> Result<(), CoreError> {
        let o = self.run(&["compose", "-p", slug, "start"]).await?;
        if o.code != 0 {
            return Err(CoreError::unavailable(
                format!("the Project could not be started again: {}", o.stderr),
                std::io::Error::other("compose start"),
            ));
        }
        Ok(())
    }
}

fn failure(what: &str, code: Option<i32>, stderr: &str) -> CoreError {
    let code = code.map_or_else(|| "a signal".to_owned(), |c| format!("exit code {c}"));
    CoreError::new(
        yanto_core::ErrorKind::Unavailable,
        if stderr.is_empty() {
            format!("{what} failed ({code})")
        } else {
            format!("{what} failed ({code}): {stderr}")
        },
    )
}

/// `${NAME}` for a declared variable name, or the engine default expression.
fn var(declared: Option<&str>, default: &str) -> String {
    declared.map_or_else(|| default.to_owned(), |n| format!("${{{n}}}"))
}

/// The shell program run inside the database container. Names are validated by the service
/// (`^[A-Za-z_][A-Za-z0-9_]*$`), so interpolating them is safe; the database name arrives as
/// `$YANTO_DB`, never inside the script text.
fn db_script(kind: BackupKind, db: &DbTarget, restore: bool) -> String {
    match kind {
        BackupKind::ProjectPostgres => {
            let user = db.user_env.as_deref().map_or_else(
                || "${POSTGRES_USER:-postgres}".to_owned(),
                |n| format!("${{{n}:-postgres}}"),
            );
            let pass = var(db.password_env.as_deref(), "${POSTGRES_PASSWORD}");
            let name = format!("${{YANTO_DB:-${{POSTGRES_DB:-{user}}}}}");
            let tool = if restore {
                format!(
                    "psql -q -U \"{user}\" -d \"{name}\" -v ON_ERROR_STOP=1 --single-transaction"
                )
            } else {
                format!("pg_dump -U \"{user}\" -d \"{name}\" --clean --if-exists --no-owner")
            };
            format!("PGPASSWORD=\"{pass}\"; export PGPASSWORD; exec {tool}")
        }
        _ => {
            let user = db
                .user_env
                .as_deref()
                .map_or_else(|| "root".to_owned(), |n| format!("${{{n}:-root}}"));
            let pass = var(
                db.password_env.as_deref(),
                "${MYSQL_ROOT_PASSWORD:-$MARIADB_ROOT_PASSWORD}",
            );
            let find = if restore {
                "bin=\"$(command -v mariadb || command -v mysql)\" || { echo 'no mariadb or mysql client in the container' >&2; exit 127; }"
            } else {
                "bin=\"$(command -v mariadb-dump || command -v mysqldump)\" || { echo 'no mariadb-dump or mysqldump in the container' >&2; exit 127; }"
            };
            let run = if restore {
                format!("exec \"$bin\" -u \"{user}\"")
            } else {
                format!(
                    "db=\"${{YANTO_DB:-${{MYSQL_DATABASE:-$MARIADB_DATABASE}}}}\"; \
                     if [ -n \"$db\" ]; then exec \"$bin\" -u \"{user}\" --single-transaction --routines --triggers --databases \"$db\"; \
                     else exec \"$bin\" -u \"{user}\" --single-transaction --routines --triggers --all-databases; fi"
                )
            };
            format!("{find}; MYSQL_PWD=\"{pass}\"; export MYSQL_PWD; {run}")
        }
    }
}

impl DockerProjectBackups {
    fn exec_cmd(&self, container: &str, db: &DbTarget, script: &str, interactive: bool) -> Command {
        let mut args: Vec<String> = vec!["exec".into()];
        if interactive {
            args.push("-i".into());
        }
        if let Some(name) = &db.database {
            args.push("-e".into());
            args.push(format!("YANTO_DB={name}"));
        }
        args.extend([
            container.to_owned(),
            "sh".into(),
            "-c".into(),
            script.to_owned(),
        ]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cmd(&refs)
    }
}

#[async_trait::async_trait]
impl ProjectBackupPort for DockerProjectBackups {
    async fn create(&self, req: &ProjectBackupRequest, dest: &Path) -> Result<(), CoreError> {
        match req.kind {
            BackupKind::ProjectVolumes => {
                let volume = req
                    .target
                    .as_deref()
                    .ok_or_else(|| CoreError::validation("target", "name the volume to archive"))?;
                self.check_volume(&req.project_slug, volume).await?;
                self.ensure_helper().await?;
                let mut guard = if req.stop_project {
                    Some(self.stop_project(&req.project_slug).await?)
                } else {
                    None
                };
                let cmd = self.volume_run(
                    volume,
                    "ro",
                    false,
                    &["tar", "-C", "/data", "-cf", "-", "."],
                );
                let r = self.stream_to_zstd(cmd, dest, "the volume archive").await;
                if let Some(g) = guard.as_mut() {
                    self.start_project(&req.project_slug).await?;
                    g.armed = false;
                }
                r
            }
            BackupKind::ProjectPostgres | BackupKind::ProjectMysql => {
                let db = req.db.as_ref().ok_or_else(|| {
                    CoreError::validation("target", "no database declaration for this backup")
                })?;
                let container = self
                    .running_container(&req.project_slug, &db.compose_service)
                    .await?;
                let script = db_script(req.kind, db, false);
                let cmd = self.exec_cmd(&container, db, &script, false);
                self.stream_to_zstd(cmd, dest, "the database dump").await
            }
            BackupKind::YantoSqlite => {
                Err(CoreError::validation("kind", "not a Project backup kind"))
            }
        }
    }

    async fn restore(&self, req: &ProjectRestoreRequest, source: &Path) -> Result<(), CoreError> {
        match req.kind {
            BackupKind::ProjectVolumes => {
                let volume = req
                    .target
                    .as_deref()
                    .ok_or_else(|| CoreError::validation("target", "name the volume to restore"))?;
                self.ensure_helper().await?;
                match &req.mode {
                    RestoreMode::NewVolume(name) => {
                        let label = format!("{COMPOSE_PROJECT}={}", req.project_slug);
                        let o = self
                            .run(&[
                                "volume",
                                "create",
                                "--label",
                                &label,
                                "--label",
                                "yanto.managed=restore",
                                name,
                            ])
                            .await?;
                        if o.code != 0 {
                            return Err(failure(
                                "creating the new volume",
                                Some(o.code),
                                &o.stderr,
                            ));
                        }
                        let cmd =
                            self.volume_run(name, "rw", true, &["tar", "-C", "/data", "-xf", "-"]);
                        let r = self.feed_from_zstd(cmd, source, "the volume restore").await;
                        if r.is_err() {
                            // The new volume is ours and holds a partial extract: remove it.
                            let _ = self.run(&["volume", "rm", "--force", name]).await;
                        }
                        r
                    }
                    RestoreMode::InPlace => {
                        self.check_volume(&req.project_slug, volume).await?;
                        // On failure the Project stays stopped: the data may be half written.
                        let mut stopped = self.stop_project(&req.project_slug).await?;
                        stopped.armed = false;
                        let cmd = self.volume_run(
                            volume,
                            "rw",
                            true,
                            &[
                                "sh",
                                "-c",
                                "set -e; find /data -mindepth 1 -maxdepth 1 -exec rm -rf {} +; tar -C /data -xf -",
                            ],
                        );
                        self.feed_from_zstd(cmd, source, "the volume restore")
                            .await?;
                        self.start_project(&req.project_slug).await
                    }
                }
            }
            BackupKind::ProjectPostgres | BackupKind::ProjectMysql => {
                let db = req.db.as_ref().ok_or_else(|| {
                    CoreError::validation("target", "no database declaration for this restore")
                })?;
                let container = self
                    .running_container(&req.project_slug, &db.compose_service)
                    .await?;
                let script = db_script(req.kind, db, true);
                let cmd = self.exec_cmd(&container, db, &script, true);
                self.feed_from_zstd(cmd, source, "the database restore")
                    .await
            }
            BackupKind::YantoSqlite => {
                Err(CoreError::validation("kind", "not a Project backup kind"))
            }
        }
    }

    async fn list_volumes(&self, slug: &str) -> Result<Vec<String>, CoreError> {
        let o = self
            .run(&[
                "volume",
                "ls",
                "--quiet",
                "--filter",
                &format!("label={COMPOSE_PROJECT}={slug}"),
            ])
            .await?;
        if o.code != 0 {
            return Err(CoreError::unavailable(
                format!("docker volume ls failed: {}", o.stderr),
                std::io::Error::other("docker volume ls"),
            ));
        }
        let mut v: Vec<String> = o
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        v.sort();
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(user: Option<&str>, pass: Option<&str>) -> DbTarget {
        DbTarget {
            compose_service: "db".into(),
            database: None,
            user_env: user.map(str::to_owned),
            password_env: pass.map(str::to_owned),
        }
    }

    #[test]
    fn postgres_script_reads_credentials_inside_the_container() {
        let s = db_script(BackupKind::ProjectPostgres, &target(None, None), false);
        assert!(s.contains("${POSTGRES_PASSWORD}") && s.contains("${POSTGRES_USER:-postgres}"));
        assert!(s.contains("exec pg_dump") && s.contains("--clean --if-exists --no-owner"));
        assert!(s.contains("${YANTO_DB:-${POSTGRES_DB:-"));
        let custom = db_script(
            BackupKind::ProjectPostgres,
            &target(Some("APP_USER"), Some("APP_PASS")),
            true,
        );
        assert!(custom.contains("${APP_PASS}") && custom.contains("${APP_USER:-postgres}"));
        assert!(
            custom.contains("exec psql") && custom.contains("ON_ERROR_STOP=1 --single-transaction")
        );
    }

    #[test]
    fn mysql_script_picks_the_binary_available_in_the_image() {
        let s = db_script(BackupKind::ProjectMysql, &target(None, None), false);
        assert!(s.contains("mariadb-dump") && s.contains("mysqldump"));
        assert!(s.contains("--single-transaction --routines --triggers --databases"));
        assert!(s.contains("MYSQL_ROOT_PASSWORD"));
        let r = db_script(
            BackupKind::ProjectMysql,
            &target(Some("U"), Some("P")),
            true,
        );
        assert!(
            r.contains("command -v mariadb ") && r.contains("${P}") && r.contains("${U:-root}")
        );
    }
}
