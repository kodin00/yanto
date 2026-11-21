//! `/audit`: the audit log. Owner sees everything; a Member sees entries for granted Projects.

use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use yanto_core::service::audit::AuditQuery;

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_ts, render};
use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/audit", get(index))
}

#[derive(Debug, Deserialize)]
struct Params {
    project_id: Option<String>,
    cursor: Option<String>,
}

struct EntryView {
    when: String,
    actor: String,
    channel: String,
    action: String,
    entity: String,
    project: String,
    outcome: String,
    ip: String,
}

#[derive(Template)]
#[template(path = "audit.html")]
struct AuditPage {
    ctx: PageCtx,
    entries: Vec<EntryView>,
    project_id: String,
    next_url: Option<String>,
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(p): Query<Params>,
) -> Result<Response, DashError> {
    let project_id = p.project_id.filter(|v| !v.is_empty());
    let page = s
        .services
        .audit
        .list(
            &session.0.actor,
            &AuditQuery {
                project_id: project_id.clone(),
                limit: Some(50),
                cursor: p.cursor.filter(|v| !v.is_empty()),
            },
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let next_url = page.next_cursor.as_ref().map(|c| {
        let mut q = form_urlencoded::Serializer::new(String::new());
        q.append_pair("cursor", c);
        if let Some(p) = &project_id {
            q.append_pair("project_id", p);
        }
        format!("/audit?{}", q.finish())
    });
    Ok(render(&AuditPage {
        ctx: PageCtx::new(&session, "Audit log", "/audit"),
        entries: page
            .entries
            .into_iter()
            .map(|e| EntryView {
                when: fmt_ts(e.created_at),
                actor: format!("{} ({})", e.actor_label, e.actor_type),
                channel: e.channel,
                action: e.action,
                entity: match e.entity_id {
                    Some(id) => format!("{} {id}", e.entity_type),
                    None => e.entity_type,
                },
                project: e.project_id.unwrap_or_default(),
                outcome: e.outcome,
                ip: e.ip.unwrap_or_default(),
            })
            .collect(),
        project_id: project_id.unwrap_or_default(),
        next_url,
    }))
}
