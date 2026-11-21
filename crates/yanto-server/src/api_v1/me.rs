//! `GET /me` (`whoami`).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use yanto_core::service::tokens::WhoAmI;

use super::ApiActor;
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new().route("/me", get(whoami))
}

async fn whoami(
    State(s): State<AppState>,
    ApiActor(actor): ApiActor,
) -> Result<Json<WhoAmI>, ApiError> {
    Ok(Json(s.services.tokens.whoami(&actor).await?))
}
