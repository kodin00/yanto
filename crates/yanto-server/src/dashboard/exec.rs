//! The exec terminal (docs/rewrite/04 2.2, D13/D24): a page plus a WebSocket that pumps bytes
//! between xterm.js and a TTY exec inside the container.
//!
//! Dashboard only: the route lives behind the session middleware, so API Tokens cannot reach it,
//! and the operation itself is `session_only`. Because a WebSocket upgrade is a GET, the CSRF
//! layer does not run; the `Origin` check here is what blocks cross-site WebSocket hijacking.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;
use yanto_core::CoreError;
use yanto_core::ports::ExecIo;
use yanto_core::service::containers::ExecTicket;

use super::session::Session;
use super::view::DashError;
use crate::app::AppState;
use crate::net::same_origin;

/// One WebSocket frame is capped at 64 KiB (04 2.2 step 4).
const MAX_FRAME: usize = 64 * 1024;
const WS_CLOSE_NORMAL: u16 = 1000;
const WS_CLOSE_POLICY: u16 = 1008;

/// Terminal timeouts. ASSUMPTION values from 04 2.2 / OQ-25: idle 15 min, at most 4 h.
#[derive(Debug, Clone, Copy)]
pub struct ExecLimits {
    pub idle: Duration,
    pub max_session: Duration,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(15 * 60),
            max_session: Duration::from_secs(4 * 3600),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SizeParams {
    cols: Option<u16>,
    rows: Option<u16>,
}

pub async fn ws(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(size): Query<SizeParams>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, DashError> {
    if !same_origin(&headers, &s.config.server.public_url) {
        return Err(DashError::new(
            CoreError::forbidden("cross-origin WebSocket blocked"),
            &headers,
        ));
    }
    // Everything that can refuse (permission, stopped container, session caps) happens before
    // the upgrade, so refusals are plain HTTP statuses and no exec exists on failure.
    let (io, ticket) = s
        .services
        .containers
        .exec_open(
            &session.0.actor,
            &id,
            size.cols.unwrap_or(80),
            size.rows.unwrap_or(24),
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let limits = s.exec_limits;
    Ok(upgrade
        .max_frame_size(MAX_FRAME)
        .max_message_size(MAX_FRAME)
        .on_upgrade(move |socket| pump(socket, io, ticket, limits)))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Control {
    Resize { cols: u16, rows: u16 },
}

/// Move bytes both ways until either side ends, then hang the shell up and audit.
async fn pump(mut socket: WebSocket, io: ExecIo, ticket: ExecTicket, limits: ExecLimits) {
    let ExecIo {
        mut output,
        mut input,
        ..
    } = io;
    let (mut bytes_in, mut bytes_out) = (0u64, 0u64);
    let started = Instant::now();
    let mut idle_deadline = started + limits.idle;
    let hard_deadline = started + limits.max_session;

    let reason: &str = loop {
        let deadline = idle_deadline.min(hard_deadline);
        tokio::select! {
            msg = socket.recv() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break "closed",
                Some(Ok(Message::Binary(b))) => {
                    idle_deadline = Instant::now() + limits.idle;
                    bytes_in += b.len() as u64;
                    if input.write_all(&b).await.is_err() {
                        break "exited";
                    }
                    let _ = input.flush().await;
                }
                Some(Ok(Message::Text(t))) => {
                    // Text frames are control messages only; typed input travels as binary.
                    if let Ok(Control::Resize { cols, rows }) = serde_json::from_str(t.as_str())
                        && let Err(e) = ticket.resize(cols, rows).await
                    {
                        tracing::debug!(error = %e, "exec resize failed");
                    }
                }
                Some(Ok(_)) => {}
            },
            chunk = output.next() => match chunk {
                Some(Ok(bytes)) => {
                    bytes_out += bytes.len() as u64;
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break "closed";
                    }
                }
                None => break "exited",
                Some(Err(_)) => break "error",
            },
            () = tokio::time::sleep_until(deadline) => {
                break if Instant::now() >= hard_deadline { "max_duration" } else { "idle" };
            }
        }
    };

    let (code, text) = match reason {
        "idle" => (WS_CLOSE_POLICY, "idle timeout"),
        "max_duration" => (WS_CLOSE_POLICY, "session time limit reached"),
        _ => (WS_CLOSE_NORMAL, "session ended"),
    };
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: text.into(),
        })))
        .await;
    ticket.finish(input, bytes_in, bytes_out, reason).await;
}
