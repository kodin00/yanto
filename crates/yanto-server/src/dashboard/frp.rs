//! `/frp` (frps status, config, live proxies) and `/daemons` (Host Daemon status and control).
//! Owner only (docs/rewrite/03 b, 04 section 8). Applying a saved frps config restarts `frps`,
//! which drops every live tunnel (D28): the restart button asks for confirmation with that warning.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::ingress::frps::{Change, Patch, PortRange};
use yanto_core::secrets::Secret;
use yanto_core::service::daemons::DaemonDto;
use yanto_core::service::frps::{FrpOverview, FrpSettingsInput, ProxyDto};

use super::session::Session;
use super::view::{DashError, PageCtx, render};
use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/frp", get(index))
        .route("/frp/proxies", get(proxies))
        .route("/frp/settings", axum::routing::put(settings))
        .route("/frp/restore", post(restore))
        .route("/frp/token/reveal", post(token_reveal))
        .route("/daemons", get(daemons))
        .route("/daemons/{name}/{action}", post(control))
}

fn hx_redirect(to: &'static str) -> Response {
    let mut res = (StatusCode::OK, Html("")).into_response();
    res.headers_mut()
        .insert("hx-redirect", HeaderValue::from_static(to));
    res
}

struct ProxyView {
    name: String,
    proxy_type: String,
    remote_port: String,
    status: String,
    traffic: String,
    connections: u64,
    client: String,
}

impl From<ProxyDto> for ProxyView {
    fn from(p: ProxyDto) -> Self {
        Self {
            remote_port: p.remote_port.map(|n| n.to_string()).unwrap_or_default(),
            traffic: format!(
                "{} in / {} out",
                yanto_core::service::fmt_bytes(p.traffic_in),
                yanto_core::service::fmt_bytes(p.traffic_out)
            ),
            name: p.name,
            proxy_type: p.proxy_type,
            status: p.status,
            connections: p.connections,
            client: p.client.unwrap_or_default(),
        }
    }
}

fn range_text(list: &[PortRange]) -> String {
    list.iter()
        .map(|r| match r {
            PortRange::Single { port } => port.to_string(),
            PortRange::Range { start, end } => format!("{start}-{end}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_ranges(text: &str) -> Result<Vec<PortRange>, CoreError> {
    let bad = |t: &str| {
        CoreError::validation(
            "allow_ports",
            format!("{t:?} is not a port or a range like 2000-3000"),
        )
    };
    let mut out = Vec::new();
    for t in text
        .split(['\n', ','])
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        out.push(match t.split_once('-') {
            Some((a, b)) => PortRange::Range {
                start: a.trim().parse().map_err(|_| bad(t))?,
                end: b.trim().parse().map_err(|_| bad(t))?,
            },
            None => PortRange::Single {
                port: t.parse().map_err(|_| bad(t))?,
            },
        });
    }
    Ok(out)
}

#[derive(Template)]
#[template(path = "frp/index.html")]
struct IndexPage {
    ctx: PageCtx,
    o: FrpView,
}

struct FrpView {
    status: &'static str,
    status_key: &'static str,
    installed: bool,
    version: String,
    instructions: Vec<String>,
    sample_unit: String,
    unit: String,
    has_config: bool,
    config_error: String,
    config_path: String,
    bind_addr: String,
    bind_port: String,
    auth_method: String,
    token_set: bool,
    web_addr: String,
    web_port: String,
    web_user: String,
    password_set: bool,
    allow_ports: String,
    unowned: String,
    pending_restart: bool,
    backup: bool,
    public_host: String,
    warnings: Vec<String>,
}

impl From<FrpOverview> for FrpView {
    fn from(o: FrpOverview) -> Self {
        let c = o.config.as_ref();
        Self {
            status: o.status_label,
            status_key: match o.status {
                yanto_core::service::frps::FrpStatus::Running => "active",
                yanto_core::service::frps::FrpStatus::NotInstalled
                | yanto_core::service::frps::FrpStatus::InstalledInactive => "disabled",
                _ => "warn",
            },
            installed: o.daemon.installed,
            version: o.version.unwrap_or_default(),
            instructions: o.daemon.instructions.clone(),
            sample_unit: o.daemon.sample_unit.map(str::to_owned).unwrap_or_default(),
            unit: o.daemon.unit.clone(),
            has_config: c.is_some(),
            config_error: o.config_error.unwrap_or_default(),
            config_path: c.map(|c| c.path.clone()).unwrap_or_default(),
            bind_addr: c.and_then(|c| c.bind_addr.clone()).unwrap_or_default(),
            bind_port: c
                .and_then(|c| c.bind_port)
                .map(|p| p.to_string())
                .unwrap_or_default(),
            auth_method: c.and_then(|c| c.auth_method.clone()).unwrap_or_default(),
            token_set: c.is_some_and(|c| c.auth_token_set),
            web_addr: c.and_then(|c| c.web_addr.clone()).unwrap_or_default(),
            web_port: c
                .and_then(|c| c.web_port)
                .map(|p| p.to_string())
                .unwrap_or_default(),
            web_user: c.and_then(|c| c.web_user.clone()).unwrap_or_default(),
            password_set: c.is_some_and(|c| c.web_password_set),
            allow_ports: c.map(|c| range_text(&c.allow_ports)).unwrap_or_default(),
            unowned: c.map(|c| c.unowned_raw.clone()).unwrap_or_default(),
            pending_restart: o.pending_restart,
            backup: o.backup_available,
            public_host: o.public_host,
            warnings: o.warnings,
        }
    }
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let o = s
        .services
        .frps
        .overview(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&IndexPage {
        ctx: PageCtx::new(&session, "FRP", "/frp"),
        o: o.into(),
    }))
}

#[derive(Template)]
#[template(path = "frp/_proxies.html")]
struct ProxiesFragment {
    status: &'static str,
    proxies: Vec<ProxyView>,
}

/// Polled every 5 s by the page (htmx); reads the frps dashboard API with a 2 s timeout.
async fn proxies(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let o = s
        .services
        .frps
        .overview(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&ProxiesFragment {
        status: o.status_label,
        proxies: o.proxies.into_iter().map(Into::into).collect(),
    }))
}

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    public_host: String,
    #[serde(default)]
    bind_addr: String,
    #[serde(default)]
    bind_port: String,
    #[serde(default)]
    auth_method: String,
    #[serde(default)]
    auth_token: String,
    #[serde(default)]
    generate_token: Option<String>,
    #[serde(default)]
    web_addr: String,
    #[serde(default)]
    web_port: String,
    #[serde(default)]
    web_user: String,
    #[serde(default)]
    web_password: String,
    #[serde(default)]
    allow_ports: String,
}

