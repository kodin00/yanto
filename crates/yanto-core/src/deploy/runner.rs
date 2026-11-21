//! Runs one Deployment (docs/rewrite/04 1.3): git, config files, registry auth, pull,
//! declared ports, `docker compose up`. Every host effect goes through the [`CommandPort`];
//! output streams to the Deployment log; all failures end as a terminal status.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::compose::{self, DesiredPort};
use super::engine::Engine;
use super::files::{self, KeyFile};
use super::hub::HubEvent;
use super::logs::{self, DeployLogger, Redactor};
use crate::actor::Channel;
use crate::crypto::new_id;
use crate::db::repo::deployments::{self, DeploymentRow};
use crate::db::repo::projects::{self, PortRow, ProjectRow};
use crate::db::repo::{db_err, deploy_keys, registry_credentials};
use crate::error::CoreError;
use crate::ports::{
    AuthEntry, CancellationToken, CommandOutput, CommandSpec, EXIT_CANCELLED, LogSink,
};
use crate::secrets::Secret;
use crate::service::audit::{self, AuditActor, AuditEvent};

const GIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const PULL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const QUICK_TIMEOUT: Duration = Duration::from_secs(60);
pub const OVERRIDE_FILE: &str = ".yanto.restart.override.yml";

#[derive(Debug)]
pub struct StepError {
    pub message: String,
    pub cancelled: bool,
    pub exit_code: Option<i64>,
}

impl StepError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            exit_code: None,
        }
    }

    fn cancelled() -> Self {
        Self {
            message: "cancelled".into(),
            cancelled: true,
            exit_code: Some(i64::from(EXIT_CANCELLED)),
        }
    }
}

impl From<CoreError> for StepError {
    fn from(e: CoreError) -> Self {
        // Internal details stay in the log of the process, not in the Deployment row.
        tracing::warn!(error = %e, source = ?e.source, "deployment step failed");
        StepError::failed(e.message)
    }
}

type Step<T> = Result<T, StepError>;

struct RegGuard {
    eng: Arc<Engine>,
    scope: String,
}

impl Drop for RegGuard {
    fn drop(&mut self) {
        self.eng.host.registry_auth.remove(&self.scope);
    }
}

struct Run {
    eng: Arc<Engine>,
    dep: DeploymentRow,
    project: ProjectRow,
    dir: PathBuf,
    log: Arc<DeployLogger>,
    cancel: CancellationToken,
    /// Environment for every docker command (`DOCKER_CONFIG` when registry auth is in use).
    docker_env: Vec<(String, String)>,
}

/// Execute `dep` to a terminal status. Never panics on step failures.
pub async fn execute(eng: Arc<Engine>, dep: DeploymentRow, cancel: CancellationToken) {
    let id = dep.id.clone();
    let project = match load_project(&eng, &dep.project_id).await {
        Ok(Some(p)) => p,
        _ => {
            finalize(
                &eng,
                &dep,
                None,
                Err(StepError::failed("project no longer exists")),
            )
            .await;
            return;
        }
    };
    let redactor = build_redactor(&eng, &project, &dep).await;
    let handle = logs::spawn(
        eng.clone(),
        dep.id.clone(),
        dep.project_id.clone(),
        redactor,
    );
    eng.hub.publish(HubEvent::Status {
        deployment_id: dep.id.clone(),
        project_id: dep.project_id.clone(),
        status: "running".into(),
    });
    let mut run = Run {
        eng: eng.clone(),
        dir: PathBuf::new(),
        dep: dep.clone(),
        project: project.clone(),
        log: handle.sink.clone(),
        cancel,
        docker_env: Vec::new(),
    };
    let result = run.steps().await;
    match &result {
        Ok(()) => run.log.system("Deployment succeeded."),
        Err(e) if e.cancelled => run.log.system(
            "Deployment cancelled. A partially started stack may remain; use Redeploy or Rollback to bring it to a known state.",
        ),
        Err(e) => run.log.system(&format!("Deployment failed: {}", e.message)),
    }
    handle.close().await;
    finalize(&eng, &dep, Some(&project), result).await;
    eng.unregister_cancel(&id);
}

