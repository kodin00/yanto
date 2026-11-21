//! `/cloudflare` (clients, zones, tunnels) and `/dns` (records): Owner only (docs/rewrite/03 b,
//! 04 sections 9.2 and 9.4). The tunnel connector token is shown once, on a `no-store` page, after
//! an audited reveal; it is never stored or sent anywhere else.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::secrets::Secret;
use yanto_core::service::cloudflare::{
    ClientDto, DnsDto, DnsInput, TOKEN_PERMISSIONS, TunnelDto, ZoneDto,
};

use super::session::Session;
use super::view::{DashError, PageCtx, escape, fmt_opt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cloudflare", get(index))
        .route("/cloudflare/clients", post(client_create))
        .route("/cloudflare/clients/validate", post(client_validate))
        .route(
            "/cloudflare/clients/{id}",
            axum::routing::put(client_update).delete(client_delete),
        )
        .route("/cloudflare/clients/{id}/zones", get(zones))
        .route("/cloudflare/tunnels", post(tunnel_create))
        .route(
            "/cloudflare/tunnels/{id}",
            axum::routing::delete(tunnel_delete),
        )
        .route("/cloudflare/tunnels/{id}/activate", post(tunnel_activate))
        .route("/cloudflare/tunnels/{id}/status", get(tunnel_status))
        .route("/cloudflare/tunnels/{id}/token/reveal", post(token_reveal))
        .route("/dns", get(dns_index).post(dns_create))
        .route(
            "/dns/{id}",
            axum::routing::put(dns_update).delete(dns_delete),
        )
}

fn hx_redirect(to: &str) -> Response {
    let mut res = (StatusCode::OK, Html("")).into_response();
    if let Ok(v) = HeaderValue::from_str(to) {
        res.headers_mut().insert("hx-redirect", v);
    }
    res
}

struct ClientView {
    id: String,
    name: String,
    account_id: String,
    tunnels: i64,
}

impl From<ClientDto> for ClientView {
    fn from(c: ClientDto) -> Self {
        Self {
            id: c.id,
            name: c.name,
            account_id: c.account_id,
            tunnels: c.tunnels,
        }
    }
}

struct TunnelView {
    id: String,
    name: String,
    active: bool,
    status: String,
    checked: String,
    hostnames: i64,
}

impl From<TunnelDto> for TunnelView {
    fn from(t: TunnelDto) -> Self {
        Self {
            checked: fmt_opt_ts(t.last_health_at),
            id: t.id,
            name: t.name,
            active: t.active,
            status: t.status,
            hostnames: t.hostnames,
        }
    }
}

#[derive(Template)]
#[template(path = "cloudflare/index.html")]
struct IndexPage {
    ctx: PageCtx,
    clients: Vec<ClientView>,
    tunnels: Vec<TunnelView>,
    permissions: &'static [&'static str],
    error: Option<String>,
    form_name: String,
    form_account: String,
}

async fn index_page(
    s: &AppState,
    session: &Session,
    error: Option<String>,
    form: (String, String),
) -> Result<IndexPage, CoreError> {
    let a = &session.0.actor;
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Cloudflare", "/cloudflare"),
        clients: s
            .services
            .cloudflare
            .clients_list(a)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
        tunnels: s
            .services
            .cloudflare
            .tunnels_list(a)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
        permissions: TOKEN_PERMISSIONS,
        error,
        form_name: form.0,
        form_account: form.1,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = index_page(&s, &session, None, Default::default())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct ClientForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    api_token: String,
}

fn is_form_error(e: &CoreError) -> bool {
    matches!(
        e.kind,
        yanto_core::ErrorKind::Validation(_)
            | yanto_core::ErrorKind::Conflict
            | yanto_core::ErrorKind::Unavailable
    )
}

