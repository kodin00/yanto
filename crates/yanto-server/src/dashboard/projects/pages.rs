//! `/projects`: list, create, detail and every Project action.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::actor::Actor;
use yanto_core::deploy::env::VarChange;
use yanto_core::secrets::Secret;
use yanto_core::service::deployments::{
    self as dep, DeployRequest, RollbackPreview, RollbackRequest,
};
use yanto_core::service::project_databases::{DatabaseDto, DatabaseInput};
use yanto_core::service::projects::{
    self as prj, DeleteOptions, PortInput, ProjectDto, ProjectInput, ProjectPatch, SourceInput,
};
use yanto_core::{CoreError, ErrorKind};

use super::{redirect, toast};
use crate::app::AppState;
use crate::dashboard::session::Session;
use crate::dashboard::view::{DashError, PageCtx, escape, fmt_ts, render, render_status};
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(index).post(create))
        .route("/projects/new", get(new_form))
        .route("/projects/{id}", get(detail).delete(delete_project))
        .route("/projects/{id}/settings", post(settings))
        .route("/projects/{id}/deploy", post(deploy))
        .route("/projects/{id}/stop", post(stop))
        .route("/projects/{id}/restart", post(restart))
        .route("/projects/{id}/rollback/preview", post(rollback_preview))
        .route("/projects/{id}/rollback", post(rollback))
        .route("/projects/{id}/compose", get(compose))
        .route(
            "/projects/{id}/env",
            get(env_get).put(env_put).patch(env_patch),
        )
        .route("/projects/{id}/env/preview", post(env_preview))
        .route("/projects/{id}/deploy-token/reveal", post(token_reveal))
        .route("/projects/{id}/deploy-token/rotate", post(token_rotate))
        .route("/projects/{id}/ports", get(ports_get).put(ports_put))
        .route(
            "/projects/{id}/databases",
            axum::routing::put(databases_put),
        )
}

fn err(e: CoreError, headers: &HeaderMap) -> DashError {
    DashError::new(e, headers)
}

struct ProjectRowView {
    id: String,
    name: String,
    slug: String,
    source: String,
    status: String,
    status_class: String,
    ports: String,
    last: String,
}

fn source_label(p: &ProjectDto) -> String {
    match (&p.git_url, &p.image_ref) {
        (Some(g), _) => format!("git {g} @ {}", p.branch.as_deref().unwrap_or("master")),
        (_, Some(i)) => format!("image {i}"),
        _ => String::new(),
    }
}

impl From<&ProjectDto> for ProjectRowView {
    fn from(p: &ProjectDto) -> Self {
        let (status, last) = match &p.last_deployment {
            Some(d) => (d.status.clone(), fmt_ts(d.queued_at)),
            None => ("never deployed".to_owned(), String::new()),
        };
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            slug: p.slug.clone(),
            source: source_label(p),
            status_class: status.replace(' ', "-"),
            status,
            ports: p
                .ports
                .iter()
                .map(|x| format!("{}:{}", x.bind_address, x.host_port))
                .collect::<Vec<_>>()
                .join(", "),
            last,
        }
    }
}

#[derive(Template)]
#[template(path = "projects/index.html")]
struct IndexPage {
    ctx: PageCtx,
    projects: Vec<ProjectRowView>,
    can_create: bool,
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    let projects = s
        .services
        .projects
        .list(actor)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(render(&IndexPage {
        ctx: PageCtx::new(&session, "Projects", "/projects"),
        projects: projects.iter().map(Into::into).collect(),
        can_create: actor.authorize(&prj::OP_PROJECT_CREATE, None).is_ok(),
    }))
}

struct Choice {
    id: String,
    label: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "projects/new.html")]
struct NewPage {
    ctx: PageCtx,
    error: Option<String>,
    form: NewForm,
    keys: Vec<Choice>,
    creds: Vec<Choice>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct NewForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    compose_path: String,
    #[serde(default)]
    env_file: String,
    #[serde(default)]
    compose_content: String,
    #[serde(default)]
    deploy_key_id: String,
    #[serde(default)]
    registry_credential_id: String,
    #[serde(default)]
    generate_deploy_key: Option<String>,
    #[serde(default)]
    auto_start: Option<String>,
}

