//! `/hostnames`, `/traefik`, `/projects/{id}/hostnames` (docs/rewrite/03 b, 04 section 9).
//!
//! Members with the `hostnames` permission manage Hostnames of their Projects; the Hosted Registry
//! target, Re-sync, the Host public IP and the Traefik page are Owner only (the service enforces it).

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::service::hostnames::{
    DiagCheck, DiagStatus, HostnameDto, HostnameInput, HostnameOptions, TargetInput, TraefikStatus,
};

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_opt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hostnames", get(index).post(create))
        .route("/hostnames/resync", post(resync))
        .route("/hostnames/public-ip", post(public_ip))
        .route("/hostnames/diagnostics", get(diagnostics))
        .route("/hostnames/{id}", axum::routing::delete(delete))
        .route("/hostnames/{id}/retry", post(retry))
        .route("/hostnames/{id}/enable", post(enable))
        .route("/hostnames/{id}/disable", post(disable))
        .route("/traefik", get(traefik))
        .route("/projects/{id}/hostnames", get(project_fragment))
}

struct RowView {
    id: String,
    hostname: String,
    provider: &'static str,
    target: String,
    status: String,
    error: String,
    published: String,
    enabled: bool,
}

impl From<HostnameDto> for RowView {
    fn from(h: HostnameDto) -> Self {
        let target = if h.target_kind == "registry" {
            "Hosted Registry".to_owned()
        } else {
            format!(
                "{} (127.0.0.1:{})",
                h.project_slug.as_deref().unwrap_or("?"),
                h.host_port.unwrap_or_default()
            )
        };
        Self {
            provider: if h.ingress_provider == "traefik" {
                "Traefik"
            } else {
                "Cloudflare Tunnel"
            },
            published: fmt_opt_ts(h.last_published_at),
            id: h.id,
            hostname: h.hostname,
            target,
            status: h.sync_status,
            error: h.last_error.unwrap_or_default(),
            enabled: h.enabled,
        }
    }
}

struct TargetOption {
    value: String,
    label: String,
}

struct ZoneOption {
    id: String,
    name: String,
}

#[derive(Template)]
#[template(path = "hostnames/index.html")]
struct IndexPage {
    ctx: PageCtx,
    rows: Vec<RowView>,
    targets: Vec<TargetOption>,
    is_owner: bool,
    tunnel_name: Option<String>,
    traefik_ready: bool,
    traefik_reason: String,
    zones: Vec<ZoneOption>,
    public_ip_set: bool,
    error: Option<String>,
    notices: Vec<String>,
    form_hostname: String,
}

