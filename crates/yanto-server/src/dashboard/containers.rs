//! `/containers`: list, detail, start/stop/restart, logs (snapshot + SSE follow) and the
//! entry page of the exec terminal (the WebSocket is in `exec.rs`).

use std::convert::Infallible;

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::ports::{LogLine, LogStream};
use yanto_core::service::containers::ContainerDto;
use yanto_core::service::fmt_bytes;

use super::exec;
use super::session::Session;
use super::view::{DashError, PageCtx, fmt_ts, render};
use crate::app::AppState;

const TERMINAL_CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; \
connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/containers", get(index))
        .route("/containers/rows", get(rows))
        .route("/containers/{id}", get(detail))
        .route("/containers/{id}/start", post(start))
        .route("/containers/{id}/stop", post(stop))
        .route("/containers/{id}/restart", post(restart))
        .route("/containers/{id}/logs", get(logs))
        .route("/containers/{id}/logs/stream", get(logs_stream))
        .route("/containers/{id}/exec", get(exec_page))
        .route("/containers/{id}/exec/ws", get(exec::ws))
}

struct RowView {
    id: String,
    short_id: String,
    name: String,
    image: String,
    state: String,
    status: String,
    ports: String,
    project: String,
    cpu: String,
    mem: String,
    created: String,
    running: bool,
    managed: bool,
}

impl From<ContainerDto> for RowView {
    fn from(c: ContainerDto) -> Self {
        let project = match (&c.compose_project, &c.compose_service) {
            (Some(p), Some(s)) => format!("{p} / {s}"),
            (Some(p), None) => p.clone(),
            _ => String::new(),
        };
        Self {
            running: c.state == "running",
            managed: c.managed.is_some(),
            cpu: c
                .cpu_percent
                .map_or_else(String::new, |v| format!("{v:.1}%")),
            mem: match (c.mem_usage, c.mem_limit) {
                (Some(u), Some(l)) if l > 0 => {
                    format!("{} ({:.0}%)", fmt_bytes(u), u as f64 / l as f64 * 100.0)
                }
                (Some(u), _) => fmt_bytes(u),
                _ => String::new(),
            },
            created: fmt_ts(c.created.saturating_mul(1000)),
            ports: c.ports.join(", "),
            id: c.id,
            short_id: c.short_id,
            name: c.name,
            image: c.image,
            state: c.state,
            status: c.status,
            project,
        }
    }
}

async fn row_views(s: &AppState, session: &Session) -> Result<Vec<RowView>, CoreError> {
    Ok(s.services
        .containers
        .list(&session.0.actor)
        .await?
        .containers
        .into_iter()
        .map(Into::into)
        .collect())
}

#[derive(Template)]
#[template(path = "containers/index.html")]
struct IndexPage {
    ctx: PageCtx,
    rows: Vec<RowView>,
    docker_error: Option<String>,
}

#[derive(Template)]
#[template(path = "containers/_rows.html")]
struct RowsFragment {
    rows: Vec<RowView>,
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let (rows, docker_error) = match row_views(&s, &session).await {
        Ok(r) => (r, None),
        Err(e) if matches!(e.kind, yanto_core::ErrorKind::Unavailable) => {
            (Vec::new(), Some("Docker unavailable".to_owned()))
        }
        Err(e) => return Err(DashError::new(e, &headers)),
    };
    Ok(render(&IndexPage {
        ctx: PageCtx::new(&session, "Containers", "/containers"),
        rows,
        docker_error,
    }))
}

async fn rows(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let rows = row_views(&s, &session)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&RowsFragment { rows }))
}

struct DetailView {
    row: RowView,
    command: String,
    restart_count: i64,
    started_at: String,
    finished_at: String,
    exit_code: String,
    health: String,
    mounts: Vec<(String, String, bool)>,
    networks: String,
}

#[derive(Template)]
#[template(path = "containers/detail.html")]
struct DetailPage {
    ctx: PageCtx,
    d: DetailView,
}

