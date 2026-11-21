//! `/system` (Owner only): host usage, Docker info, dependency health and the Docker prune
//! with preview. Volumes are never prune candidates.

use askama::Template;
use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::ports::{DiskUsage, PruneKind, UsageBucket};
use yanto_core::service::fmt_bytes;
use yanto_core::service::system::{CONFIRM_CONTAINERS, CleanupSelection, HealthStatus, PruneGroup};

use super::session::Session;
use super::view::{DashError, PageCtx, render};
use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/system", get(index))
        .route("/system/cleanup", post(cleanup))
        .route("/system/logs/stream", get(logs_stream))
}

struct Kv(String, String);

struct DiskRow {
    mount: String,
    total: String,
    free: String,
    used_percent: u32,
    paths: String,
}

struct CheckRow {
    name: &'static str,
    status: &'static str,
    detail: String,
}

struct GroupView {
    field: &'static str,
    label: &'static str,
    count: usize,
    size: String,
    default_selected: bool,
    needs_confirm: bool,
    items: Vec<String>,
    more: usize,
}

struct UsageRow(&'static str, String, String, String);

#[derive(Template)]
#[template(path = "system/index.html")]
struct SystemPage {
    ctx: PageCtx,
    host: Vec<Kv>,
    disks: Vec<DiskRow>,
    docker: Vec<Kv>,
    docker_error: Option<String>,
    df: Vec<UsageRow>,
    health_status: &'static str,
    checks: Vec<CheckRow>,
    groups: Vec<GroupView>,
    preview_error: Option<String>,
    confirm_phrase: &'static str,
    app_logs: Vec<LogRow>,
    last_seq: u64,
}

struct LogRow {
    level: String,
    text: String,
}

fn status_str(s: HealthStatus) -> &'static str {
    match s {
        HealthStatus::Ok => "ok",
        HealthStatus::Degraded => "degraded",
        HealthStatus::Down => "down",
    }
}

