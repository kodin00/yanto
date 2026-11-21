//! Shared test helpers (enabled by the `testing` feature or `cfg(test)`).
//!
//! * [`FakeClock`]: a movable [`Clock`].
//! * [`TestCore`]: a temp-dir SQLite database, fake clock, cheap password hashing and
//!   fully wired [`Services`], plus fixtures for the Owner, Members and API Tokens.
//!
//! Fakes for host-side ports live next to this file, one per port
//! (docs/rewrite/08-conventions.md "Ports and fakes").

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use crate::actor::{Actor, Channel, Origin, Permission, Scope};
use crate::config::Config;
use crate::db::Db;
use crate::deploy::HostPorts;
use crate::password::Passwords;
use crate::ports::Clock;
use crate::secrets::Secrets;
use crate::service::users::GrantInput;
use crate::service::{ServiceDeps, Services};

pub mod command;
pub mod host_probe;
pub mod registry_auth;

pub use command::{FakeCommands, Reply};
pub use host_probe::FakeHostProbe;
pub use registry_auth::FakeRegistryAuth;

pub const OWNER_PASSWORD: &str = "owner-password-123";
pub const MEMBER_PASSWORD: &str = "member-password-123";

/// A clock that only moves when told to. Starts at 2026-01-01T00:00:00Z.
#[derive(Debug)]
pub struct FakeClock(AtomicI64);

impl FakeClock {
    pub fn new() -> Self {
        Self(AtomicI64::new(1_767_225_600_000))
    }

    pub fn advance(&self, by: Duration) {
        self.0.fetch_add(
            i64::try_from(by.as_millis()).expect("duration fits"),
            Ordering::SeqCst,
        );
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A signed-in dashboard user created by a fixture.
#[derive(Debug, Clone)]
pub struct TestUser {
    pub id: String,
    pub username: String,
    pub password: String,
}

/// A throwaway Yanto core: temp DB, fake clock, real services.
pub struct TestCore {
    pub dir: tempfile::TempDir,
    pub db: Db,
    pub clock: Arc<FakeClock>,
    pub services: Services,
    pub config: Config,
    /// Scripted commands (`git`, `docker`); inspect `commands.calls()`.
    pub commands: Arc<FakeCommands>,
    pub probe: Arc<FakeHostProbe>,
    pub registry_auth: Arc<FakeRegistryAuth>,
}

impl TestCore {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// Like [`TestCore::new`], letting the test adjust the config (limits, timeouts).
    pub async fn with_config(adjust: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.paths.data_dir = dir.path().to_path_buf();
        adjust(&mut config);
        let db = Db::open(&config.db_path()).await.expect("open db");
        let clock = Arc::new(FakeClock::new());
        let commands = FakeCommands::new();
        let probe = Arc::new(FakeHostProbe::default());
        let registry_auth = Arc::new(FakeRegistryAuth::default());
        let services = Services::new(ServiceDeps {
            db: db.clone(),
            config: &config,
            clock: clock.clone(),
            secrets: Arc::new(Secrets::ephemeral()),
            passwords: Passwords::cheap_for_tests(),
            host: HostPorts {
                command: commands.clone(),
                probe: probe.clone(),
                registry_auth: registry_auth.clone(),
            },
        });
        Self {
            dir,
            db,
            clock,
            services,
            config,
            commands,
            probe,
            registry_auth,
        }
    }

    /// Insert a minimal image Project row with this id if it does not exist (grants and
    /// audit rows need a real Project since 0201). Idempotent; the slug derives from the id.
    pub async fn ensure_project(&self, id: &str) {
        let slug = id
            .to_ascii_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
        let secrets = &self.services.ctx().secrets;
        let token = crate::secrets::Secret::new(format!("ydp_stub_{id}"));
        let enc = secrets
            .seal("projects", "deploy_token_enc", id, &token)
            .expect("seal stub token");
        sqlx::query(
            "INSERT OR IGNORE INTO projects (id, name, slug, source_kind, image_ref, deploy_token_hash,
                deploy_token_enc, created_at, updated_at)
             VALUES (?, ?, ?, 'image', 'nginx:alpine', ?, ?, 0, 0)",
        )
        .bind(id)
        .bind(id)
        .bind(slug)
        .bind(crate::crypto::sha256_hex(token.expose()))
        .bind(enc)
        .execute(self.db.writer())
        .await
        .expect("insert test project");
    }

    pub fn origin(&self) -> Origin {
        Origin::new(Channel::Dashboard, Some("203.0.113.7".into()))
    }

    /// Issue a setup code with the CLI actor and create the Owner.
    pub async fn setup_owner(&self) -> TestUser {
        let code = self
            .services
            .auth
            .issue_setup_code(&Actor::cli())
            .await
            .expect("issue setup code");
        let out = self
            .services
            .auth
            .owner_setup(
                &self.origin(),
                code.code.expose(),
                "owner",
                OWNER_PASSWORD,
                None,
            )
            .await
            .expect("owner setup");
        TestUser {
            id: out.user_id,
            username: out.username,
            password: OWNER_PASSWORD.into(),
        }
    }

    /// Create a Member through the real invite flow, so the account is `active` with a password.
    pub async fn create_member(
        &self,
        username: &str,
        grants: &[(&str, &[Permission])],
    ) -> TestUser {
        for (p, _) in grants {
            self.ensure_project(p).await;
        }
        let inputs: Vec<GrantInput> = grants
            .iter()
            .map(|(p, perms)| GrantInput {
                project_id: (*p).to_owned(),
                permissions: perms.to_vec(),
            })
            .collect();
        let created = self
            .services
            .users
            .create(&Actor::cli(), username, &inputs)
            .await
            .expect("create member");
        let token = token_from_link(created.invite.url.expose());
        self.services
            .auth
            .account_setup(&self.origin(), &token, MEMBER_PASSWORD, None)
            .await
            .expect("accept invite");
        TestUser {
            id: created.user.id,
            username: created.user.username,
            password: MEMBER_PASSWORD.into(),
        }
    }

    /// Sign in and return the session [`Actor`] (what the dashboard would build per request).
    pub async fn session_actor(&self, user: &TestUser) -> Actor {
        let login = self
            .services
            .auth
            .login(&self.origin(), &user.username, &user.password, None)
            .await
            .expect("login");
        self.services
            .auth
            .authenticate_session(&self.origin(), login.session.cookie.expose())
            .await
            .expect("session")
            .actor
    }

    /// Create an API Token for `user` and return its bearer string plus the token [`Actor`].
    pub async fn token_actor(&self, user: &TestUser, scope: Scope) -> (String, Actor) {
        let session = self.session_actor(user).await;
        let name = format!("t-{}-{}", scope, crate::crypto::random_hex::<3>());
        let created = self
            .services
            .tokens
            .create(&session, &name, scope, None)
            .await
            .expect("create token");
        let bearer = created.token.expose().to_owned();
        let origin = Origin::new(Channel::Rest, Some("203.0.113.7".into()));
        let actor = self
            .services
            .tokens
            .authenticate(&origin, &bearer)
            .await
            .expect("authenticate token");
        (bearer, actor)
    }
}

/// Extract the token from an invite/reset link (`.../account/setup#token=<token>`).
pub fn token_from_link(link: &str) -> String {
    link.split_once("#token=")
        .map(|(_, t)| t.to_owned())
        .expect("link has a token fragment")
}
