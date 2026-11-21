//! `/cron`: Cron Entries (docs/rewrite/03 b, 04 section 5). Owner only; the service enforces it.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::cron::{EntryInput, Overlap};
use yanto_core::ports::{CronContent, CronSource};
use yanto_core::service::cron::{CronEntryDto, CronOverview, CronRunDto, FileStatus};

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cron", get(index).post(create))
        .route("/cron/system-files", get(system_files))
        .route("/cron/preview", post(preview))
        .route("/cron/reapply", post(reapply))
        .route("/cron/{id}", get(edit_page).put(update).delete(delete))
        .route("/cron/{id}/runs", get(runs))
        .route("/cron/{id}/toggle", post(toggle))
}

struct EntryView {
    id: String,
    name: String,
    schedule: String,
    run_as_user: String,
    command: String,
    enabled: bool,
    invalid_user: bool,
    next_run: String,
    last_status: String,
    last_run: String,
}

impl From<CronEntryDto> for EntryView {
    fn from(e: CronEntryDto) -> Self {
        Self {
            next_run: e.next_run.map_or_else(|| "-".into(), fmt_ts),
            last_status: e
                .last_run
                .as_ref()
                .map_or_else(|| "never".into(), |r| r.status.clone()),
            last_run: fmt_opt_ts(e.last_run.as_ref().map(|r| r.started_at)),
            id: e.id,
            name: e.name,
            schedule: e.schedule,
            run_as_user: e.run_as_user,
            command: e.command,
            enabled: e.enabled,
            invalid_user: e.invalid_user,
        }
    }
}

/// Values shown in the create form (kept when validation fails).
#[derive(Default, Clone)]
struct FormValues {
    name: String,
    schedule: String,
    run_as_user: String,
    command: String,
    overlap_skip: bool,
    timeout_s: String,
}

#[derive(Template)]
#[template(path = "cron/index.html")]
struct IndexPage {
    ctx: PageCtx,
    entries: Vec<EntryView>,
    drift: bool,
    drift_missing: bool,
    unsafe_reason: Option<String>,
    service_inactive: bool,
    users: Vec<String>,
    form: FormValues,
    error: Option<String>,
}