fn text(v: &str) -> Change<String> {
    match v.trim() {
        "" => Change::Clear,
        t => Change::Set(t.to_owned()),
    }
}

/// Empty keeps the stored secret.
fn secret(v: &str) -> Change<Secret> {
    if v.is_empty() {
        Change::Keep
    } else {
        Change::Set(Secret::new(v))
    }
}

fn port(field: &str, v: &str) -> Result<Change<u16>, CoreError> {
    match v.trim() {
        "" => Ok(Change::Clear),
        t => t
            .parse()
            .map(Change::Set)
            .map_err(|_| CoreError::validation(field.to_owned(), "port must be 1-65535")),
    }
}

impl SettingsForm {
    fn input(&self) -> Result<FrpSettingsInput, CoreError> {
        Ok(FrpSettingsInput {
            public_host: Some(self.public_host.clone()),
            generate_token: self.generate_token.is_some(),
            patch: Patch {
                bind_addr: text(&self.bind_addr),
                bind_port: port("bind_port", &self.bind_port)?,
                auth_method: text(&self.auth_method),
                auth_token: secret(&self.auth_token),
                web_addr: text(&self.web_addr),
                web_port: port("web_port", &self.web_port)?,
                web_user: text(&self.web_user),
                web_password: secret(&self.web_password),
                allow_ports: match parse_ranges(&self.allow_ports)? {
                    r if r.is_empty() => Change::Clear,
                    r => Change::Set(r),
                },
            },
        })
    }
}

async fn settings(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<SettingsForm>,
) -> Result<Response, DashError> {
    let go = async {
        let input = f.input()?;
        s.services
            .frps
            .settings_update(&session.0.actor, &input)
            .await
    };
    go.await.map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/frp"))
}

async fn restore(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    s.services
        .frps
        .config_restore(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Redirect::to("/frp").into_response())
}

#[derive(Template)]
#[template(path = "frp/token.html")]
struct TokenPage {
    ctx: PageCtx,
    token: String,
}

async fn token_reveal(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let t = s
        .services
        .frps
        .token_reveal(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&TokenPage {
        ctx: PageCtx::new(&session, "frps auth token", "/frp"),
        token: t.expose().to_owned(),
    }))
}

// ---- daemons ------------------------------------------------------------------------------

struct DaemonView {
    name: &'static str,
    unit: String,
    state: &'static str,
    state_key: &'static str,
    installed: bool,
    version: String,
    unit_file: String,
    instructions: Vec<String>,
    sample_unit: String,
    warning: &'static str,
}

impl From<DaemonDto> for DaemonView {
    fn from(d: DaemonDto) -> Self {
        use yanto_core::ingress::daemon::DaemonState;
        Self {
            state_key: match d.state {
                DaemonState::Running => "active",
                DaemonState::Failed => "fail",
                _ => "disabled",
            },
            warning: if d.name == "frps" {
                "Restarting or stopping frps drops every live frp tunnel. Continue?"
            } else {
                "Continue?"
            },
            name: d.name,
            unit: d.unit,
            state: d.state_label,
            installed: d.installed,
            version: d.version.unwrap_or_default(),
            unit_file: d.unit_file.unwrap_or_default(),
            instructions: d.instructions,
            sample_unit: d.sample_unit.map(str::to_owned).unwrap_or_default(),
        }
    }
}

#[derive(Template)]
#[template(path = "daemons/index.html")]
struct DaemonsPage {
    ctx: PageCtx,
    daemons: Vec<DaemonView>,
}

async fn daemons(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let d = s
        .services
        .daemons
        .status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&DaemonsPage {
        ctx: PageCtx::new(&session, "Host Daemons", "/daemons"),
        daemons: d.into_iter().map(Into::into).collect(),
    }))
}

#[derive(Deserialize, Default)]
struct ControlForm {
    #[serde(default)]
    confirm: String,
}

async fn control(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path((name, action)): Path<(String, String)>,
    Form(f): Form<ControlForm>,
) -> Result<Response, DashError> {
    let out = s
        .services
        .daemons
        .control(&session.0.actor, &name, &action, f.confirm == "true")
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let target = if out.name == "frps" {
        "/frp"
    } else {
        "/daemons"
    };
    if super::view::is_htmx(&headers) {
        return Ok(hx_redirect(if out.name == "frps" {
            "/frp"
        } else {
            "/daemons"
        }));
    }
    Ok(Redirect::to(target).into_response())
}
