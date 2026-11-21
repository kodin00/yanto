//! REST for Host Daemons, Cloudflare, Hostnames, Traefik and FRP (M7).
//!
//! `cloudflare_tunnel_token_reveal` and `frps_token_reveal` are deliberately absent: connector
//! and frp tokens are never returned over REST (docs/rewrite/03 a.5). Secrets in request bodies
//! (`api_token`, `web_password`, `auth_token`) are wrapped at the edge and never echoed.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yanto_core::ingress::frps::{Change, Patch, PortRange};
use yanto_core::secrets::Secret;
use yanto_core::service::cloudflare::DnsInput;
use yanto_core::service::frps::FrpSettingsInput;
use yanto_core::service::hostnames::{HostnameInput, TargetInput};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hostnames", get(hostnames_list).post(hostname_create))
        .route("/hostnames/diagnostics", get(hostnames_diagnostics))
        .route("/hostnames/options", get(hostnames_options))
        .route("/hostnames/resync", post(hostnames_resync))
        .route("/hostnames/public-ip", post(public_ip_set))
        .route("/hostnames/{id}", axum::routing::delete(hostname_delete))
        .route("/hostnames/{id}/retry", post(hostname_retry))
        .route("/hostnames/{id}/enable", post(hostname_enable))
        .route("/hostnames/{id}/disable", post(hostname_disable))
        .route("/traefik", get(traefik_status))
        .route("/daemons", get(daemons_status))
        .route("/daemons/{name}/{action}", post(daemon_control))
        .route("/cloudflare/clients", get(clients_list).post(client_create))
        .route("/cloudflare/clients/validate", post(client_validate))
        .route(
            "/cloudflare/clients/{id}",
            axum::routing::patch(client_update).delete(client_delete),
        )
        .route("/cloudflare/clients/{id}/zones", get(zones_list))
        .route("/cloudflare/tunnels", get(tunnels_list).post(tunnel_create))
        .route(
            "/cloudflare/tunnels/{id}",
            axum::routing::delete(tunnel_delete),
        )
        .route("/cloudflare/tunnels/{id}/activate", post(tunnel_activate))
        .route("/cloudflare/tunnels/{id}/status", get(tunnel_status))
        .route("/cloudflare/dns", get(dns_list).post(dns_create))
        .route(
            "/cloudflare/dns/{id}",
            axum::routing::patch(dns_update).delete(dns_delete),
        )
        .route("/frp", get(frps_overview))
        .route("/frp/settings", axum::routing::patch(frps_settings_update))
        .route("/frp/restore", post(frps_restore))
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

// ---- hostnames ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProjectQuery {
    project_id: Option<String>,
}

async fn hostnames_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Query(q): Query<ProjectQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(
        s.services
            .hostnames
            .list(&a, q.project_id.as_deref())
            .await?,
    )
    .into_response())
}

#[derive(Deserialize)]
struct CreateHostname {
    /// `project` (default) or `registry` (Owner, `admin` scope).
    #[serde(default)]
    target_kind: Option<String>,
    project_id: Option<String>,
    project_port_id: Option<String>,
    hostname: String,
    ingress_provider: String,
    tunnel_id: Option<String>,
    zone_id: Option<String>,
    #[serde(default)]
    manage_dns: bool,
}

async fn hostname_create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CreateHostname>,
) -> Result<Response, ApiError> {
    let target = if b.target_kind.as_deref() == Some("registry") {
        TargetInput::Registry
    } else {
        TargetInput::Project {
            project_id: b.project_id.unwrap_or_default(),
            project_port_id: b.project_port_id.unwrap_or_default(),
        }
    };
    let out = s
        .services
        .hostnames
        .create(
            &a,
            &HostnameInput {
                target,
                hostname: b.hostname,
                ingress_provider: b.ingress_provider,
                tunnel_id: b.tunnel_id,
                zone_id: b.zone_id,
                manage_dns: b.manage_dns,
            },
        )
        .await?;
    Ok(created(out))
}

async fn hostname_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.delete(&a, &id).await?).into_response())
}

async fn hostname_retry(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.retry(&a, &id).await?).into_response())
}

async fn hostname_enable(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.enable(&a, &id).await?).into_response())
}

async fn hostname_disable(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.disable(&a, &id).await?).into_response())
}

#[derive(Deserialize)]
struct DiagQuery {
    id: Option<String>,
    #[serde(default)]
    public: bool,
}