async fn index_page(
    s: &AppState,
    session: &Session,
    form: FormValues,
    error: Option<String>,
) -> Result<IndexPage, CoreError> {
    let o: CronOverview = s.services.cron.list(&session.0.actor).await?;
    let (drift, drift_missing, unsafe_reason) = match o.file {
        FileStatus::InSync => (false, false, None),
        FileStatus::Drift { missing } => (true, missing, None),
        FileStatus::Unsafe { reason } => (false, false, Some(reason)),
    };
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Cron", "/cron"),
        entries: o.entries.into_iter().map(Into::into).collect(),
        drift,
        drift_missing,
        unsafe_reason,
        service_inactive: o.service_active == Some(false),
        users: o.login_users,
        form,
        error,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = index_page(&s, &session, FormValues::default(), None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct EntryForm {
    name: String,
    schedule: String,
    run_as_user: String,
    command: String,
    #[serde(default)]
    overlap: String,
    #[serde(default)]
    timeout_s: String,
}

impl EntryForm {
    /// `enabled` is set by the caller (create: on, edit: unchanged).
    fn input(&self) -> Result<EntryInput, CoreError> {
        let timeout_s = match self.timeout_s.trim() {
            "" => None,
            t => Some(t.parse::<i64>().map_err(|_| {
                CoreError::validation("timeout_s", "timeout must be a number of seconds")
            })?),
        };
        Ok(EntryInput {
            name: self.name.clone(),
            schedule: self.schedule.clone(),
            run_as_user: self.run_as_user.clone(),
            command: self.command.clone(),
            overlap: Overlap::parse(&self.overlap).unwrap_or(Overlap::Allow),
            timeout_s,
            enabled: true,
        })
    }

    fn values(&self) -> FormValues {
        FormValues {
            name: self.name.clone(),
            schedule: self.schedule.clone(),
            run_as_user: self.run_as_user.clone(),
            command: self.command.clone(),
            overlap_skip: self.overlap == "skip",
            timeout_s: self.timeout_s.clone(),
        }
    }
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<EntryForm>,
) -> Result<Response, DashError> {
    // New entries from the page are enabled unless unchecked.
    let result = async {
        let input = f.input()?;
        s.services.cron.create(&session.0.actor, &input).await
    }
    .await;
    match result {
        Ok(_) => Ok(Redirect::to("/cron").into_response()),
        Err(e) if is_form_error(&e) => {
            let p = index_page(&s, &session, f.values(), Some(public_message(&e)))
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

/// Errors the form shows inline: bad input, duplicate names, or a refused file write.
fn is_form_error(e: &CoreError) -> bool {
    matches!(
        e.kind,
        yanto_core::ErrorKind::Validation(_)
            | yanto_core::ErrorKind::Conflict
            | yanto_core::ErrorKind::Unavailable
    )
}

#[derive(Template)]
#[template(path = "cron/edit.html")]
struct EditPage {
    ctx: PageCtx,
    id: String,
    form: FormValues,
    enabled: bool,
    users: Vec<String>,
    runs: Vec<RunView>,
}

struct RunView {
    status: String,
    exit_code: String,
    started: String,
    duration: String,
    output: String,
}

impl From<CronRunDto> for RunView {
    fn from(r: CronRunDto) -> Self {
        Self {
            exit_code: r.exit_code.map_or_else(|| "-".into(), |c| c.to_string()),
            started: fmt_ts(r.started_at),
            duration: r
                .duration_ms
                .map_or_else(|| "-".into(), |d| format!("{} ms", d)),
            output: r.output_tail.unwrap_or_default(),
            status: r.status,
        }
    }
}

async fn edit_page(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let go = async {
        let a = &session.0.actor;
        let e = s.services.cron.get(a, &id).await?;
        let o = s.services.cron.list(a).await?;
        let runs = s.services.cron.runs_list(a, &id).await?;
        Ok::<_, CoreError>(EditPage {
            ctx: PageCtx::new(&session, format!("Cron: {}", e.name), "/cron"),
            id: e.id.clone(),
            enabled: e.enabled,
            form: FormValues {
                name: e.name,
                schedule: e.schedule,
                run_as_user: e.run_as_user,
                command: e.command,
                overlap_skip: e.overlap == "skip",
                timeout_s: e.timeout_s.map(|t| t.to_string()).unwrap_or_default(),
            },
            users: o.login_users,
            runs: runs.into_iter().map(Into::into).collect(),
        })
    };
    Ok(render(&go.await.map_err(|e| DashError::new(e, &headers))?))
}

/// htmx `hx-put` from the edit form: success redirects to the list, errors become a toast.
async fn update(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<EntryForm>,
) -> Result<Response, DashError> {
    let go = async {
        let current = s.services.cron.get(&session.0.actor, &id).await?;
        let mut input = f.input()?;
        input.enabled = current.enabled;
        s.services.cron.update(&session.0.actor, &id, &input).await
    };
    go.await.map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/cron"))
}

fn hx_redirect(to: &'static str) -> Response {
    let mut res = (StatusCode::OK, Html("")).into_response();
    res.headers_mut()
        .insert("hx-redirect", HeaderValue::from_static(to));
    res
}

async fn delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .cron
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

async fn toggle(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let go = async {
        let e = s.services.cron.get(&session.0.actor, &id).await?;
        s.services
            .cron
            .toggle(&session.0.actor, &id, !e.enabled)
            .await
    };
    go.await.map_err(|e| DashError::new(e, &headers))?;
    Ok(hx_redirect("/cron"))
}

#[derive(Deserialize, Default)]
struct ReapplyForm {
    #[serde(default)]
    replace_unsafe: Option<String>,
}

async fn reapply(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<ReapplyForm>,
) -> Result<Response, DashError> {
    s.services
        .cron
        .reapply(&session.0.actor, f.replace_unsafe.is_some())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Redirect::to("/cron").into_response())
}

#[derive(Template)]
#[template(path = "cron/_runs.html")]
struct RunsFragment {
    runs: Vec<RunView>,
}

async fn runs(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let runs = s
        .services
        .cron
        .runs_list(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&RunsFragment {
        runs: runs.into_iter().map(Into::into).collect(),
    }))
}

struct SourceView {
    path: String,
    kind: &'static str,
    text: String,
    note: Option<String>,
}

#[derive(Template)]
#[template(path = "cron/_system_files.html")]
struct SystemFilesFragment {
    sources: Vec<SourceView>,
}

async fn system_files(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let sources: Vec<CronSource> = s
        .services
        .cron
        .files_list(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let sources = sources
        .into_iter()
        .map(|c| {
            let (text, note) = match c.content {
                CronContent::Text(t) => (t, None),
                CronContent::Truncated(t) => (t, Some("truncated at 256 KiB".to_owned())),
                CronContent::Unreadable(why) => (String::new(), Some(format!("not shown: {why}"))),
                CronContent::NameOnly => (String::new(), None),
            };
            SourceView {
                path: c.path,
                kind: match c.kind {
                    yanto_core::ports::CronSourceKind::Crontab => "crontab",
                    yanto_core::ports::CronSourceKind::CronD => "cron.d",
                    yanto_core::ports::CronSourceKind::Periodic => "periodic",
                    yanto_core::ports::CronSourceKind::UserSpool => "user crontab",
                },
                text,
                note,
            }
        })
        .collect();
    Ok(render(&SystemFilesFragment { sources }))
}

#[derive(Deserialize)]
struct PreviewForm {
    schedule: String,
}

/// htmx fragment under the schedule field: the next 5 run times, or the validation message.
async fn preview(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<PreviewForm>,
) -> Response {
    match s
        .services
        .cron
        .preview(&session.0.actor, &f.schedule, 5)
        .await
    {
        Ok(p) => {
            let mut html = String::from("<ul class=\"muted\">");
            for t in &p.next_runs {
                html.push_str(&format!("<li>{}</li>", fmt_ts(*t)));
            }
            if p.next_runs.is_empty() {
                html.push_str("<li>runs at boot</li>");
            }
            html.push_str("</ul>");
            if p.day_fields_or {
                html.push_str("<p class=\"muted\">Day-of-month and day-of-week are both set: cron runs when either matches.</p>");
            }
            html.push_str("<p class=\"muted\">Times are in UTC.</p>");
            Html(html).into_response()
        }
        Err(e) if matches!(e.kind, yanto_core::ErrorKind::Validation(_)) => Html(format!(
            "<p class=\"error\">{}</p>",
            super::view::escape(&e.message)
        ))
        .into_response(),
        Err(e) => DashError::new(e, &HeaderMap::new()).into_response(),
    }
}
