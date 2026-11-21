//! Principals and authorization (docs/rewrite/01 section 4, 03 a.1 / a.2 / a.5).
//!
//! Every service function takes an [`Actor`] and starts with
//! `actor.authorize(&OP_X, project)?`. Authorization is deny-by-default: an
//! operation is described by an [`OpSpec`] constant next to its service fn, and
//! `authorize` is the only place that turns an actor plus a spec into a yes/no.
//! Adapters never decide permissions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use crate::error::CoreError;

pub type UserId = String;
pub type ProjectId = String;

/// Per-Project permission (docs/rewrite/03 a.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Permission {
    Deploy,
    Runtime,
    Config,
    Secrets,
    Backups,
    Hostnames,
}

impl Permission {
    pub const ALL: [Permission; 6] = [
        Permission::Deploy,
        Permission::Runtime,
        Permission::Config,
        Permission::Secrets,
        Permission::Backups,
        Permission::Hostnames,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Deploy => "deploy",
            Permission::Runtime => "runtime",
            Permission::Config => "config",
            Permission::Secrets => "secrets",
            Permission::Backups => "backups",
            Permission::Hostnames => "hostnames",
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Permission {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, CoreError> {
        Permission::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| CoreError::validation("permission", format!("unknown permission {s:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Owner,
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Member => "member",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Role::Owner),
            "member" => Some(Role::Member),
            _ => None,
        }
    }
}

/// API Token scope; `read < write < admin` (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Read, Scope::Write, Scope::Admin];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scope {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, CoreError> {
        Scope::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| CoreError::validation("scope", "scope must be read, write or admin"))
    }
}

/// Which adapter the request came through; recorded in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Dashboard,
    Rest,
    Mcp,
    DeployToken,
    Webhook,
    Cli,
    Cron,
    System,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Dashboard => "dashboard",
            Channel::Rest => "rest",
            Channel::Mcp => "mcp",
            Channel::DeployToken => "deploy-token",
            Channel::Webhook => "webhook",
            Channel::Cli => "cli",
            Channel::Cron => "cron",
            Channel::System => "system",
        }
    }
}

/// Where a request came from before anyone is authenticated (login, setup).
#[derive(Debug, Clone)]
pub struct Origin {
    pub channel: Channel,
    pub ip: Option<String>,
    pub request_id: Option<String>,
}

impl Origin {
    pub fn new(channel: Channel, ip: Option<String>) -> Self {
        Self {
            channel,
            ip,
            request_id: None,
        }
    }