async fn client_create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<ClientForm>,
) -> Result<Response, DashError> {
    let token = Secret::new(f.api_token.clone());
    match s
        .services
        .cloudflare
        .client_create(&session.0.actor, &f.name, &f.account_id, &token)
        .await
    {
        Ok(_) => Ok(Redirect::to("/cloudflare").into_response()),
        Err(e) if is_form_error(&e) => {
            // The token is not put back into the form.
            let p = index_page(
                &s,
                &session,
                Some(public_message(&e)),
                (f.name, f.account_id),
            )
            .await
            .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

/// htmx fragment: the validation result, without saving anything.
async fn client_validate(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<ClientForm>,
) -> Response {
    let token = Secret::new(f.api_token);
    match s
        .services
        .cloudflare
        .client_validate(&session.0.actor, &f.account_id, &token)
        .await
    {
        Ok(r) => Html(format!(
            "<p class=\"muted\">Token is valid. Zones: {}</p>",
            escape(&r.zones.join(", "))
        ))
        .into_response(),
        Err(e) if is_form_error(&e) => Html(format!(
            "<p class=\"error\">{}</p>",
            escape(&public_message(&e))
        ))
        .into_response(),
        Err(e) => DashError::new(e, &HeaderMap::new()).into_response(),
    }
}

async fn client_update(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ClientForm>,
) -> Result<Response, DashError> {
    let token = (!f.api_token.is_empty()).then(|| Secret::new(f.api_token));
    s.services
        .cloudflare
        .client_update(
            &session.0.actor,
            &id,
            Some(f.name.as_str()).filter(|n| !n.is_empty()),
            token.as_ref(),
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/cloudflare"))
}

async fn client_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .cloudflare
        .client_delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

#[derive(Deserialize)]
struct RefreshQuery {
    refresh: Option<String>,
}

async fn zones(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<RefreshQuery>,
) -> Result<Response, DashError> {
    let zs: Vec<ZoneDto> = s
        .services
        .cloudflare
        .zones_list(&session.0.actor, &id, q.refresh.is_some())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut html = String::from("<ul>");
    for z in zs {
        html.push_str(&format!(
            "<li><a href=\"/dns?client_id={}&amp;zone_id={}\">{}</a> <span class=\"muted\">{}</span></li>",
            escape(&id),
            escape(&z.id),
            escape(&z.name),
            escape(z.status.as_deref().unwrap_or(""))
        ));
    }
    html.push_str("</ul>");
    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct TunnelForm {
    client_id: String,
    name: String,
}

async fn tunnel_create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<TunnelForm>,
) -> Result<Response, DashError> {
    match s
        .services
        .cloudflare
        .tunnel_create(&session.0.actor, &f.client_id, &f.name)
        .await
    {
        Ok(_) => Ok(Redirect::to("/cloudflare").into_response()),
        Err(e) if is_form_error(&e) => {
            let p = index_page(&s, &session, Some(public_message(&e)), Default::default())
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

#[derive(Deserialize)]
struct ForceQuery {
    force: Option<String>,
}

async fn tunnel_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ForceQuery>,
) -> Result<Response, DashError> {
    s.services
        .cloudflare
        .tunnel_delete(&session.0.actor, &id, q.force.is_some())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

async fn tunnel_activate(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .cloudflare
        .tunnel_activate(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/cloudflare"))
}

async fn tunnel_status(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let st = s
        .services
        .cloudflare
        .tunnel_status(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut html = format!(
        "<p>API status: <strong>{}</strong>. Host cloudflared: <strong>{}</strong>.</p>",
        escape(&st.api_status),
        escape(st.cloudflared.state_label)
    );
    if let Some(h) = st.hint {
        html.push_str(&format!("<p class=\"muted\">{}</p>", escape(&h)));
    }
    Ok(Html(html).into_response())
}

#[derive(Template)]
#[template(path = "cloudflare/token.html")]
struct TokenPage {
    ctx: PageCtx,
    tunnel: String,
    command: String,
}

/// POST so it needs the CSRF token and is never reachable by a link; audited by the service.
async fn token_reveal(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .cloudflare
        .tunnel_token_reveal(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&TokenPage {
        ctx: PageCtx::new(&session, "Tunnel install command", "/cloudflare"),
        tunnel: r.tunnel_name,
        command: r.install_command.expose().to_owned(),
    }))
}

// ---- DNS ----------------------------------------------------------------------------------

struct RecordView {
    id: String,
    record_type: String,
    name: String,
    content: String,
    ttl: String,
    proxied: bool,
    priority: String,
    comment: String,
    managed: bool,
}

impl From<DnsDto> for RecordView {
    fn from(r: DnsDto) -> Self {
        Self {
            ttl: if r.ttl == 1 {
                "auto".into()
            } else {
                r.ttl.to_string()
            },
            priority: r.priority.map(|p| p.to_string()).unwrap_or_default(),
            comment: r.comment.unwrap_or_default(),
            id: r.id,
            record_type: r.record_type,
            name: r.name,
            content: r.content,
            proxied: r.proxied,
            managed: r.managed,
        }
    }
}

struct ZoneOpt {
    id: String,
    name: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "cloudflare/dns.html")]
struct DnsPage {
    ctx: PageCtx,
    clients: Vec<ClientView>,
    client_id: String,
    zone_id: String,
    zones: Vec<ZoneOpt>,
    records: Vec<RecordView>,
    error: Option<String>,
}

#[derive(Deserialize, Default)]
struct DnsQuery {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    zone_id: String,
    refresh: Option<String>,
}

async fn dns_page(
    s: &AppState,
    session: &Session,
    q: &DnsQuery,
    error: Option<String>,
) -> Result<DnsPage, CoreError> {
    let a = &session.0.actor;
    let clients = s.services.cloudflare.clients_list(a).await?;
    let client_id = if q.client_id.is_empty() {
        clients.first().map(|c| c.id.clone()).unwrap_or_default()
    } else {
        q.client_id.clone()
    };
    let (mut zones, mut records, mut zone_id) = (Vec::new(), Vec::new(), q.zone_id.clone());
    if !client_id.is_empty() {
        let zs = s
            .services
            .cloudflare
            .zones_list(a, &client_id, false)
            .await?;
        if zone_id.is_empty() {
            zone_id = zs.first().map(|z| z.id.clone()).unwrap_or_default();
        }
        zones = zs
            .into_iter()
            .map(|z| ZoneOpt {
                selected: z.id == zone_id,
                id: z.id,
                name: z.name,
            })
            .collect();
        if !zone_id.is_empty() {
            records = s
                .services
                .cloudflare
                .dns_list(a, &client_id, &zone_id, q.refresh.is_some())
                .await?
                .into_iter()
                .map(Into::into)
                .collect();
        }
    }
    Ok(DnsPage {
        ctx: PageCtx::new(session, "DNS", "/dns"),
        clients: clients.into_iter().map(Into::into).collect(),
        client_id,
        zone_id,
        zones,
        records,
        error,
    })
}

async fn dns_index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(q): Query<DnsQuery>,
) -> Result<Response, DashError> {
    let p = dns_page(&s, &session, &q, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct DnsForm {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    zone_id: String,
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    content: String,
    #[serde(default)]
    ttl: String,
    #[serde(default)]
    proxied: Option<String>,
    #[serde(default)]
    priority: String,
    #[serde(default)]
    comment: String,
}

impl DnsForm {
    fn input(&self) -> Result<DnsInput, CoreError> {
        let num = |f: &str, v: &str| -> Result<Option<i64>, CoreError> {
            match v.trim() {
                "" | "auto" => Ok(None),
                n => n
                    .parse()
                    .map(Some)
                    .map_err(|_| CoreError::validation(f.to_owned(), "must be a number")),
            }
        };
        Ok(DnsInput {
            record_type: self.record_type.clone(),
            name: self.name.clone(),
            content: self.content.clone(),
            ttl: num("ttl", &self.ttl)?,
            proxied: self.proxied.is_some(),
            priority: num("priority", &self.priority)?,
            comment: Some(self.comment.clone()).filter(|c| !c.is_empty()),
        })
    }
}

fn dns_url(client: &str, zone: &str) -> String {
    format!(
        "/dns?client_id={}&zone_id={}",
        form_urlencoded_component(client),
        form_urlencoded_component(zone)
    )
}

fn form_urlencoded_component(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn dns_create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<DnsForm>,
) -> Result<Response, DashError> {
    let res = async {
        s.services
            .cloudflare
            .dns_create(&session.0.actor, &f.client_id, &f.zone_id, &f.input()?)
            .await
    }
    .await;
    match res {
        Ok(_) => Ok(Redirect::to(&dns_url(&f.client_id, &f.zone_id)).into_response()),
        Err(e) if is_form_error(&e) => {
            let q = DnsQuery {
                client_id: f.client_id,
                zone_id: f.zone_id,
                refresh: None,
            };
            let p = dns_page(&s, &session, &q, Some(public_message(&e)))
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

async fn dns_update(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<DnsForm>,
) -> Result<Response, DashError> {
    let go = async {
        s.services
            .cloudflare
            .dns_update(&session.0.actor, &f.client_id, &f.zone_id, &id, &f.input()?)
            .await
    };
    go.await.map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect(&dns_url(&f.client_id, &f.zone_id)))
}

async fn dns_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DnsQuery>,
) -> Result<Response, DashError> {
    s.services
        .cloudflare
        .dns_delete(&session.0.actor, &q.client_id, &q.zone_id, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}
