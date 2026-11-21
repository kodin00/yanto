//! REST for Projects, Deployments, Deploy Keys and Registry Credentials (03 c).
//! One `routes()` for the central registration line.

mod credentials;
mod crud;
mod deployments;
mod keys;

use axum::Router;
use serde::de::{Deserialize, DeserializeOwned};

use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(crud::routes())
        .merge(deployments::routes())
        .merge(keys::routes())
        .merge(credentials::routes())
}

/// `Some(None)` for an explicit JSON `null` (clear), `Some(Some(v))` for a value, `None` when absent.
pub(super) fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Optional JSON body: an empty body means "all defaults".
pub(super) fn optional_body<T: DeserializeOwned + Default>(
    bytes: &[u8],
) -> Result<T, crate::error::ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes)
        .map_err(|_| crate::error::ApiError::bad_request("invalid JSON request body"))
}