async fn choices(
    s: &AppState,
    actor: &Actor,
    selected_key: &str,
    selected_cred: &str,
) -> Result<(Vec<Choice>, Vec<Choice>), CoreError> {
    let keys = s.services.deploy_keys.list(actor).await?;
    let creds = s.services.registry_credentials.list(actor).await?;
    Ok((
        keys.into_iter()
            .map(|k| Choice {
                selected: k.id == selected_key,
                label: format!("{} ({})", k.name, k.fingerprint),
                id: k.id,
            })
            .collect(),
        creds
            .into_iter()
            .map(|c| Choice {
                selected: c.id == selected_cred,
                label: format!("{} ({})", c.name, c.registry_host),
                id: c.id,
            })
            .collect(),
    ))
}

async fn new_form(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    actor
        .authorize(&prj::OP_PROJECT_CREATE, None)
        .map_err(|e| err(e, &headers))?;
    let (keys, creds) = choices(&s, actor, "", "")
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(render(&NewPage {
        ctx: PageCtx::new(&session, "New project", "/projects"),
        error: None,
        form: NewForm {
            auto_start: Some("on".into()),
            ..Default::default()
        },
        keys,
        creds,
    }))
}

#[derive(Template)]
#[template(path = "projects/created.html")]
struct CreatedPage {
    ctx: PageCtx,
    project_id: String,
    name: String,
    token: String,
    public_key: Option<String>,
    fingerprint: String,
    base_url: String,
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<NewForm>,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    let source = if f.branch.trim().is_empty() {
        SourceInput::Auto(f.source.clone())
    } else {
        SourceInput::Git {
            url: f.source.clone(),
            branch: opt(&f.branch),
        }
    };
    let input = ProjectInput {
        name: f.name.clone(),
        slug: opt(&f.slug),
        source,
        compose_path: opt(&f.compose_path),
        compose_content: opt(&f.compose_content),
        env_file: opt(&f.env_file),
        auto_start: Some(f.auto_start.is_some()),
        manual_deploy_enabled: None,
        github_webhook_enabled: None,
        deploy_key_id: opt(&f.deploy_key_id),
        generate_deploy_key: f.generate_deploy_key.is_some(),
        registry_credential_id: opt(&f.registry_credential_id),
    };
    match s.services.projects.create(actor, &input).await {
        Ok(out) => Ok(render(&CreatedPage {
            ctx: PageCtx::new(&session, "Project created", "/projects"),
            project_id: out.project.id,
            name: out.project.name,
            token: out.deploy_token.expose().to_owned(),
            fingerprint: out
                .deploy_key
                .as_ref()
                .map(|k| k.fingerprint.clone())
                .unwrap_or_default(),
            public_key: out.deploy_key.map(|k| k.public_key),
            base_url: s.config.server.public_url.trim_end_matches('/').to_owned(),
        })),
        Err(e) if matches!(e.kind, ErrorKind::Validation(_) | ErrorKind::Conflict) => {
            let (keys, creds) = choices(&s, actor, &f.deploy_key_id, &f.registry_credential_id)
                .await
                .map_err(|e| err(e, &headers))?;
            Ok(render_status(
                status_for(&e.kind),
                &NewPage {
                    ctx: PageCtx::new(&session, "New project", "/projects"),
                    error: Some(public_message(&e)),
                    form: f,
                    keys,
                    creds,
                },
            ))
        }
        Err(e) => Err(err(e, &headers)),
    }
}

struct DeploymentRowView {
    id: String,
    status: String,
    trigger: String,
    revision: String,
    queued: String,
    rollback_target: bool,
}

