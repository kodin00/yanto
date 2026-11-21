//! `/api/v1/containers`: list, inspect, logs (text and SSE) and start/stop/restart.
//! There is deliberately no exec endpoint (D24): terminals exist only in the dashboard.

use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use yanto_core::ports::{LogLine, LogStream};
use yanto_core::service::containers::{ContainerDetailDto, ContainerList};

use super::ApiActor;
use crate::app::AppState;
use crate::error::{ApiError, public_message};
use crate::stream_guard::{RECHECK, lost_access, revocable, token_check};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/containers", get(list))
        .route("/containers/{id}", get(get_one))
        .route("/containers/{id}/logs", get(logs))
        .route("/containers/{id}/logs/stream", get(logs_stream))
        .route("/containers/{id}/start", post(start))
        .route("/containers/{id}/stop", post(stop))
        .route("/containers/{id}/restart", post(restart))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<ContainerList>, ApiError> {
    Ok(Json(s.services.containers.list(&a).await?))
}

async fn get_one(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<ContainerDetailDto>, ApiError> {
    Ok(Json(s.services.containers.get(&a, &id).await?))
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    tail: Option<u32>,
}

fn render_line(l: &LogLine) -> String {
    let stream = match l.stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    };
    format!(
        "{} [{stream}] {}",
        l.timestamp.as_deref().unwrap_or("-"),
        l.text
    )
}

async fn logs(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    params: Result<Query<LogsParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(p) = params.map_err(|_| ApiError::bad_request("invalid query string"))?;
    let lines = s.services.containers.logs(&a, &id, p.tail).await?;
    let body: String = lines.iter().map(|l| render_line(l) + "\n").collect();
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
}

#[derive(Debug, Deserialize)]
struct StreamParams {
    since: Option<i64>,
}

async fn logs_stream(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    params: Result<Query<StreamParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let Query(p) = params.map_err(|_| ApiError::bad_request("invalid query string"))?;
    let stream = s.services.containers.logs_stream(&a, &id, p.since).await?;
    let events = stream.map(|item| {
        Ok(match item {
            Ok(line) => Event::default()
                .json_data(&line)
                .unwrap_or_else(|_| Event::default().event("error").data("encode failed")),
            Err(e) => Event::default().event("error").data(public_message(&e)),
        })
    });
    let check = token_check(s.services.clone(), &headers, a.ip.clone(), move |svc, a| {
        let id = id.clone();
        async move {
            match svc.containers.get(&a, &id).await {
                Ok(_) => true,
                Err(e) => !lost_access(&e),
            }
        }
    });
    Ok(Sse::new(revocable(events, check, RECHECK)).keep_alive(KeepAlive::default()))
}

async fn start(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.containers.start(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn stop(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.containers.stop(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn restart(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.containers.restart(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