async fn load_project(eng: &Engine, id: &str) -> Result<Option<ProjectRow>, CoreError> {
    let mut conn = eng.ctx.db.reader().acquire().await.map_err(db_err)?;
    projects::get(&mut conn, id).await
}

/// Secrets that must never reach the stored log: Deploy Token, registry secret, env values.
async fn build_redactor(eng: &Engine, p: &ProjectRow, dep: &DeploymentRow) -> Redactor {
    let mut secrets = Vec::new();
    if let Ok(t) = eng
        .ctx
        .secrets
        .open("projects", "deploy_token_enc", &p.id, &p.deploy_token_enc)
    {
        secrets.push(t.expose().to_owned());
    }
    if let Some(cid) = &p.registry_credential_id
        && let Ok(mut conn) = eng.ctx.db.reader().acquire().await
        && let Ok(Some(c)) = registry_credentials::get(&mut conn, cid).await
        && let Ok(s) =
            eng.ctx
                .secrets
                .open("registry_credentials", "secret_enc", &c.id, &c.secret_enc)
    {
        secrets.push(s.expose().to_owned());
    }
    if let Ok(dir) = files::project_dir(&eng.data_dir, &p.slug)
        && let Ok(Some(text)) = files::read_optional(&dir.join(&p.env_file))
    {
        secrets.extend(
            super::env::parse(&text)
                .vars()
                .into_iter()
                .map(|(_, v)| v.to_owned()),
        );
    }
    // Env values this Deployment is about to write (payload or restored snapshot).
    for (col, enc) in [
        ("pending_env_enc", dep.pending_env_enc.as_deref()),
        ("env_snapshot_enc", dep.env_snapshot_enc.as_deref()),
    ] {
        if let Some(e) = enc
            && let Ok(text) = eng.ctx.secrets.open("deployments", col, &dep.id, e)
        {
            secrets.extend(
                super::env::parse(text.expose())
                    .vars()
                    .into_iter()
                    .map(|(_, v)| v.to_owned()),
            );
        }
    }
    Redactor::new(secrets)
}

