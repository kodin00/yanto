//! `GET /audit` (`audit_list`).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::service::audit::{AuditPage, AuditQuery};

use super::ApiActor;
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new().route("/audit", get(list))
}

#[derive(Debug, Deserialize)]
struct Params {
    project_id: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list(
    State(s): State<AppState>,
    ApiActor(actor): ApiActor,
    params: Result<Query<Params>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AuditPage>, ApiError> {
    let Query(p) = params.map_err(|_| ApiError::bad_request("invalid query string"))?;
    let q = AuditQuery {
        project_id: p.project_id,
        limit: p.limit,
        cursor: p.cursor,
    };
    Ok(Json(s.services.audit.list(&actor, &q).await?))
}
