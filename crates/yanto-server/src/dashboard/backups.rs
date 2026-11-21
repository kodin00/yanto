//! `/backups`: local, downloadable Backups (docs/rewrite/04 section 7). Members with `backups`
//! on a Project see that Project's rows; the Yanto-DB archive (it contains `app.key`) is Owner only.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::backup::{BackupKind, Trigger};
use yanto_core::service::backups::BackupDto;

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/backups", get(index).post(create))
        .route("/backups/{id}", axum::routing::delete(delete))
        .route("/backups/{id}/download", get(download))
}

struct BackupView {
    id: String,
    kind: String,
    status: String,
    trigger: String,
    filename: String,
    size: String,
    sha256: String,
    created: String,
    downloaded: String,
    downloads: i64,
    sensitive: bool,
    error: String,
    can_download: bool,
}

fn human(bytes: i64) -> String {
    let b = bytes as f64;
    match bytes {
        n if n >= 1 << 30 => format!("{:.1} GiB", b / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MiB", b / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{:.1} KiB", b / 1024.0),
        n => format!("{n} B"),
    }
}

impl From<BackupDto> for BackupView {
    fn from(b: BackupDto) -> Self {
        Self {
            size: b.size_bytes.map_or_else(|| "-".into(), human),
            sha256: b.sha256.clone().unwrap_or_default(),
            created: fmt_ts(b.created_at),
            downloaded: fmt_opt_ts(b.downloaded_at),
            can_download: b.status == "success",
            id: b.id,
            kind: b.kind,
            status: b.status,
            trigger: b.trigger,
            filename: b.filename,
            downloads: b.download_count,
            sensitive: b.sensitive,
            error: b.error.unwrap_or_default(),
        }
    }
}

#[derive(Template)]
#[template(path = "backups/index.html")]
struct IndexPage {
    ctx: PageCtx,
    backups: Vec<BackupView>,
    can_create_yanto: bool,
    error: Option<String>,
}

async fn page(
    s: &AppState,
    session: &Session,
    error: Option<String>,
) -> Result<IndexPage, yanto_core::CoreError> {
    let a = &session.0.actor;
    let backups = s.services.backups.list(a).await?;
    let can_create_yanto = s
        .services
        .backups
        .targets(a)
        .await?
        .iter()
        .any(|t| t.kind == BackupKind::YantoSqlite);
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Backups", "/backups"),
        backups: backups.into_iter().map(Into::into).collect(),
        can_create_yanto,
        error,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = page(&s, &session, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct CreateForm {
    kind: String,
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CreateForm>,
) -> Result<Response, DashError> {
    let result = match BackupKind::parse(&f.kind) {
        Some(BackupKind::YantoSqlite) => s
            .services
            .backups
            .create_yanto(&session.0.actor, Trigger::Manual, None)
            .await
            .map(|_| ()),
        // TODO(M2-merge): Project kinds are created from the Project's Backups tab.
        _ => Err(yanto_core::CoreError::validation(
            "kind",
            "only the Yanto database can be backed up from this page until Projects are available",
        )),
    };
    match result {
        Ok(()) => Ok(Redirect::to("/backups").into_response()),
        Err(e)
            if matches!(
                e.kind,
                yanto_core::ErrorKind::Validation(_)
                    | yanto_core::ErrorKind::Conflict
                    | yanto_core::ErrorKind::Unavailable
            ) =>
        {
            let p = page(&s, &session, Some(public_message(&e)))
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

async fn delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .backups
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

async fn download(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let file = s
        .services
        .backups
        .download(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    crate::download::file_response(file)
        .await
        .map_err(|e| DashError::new(e, &headers))
}