    pub fn cli() -> Self {
        Self::new(Channel::Cli, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorKind {
    /// A signed-in dashboard user (session cookie).
    User,
    /// An API Token acting as its owning user (D23).
    ApiToken {
        token_id: String,
        token_name: String,
        scope: Scope,
    },
    /// The local root operator running the `yanto` CLI (ADR 0007): trusted.
    Cli,
}

/// Who is performing an operation, with everything authorization needs loaded.
#[derive(Debug, Clone)]
pub struct Actor {
    pub kind: ActorKind,
    /// `None` for the CLI actor.
    pub user_id: Option<UserId>,
    /// Username, or `cli`; denormalized into audit rows.
    pub label: String,
    pub role: Role,
    /// Member grants: Project id to permissions. Empty for the Owner (bypass).
    pub grants: BTreeMap<ProjectId, BTreeSet<Permission>>,
    pub channel: Channel,
    pub ip: Option<String>,
    pub request_id: Option<String>,
}

/// What an operation needs on a Project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Any access row on the Project (view).
    View,
    Permission(Permission),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No actor needed: the operation validates its own credential (login, setup code, invite token).
    Public,
    /// Any authenticated actor (still subject to `token_allowed` / `min_scope`).
    Authenticated,
    /// Owner only; Members and their tokens always get 403.
    Owner,
    /// Owner, or a Member with a grant on the Project.
    Project(Need),
}

/// Static description of one service operation. Defined as a `pub const` next
/// to the service function and listed in `service::all_operations()`.
#[derive(Debug, Clone, Copy)]
pub struct OpSpec {
    /// `noun_verb` name shared by dashboard, REST and MCP (03 "Alignment rule").
    pub name: &'static str,
    pub access: Access,
    /// Minimum API Token scope (03 a.5). Ignored for non-token actors.
    pub min_scope: Scope,
    /// `false` for dashboard/CLI-only operations (token issuance, sessions, ...).
    pub token_allowed: bool,
}

impl OpSpec {
    pub const fn new(name: &'static str, access: Access, min_scope: Scope) -> Self {
        Self {
            name,
            access,
            min_scope,
            token_allowed: true,
        }
    }

    /// Mark as not reachable with an API Token (privilege-laundering guard, 03 a.5).
    pub const fn session_only(mut self) -> Self {
        self.token_allowed = false;
        self
    }
}

impl Actor {
    pub fn cli() -> Self {
        Self {
            kind: ActorKind::Cli,
            user_id: None,
            label: "cli".into(),
            role: Role::Owner,
            grants: BTreeMap::new(),
            channel: Channel::Cli,
            ip: None,
            request_id: None,
        }
    }

    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }

    pub fn token_scope(&self) -> Option<Scope> {
        match &self.kind {
            ActorKind::ApiToken { scope, .. } => Some(*scope),
            _ => None,
        }
    }

    /// Deny-by-default check. `project` is required for `Access::Project`.
    ///
    /// Errors: 403 for missing scope, Owner-only, or a missing permission on a
    /// visible Project; 404 when the Project is not visible to the actor (hides
    /// existence, 03 a.2).
    pub fn authorize(&self, op: &OpSpec, project: Option<&str>) -> Result<(), CoreError> {
        if let ActorKind::ApiToken { scope, .. } = &self.kind {
            if !op.token_allowed {
                return Err(CoreError::forbidden(format!(
                    "{} is not available with an API Token",
                    op.name
                )));
            }
            if *scope < op.min_scope {
                return Err(CoreError::forbidden(format!(
                    "{} requires an API Token with {} scope",
                    op.name, op.min_scope
                )));
            }
        }
        match op.access {
            Access::Public | Access::Authenticated => Ok(()),
            Access::Owner => {
                if self.is_owner() {
                    Ok(())
                } else {
                    Err(CoreError::forbidden("only the Owner can do this"))
                }
            }
            Access::Project(need) => {
                if self.is_owner() {
                    return Ok(());
                }
                let project = project.ok_or_else(|| {
                    CoreError::new(
                        crate::ErrorKind::Internal,
                        format!("{} needs a Project but none was given", op.name),
                    )
                })?;
                let Some(perms) = self.grants.get(project) else {
                    return Err(CoreError::not_found("project not found"));
                };
                match need {
                    Need::View => Ok(()),
                    Need::Permission(p) if perms.contains(&p) => Ok(()),
                    Need::Permission(p) => Err(CoreError::forbidden(format!(
                        "requires the {p} permission on this project"
                    ))),
                }
            }
        }
    }

    /// Projects this actor can see: `None` means all (Owner / CLI).
    pub fn visible_projects(&self) -> Option<Vec<&str>> {
        if self.is_owner() {
            None
        } else {
            Some(self.grants.keys().map(String::as_str).collect())
        }
    }

    /// Derived app views (docs/rewrite/02 4.1, 03 a.2). Not stored.
    pub fn app_views(&self) -> BTreeSet<AppView> {
        if self.is_owner() {
            return AppView::ALL.into_iter().collect();
        }
        let mut v: BTreeSet<AppView> = [
            AppView::Dashboard,
            AppView::Projects,
            AppView::Deployments,
            AppView::Containers,
            AppView::Audit,
        ]
        .into_iter()
        .collect();
        let any = |p: Permission| self.grants.values().any(|s| s.contains(&p));
        if any(Permission::Backups) {
            v.insert(AppView::Backups);
        }
        if any(Permission::Hostnames) {
            v.insert(AppView::Hostnames);
        }
        v
    }
}

/// Dashboard sections. `Users` is an M1 addition to the list in 03 a.2 (Owner only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AppView {
    Dashboard,
    Projects,
    Deployments,
    Containers,
    Backups,
    Hostnames,
    Registry,
    Cron,
    Ssh,
    Frp,
    Dns,
    Audit,
    Users,
    Settings,
    System,
}

