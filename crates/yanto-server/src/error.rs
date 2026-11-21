//! Error to HTTP mapping (docs/rewrite/08-conventions.md "Error mapping").
//!
//! | `ErrorKind`       | HTTP | REST `error.code`   |
//! |-------------------|------|---------------------|
//! | `Validation`      | 422  | `validation_failed` |
//! | `Unauthenticated` | 401  | `unauthenticated`   |
//! | `Forbidden`       | 403  | `forbidden`         |
//! | `NotFound`        | 404  | `not_found`         |
//! | `Conflict`        | 409  | `conflict`          |
//! | `RateLimited`     | 429  | `rate_limited`      |
//! | `Unavailable`     | 503  | `unavailable`       |
//! | `Internal`        | 500  | `internal`          |
//!
//! Malformed requests that never reach a service are 400 `bad_request`.
//! `Internal` details go to the log, never to the caller.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use yanto_core::{CoreError, ErrorKind};

pub fn status_for(kind: &ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub fn code_for(kind: &ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation(_) => "validation_failed",
        ErrorKind::Unauthenticated => "unauthenticated",
        ErrorKind::Forbidden => "forbidden",
        ErrorKind::NotFound => "not_found",
        ErrorKind::Conflict => "conflict",
        ErrorKind::RateLimited => "rate_limited",
        ErrorKind::Unavailable => "unavailable",
        ErrorKind::Internal => "internal",
    }
}

/// The message safe to show a caller. Internal errors are logged and replaced.
pub fn public_message(err: &CoreError) -> String {
    if matches!(err.kind, ErrorKind::Internal | ErrorKind::Unavailable) {
        tracing::error!(error = %err, source = ?err.source, "request failed");
        if matches!(err.kind, ErrorKind::Internal) {
            return "internal error".into();
        }
    }
    err.message.clone()
}

/// REST error: `{"error":{"code","message","fields"?}}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub fields: Vec<(String, String)>,
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
            fields: Vec::new(),
            retry_after: None,
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        let fields = match &err.kind {
            ErrorKind::Validation(f) => f
                .iter()
                .map(|f| (f.field.clone(), f.message.clone()))
                .collect(),
            _ => Vec::new(),
        };
        Self {
            status: status_for(&err.kind),
            code: code_for(&err.kind),
            message: public_message(&err),
            fields,
            retry_after: err.retry_after_secs,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut error = json!({ "code": self.code, "message": self.message });
        if !self.fields.is_empty() {
            error["fields"] = self
                .fields
                .iter()
                .map(|(f, m)| json!({ "field": f, "message": m }))
                .collect();
        }
        let mut res = (self.status, Json(json!({ "error": error }))).into_response();
        let h = res.headers_mut();
        if let Some(secs) = self.retry_after
            && let Ok(v) = HeaderValue::from_str(&secs.to_string())
        {
            h.insert(header::RETRY_AFTER, v);
        }
        if self.status == StatusCode::UNAUTHORIZED {
            h.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yanto_core::error::FieldError;

    #[test]
    fn every_kind_has_a_distinct_mapping() {
        let kinds = [
            ErrorKind::Validation(vec![FieldError {
                field: "f".into(),
                message: "m".into(),
            }]),
            ErrorKind::Unauthenticated,
            ErrorKind::Forbidden,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::RateLimited,
            ErrorKind::Unavailable,
            ErrorKind::Internal,
        ];
        let codes: Vec<u16> = kinds.iter().map(|k| status_for(k).as_u16()).collect();
        assert_eq!(codes, [422, 401, 403, 404, 409, 429, 503, 500]);
        let names: std::collections::BTreeSet<_> = kinds.iter().map(code_for).collect();
        assert_eq!(names.len(), kinds.len());
    }

    #[test]
    fn internal_details_are_hidden() {
        let e = CoreError::internal("db exploded: secret path", std::io::Error::other("boom"));
        assert_eq!(public_message(&e), "internal error");
        assert_eq!(ApiError::from(e).message, "internal error");
    }

    #[test]
    fn rate_limited_sets_retry_after() {
        let api = ApiError::from(CoreError::rate_limited(42));
        assert_eq!(api.retry_after, Some(42));
        assert_eq!(api.into_response().headers()[header::RETRY_AFTER], "42");
    }
}
