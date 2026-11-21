//! Audit log core: writing (inside the caller's transaction) and the audit view.
//!
//! Security events (login, user/token changes) are written with [`write`] on the
//! same connection/transaction as the change they describe, so a change cannot
//! commit without its record (docs/rewrite/02 4.5). Metadata must never contain
//! secret values; store key names and counts only.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::Ctx;
use crate::actor::{Access, Actor, ActorKind, Channel, OpSpec, Origin, Scope};
use crate::crypto::new_id;
use crate::db::repo::audit::{self, AuditFilter, AuditRow};
use crate::error::CoreError;

pub const OP_AUDIT_LIST: OpSpec = OpSpec::new("audit_list", Access::Authenticated, Scope::Read);

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

/// Who the audit row is attributed to.
#[derive(Debug, Clone)]
pub struct AuditActor {
    pub actor_type: &'static str,
    pub actor_id: Option<String>,
    pub label: String,
    pub channel: Channel,
    pub ip: Option<String>,
    pub request_id: Option<String>,
}

impl AuditActor {
    pub fn from_actor(a: &Actor) -> Self {
        let (actor_type, actor_id, label) = match &a.kind {
            ActorKind::User => ("user", a.user_id.clone(), a.label.clone()),
            ActorKind::ApiToken {
                token_id,
                token_name,
                ..
            } => ("api_token", Some(token_id.clone()), token_name.clone()),
            ActorKind::Cli => ("cli", None, a.label.clone()),
        };
        Self {
            actor_type,
            actor_id,
            label,
            channel: a.channel,
            ip: a.ip.clone(),
            request_id: a.request_id.clone(),
        }
    }

    /// A known user acting before a full `Actor` exists (login, account setup).
    pub fn user(user_id: &str, username: &str, origin: &Origin) -> Self {
        Self {
            actor_type: "user",
            actor_id: Some(user_id.to_owned()),
            label: username.to_owned(),
            channel: origin.channel,
            ip: origin.ip.clone(),
            request_id: origin.request_id.clone(),
        }
    }

    /// An unauthenticated caller (unknown username, bad token); `label` is `<unknown>` for those.
    pub fn unknown(origin: &Origin, label: &str) -> Self {
        Self {
            actor_type: "user",
            actor_id: None,
            label: label.to_owned(),
            channel: origin.channel,
            ip: origin.ip.clone(),
            request_id: origin.request_id.clone(),
        }
    }

    pub fn system(origin: &Origin) -> Self {
        Self {
            actor_type: "system",
            actor_id: None,
            label: "system".into(),
            channel: origin.channel,
            ip: origin.ip.clone(),
            request_id: origin.request_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Denied,
    Error,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Denied => "denied",
            Outcome::Error => "error",
        }
    }
}

/// One thing that happened. Build with [`AuditEvent::new`] and the chained setters.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub action: &'static str,
    pub entity_type: &'static str,
    pub entity_id: Option<String>,
    pub project_id: Option<String>,
    pub outcome: Outcome,
    pub metadata: Value,
}

impl AuditEvent {
    pub fn new(action: &'static str, entity_type: &'static str) -> Self {
        Self {
            action,
            entity_type,
            entity_id: None,
            project_id: None,
            outcome: Outcome::Ok,
            metadata: json!({}),
        }
    }

    pub fn entity(mut self, id: impl Into<String>) -> Self {
        self.entity_id = Some(id.into());
        self
    }

    pub fn project(mut self, id: impl Into<String>) -> Self {
        self.project_id = Some(id.into());
        self
    }

    pub fn denied(mut self) -> Self {
        self.outcome = Outcome::Denied;
        self
    }

    pub fn meta(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Insert an audit row on `conn` (use the same transaction as the change).
pub async fn write(
    ctx: &Ctx,
    conn: &mut SqliteConnection,
    actor: &AuditActor,
    ev: &AuditEvent,
) -> Result<(), CoreError> {
    let row = AuditRow {
        id: new_id("aud"),
        actor_type: actor.actor_type.into(),
        actor_id: actor.actor_id.clone(),
        actor_label: actor.label.clone(),
        action: ev.action.into(),
        entity_type: ev.entity_type.into(),
        entity_id: ev.entity_id.clone(),
        project_id: ev.project_id.clone(),
        ip: actor.ip.clone(),
        channel: actor.channel.as_str().into(),
        outcome: ev.outcome.as_str().into(),
        request_id: actor.request_id.clone(),
        metadata: ev.metadata.to_string(),
        created_at: ctx.now(),
    };
    audit::insert(conn, &row).await
}

/// Write one event in its own transaction (for events with no accompanying change,
/// such as a failed login).
pub async fn write_standalone(
    ctx: &Ctx,
    actor: &AuditActor,
    ev: &AuditEvent,
) -> Result<(), CoreError> {
    let mut tx = ctx.db.begin_write().await?;
    write(ctx, &mut tx, actor, ev).await?;
    tx.commit().await.map_err(crate::db::repo::db_err)
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntryDto {
    pub id: String,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub actor_label: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub project_id: Option<String>,
    pub ip: Option<String>,
    pub channel: String,
    pub outcome: String,
    pub metadata: Value,
    pub created_at: i64,
}

impl From<AuditRow> for AuditEntryDto {
    fn from(r: AuditRow) -> Self {
        Self {
            metadata: serde_json::from_str(&r.metadata).unwrap_or(Value::Null),
            id: r.id,
            actor_type: r.actor_type,
            actor_id: r.actor_id,
            actor_label: r.actor_label,
            action: r.action,
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            project_id: r.project_id,
            ip: r.ip,
            channel: r.channel,
            outcome: r.outcome,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub project_id: Option<String>,
    pub limit: Option<i64>,
    /// Opaque cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditPage {
    pub entries: Vec<AuditEntryDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditService {
    ctx: Ctx,
}

impl AuditService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Owner/CLI: every entry. Member (or a Member's token): only entries whose
    /// Project is in their grants; asking for another Project is 404.
    pub async fn list(&self, actor: &Actor, q: &AuditQuery) -> Result<AuditPage, CoreError> {
        actor.authorize(&OP_AUDIT_LIST, None)?;
        let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let visible = actor
            .visible_projects()
            .map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>());
        if let (Some(visible), Some(p)) = (&visible, &q.project_id)
            && !visible.contains(p)
        {
            return Err(CoreError::not_found("project not found"));
        }
        let before = q.cursor.as_deref().map(parse_cursor).transpose()?;
        let filter = AuditFilter {
            visible_projects: visible,
            project_id: q.project_id.clone(),
            before,
            limit: limit + 1,
        };
        let mut conn = self
            .ctx
            .db
            .reader()
            .acquire()
            .await
            .map_err(crate::db::repo::db_err)?;
        let mut rows = audit::list(&mut conn, &filter).await?;
        let next_cursor = if rows.len() as i64 > limit {
            rows.truncate(usize::try_from(limit).unwrap_or(50));
            rows.last().map(|r| format!("{}:{}", r.created_at, r.id))
        } else {
            None
        };
        Ok(AuditPage {
            entries: rows.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    }
}

fn parse_cursor(c: &str) -> Result<(i64, String), CoreError> {
    c.split_once(':')
        .and_then(|(t, id)| Some((t.parse().ok()?, id.to_owned())))
        .ok_or_else(|| CoreError::validation("cursor", "invalid cursor"))
}