async fn hostnames_diagnostics(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Query(q): Query<DiagQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(
        s.services
            .hostnames
            .diagnostics(&a, q.id.as_deref(), q.public)
            .await?,
    )
    .into_response())
}

async fn hostnames_options(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.options(&a).await?).into_response())
}

async fn hostnames_resync(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.resync(&a).await?).into_response())
}

#[derive(Deserialize)]
struct PublicIp {
    v4: Option<String>,
    v6: Option<String>,
}

async fn public_ip_set(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<PublicIp>,
) -> Result<Response, ApiError> {
    s.services
        .hostnames
        .public_ip_set(&a, b.v4.as_deref(), b.v6.as_deref())
        .await?;
    Ok(no_content())
}

async fn traefik_status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.hostnames.traefik_status(&a).await?).into_response())
}

// ---- daemons ------------------------------------------------------------------------------

async fn daemons_status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.daemons.status(&a).await?).into_response())
}

#[derive(Deserialize, Default)]
struct ControlBody {
    #[serde(default)]
    confirm: bool,
}

async fn daemon_control(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path((name, action)): Path<(String, String)>,
    body: Option<Json<ControlBody>>,
) -> Result<Response, ApiError> {
    let confirm = body.is_some_and(|b| b.0.confirm);
    Ok(Json(
        s.services
            .daemons
            .control(&a, &name, &action, confirm)
            .await?,
    )
    .into_response())
}

// ---- Cloudflare ---------------------------------------------------------------------------

async fn clients_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.cloudflare.clients_list(&a).await?).into_response())
}

#[derive(Deserialize)]
struct ClientBody {
    #[serde(default)]
    name: String,
    #[serde(default)]
    account_id: String,
    api_token: String,
}

async fn client_validate(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<ClientBody>,
) -> Result<Response, ApiError> {
    let token = Secret::new(b.api_token);
    Ok(Json(
        s.services
            .cloudflare
            .client_validate(&a, &b.account_id, &token)
            .await?,
    )
    .into_response())
}

async fn client_create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<ClientBody>,
) -> Result<Response, ApiError> {
    let token = Secret::new(b.api_token);
    Ok(created(
        s.services
            .cloudflare
            .client_create(&a, &b.name, &b.account_id, &token)
            .await?,
    ))
}

#[derive(Deserialize)]
struct ClientPatch {
    name: Option<String>,
    api_token: Option<String>,
}

async fn client_update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<ClientPatch>,
) -> Result<Response, ApiError> {
    let token = b.api_token.map(Secret::new);
    Ok(Json(
        s.services
            .cloudflare
            .client_update(&a, &id, b.name.as_deref(), token.as_ref())
            .await?,
    )
    .into_response())
}

async fn client_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.cloudflare.client_delete(&a, &id).await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct RefreshQuery {
    #[serde(default)]
    refresh: bool,
}

async fn zones_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<RefreshQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.cloudflare.zones_list(&a, &id, q.refresh).await?).into_response())
}

async fn tunnels_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.cloudflare.tunnels_list(&a).await?).into_response())
}

#[derive(Deserialize)]
struct TunnelBody {
    client_id: String,
    name: String,
}

async fn tunnel_create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<TunnelBody>,
) -> Result<Response, ApiError> {
    Ok(created(
        s.services
            .cloudflare
            .tunnel_create(&a, &b.client_id, &b.name)
            .await?,
    ))
}

async fn tunnel_activate(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.cloudflare.tunnel_activate(&a, &id).await?;
    Ok(no_content())
}

async fn tunnel_status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.cloudflare.tunnel_status(&a, &id).await?).into_response())
}

#[derive(Deserialize)]
struct ForceQuery {
    #[serde(default)]
    force: bool,
}

async fn tunnel_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<ForceQuery>,
) -> Result<Response, ApiError> {
    s.services
        .cloudflare
        .tunnel_delete(&a, &id, q.force)
        .await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct ZoneQuery {
    client_id: String,
    zone_id: String,
    #[serde(default)]
    refresh: bool,
}

async fn dns_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Query(q): Query<ZoneQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(
        s.services
            .cloudflare
            .dns_list(&a, &q.client_id, &q.zone_id, q.refresh)
            .await?,
    )
    .into_response())
}