#[derive(Template)]
#[template(path = "projects/detail.html")]
struct DetailPage {
    ctx: PageCtx,
    p: ProjectDto,
    source: String,
    is_git: bool,
    can_deploy: bool,
    can_runtime: bool,
    can_config: bool,
    can_secrets: bool,
    can_delete: bool,
    can_rollback: bool,
    status: String,
    updated: String,
    deployments: Vec<DeploymentRowView>,
    compose: String,
    compose_source: &'static str,
    env_text: String,
    env_file: String,
    keys: Vec<Choice>,
    creds: Vec<Choice>,
    ports: Vec<prj::PortDto>,
    databases: Vec<DatabaseDto>,
    webhook_base: String,
}

async fn detail(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    let e = |c| err(c, &headers);
    let p = s.services.projects.get(actor, &id).await.map_err(e)?;
    let can = |op: &yanto_core::actor::OpSpec| actor.authorize(op, Some(&id)).is_ok();
    let (can_config, can_secrets) = (can(&prj::OP_PROJECT_UPDATE), can(&prj::OP_PROJECT_ENV_READ));
    let deps = s
        .services
        .deployments
        .list(
            actor,
            &dep::DeploymentQuery {
                project_id: Some(id.clone()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .map_err(e)?;
    let (compose, compose_source) = if can_config {
        match s.services.projects.compose_read(actor, &id).await {
            Ok(c) => (c.content.unwrap_or_default(), c.source),
            Err(_) => (String::new(), "none"),
        }
    } else {
        (String::new(), "none")
    };
    let env_text = if can_secrets {
        s.services
            .projects
            .env_content_read(actor, &id, false)
            .await
            .unwrap_or_default()
    } else {
        String::new()
    };
    let (keys, creds) = if can_config {
        choices(
            &s,
            actor,
            p.deploy_key_id.as_deref().unwrap_or(""),
            p.registry_credential_id.as_deref().unwrap_or(""),
        )
        .await
        .map_err(e)?
    } else {
        (Vec::new(), Vec::new())
    };
    let status = p
        .last_deployment
        .as_ref()
        .map_or_else(|| "never deployed".to_owned(), |d| d.status.clone());
    let databases = s
        .services
        .projects
        .databases_list(actor, &id)
        .await
        .map_err(e)?;
    Ok(render(&DetailPage {
        ctx: PageCtx::new(&session, p.name.clone(), "/projects"),
        source: source_label(&p),
        is_git: p.source_kind == "git",
        can_deploy: can(&dep::OP_PROJECT_DEPLOY),
        can_runtime: can(&prj::OP_PROJECT_STOP),
        can_config,
        can_secrets,
        can_delete: actor.authorize(&prj::OP_PROJECT_DELETE, None).is_ok(),
        can_rollback: can(&dep::OP_PROJECT_ROLLBACK),
        status,
        updated: fmt_ts(p.updated_at),
        deployments: deps
            .iter()
            .map(|d| DeploymentRowView {
                id: d.id.clone(),
                status: d.status.clone(),
                trigger: d.trigger.clone(),
                revision: d
                    .commit_sha
                    .as_deref()
                    .map(|c| c.chars().take(8).collect())
                    .or_else(|| d.image_digest.clone())
                    .unwrap_or_default(),
                queued: fmt_ts(d.queued_at),
                rollback_target: d.status == "success",
            })
            .collect(),
        compose,
        compose_source,
        env_file: p.env_file.clone(),
        env_text,
        keys,
        creds,
        ports: p.ports.clone(),
        databases,
        webhook_base: s.config.server.public_url.trim_end_matches('/').to_owned(),
        p,
    }))
}

#[derive(Deserialize, Default)]
struct SettingsForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    git_url: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    image_ref: String,
    #[serde(default)]
    compose_path: String,
    #[serde(default)]
    env_file: String,
    #[serde(default)]
    compose_content: String,
    #[serde(default)]
    deploy_key_id: String,
    #[serde(default)]
    registry_credential_id: String,
    auto_start: Option<String>,
    manual_deploy_enabled: Option<String>,
    github_webhook_enabled: Option<String>,
}

async fn settings(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SettingsForm>,
) -> Result<Response, DashError> {
    let patch = ProjectPatch {
        name: opt(&f.name),
        git_url: opt(&f.git_url),
        branch: opt(&f.branch),
        image_ref: opt(&f.image_ref),
        compose_path: opt(&f.compose_path),
        compose_content: Some(opt(&f.compose_content)),
        env_file: opt(&f.env_file),
        auto_start: Some(f.auto_start.is_some()),
        manual_deploy_enabled: Some(f.manual_deploy_enabled.is_some()),
        github_webhook_enabled: Some(f.github_webhook_enabled.is_some()),
        deploy_key_id: Some(opt(&f.deploy_key_id)),
        registry_credential_id: Some(opt(&f.registry_credential_id)),
    };
    s.services
        .projects
        .update(&session.0.actor, &id, &patch)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(redirect(&headers, &format!("/projects/{id}")))
}

#[derive(Deserialize, Default)]
struct DeleteQuery {
    #[serde(default)]
    stop_containers: Option<String>,
    #[serde(default)]
    delete_volumes: Option<String>,
    #[serde(default)]
    confirm_slug: String,
}

async fn delete_project(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Result<Response, DashError> {
    s.services
        .projects
        .delete(
            &session.0.actor,
            &id,
            &DeleteOptions {
                stop_containers: q.stop_containers.is_some(),
                delete_volumes: q.delete_volumes.is_some(),
                confirm_slug: opt(&q.confirm_slug),
            },
        )
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(redirect(&headers, "/projects"))
}

#[derive(Deserialize, Default)]
struct DeployForm {
    #[serde(default)]
    target_ref: String,
}

async fn deploy(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<DeployForm>,
) -> Result<Response, DashError> {
    let out = s
        .services
        .deployments
        .deploy(
            &session.0.actor,
            &id,
            &DeployRequest {
                target_ref: opt(&f.target_ref),
                env: None,
            },
        )
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(redirect(
        &headers,
        &format!("/deployments/{}", out.deployment.id),
    ))
}

async fn stack(
    out: Result<prj::StackOutput, CoreError>,
    headers: &HeaderMap,
    verb: &str,
) -> Result<Response, DashError> {
    let out = out.map_err(|e| err(e, headers))?;
    let msg = if out.ok {
        format!("{verb} done.")
    } else {
        format!("{verb} failed: {}", out.output)
    };
    Ok(toast(&msg, !out.ok).into_response())
}

async fn stop(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    stack(
        s.services.projects.stop(&session.0.actor, &id).await,
        &headers,
        "Stop",
    )
    .await
}

async fn restart(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    stack(
        s.services.projects.restart(&session.0.actor, &id).await,
        &headers,
        "Restart",
    )
    .await
}

#[derive(Deserialize, Default)]
struct RollbackForm {
    #[serde(default)]
    deployment_id: String,
    #[serde(default)]
    git_ref: String,
}

impl RollbackForm {
    fn request(&self) -> RollbackRequest {
        RollbackRequest {
            deployment_id: opt(&self.deployment_id),
            git_ref: opt(&self.git_ref),
        }
    }
}

#[derive(Template)]
#[template(path = "projects/_rollback_preview.html")]
struct PreviewFragment {
    ctx: PageCtx,
    project_id: String,
    pv: RollbackPreview,
    target_deployment: String,
    git_ref: String,
    diff_lines: Vec<(String, String)>,
}

async fn rollback_preview(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<RollbackForm>,
) -> Result<Response, DashError> {
    let pv = s
        .services
        .deployments
        .rollback_preview(&session.0.actor, &id, &f.request())
        .await
        .map_err(|e| err(e, &headers))?;
    let diff_lines = pv
        .compose_diff
        .iter()
        .map(|l| {
            let class = match l.kind {
                '+' => "add",
                '-' => "del",
                _ => "ctx",
            };
            (class.to_owned(), format!("{} {}", l.kind, l.text))
        })
        .collect();
    let html = PreviewFragment {
        ctx: PageCtx::new(&session, "", "/projects"),
        project_id: id,
        target_deployment: pv.target.deployment_id.clone().unwrap_or_default(),
        git_ref: f.git_ref.clone(),
        diff_lines,
        pv,
    }
    .render()
    .map_err(|e| err(CoreError::internal("template render failed", e), &headers))?;
    Ok(Html(html).into_response())
}

async fn rollback(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<RollbackForm>,
) -> Result<Response, DashError> {
    let out = s
        .services
        .deployments
        .rollback(&session.0.actor, &id, &f.request())
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(redirect(
        &headers,
        &format!("/deployments/{}", out.deployment.id),
    ))
}

async fn compose(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let c = s
        .services
        .projects
        .compose_read(&session.0.actor, &id)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(Html(format!(
        "<p class=\"muted\">{} ({})</p><pre>{}</pre>",
        escape(&c.path),
        c.source,
        escape(c.content.as_deref().unwrap_or(""))
    ))
    .into_response())
}

#[derive(Deserialize, Default)]
struct ContentForm {
    #[serde(default)]
    content: String,
}

async fn env_get(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let text = s
        .services
        .projects
        .env_content_read(&session.0.actor, &id, false)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(Html(format!("<pre>{}</pre>", escape(&text))).into_response())
}

async fn env_put(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ContentForm>,
) -> Result<Response, DashError> {
    let v = s
        .services
        .projects
        .env_write(&session.0.actor, &id, &f.content)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(toast(
        &format!(
            "Saved {} variable(s) to {}. Values apply on the next deployment.",
            v.variables.len(),
            v.env_file
        ),
        false,
    )
    .into_response())
}

#[derive(Deserialize, Default)]
struct VarForm {
    key: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    remove: Option<String>,
}

async fn env_patch(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<VarForm>,
) -> Result<Response, DashError> {
    let change = VarChange {
        key: f.key.trim().to_owned(),
        value: if f.remove.is_some() {
            None
        } else {
            Some(f.value)
        },
    };
    s.services
        .projects
        .env_variables_write(&session.0.actor, &id, &[change])
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(toast("Variable saved.", false).into_response())
}

async fn env_preview(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ContentForm>,
) -> Result<Response, DashError> {
    let v = s
        .services
        .projects
        .env_preview(&session.0.actor, Some(&id), &f.content)
        .await
        .map_err(|e| err(e, &headers))?;
    let mut html = String::from("<ul>");
    for var in &v.variables {
        html.push_str(&format!(
            "<li><code>{}</code> = {}</li>",
            escape(&var.key),
            escape(&var.value)
        ));
    }
    html.push_str("</ul>");
    if !v.invalid_lines.is_empty() {
        html.push_str(&format!(
            "<p class=\"error\">Not KEY=VALUE on line(s) {}.</p>",
            v.invalid_lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(Html(html).into_response())
}

fn token_fragment(token: &Secret, note: &str) -> Response {
    // Rendered into `#token-box`; `render()`-style no-store applies via the headers below.
    let html = format!(
        "<div class=\"secret\"><p>{}</p><code class=\"link\">{}</code></div>",
        escape(note),
        escape(token.expose())
    );
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Html(html),
    )
        .into_response()
}

async fn token_reveal(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let t = s
        .services
        .projects
        .deploy_token_reveal(&session.0.actor, &id)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(token_fragment(
        &t,
        "Deploy Token (also the GitHub webhook secret). This reveal is audited.",
    ))
}

async fn token_rotate(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let t = s
        .services
        .projects
        .deploy_token_rotate(&session.0.actor, &id)
        .await
        .map_err(|e| err(e, &headers))?;
    Ok(token_fragment(
        &t,
        "New Deploy Token. The old token and the GitHub webhook secret stopped working; update both.",
    ))
}

async fn ports_get(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let ports = s
        .services
        .projects
        .ports_list(&session.0.actor, &id)
        .await
        .map_err(|e| err(e, &headers))?;
    let mut html =
        String::from("<table><tr><th>Label</th><th>Bind</th><th>Host</th><th>Container</th></tr>");
    for p in ports {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}/{}</td></tr>",
            escape(&p.label),
            escape(&p.bind_address),
            p.host_port,
            p.container_port,
            escape(&p.protocol)
        ));
    }
    html.push_str("</table>");
    Ok(Html(html).into_response())
}

/// Keep the checked rows (`keep=<port id>`) and add one optional new row.
async fn ports_put(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, DashError> {
    let e = |c| err(c, &headers);
    let actor = &session.0.actor;
    let current = s
        .services
        .projects
        .ports_list(actor, &id)
        .await
        .map_err(e)?;
    let mut keep: Vec<String> = Vec::new();
    let (mut label, mut host, mut container, mut proto) = (
        String::new(),
        String::new(),
        String::new(),
        "tcp".to_owned(),
    );
    for (k, v) in form_urlencoded::parse(&body) {
        match k.as_ref() {
            "keep" => keep.push(v.into_owned()),
            "new_label" => label = v.into_owned(),
            "new_host" => host = v.into_owned(),
            "new_container" => container = v.into_owned(),
            "new_protocol" => proto = v.into_owned(),
            _ => {}
        }
    }
    let mut set: Vec<PortInput> = current
        .iter()
        .filter(|p| keep.contains(&p.id))
        .map(|p| PortInput {
            label: p.label.clone(),
            compose_service: p.compose_service.clone(),
            host_port: u16::try_from(p.host_port).unwrap_or(0),
            container_port: u16::try_from(p.container_port).unwrap_or(0),
            bind_address: p.bind_address.clone(),
            protocol: p.protocol.clone(),
        })
        .collect();
    if !host.trim().is_empty() {
        let parse = |field: &str, v: &str| {
            v.trim()
                .parse::<u16>()
                .map_err(|_| CoreError::validation(field, "must be a port number 1-65535"))
        };
        set.push(PortInput {
            label: label.trim().to_owned(),
            compose_service: None,
            host_port: parse("new_host", &host).map_err(e)?,
            container_port: parse("new_container", &container).map_err(e)?,
            bind_address: "127.0.0.1".into(),
            protocol: proto,
        });
    }
    s.services
        .projects
        .ports_set(actor, &id, &set)
        .await
        .map_err(e)?;
    Ok(toast("Ports saved.", false).into_response())
}

/// Keep the checked rows (`keep=<service>`) and add one optional new declaration.
async fn databases_put(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, DashError> {
    let e = |c| err(c, &headers);
    let actor = &session.0.actor;
    let current = s
        .services
        .projects
        .databases_list(actor, &id)
        .await
        .map_err(e)?;
    let mut keep: Vec<String> = Vec::new();
    let (mut engine, mut service, mut database, mut user_env, mut password_env) = (
        String::from("postgres"),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    for (k, v) in form_urlencoded::parse(&body) {
        match k.as_ref() {
            "keep" => keep.push(v.into_owned()),
            "new_engine" => engine = v.into_owned(),
            "new_service" => service = v.into_owned(),
            "new_database" => database = v.into_owned(),
            "new_user_env" => user_env = v.into_owned(),
            "new_password_env" => password_env = v.into_owned(),
            _ => {}
        }
    }
    let mut set: Vec<DatabaseInput> = current
        .into_iter()
        .filter(|d| keep.contains(&d.compose_service))
        .map(|d| DatabaseInput {
            engine: d.engine,
            compose_service: d.compose_service,
            database: d.database,
            user_env: d.user_env,
            password_env: d.password_env,
        })
        .collect();
    if !service.trim().is_empty() {
        set.retain(|d| d.compose_service != service.trim());
        set.push(DatabaseInput {
            engine,
            compose_service: service,
            database: Some(database),
            user_env: Some(user_env),
            password_env: Some(password_env),
        });
    }
    s.services
        .projects
        .databases_set(actor, &id, &set)
        .await
        .map_err(e)?;
    Ok(toast("Databases saved.", false).into_response())
}