fn df_rows(df: &DiskUsage) -> Vec<UsageRow> {
    let row = |name, b: &Option<UsageBucket>| {
        b.map(|b| {
            UsageRow(
                name,
                format!("{} / {}", b.active_count, b.total_count),
                fmt_bytes(u64::try_from(b.size).unwrap_or(0)),
                fmt_bytes(u64::try_from(b.reclaimable).unwrap_or(0)),
            )
        })
    };
    [
        row("Images", &df.images),
        row("Containers", &df.containers),
        row("Volumes", &df.volumes),
        row("Build cache", &df.build_cache),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn group_view(g: PruneGroup) -> GroupView {
    let (field, label) = match g.kind {
        PruneKind::BuildCache => ("build_cache", "Build cache"),
        PruneKind::Images => ("images", "Dangling images"),
        PruneKind::Networks => ("networks", "Unused networks"),
        PruneKind::Containers => ("containers", "Stopped containers"),
    };
    const SHOWN: usize = 15;
    GroupView {
        field,
        label,
        count: g.items.len(),
        size: fmt_bytes(g.total_size),
        default_selected: g.default_selected,
        needs_confirm: g.needs_confirm,
        more: g.items.len().saturating_sub(SHOWN),
        items: g
            .items
            .into_iter()
            .take(SHOWN)
            .map(|i| match i.project {
                Some(p) => format!("{} (project {p})", i.name),
                None => i.name,
            })
            .collect(),
    }
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    let svc = &s.services.system;
    let err = |e: CoreError| DashError::new(e, &headers);
    let usage = svc.usage(actor).await.map_err(err)?;
    let health = svc.health(actor).await.map_err(err)?;
    let app_logs = svc.logs(actor, None).await.map_err(err)?;
    let (groups, preview_error) = match svc.cleanup_preview(actor).await {
        Ok(p) => (p.groups.into_iter().map(group_view).collect(), None),
        Err(e) if matches!(e.kind, yanto_core::ErrorKind::Unavailable) => (
            Vec::new(),
            Some("Docker unavailable: no cleanup preview.".to_owned()),
        ),
        Err(e) => return Err(err(e)),
    };

    let h = &usage.host;
    let mem_used = h.mem_total.saturating_sub(h.mem_available);
    let pct = |a: u64, b: u64| {
        if b == 0 {
            0.0
        } else {
            a as f64 / b as f64 * 100.0
        }
    };
    let mut host = vec![
        Kv(
            "CPU".into(),
            format!("{:.1}% of {} cores", h.cpu_percent, h.cpu_count),
        ),
        Kv(
            "Memory".into(),
            format!(
                "{} used of {} ({:.0}%)",
                fmt_bytes(mem_used),
                fmt_bytes(h.mem_total),
                pct(mem_used, h.mem_total)
            ),
        ),
        Kv(
            "Swap".into(),
            format!(
                "{} used of {}",
                fmt_bytes(h.swap_used),
                fmt_bytes(h.swap_total)
            ),
        ),
        Kv(
            "Load average".into(),
            format!(
                "{:.2} {:.2} {:.2}",
                h.load_average[0], h.load_average[1], h.load_average[2]
            ),
        ),
    ];
    let up = h.uptime_secs;
    host.push(Kv(
        "Uptime".into(),
        format!(
            "{}d {}h {}m",
            up / 86_400,
            up % 86_400 / 3600,
            up % 3600 / 60
        ),
    ));
    let disks = h
        .disks
        .iter()
        .map(|d| DiskRow {
            mount: d.mount.clone(),
            total: fmt_bytes(d.total_bytes),
            free: fmt_bytes(d.available_bytes),
            used_percent: pct(
                d.total_bytes.saturating_sub(d.available_bytes),
                d.total_bytes,
            ) as u32,
            paths: d.paths.join(", "),
        })
        .collect();
    let (docker, df) = match &usage.docker.info {
        Some(i) => (
            vec![
                Kv(
                    "Engine".into(),
                    format!("{} (API {})", i.server_version, i.api_version),
                ),
                Kv("OS".into(), format!("{} / {}", i.os, i.kernel)),
                Kv("Storage driver".into(), i.storage_driver.clone()),
                Kv("Root dir".into(), i.root_dir.clone()),
                Kv(
                    "Containers".into(),
                    format!("{} running of {}", i.containers_running, i.containers),
                ),
                Kv("Images".into(), i.images.to_string()),
            ],
            usage
                .docker
                .disk_usage
                .as_ref()
                .map(df_rows)
                .unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    Ok(render(&SystemPage {
        ctx: PageCtx::new(&session, "System", "/system"),
        host,
        disks,
        docker,
        docker_error: usage.docker.error,
        df,
        health_status: status_str(health.status),
        checks: health
            .checks
            .into_iter()
            .map(|c| CheckRow {
                name: c.name,
                status: status_str(c.status),
                detail: c.detail,
            })
            .collect(),
        groups,
        preview_error,
        confirm_phrase: CONFIRM_CONTAINERS,
        last_seq: app_logs.last().map_or(0, |l| l.seq),
        app_logs: app_logs
            .into_iter()
            .map(|l| LogRow {
                level: l.level.to_ascii_lowercase(),
                text: format!("{} {}", l.target, l.text),
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
struct LogsStreamParams {
    /// Highest `seq` the page already shows; only newer lines are sent.
    after: Option<u64>,
}

/// SSE: one JSON event (`{seq, ts, level, target, text}`) per new application log line.
async fn logs_stream(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(p): Query<LogsStreamParams>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, DashError> {
    let after = p.after.unwrap_or(0);
    let (backlog, rx) = s
        .services
        .system
        .logs_stream(&session.0.actor, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let live = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(l) => return Some((l, rx)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let lines = futures_util::StreamExt::chain(futures_util::stream::iter(backlog), live);
    let events = futures_util::StreamExt::filter_map(lines, move |l| {
        futures_util::future::ready((l.seq > after).then(|| {
            Ok(Event::default()
                .json_data(&l)
                .unwrap_or_else(|_| Event::default().event("error").data("encode failed")))
        }))
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
struct CleanupForm {
    build_cache: Option<String>,
    images: Option<String>,
    networks: Option<String>,
    containers: Option<String>,
    #[serde(default)]
    confirm: String,
}

struct StepView {
    label: &'static str,
    ok: bool,
    deleted: u64,
    reclaimed: String,
    error: String,
}

#[derive(Template)]
#[template(path = "system/cleanup_result.html")]
struct ResultPage {
    ctx: PageCtx,
    steps: Vec<StepView>,
    total: String,
}

async fn cleanup(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CleanupForm>,
) -> Result<Response, DashError> {
    let sel = CleanupSelection {
        build_cache: f.build_cache.is_some(),
        images: f.images.is_some(),
        networks: f.networks.is_some(),
        containers: f.containers.is_some(),
        confirm: Some(f.confirm),
    };
    let out = s
        .services
        .system
        .cleanup(&session.0.actor, &sel)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&ResultPage {
        ctx: PageCtx::new(&session, "Cleanup result", "/system"),
        total: fmt_bytes(out.reclaimed_bytes),
        steps: out
            .steps
            .into_iter()
            .map(|st| StepView {
                label: match st.kind {
                    PruneKind::BuildCache => "Build cache",
                    PruneKind::Images => "Dangling images",
                    PruneKind::Networks => "Unused networks",
                    PruneKind::Containers => "Stopped containers",
                },
                ok: st.ok,
                deleted: st.deleted,
                reclaimed: fmt_bytes(st.reclaimed_bytes),
                error: st.error.unwrap_or_default(),
            })
            .collect(),
    }))
}
