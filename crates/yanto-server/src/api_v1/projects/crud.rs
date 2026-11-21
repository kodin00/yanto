//! `/projects`: CRUD, compose, env, ports, deploy token, stop/restart.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yanto_core::CoreError;
use yanto_core::deploy::env::VarChange;
use yanto_core::secrets::Secret;
use yanto_core::service::projects::{
    ComposeView, DeleteOptions, EnvView, PortDto, PortInput, ProjectDto, ProjectInput,
    ProjectPatch, SourceInput, StackOutput,
};

use super::double_option;
use crate::api_v1::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/{id}", get(get_one).patch(update).delete(delete))
        .route("/projects/{id}/compose", get(compose))
        .route(
            "/projects/{id}/env",
            get(env_read).put(env_put).patch(env_patch),
        )
        .route("/projects/{id}/env/content", get(env_content))
        .route("/env/preview", post(env_preview))
        .route("/projects/{id}/ports", get(ports_list).put(ports_set))
        .route("/projects/{id}/deploy-token/reveal", post(token_reveal))
        .route("/projects/{id}/deploy-token/rotate", post(token_rotate))
        .route("/projects/{id}/stop", post(stop))
        .route("/projects/{id}/restart", post(restart))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<ProjectDto>>, ApiError> {
    Ok(Json(s.services.projects.list(&a).await?))
}

async fn get_one(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<ProjectDto>, ApiError> {
    Ok(Json(s.services.projects.get(&a, &id).await?))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    slug: Option<String>,
    git_url: Option<String>,
    branch: Option<String>,
    image_ref: Option<String>,
    /// One free-form box instead of `git_url` / `image_ref`.
    source: Option<String>,
    compose_path: Option<String>,
    compose_content: Option<String>,
    env_file: Option<String>,
    auto_start: Option<bool>,
    manual_deploy_enabled: Option<bool>,
    github_webhook_enabled: Option<bool>,
    deploy_key_id: Option<String>,
    #[serde(default)]
    generate_deploy_key: bool,
    registry_credential_id: Option<String>,
}

/// Reveals the Deploy Token once.
async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let source = match (b.git_url, b.image_ref, b.source) {
        (Some(url), None, None) => SourceInput::Git {
            url,
            branch: b.branch,
        },
        (None, Some(image_ref), None) => SourceInput::Image { image_ref },
        (None, None, Some(text)) => SourceInput::Auto(text),
        _ => {
            return Err(CoreError::validation(
                "source",
                "give exactly one of git_url, image_ref or source",
            )
            .into());
        }
    };
    let out = s
        .services
        .projects
        .create(
            &a,
            &ProjectInput {
                name: b.name,
                slug: b.slug,
                source,
                compose_path: b.compose_path,
                compose_content: b.compose_content,
                env_file: b.env_file,
                auto_start: b.auto_start,
                manual_deploy_enabled: b.manual_deploy_enabled,
                github_webhook_enabled: b.github_webhook_enabled,
                deploy_key_id: b.deploy_key_id,
                generate_deploy_key: b.generate_deploy_key,
                registry_credential_id: b.registry_credential_id,
            },
        )
        .await?;
    Ok(created(json!({
        "project": out.project,
        "deploy_token": out.deploy_token.expose(),
        "deploy_key": out.deploy_key,
    })))
}

#[derive(Debug, Deserialize, Default)]
struct PatchBody {
    name: Option<String>,
    git_url: Option<String>,
    branch: Option<String>,
    image_ref: Option<String>,
    compose_path: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    compose_content: Option<Option<String>>,
    env_file: Option<String>,
    auto_start: Option<bool>,
    manual_deploy_enabled: Option<bool>,
    github_webhook_enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    deploy_key_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    registry_credential_id: Option<Option<String>>,
}

async fn update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<PatchBody>,
) -> Result<Json<ProjectDto>, ApiError> {
    let patch = ProjectPatch {
        name: b.name,
        git_url: b.git_url,
        branch: b.branch,
        image_ref: b.image_ref,
        compose_path: b.compose_path,
        compose_content: b.compose_content,
        env_file: b.env_file,
        auto_start: b.auto_start,
        manual_deploy_enabled: b.manual_deploy_enabled,
        github_webhook_enabled: b.github_webhook_enabled,
        deploy_key_id: b.deploy_key_id,
        registry_credential_id: b.registry_credential_id,
    };
    Ok(Json(s.services.projects.update(&a, &id, &patch).await?))
}