async fn index_page(
    s: &AppState,
    session: &Session,
    error: Option<String>,
    notices: Vec<String>,
    form_hostname: String,
) -> Result<IndexPage, CoreError> {
    let a = &session.0.actor;
    let rows = s.services.hostnames.list(a, None).await?;
    let opts: HostnameOptions = s.services.hostnames.options(a).await?;
    // Targets: Projects the actor may publish (Owner: all; Member: with `hostnames`).
    let mut targets = Vec::new();
    for p in s.services.projects.list(a).await? {
        let allowed = a.is_owner()
            || a.grants
                .get(&p.id)
                .is_some_and(|g| g.contains(&yanto_core::actor::Permission::Hostnames));
        if !allowed {
            continue;
        }
        for port in p
            .ports
            .iter()
            .filter(|x| x.bind_address == "127.0.0.1" && x.protocol == "tcp")
        {
            targets.push(TargetOption {
                value: format!("{}|{}", p.id, port.id),
                label: format!("{}: {} (127.0.0.1:{})", p.name, port.label, port.host_port),
            });
        }
    }
    if a.is_owner() {
        targets.push(TargetOption {
            value: "registry".into(),
            label: "Hosted Registry (Owner only)".into(),
        });
    }
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Hostnames", "/hostnames"),
        rows: rows.into_iter().map(Into::into).collect(),
        targets,
        is_owner: a.is_owner(),
        tunnel_name: opts.tunnel.map(|t| t.name),
        traefik_ready: opts.traefik_ready,
        traefik_reason: opts.traefik_reason.unwrap_or_default(),
        zones: opts
            .zones
            .into_iter()
            .map(|z| ZoneOption {
                id: z.id,
                name: z.name,
            })
            .collect(),
        public_ip_set: opts.public_ip_set,
        error,
        notices,
        form_hostname,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = index_page(&s, &session, None, Vec::new(), String::new())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct CreateForm {
    target: String,
    hostname: String,
    provider: String,
    #[serde(default)]
    zone_id: String,
    #[serde(default)]
    manage_dns: Option<String>,
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CreateForm>,
) -> Result<Response, DashError> {
    let target = match f.target.split_once('|') {
        Some((p, pp)) => TargetInput::Project {
            project_id: p.to_owned(),
            project_port_id: pp.to_owned(),
        },
        None => TargetInput::Registry,
    };
    let input = HostnameInput {
        target,
        hostname: f.hostname.clone(),
        ingress_provider: f.provider.clone(),
        tunnel_id: None,
        zone_id: Some(f.zone_id.clone()).filter(|z| !z.is_empty()),
        manage_dns: f.manage_dns.is_some(),
    };
    match s.services.hostnames.create(&session.0.actor, &input).await {
        Ok(out) => {
            let mut notices = out.warnings;
            if let Some(e) = &out.hostname.last_error {
                notices.insert(
                    0,
                    format!("{} is saved but not live: {e}", out.hostname.hostname),
                );
            }
            let p = index_page(&s, &session, None, notices, String::new())
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render(&p))
        }
        Err(e)
            if matches!(
                e.kind,
                yanto_core::ErrorKind::Validation(_) | yanto_core::ErrorKind::Conflict
            ) =>
        {
            let p = index_page(
                &s,
                &session,
                Some(public_message(&e)),
                Vec::new(),
                f.hostname,
            )
            .await
            .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

fn hx_redirect(to: &'static str) -> Response {
    let mut res = (StatusCode::OK, Html("")).into_response();
    res.headers_mut()
        .insert("hx-redirect", HeaderValue::from_static(to));
    res
}

async fn retry(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .retry(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/hostnames"))
}

async fn enable(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .enable(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/hostnames"))
}

async fn disable(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .disable(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/hostnames"))
}

async fn delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

async fn resync(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .resync(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Redirect::to("/hostnames").into_response())
}

#[derive(Deserialize)]
struct IpForm {
    #[serde(default)]
    v4: String,
    #[serde(default)]
    v6: String,
}

async fn public_ip(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<IpForm>,
) -> Result<Response, DashError> {
    s.services
        .hostnames
        .public_ip_set(&session.0.actor, Some(&f.v4), Some(&f.v6))
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Redirect::to("/hostnames").into_response())
}

struct CheckView {
    name: &'static str,
    status: &'static str,
    detail: String,
    hint: String,
}

impl From<DiagCheck> for CheckView {
    fn from(c: DiagCheck) -> Self {
        Self {
            name: c.name,
            status: match c.status {
                DiagStatus::Pass => "pass",
                DiagStatus::Warn => "warn",
                DiagStatus::Fail => "fail",
            },
            detail: c.detail,
            hint: c.hint.unwrap_or_default(),
        }
    }
}

struct DiagView {
    hostname: String,
    provider: &'static str,
    checks: Vec<CheckView>,
}

#[derive(Template)]
#[template(path = "hostnames/diagnostics.html")]
struct DiagPage {
    ctx: PageCtx,
    items: Vec<DiagView>,
    public: bool,
    id: String,
}

#[derive(Deserialize)]
struct DiagQuery {
    id: Option<String>,
    public: Option<String>,
}

async fn diagnostics(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(q): Query<DiagQuery>,
) -> Result<Response, DashError> {
    let public = q.public.is_some();
    let items = s
        .services
        .hostnames
        .diagnostics(&session.0.actor, q.id.as_deref(), public)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&DiagPage {
        ctx: PageCtx::new(&session, "Hostname diagnostics", "/hostnames"),
        public,
        id: q.id.unwrap_or_default(),
        items: items
            .into_iter()
            .map(|d| DiagView {
                provider: if d.hostname.ingress_provider == "traefik" {
                    "Traefik"
                } else {
                    "Cloudflare Tunnel"
                },
                hostname: d.hostname.hostname,
                checks: d.checks.into_iter().map(Into::into).collect(),
            })
            .collect(),
    }))
}

struct ChecklistView {
    name: &'static str,
    status: &'static str,
    detail: String,
}

#[derive(Template)]
#[template(path = "hostnames/traefik.html")]
struct TraefikPage {
    ctx: PageCtx,
    daemon_state: &'static str,
    installed: bool,
    version: String,
    instructions: Vec<String>,
    dynamic_dir: String,
    dir_exists: bool,
    dir_writable: bool,
    checklist: Vec<ChecklistView>,
    ready: bool,
    reason: String,
    file_state: &'static str,
    watch_off: bool,
    hostnames: usize,
    prerequisites: String,
}

async fn traefik(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let t: TraefikStatus = s
        .services
        .hostnames
        .traefik_status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    use yanto_core::ingress::traefik::CheckStatus;
    use yanto_core::service::hostnames::FileState;
    Ok(render(&TraefikPage {
        ctx: PageCtx::new(&session, "Traefik", "/hostnames"),
        daemon_state: t.daemon.state_label,
        installed: t.daemon.installed,
        version: t.daemon.version.unwrap_or_default(),
        instructions: t.daemon.instructions,
        dynamic_dir: t.dynamic_dir,
        dir_exists: t.dir_exists,
        dir_writable: t.dir_writable,
        checklist: t
            .checklist
            .into_iter()
            .map(|c| ChecklistView {
                name: c.name,
                status: match c.status {
                    CheckStatus::Pass => "pass",
                    CheckStatus::Fail => "fail",
                    CheckStatus::Unverifiable => "unverifiable",
                },
                detail: c.detail,
            })
            .collect(),
        ready: t.ready,
        reason: t.not_ready_reason.unwrap_or_default(),
        file_state: match t.file_state {
            FileState::InSync => "in sync",
            FileState::Drift => "drift (use Re-sync)",
            FileState::Missing => "missing (use Re-sync)",
        },
        watch_off: t.watch == Some(false),
        hostnames: t.hostnames,
        prerequisites: t.prerequisites,
    }))
}

#[derive(Template)]
#[template(path = "hostnames/_rows.html")]
struct RowsFragment {
    rows: Vec<RowView>,
}

/// The project page's Hostnames tab (names are visible only with the `hostnames` permission).
async fn project_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let rows = s
        .services
        .hostnames
        .list(&session.0.actor, Some(&id))
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&RowsFragment {
        rows: rows.into_iter().map(Into::into).collect(),
    }))
}
