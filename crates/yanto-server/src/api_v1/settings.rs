//! `/settings`: runtime settings (`settings_read`, `settings_update`). Owner only.
//! Host paths come from `config.toml` and are read-only. Changes apply after a restart.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use yanto_core::service::settings::{SettingsPatch, SettingsView};

use super::{ApiActor, ApiJson};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(read).patch(update))
}

async fn read(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<SettingsView>, ApiError> {
    Ok(Json(s.services.settings.read(&a).await?))
}

/// Body: `{ "<key>": <value>, ... }`; `null` removes the stored value.
async fn update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(patch): ApiJson<SettingsPatch>,
) -> Result<Json<SettingsView>, ApiError> {
    Ok(Json(s.services.settings.update(&a, &patch).await?))
}