async fn detail(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let out = s
        .services
        .containers
        .get(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let d = out.detail;
    let title = format!("Container {}", d.info.name);
    Ok(render(&DetailPage {
        ctx: PageCtx::new(&session, title, "/containers"),
        d: DetailView {
            row: out.summary.into(),
            command: d.command,
            restart_count: d.restart_count,
            started_at: d.started_at.unwrap_or_default(),
            finished_at: d.finished_at.unwrap_or_default(),
            exit_code: d.exit_code.map(|c| c.to_string()).unwrap_or_default(),
            health: d.health.unwrap_or_default(),
            mounts: d
                .mounts
                .into_iter()
                .map(|m| (m.source, m.destination, m.read_write))
                .collect(),
            networks: d.networks.join(", "),
        },
    }))
}

#[derive(Clone, Copy)]
enum Act {
    Start,
    Stop,
    Restart,
}

async fn act(
    s: AppState,
    session: Session,
    headers: HeaderMap,
    id: String,
    act: Act,
) -> Result<Response, DashError> {
    let svc = &s.services.containers;
    let actor = &session.0.actor;
    match act {
        Act::Start => svc.start(actor, &id).await,
        Act::Stop => svc.stop(actor, &id).await,
        Act::Restart => svc.restart(actor, &id).await,
    }
    .map_err(|e| DashError::new(e, &headers))?;
    if super::view::is_htmx(&headers) {
        // Refresh the rows the button lives in.
        let rows = row_views(&s, &session)
            .await
            .map_err(|e| DashError::new(e, &headers))?;
        return Ok(render(&RowsFragment { rows }));
    }
    Ok(Redirect::to("/containers").into_response())
}

async fn start(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    act(s, session, headers, id, Act::Start).await
}

async fn stop(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    act(s, session, headers, id, Act::Stop).await
}

async fn restart(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    act(s, session, headers, id, Act::Restart).await
}

struct LogView {
    stream: &'static str,
    ts: String,
    text: String,
}

fn log_view(l: LogLine) -> LogView {
    LogView {
        stream: match l.stream {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
        },
        ts: l.timestamp.unwrap_or_default(),
        text: l.text,
    }
}

#[derive(Template)]
#[template(path = "containers/logs.html")]
struct LogsPage {
    ctx: PageCtx,
    id: String,
    name: String,
    lines: Vec<LogView>,
}

#[derive(Deserialize)]
struct LogsParams {
    tail: Option<u32>,
}

async fn logs(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<LogsParams>,
) -> Result<Response, DashError> {
    let svc = &s.services.containers;
    let name = svc
        .get(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?
        .summary
        .name;
    let lines = svc
        .logs(&session.0.actor, &id, p.tail)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&LogsPage {
        ctx: PageCtx::new(&session, format!("Logs: {name}"), "/containers"),
        id,
        name,
        lines: lines.into_iter().map(log_view).collect(),
    }))
}

#[derive(Deserialize)]
struct StreamParams {
    since: Option<i64>,
}

/// SSE: one `data:` JSON event per log line (`{stream, timestamp, text}`); the browser escapes
/// by inserting it as text. Errors end the stream with an `error` event.
async fn logs_stream(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<StreamParams>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, DashError> {
    let stream = s
        .services
        .containers
        .logs_stream(&session.0.actor, &id, p.since)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let events = stream.map(|item| {
        Ok(match item {
            Ok(line) => Event::default()
                .json_data(&line)
                .unwrap_or_else(|_| Event::default().event("error").data("encode failed")),
            Err(e) => Event::default()
                .event("error")
                .data(crate::error::public_message(&e)),
        })
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

#[derive(Template)]
#[template(path = "containers/exec.html")]
struct ExecPage {
    ctx: PageCtx,
    id: String,
    name: String,
}

async fn exec_page(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let info = s
        .services
        .containers
        .exec_precheck(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut res = render(&ExecPage {
        ctx: PageCtx::new(&session, format!("Terminal: {}", info.name), "/containers"),
        id: info.id,
        name: info.name,
    });
    // xterm.js's DOM renderer writes its theme and cell sizes into <style> elements, which the
    // global `default-src 'self'` policy blocks. Relax styles (never scripts) on this page only.
    res.headers_mut().insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(TERMINAL_CSP),
    );
    Ok(res)
}
