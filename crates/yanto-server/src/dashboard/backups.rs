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
use yanto_core::service::backups::{BackupDto, RestoreOptions};

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/backups", get(index).post(create))
        .route("/backups/{id}", axum::routing::delete(delete))
        .route("/backups/{id}/download", get(download))
        .route("/backups/{id}/restore", axum::routing::post(restore))
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
    /// Owner only: Project backups can be restored from this page.
    can_restore: bool,
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
            can_restore: false,
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
    /// Project volumes and declared databases the actor may back up.
    targets: Vec<TargetView>,
    error: Option<String>,
    notice: Option<String>,
}

struct TargetView {
    /// `kind|project_id|target`, parsed back by [`create`].
    value: String,
    label: String,
}

async fn page(
    s: &AppState,
    session: &Session,
    error: Option<String>,
    notice: Option<String>,
) -> Result<IndexPage, yanto_core::CoreError> {
    let a = &session.0.actor;
    let backups = s.services.backups.list(a).await?;
    let all = s.services.backups.targets(a).await?;
    let can_create_yanto = all.iter().any(|t| t.kind == BackupKind::YantoSqlite);
    let targets = all
        .iter()
        .filter_map(|t| {
            let pid = t.project_id.as_deref()?;
            Some(TargetView {
                value: format!("{}|{pid}|{}", t.kind.as_str(), t.target.as_deref()?),
                label: t.label.clone(),
            })
        })
        .collect();
    let owner = a.is_owner();
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Backups", "/backups"),
        backups: backups
            .into_iter()
            .map(|b| {
                let mut v = BackupView::from(b);
                v.can_restore = owner && v.can_download && v.kind != "yanto_sqlite";
                v
            })
            .collect(),
        can_create_yanto,
        targets,
        error,
        notice,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = page(&s, &session, None, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct CreateForm {
    kind: String,
    /// A Project target: `kind|project_id|target` from the targets list.
    #[serde(default)]
    target: String,
    #[serde(default)]
    stop_project: Option<String>,
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
        Some(_) => Err(yanto_core::CoreError::validation(
            "kind",
            "pick a Project target",
        )),
        None if f.kind == "project" => {
            let mut it = f.target.splitn(3, '|');
            match (it.next(), it.next(), it.next()) {
                (Some(k), Some(pid), Some(t)) => match BackupKind::parse(k) {
                    Some(kind) if kind.is_project() => s
                        .services
                        .backups
                        .create_project(
                            &session.0.actor,
                            pid,
                            kind,
                            Some(t),
                            f.stop_project.is_some(),
                        )
                        .await
                        .map(|_| ()),
                    _ => Err(yanto_core::CoreError::validation(
                        "target",
                        "unknown target",
                    )),
                },
                _ => Err(yanto_core::CoreError::validation("target", "pick a target")),
            }
        }
        None => Err(yanto_core::CoreError::validation("kind", "unknown kind")),
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
            let p = page(&s, &session, Some(public_message(&e)), None)
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

#[derive(Deserialize)]
struct RestoreForm {
    #[serde(default)]
    in_place: Option<String>,
    #[serde(default)]
    confirm_name: String,
}

/// Owner only (`backup_restore`). Volumes default to a new `<volume>-restore-<ts>` volume.
async fn restore(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<RestoreForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .backups
        .restore(
            &session.0.actor,
            &id,
            RestoreOptions {
                in_place: f.in_place.is_some(),
                confirm_name: Some(f.confirm_name).filter(|n| !n.trim().is_empty()),
            },
        )
        .await;
    match r {
        Ok(out) => {
            let mut msg = match &out.new_volume {
                Some(v) => format!("Restored into the new volume {v}. The original is unchanged."),
                None => "Restored.".to_owned(),
            };
            if out.pre_restore.is_some() {
                msg.push_str(" A pre-restore backup of the previous state was kept.");
            }
            let p = page(&s, &session, None, Some(msg))
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render(&p))
        }
        Err(e)
            if matches!(
                e.kind,
                yanto_core::ErrorKind::Validation(_)
                    | yanto_core::ErrorKind::Conflict
                    | yanto_core::ErrorKind::Unavailable
            ) =>
        {
            let p = page(&s, &session, Some(public_message(&e)), None)
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}