impl AppView {
    pub const ALL: [AppView; 15] = [
        AppView::Dashboard,
        AppView::Projects,
        AppView::Deployments,
        AppView::Containers,
        AppView::Backups,
        AppView::Hostnames,
        AppView::Registry,
        AppView::Cron,
        AppView::Ssh,
        AppView::Frp,
        AppView::Dns,
        AppView::Audit,
        AppView::Users,
        AppView::Settings,
        AppView::System,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn member(grants: &[(&str, &[Permission])]) -> Actor {
        Actor {
            kind: ActorKind::User,
            user_id: Some("usr_m".into()),
            label: "m".into(),
            role: Role::Member,
            grants: grants
                .iter()
                .map(|(p, perms)| ((*p).to_owned(), perms.iter().copied().collect()))
                .collect(),
            channel: Channel::Dashboard,
            ip: None,
            request_id: None,
        }
    }

    fn token(mut a: Actor, scope: Scope) -> Actor {
        a.kind = ActorKind::ApiToken {
            token_id: "tok_x".into(),
            token_name: "ci".into(),
            token_name: "ci".into(),
            scope,
        };
        a.channel = Channel::Rest;
        a
    }

    const DEPLOY: OpSpec = OpSpec::new(
        "project_deploy",
        Access::Project(Need::Permission(Permission::Deploy)),
        Scope::Write,
    );
    const OWNER_ONLY: OpSpec = OpSpec::new("user_list", Access::Owner, Scope::Admin);
    const SESSION_ONLY: OpSpec =
        OpSpec::new("api_token_create", Access::Authenticated, Scope::Read).session_only();

    fn kind(r: Result<(), CoreError>) -> Option<ErrorKind> {
        r.err().map(|e| e.kind)
    }

    #[test]
    fn owner_and_cli_bypass_project_checks() {
        assert!(Actor::cli().authorize(&DEPLOY, Some("prj_1")).is_ok());
        assert!(Actor::cli().authorize(&OWNER_ONLY, None).is_ok());
    }

    #[test]
    fn member_project_rules() {
        let m = member(&[("prj_1", &[Permission::Deploy]), ("prj_2", &[])]);
        assert!(m.authorize(&DEPLOY, Some("prj_1")).is_ok());
        assert_eq!(
            kind(m.authorize(&DEPLOY, Some("prj_2"))),
            Some(ErrorKind::Forbidden)
        );
        assert_eq!(
            kind(m.authorize(&DEPLOY, Some("prj_3"))),
            Some(ErrorKind::NotFound)
        );
        assert_eq!(
            kind(m.authorize(&OWNER_ONLY, None)),
            Some(ErrorKind::Forbidden)
        );
    }

    #[test]
    fn token_scope_caps_the_user() {
        let m = member(&[("prj_1", &[Permission::Deploy])]);
        assert_eq!(
            kind(token(m.clone(), Scope::Read).authorize(&DEPLOY, Some("prj_1"))),
            Some(ErrorKind::Forbidden)
        );
        assert!(
            token(m.clone(), Scope::Write)
                .authorize(&DEPLOY, Some("prj_1"))
                .is_ok()
        );
        // Scope never grants more than the user has.
        assert_eq!(
            kind(token(m, Scope::Admin).authorize(&DEPLOY, Some("prj_9"))),
            Some(ErrorKind::NotFound)
        );
    }

    #[test]
    fn session_only_ops_reject_tokens_even_for_admin_owner() {
        let owner_token = token(Actor::cli(), Scope::Admin);
        assert_eq!(
            kind(owner_token.authorize(&SESSION_ONLY, None)),
            Some(ErrorKind::Forbidden)
        );
        assert!(Actor::cli().authorize(&SESSION_ONLY, None).is_ok());
    }

    #[test]
    fn views_are_derived_from_permissions() {
        let m = member(&[("prj_1", &[Permission::Backups])]);
        let v = m.app_views();
        assert!(v.contains(&AppView::Backups));
        assert!(!v.contains(&AppView::Hostnames));
        assert!(!v.contains(&AppView::Users));
        assert_eq!(Actor::cli().app_views().len(), AppView::ALL.len());
    }

    #[test]
    fn permission_and_scope_parse() {
        assert_eq!(
            "secrets".parse::<Permission>().unwrap(),
            Permission::Secrets
        );
        assert!("tasks".parse::<Permission>().is_err());
        assert!(Scope::Read < Scope::Write && Scope::Write < Scope::Admin);
        assert!("root".parse::<Scope>().is_err());
    }
}