async fn finalize(
    eng: &Engine,
    dep: &DeploymentRow,
    project: Option<&ProjectRow>,
    result: Step<()>,
) {
    let (status, exit_code, error) = match &result {
        Ok(()) => ("success", Some(0), None),
        Err(e) if e.cancelled => ("cancelled", Some(i64::from(EXIT_CANCELLED)), None),
        Err(e) => (
            "failed",
            Some(e.exit_code.unwrap_or(1)),
            Some(e.message.clone()),
        ),
    };
    let now = eng.ctx.now();
    let outcome = async {
        let mut tx = eng.ctx.db.begin_write().await?;
        let changed =
            deployments::finish(&mut tx, &dep.id, status, exit_code, error.as_deref(), now).await?;
        if changed {
            let mut ev = AuditEvent::new("deployment.finish", "deployment")
                .entity(&dep.id)
                .project(&dep.project_id)
                .meta(json!({ "status": status, "trigger": dep.trigger }));
            if status == "failed" {
                ev.outcome = audit::Outcome::Error;
            }
            let actor = AuditActor {
                actor_type: "system",
                actor_id: None,
                label: "system".into(),
                channel: Channel::System,
                ip: None,
                request_id: None,
            };
            audit::write(&eng.ctx, &mut tx, &actor, &ev).await?;
            if let Some(p) = project {
                deployments::prune(&mut tx, &p.id, eng.retention).await?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok::<_, CoreError>(changed)
    }
    .await;
    if let Err(err) = outcome {
        tracing::error!(error = %err, deployment = %dep.id, "could not record deployment result");
    }
    eng.containers_changed();
    eng.hub.publish(HubEvent::Done {
        deployment_id: dep.id.clone(),
        project_id: dep.project_id.clone(),
        status: status.to_owned(),
    });
}

impl Run {
    fn note(&self, line: &str) {
        self.log.system(line);
    }

    fn check_cancel(&self) -> Step<()> {
        if self.cancel.is_cancelled() {
            Err(StepError::cancelled())
        } else {
            Ok(())
        }
    }

    /// Echo `$ cmd args`, run it, and turn failures into a [`StepError`].
    async fn run(&self, spec: CommandSpec, what: &str) -> Step<CommandOutput> {
        self.check_cancel()?;
        self.note(&format!("$ {}", spec.display()));
        let out = self
            .eng
            .host
            .command
            .run(spec, self.log.clone() as Arc<dyn LogSink>, &self.cancel)
            .await
            .map_err(|e| StepError::failed(format!("{what}: {}", e.message)))?;
        if out.cancelled {
            return Err(StepError::cancelled());
        }
        if out.timed_out {
            return Err(StepError {
                message: format!("{what} timed out"),
                cancelled: false,
                exit_code: Some(i64::from(out.exit_code)),
            });
        }
        if out.exit_code != 0 {
            return Err(StepError {
                message: format!("{what} failed (exit code {})", out.exit_code),
                cancelled: false,
                exit_code: Some(i64::from(out.exit_code)),
            });
        }
        Ok(out)
    }

    fn docker(&self, args: &[&str]) -> CommandSpec {
        let mut s = CommandSpec::new("docker", args).cwd(self.dir.clone());
        s.env.clone_from(&self.docker_env);
        s
    }

    fn compose_args(&self, with_override: bool) -> Vec<String> {
        let p = &self.project;
        let mut a: Vec<String> = [
            "compose",
            "-p",
            p.slug.as_str(),
            "-f",
            p.compose_path.as_str(),
        ]
        .map(str::to_owned)
        .to_vec();
        if with_override && self.dir.join(OVERRIDE_FILE).exists() {
            a.extend(["-f".to_owned(), OVERRIDE_FILE.to_owned()]);
        }
        if p.env_file != ".env" {
            a.extend(["--env-file".to_owned(), p.env_file.clone()]);
        }
        a
    }

    fn compose(&self, tail: &[&str], with_override: bool) -> CommandSpec {
        let mut args = self.compose_args(with_override);
        args.extend(tail.iter().map(|s| (*s).to_owned()));
        let mut s = CommandSpec::args_owned("docker", args).cwd(self.dir.clone());
        s.env.clone_from(&self.docker_env);
        s
    }

    async fn steps(&mut self) -> Step<()> {
        let p = self.project.clone();
        self.note(&format!(
            "Deployment {} for project {} (trigger: {})",
            self.dep.id, p.slug, self.dep.trigger
        ));
        self.dir = files::ensure_project_dir(&self.eng.data_dir, &p.slug)?;

        // Pre-flight.
        self.run(
            self.docker(&["compose", "version"])
                .capture()
                .timeout(QUICK_TIMEOUT),
            "docker compose is not available",
        )
        .await?;

        // Registry auth first, so every later docker command can use it.
        let _guard = self.registry_auth().await?;

        if p.is_git() {
            let (sha, message) = self.git_phase().await?;
            self.persist_commit(sha.as_deref(), message.as_deref())
                .await?;
        }

        self.config_files().await?;

        let rollback = self.dep.rollback_from_deployment_id.is_some();
        if !p.is_git() {
            self.image_phase(rollback).await?;
        } else if p.registry_credential_id.is_some() {
            self.run(
                self.compose(&["pull", "--ignore-buildable"], false)
                    .timeout(PULL_TIMEOUT),
                "image pull",
            )
            .await?;
        }

        self.ports_phase().await?;
        self.up_phase().await?;

        if !p.is_git() {
            self.record_digest().await?;
        }
        Ok(())
    }

    async fn persist_commit(&self, sha: Option<&str>, message: Option<&str>) -> Step<()> {
        let mut tx = self.eng.ctx.db.begin_write().await?;
        deployments::set_commit(&mut tx, &self.dep.id, sha, message).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn registry_auth(&mut self) -> Step<Option<RegGuard>> {
        let Some(cid) = self.project.registry_credential_id.clone() else {
            return Ok(None);
        };
        let cred = {
            let mut conn = self.eng.ctx.db.reader().acquire().await.map_err(db_err)?;
            registry_credentials::get(&mut conn, &cid).await?
        };
        let Some(c) = cred else {
            return Err(StepError::failed(
                "the Project's registry credential no longer exists",
            ));
        };
        let secret = self
            .eng
            .ctx
            .secrets
            .open("registry_credentials", "secret_enc", &c.id, &c.secret_enc)
            .map_err(|_| {
                StepError::failed("the registry credential cannot be decrypted; re-enter it")
            })?;
        let entry = AuthEntry {
            registry_key: compose::auth_key(&c.registry_host),
            username: c.username.clone(),
            secret,
        };
        let dir = self.eng.host.registry_auth.create(&self.dep.id, &[entry])?;
        let guard = RegGuard {
            eng: self.eng.clone(),
            scope: self.dep.id.clone(),
        };
        self.docker_env
            .push(("DOCKER_CONFIG".to_owned(), dir.display().to_string()));
        self.note(&format!(
            "Using registry credential for {}",
            c.registry_host
        ));
        Ok(Some(guard))
    }

    fn git_env(&self, key: Option<&KeyFile>) -> Vec<(String, String)> {
        let known = self.eng.data_dir.join("ssh").join("known_hosts");
        let mut ssh = String::from("ssh");
        if let Some(k) = key {
            ssh.push_str(&format!(" -i {} -o IdentitiesOnly=yes", k.path().display()));
        }
        ssh.push_str(&format!(
            " -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile={}",
            known.display()
        ));
        vec![
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_ALLOW_PROTOCOL".into(), "ssh:https".into()),
            ("GIT_SSH_COMMAND".into(), ssh),
        ]
    }

    async fn git_phase(&self) -> Step<(Option<String>, Option<String>)> {
        let p = &self.project;
        let url = p.git_url.clone().unwrap_or_default();
        let branch = p.branch.clone().unwrap_or_else(|| "master".into());
        let key = self.deploy_key().await?;
        files::ensure_dir(&self.eng.data_dir.join("ssh"), 0o700)?;
        let env = self.git_env(key.as_ref());
        let git = |args: &[&str], timeout: Duration| {
            let mut s = CommandSpec::new("git", args)
                .cwd(self.dir.clone())
                .timeout(timeout);
            s.env.clone_from(&env);
            s
        };
        let hint = if p.deploy_key_id.is_some()
            || url.starts_with("ssh://")
            || !url.starts_with("https://")
        {
            " If this is an authentication failure, add the Deploy Key's public key to the repository and check the host key policy."
        } else {
            ""
        };
        let step = |what: &str| format!("git {what}{hint}");

        if !self.dir.join(".git").exists() {
            let empty = std::fs::read_dir(&self.dir).map_or(true, |mut d| d.next().is_none());
            if empty {
                self.run(
                    git(
                        &["clone", "--branch", &branch, "--", &url, "."],
                        GIT_TIMEOUT,
                    ),
                    &step("clone"),
                )
                .await?;
            } else {
                self.run(git(&["init"], GIT_TIMEOUT), "git init").await?;
                self.run(
                    git(&["remote", "add", "origin", &url], GIT_TIMEOUT),
                    "git remote add",
                )
                .await?;
            }
        }
        if let Some(target) = self.dep.target_ref.clone() {
            self.run(
                git(&["fetch", "--all", "--tags"], GIT_TIMEOUT),
                &step("fetch"),
            )
            .await?;
            self.run(
                git(&["checkout", &target, "--"], GIT_TIMEOUT),
                "git checkout",
            )
            .await?;
        } else {
            self.run(
                git(&["fetch", "origin", &branch], GIT_TIMEOUT),
                &step("fetch"),
            )
            .await?;
            let co = self
                .run(
                    git(&["checkout", &branch, "--"], GIT_TIMEOUT),
                    "git checkout",
                )
                .await;
            if co.is_err() {
                let remote = format!("origin/{branch}");
                self.run(
                    git(&["checkout", "-B", &branch, &remote, "--"], GIT_TIMEOUT),
                    "git checkout",
                )
                .await?;
            }
            self.run(
                git(&["pull", "--ff-only", "origin", &branch], GIT_TIMEOUT),
                &step("pull (fast-forward only)"),
            )
            .await?;
        }
        let sha = self
            .run(
                git(&["rev-parse", "HEAD"], QUICK_TIMEOUT).capture(),
                "git rev-parse",
            )
            .await?
            .stdout
            .trim()
            .to_owned();
        let msg = self
            .run(
                git(&["log", "-1", "--pretty=%s"], QUICK_TIMEOUT).capture(),
                "git log",
            )
            .await?
            .stdout
            .trim()
            .to_owned();
        self.note(&format!(
            "Checked out {}",
            sha.chars().take(12).collect::<String>()
        ));
        drop(key);
        Ok((Some(sha), Some(msg)))
    }

    async fn deploy_key(&self) -> Step<Option<KeyFile>> {
        let Some(kid) = &self.project.deploy_key_id else {
            return Ok(None);
        };
        let row = {
            let mut conn = self.eng.ctx.db.reader().acquire().await.map_err(db_err)?;
            deploy_keys::get(&mut conn, kid).await?
        };
        let Some(row) = row else {
            return Err(StepError::failed(
                "the Project's deploy key no longer exists",
            ));
        };
        let pem = self
            .eng
            .ctx
            .secrets
            .open(
                "deploy_keys",
                "private_key_enc",
                &row.id,
                &row.private_key_enc,
            )
            .map_err(|_| StepError::failed("the deploy key cannot be decrypted"))?;
        Ok(Some(KeyFile::materialize(
            &self.eng.data_dir,
            &self.project.slug,
            pem.expose(),
        )?))
    }

    /// Write compose/env files, then snapshot what actually ran (D25).
    async fn config_files(&self) -> Step<()> {
        let p = &self.project;
        let secrets = &self.eng.ctx.secrets;
        let compose_file = files::resolve_in(&self.dir, &p.compose_path)?;
        let env_file = files::resolve_in(&self.dir, &p.env_file)?;
        let rollback = self.dep.rollback_from_deployment_id.is_some();

        let content = if rollback {
            self.dep.compose_snapshot.clone()
        } else if let Some(c) = &p.compose_content {
            Some(c.clone())
        } else if !p.is_git() {
            Some(compose::generated_image_compose(
                p.image_ref.as_deref().unwrap_or(""),
            ))
        } else {
            None
        };
        if let Some(c) = content {
            files::write_atomic(&compose_file, &c, 0o640)?;
            self.note(&format!("Wrote {}", p.compose_path));
        }
        let env_text = if rollback {
            self.dep
                .env_snapshot_enc
                .as_deref()
                .map(|e| secrets.open("deployments", "env_snapshot_enc", &self.dep.id, e))
                .transpose()
                .map_err(|_| StepError::failed("the env snapshot cannot be decrypted"))?
        } else {
            self.dep
                .pending_env_enc
                .as_deref()
                .map(|e| secrets.open("deployments", "pending_env_enc", &self.dep.id, e))
                .transpose()
                .map_err(|_| StepError::failed("the pending env payload cannot be decrypted"))?
        };
        if let Some(t) = env_text {
            files::write_atomic(&env_file, t.expose(), 0o600)?;
            self.note(&format!("Wrote {}", p.env_file));
        }
        if !compose_file.exists() {
            return Err(StepError::failed(format!(
                "compose file {} not found (also tried compose.yml, compose.yaml, docker-compose.yaml)",
                p.compose_path
            )));
        }
        let compose_text = files::read_optional(&compose_file)?.unwrap_or_default();
        let env_enc = match files::read_optional(&env_file)? {
            Some(t) => Some(secrets.seal(
                "deployments",
                "env_snapshot_enc",
                &self.dep.id,
                &Secret::new(t),
            )?),
            None => None,
        };
        let mut tx = self.eng.ctx.db.begin_write().await?;
        deployments::set_snapshots(
            &mut tx,
            &self.dep.id,
            Some(&compose_text),
            env_enc.as_deref(),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn image_phase(&self, rollback: bool) -> Step<()> {
        let image = self.project.image_ref.clone().unwrap_or_default();
        if rollback {
            let Some(digest) = self.dep.target_ref.clone() else {
                return Err(StepError::failed("rollback has no image digest to restore"));
            };
            self.run(
                self.docker(&["pull", &digest]).timeout(PULL_TIMEOUT),
                &format!("image pull for {}", compose::registry_host(&image)),
            )
            .await?;
            if !image.contains('@') {
                self.run(self.docker(&["tag", &digest, &image]), "docker tag")
                    .await?;
            }
            return Ok(());
        }
        self.run(
            self.docker(&["pull", &image]).timeout(PULL_TIMEOUT),
            &format!("image pull from {}", compose::registry_host(&image)),
        )
        .await?;
        Ok(())
    }

    async fn ports_phase(&self) -> Step<()> {
        let out = self
            .run(
                self.compose(&["config", "--format", "json"], false)
                    .capture()
                    .timeout(QUICK_TIMEOUT),
                "docker compose config",
            )
            .await?;
        let config: serde_json::Value = serde_json::from_str(out.stdout.trim())
            .map_err(|_| StepError::failed("docker compose config returned unreadable output"))?;
        let desired =
            compose::ports_from_config(&config).map_err(|e| StepError::failed(e.message))?;
        if desired.is_empty() {
            self.reconcile_ports(&desired).await?;
            return Ok(());
        }
        let pc = super::portcheck::check(
            &self.eng,
            &self.project.id,
            &self.project.slug,
            &desired,
            &self.cancel,
        )
        .await?;
        if pc.containers_unchecked {
            self.note(
                "Could not list running containers; skipping the container part of the port check.",
            );
        }
        if !pc.conflicts.is_empty() {
            return Err(StepError::failed(format!(
                "port collision: {}",
                super::portcheck::describe(&pc.conflicts)
            )));
        }
        self.reconcile_ports(&desired).await?;
        Ok(())
    }

    async fn reconcile_ports(&self, desired: &[DesiredPort]) -> Step<()> {
        let mut tx = self.eng.ctx.db.begin_write().await?;
        let changed =
            reconcile_ports(&mut tx, &self.project.id, desired, self.eng.ctx.now()).await?;
        tx.commit().await.map_err(db_err)?;
        if changed {
            self.note(&format!(
                "Declared ports: {}",
                if desired.is_empty() {
                    "none".to_owned()
                } else {
                    desired
                        .iter()
                        .map(|d| {
                            format!(
                                "{}:{}->{}/{}",
                                d.bind_address, d.host_port, d.container_port, d.protocol
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ));
        }
        Ok(())
    }

    async fn up_phase(&self) -> Step<()> {
        let override_path = self.dir.join(OVERRIDE_FILE);
        if self.project.auto_start {
            let out = self
                .run(
                    self.compose(&["config", "--services"], false)
                        .capture()
                        .timeout(QUICK_TIMEOUT),
                    "docker compose config",
                )
                .await?;
            let services: Vec<String> = out
                .stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            files::write_atomic(&override_path, &compose::restart_override(&services), 0o640)?;
        } else if override_path.exists() {
            let _ = std::fs::remove_file(&override_path);
        }
        let running = self
            .run(
                self.compose(&["ps", "-q", "--status", "running"], true)
                    .capture()
                    .timeout(QUICK_TIMEOUT),
                "docker compose ps",
            )
            .await?;
        if running.stdout.trim().is_empty() {
            self.run(
                self.compose(&["up", "-d", "--build", "--remove-orphans"], true)
                    .timeout(self.eng.command_timeout),
                "docker compose up",
            )
            .await?;
        } else {
            self.run(
                self.compose(&["build"], true)
                    .timeout(self.eng.command_timeout),
                "docker compose build",
            )
            .await?;
            self.run(
                self.compose(&["up", "-d", "--remove-orphans"], true)
                    .timeout(self.eng.command_timeout),
                "docker compose up",
            )
            .await?;
        }
        Ok(())
    }

    async fn record_digest(&self) -> Step<()> {
        let image = self.project.image_ref.clone().unwrap_or_default();
        if image.contains('@') {
            let mut tx = self.eng.ctx.db.begin_write().await?;
            deployments::set_image_digest(&mut tx, &self.dep.id, &image).await?;
            tx.commit().await.map_err(db_err)?;
            return Ok(());
        }
        let out = self
            .eng
            .host
            .command
            .run(
                self.docker(&[
                    "image",
                    "inspect",
                    "--format",
                    "{{json .RepoDigests}}",
                    &image,
                ])
                .capture()
                .timeout(QUICK_TIMEOUT),
                Arc::new(crate::ports::NullSink),
                &self.cancel,
            )
            .await;
        let digest = out.ok().filter(|o| o.exit_code == 0).and_then(|o| {
            serde_json::from_str::<Vec<String>>(o.stdout.trim())
                .ok()
                .and_then(|v| v.into_iter().find(|d| d.contains("@sha256:")))
        });
        match digest {
            Some(d) => {
                self.note(&format!("Image digest {d}"));
                let mut tx = self.eng.ctx.db.begin_write().await?;
                deployments::set_image_digest(&mut tx, &self.dep.id, &d).await?;
                tx.commit().await.map_err(db_err)?;
            }
            None => self.note("No repo digest recorded (image has no registry digest)."),
        }
        Ok(())
    }
}

/// Make `project_ports` match `desired`, keeping rows that already match by
/// `(host_port, protocol)` so ids (and Hostnames that reference them) survive.
/// Returns whether anything changed.
pub async fn reconcile_ports(
    conn: &mut sqlx::SqliteConnection,
    project_id: &str,
    desired: &[DesiredPort],
    now: i64,
) -> Result<bool, CoreError> {
    let existing = projects::ports_for(conn, project_id).await?;
    let same = |e: &PortRow, d: &DesiredPort| {
        e.host_port == i64::from(d.host_port) && e.protocol == d.protocol
    };
    let mut changed = false;
    for e in &existing {
        if !desired.iter().any(|d| same(e, d)) {
            projects::delete_port(conn, &e.id).await?;
            changed = true;
        }
    }
    for d in desired {
        if let Some(e) = existing.iter().find(|e| same(e, d)) {
            if e.label != d.label
                || e.container_port != i64::from(d.container_port)
                || e.compose_service.as_deref() != Some(d.service.as_str())
            {
                projects::update_port(
                    conn,
                    &e.id,
                    &d.label,
                    Some(&d.service),
                    i64::from(d.container_port),
                )
                .await?;
                changed = true;
            }
            continue;
        }
        projects::insert_port(
            conn,
            &PortRow {
                id: new_id("ppt"),
                project_id: project_id.to_owned(),
                label: d.label.clone(),
                compose_service: Some(d.service.clone()),
                host_port: i64::from(d.host_port),
                container_port: i64::from(d.container_port),
                bind_address: d.bind_address.clone(),
                protocol: d.protocol.clone(),
                created_at: now,
            },
        )
        .await?;
        changed = true;
    }
    Ok(changed)
}