#[derive(Deserialize)]
struct DnsBody {
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    content: String,
    ttl: Option<i64>,
    #[serde(default)]
    proxied: bool,
    priority: Option<i64>,
    comment: Option<String>,
}

impl From<DnsBody> for DnsInput {
    fn from(b: DnsBody) -> Self {
        Self {
            record_type: b.record_type,
            name: b.name,
            content: b.content,
            ttl: b.ttl,
            proxied: b.proxied,
            priority: b.priority,
            comment: b.comment,
        }
    }
}

#[derive(Deserialize)]
struct DnsCreate {
    client_id: String,
    zone_id: String,
    #[serde(flatten)]
    record: DnsBody,
}

async fn dns_create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<DnsCreate>,
) -> Result<Response, ApiError> {
    Ok(created(
        s.services
            .cloudflare
            .dns_create(&a, &b.client_id, &b.zone_id, &b.record.into())
            .await?,
    ))
}

#[derive(Deserialize)]
struct ZoneRef {
    client_id: String,
    zone_id: String,
}

async fn dns_update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<ZoneRef>,
    ApiJson(b): ApiJson<DnsBody>,
) -> Result<Response, ApiError> {
    Ok(Json(
        s.services
            .cloudflare
            .dns_update(&a, &q.client_id, &q.zone_id, &id, &b.into())
            .await?,
    )
    .into_response())
}

async fn dns_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<ZoneRef>,
) -> Result<Response, ApiError> {
    s.services
        .cloudflare
        .dns_delete(&a, &q.client_id, &q.zone_id, &id)
        .await?;
    Ok(no_content())
}

// ---- FRP ----------------------------------------------------------------------------------

async fn frps_overview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.frps.overview(&a).await?).into_response())
}

/// Absent field keeps the value, `null` clears it, a value sets it.
fn change<T>(v: Option<Option<T>>) -> Change<T> {
    match v {
        None => Change::Keep,
        Some(None) => Change::Clear,
        Some(Some(x)) => Change::Set(x),
    }
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct PortRangeBody {
    single: Option<u16>,
    start: Option<u16>,
    end: Option<u16>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FrpSettingsBody {
    public_host: Option<String>,
    generate_token: bool,
    #[serde(deserialize_with = "double_option")]
    bind_addr: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    bind_port: Option<Option<u16>>,
    #[serde(deserialize_with = "double_option")]
    auth_method: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    auth_token: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    web_addr: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    web_port: Option<Option<u16>>,
    #[serde(deserialize_with = "double_option")]
    web_user: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    web_password: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    allow_ports: Option<Option<Vec<PortRangeBody>>>,
}

async fn frps_settings_update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<FrpSettingsBody>,
) -> Result<Response, ApiError> {
    let allow = match b.allow_ports {
        None => Change::Keep,
        Some(None) => Change::Clear,
        Some(Some(list)) => {
            let mut out = Vec::new();
            for r in list {
                out.push(match (r.single, r.start, r.end) {
                    (Some(port), None, None) => PortRange::Single { port },
                    (None, Some(start), Some(end)) => PortRange::Range { start, end },
                    _ => {
                        return Err(yanto_core::CoreError::validation(
                            "allow_ports",
                            "each entry is {single} or {start, end}",
                        )
                        .into());
                    }
                });
            }
            Change::Set(out)
        }
    };
    let input = FrpSettingsInput {
        public_host: b.public_host,
        generate_token: b.generate_token,
        patch: Patch {
            bind_addr: change(b.bind_addr),
            bind_port: change(b.bind_port),
            auth_method: change(b.auth_method),
            auth_token: change(b.auth_token.map(|o| o.map(Secret::new))),
            web_addr: change(b.web_addr),
            web_port: change(b.web_port),
            web_user: change(b.web_user),
            web_password: change(b.web_password.map(|o| o.map(Secret::new))),
            allow_ports: allow,
        },
    };
    let out = s.services.frps.settings_update(&a, &input).await?;
    Ok(Json(json!({
        "changed": out.changed,
        "diff": out.diff,
        "pending_restart": out.pending_restart,
        "token_generated": out.token_generated,
        "apply": "POST /api/v1/daemons/frps/restart with {\"confirm\": true}; this drops every live tunnel",
    }))
    .into_response())
}

async fn frps_restore(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Response, ApiError> {
    s.services.frps.config_restore(&a).await?;
    Ok(no_content())
}