#[derive(Debug, Deserialize, Default)]
struct DeleteQuery {
    #[serde(default)]
    stop_containers: bool,
    #[serde(default)]
    delete_volumes: bool,
    confirm_slug: Option<String>,
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    s.services
        .projects
        .delete(
            &a,
            &id,
            &DeleteOptions {
                stop_containers: q.stop_containers,
                delete_volumes: q.delete_volumes,
                confirm_slug: q.confirm_slug,
            },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compose(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<ComposeView>, ApiError> {
    Ok(Json(s.services.projects.compose_read(&a, &id).await?))
}

async fn env_read(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<EnvView>, ApiError> {
    Ok(Json(s.services.projects.env_read(&a, &id).await?))
}

#[derive(Debug, Deserialize, Default)]
struct RevealQuery {
    #[serde(default)]
    reveal: bool,
}

async fn env_content(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<RevealQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let text = s
        .services
        .projects
        .env_content_read(&a, &id, q.reveal)
        .await?;
    Ok(Json(json!({ "content": text, "masked": !q.reveal })))
}

#[derive(Debug, Deserialize)]
struct ContentBody {
    content: String,
}

async fn env_preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<ContentBody>,
) -> Result<Json<EnvView>, ApiError> {
    Ok(Json(
        s.services
            .projects
            .env_preview(&a, None, &b.content)
            .await?,
    ))
}

async fn env_put(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<ContentBody>,
) -> Result<Json<EnvView>, ApiError> {
    Ok(Json(
        s.services.projects.env_write(&a, &id, &b.content).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct VarsBody {
    variables: Vec<VarBody>,
}

#[derive(Debug, Deserialize)]
struct VarBody {
    key: String,
    /// `null` deletes the variable; `"********"` keeps the stored value.
    value: Option<String>,
}

async fn env_patch(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<VarsBody>,
) -> Result<Json<EnvView>, ApiError> {
    let changes: Vec<VarChange> = b
        .variables
        .into_iter()
        .map(|v| VarChange {
            key: v.key,
            value: v.value,
        })
        .collect();
    Ok(Json(
        s.services
            .projects
            .env_variables_write(&a, &id, &changes)
            .await?,
    ))
}

async fn ports_list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<Vec<PortDto>>, ApiError> {
    Ok(Json(s.services.projects.ports_list(&a, &id).await?))
}

#[derive(Debug, Deserialize)]
struct PortBody {
    label: String,
    compose_service: Option<String>,
    host_port: u16,
    container_port: u16,
    #[serde(default = "loopback")]
    bind_address: String,
    #[serde(default = "tcp")]
    protocol: String,
}

fn loopback() -> String {
    "127.0.0.1".into()
}

fn tcp() -> String {
    "tcp".into()
}

#[derive(Debug, Deserialize)]
struct PortsBody {
    ports: Vec<PortBody>,
}

async fn ports_set(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<PortsBody>,
) -> Result<Json<Vec<PortDto>>, ApiError> {
    let ports: Vec<PortInput> = b
        .ports
        .into_iter()
        .map(|p| PortInput {
            label: p.label,
            compose_service: p.compose_service,
            host_port: p.host_port,
            container_port: p.container_port,
            bind_address: p.bind_address,
            protocol: p.protocol,
        })
        .collect();
    Ok(Json(s.services.projects.ports_set(&a, &id, &ports).await?))
}

fn token_body(t: &Secret) -> Json<serde_json::Value> {
    Json(json!({ "deploy_token": t.expose() }))
}

async fn token_reveal(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(token_body(
        &s.services.projects.deploy_token_reveal(&a, &id).await?,
    ))
}

async fn token_rotate(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(token_body(
        &s.services.projects.deploy_token_rotate(&a, &id).await?,
    ))
}

async fn stop(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<StackOutput>, ApiError> {
    Ok(Json(s.services.projects.stop(&a, &id).await?))
}

async fn restart(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<StackOutput>, ApiError> {
    Ok(Json(s.services.projects.restart(&a, &id).await?))
}
