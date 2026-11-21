//! `/settings`: runtime settings, read-only host configuration and links to the other settings
//! areas. Owner only; changes apply after a restart.

use askama::Template;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::{Form, Router};
use serde_json::{Value, json};
use yanto_core::CoreError;
use yanto_core::service::settings::{SettingEntry, SettingsPatch, SettingsView};

use super::session::Session;
use super::view::{DashError, PageCtx, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(index).post(save))
}

struct Field {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    unit: &'static str,
    list: bool,
    /// Current text shown in the field (empty when the default applies).
    text: String,
    default: String,
    min: i64,
    max: i64,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl From<&SettingEntry> for Field {
    fn from(e: &SettingEntry) -> Self {
        Self {
            key: e.key,
            label: e.label,
            help: e.help,
            unit: e.unit,
            list: e.kind == "cidr_list",
            text: if e.overridden {
                text_of(&e.value)
            } else {
                String::new()
            },
            default: text_of(&e.default).replace('\n', ", "),
            min: e.min.unwrap_or(0),
            max: e.max.unwrap_or(0),
        }
    }
}

struct HostRow {
    label: &'static str,
    value: String,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    ctx: PageCtx,
    fields: Vec<Field>,
    host: Vec<HostRow>,
    applies: &'static str,
    unix_user: String,
    saved: bool,
    error: Option<String>,
}

fn page(session: &Session, v: SettingsView, saved: bool, error: Option<String>) -> SettingsPage {
    let unix_user = v
        .host
        .iter()
        .find(|h| h.label == "Authorized Keys user")
        .map(|h| h.value.clone())
        .unwrap_or_default();
    SettingsPage {
        ctx: PageCtx::new(session, "Settings", "/settings"),
        fields: v.runtime.iter().map(Field::from).collect(),
        host: v
            .host
            .into_iter()
            .map(|h| HostRow {
                label: h.label,
                value: h.value,
            })
            .collect(),
        applies: v.applies,
        unix_user,
        saved,
        error,
    }
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let v = s
        .services
        .settings
        .read(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&page(&session, v, false, None)))
}

/// An empty field removes the stored value (back to the default).
fn patch_from_form(form: &[(String, String)]) -> Result<SettingsPatch, CoreError> {
    let mut patch = SettingsPatch::new();
    for (key, raw) in form {
        if key == "_csrf" {
            continue;
        }
        let raw = raw.trim();
        let value = if raw.is_empty() {
            Value::Null
        } else if key == "runtime.trusted_proxies" {
            json!(
                raw.split(|c: char| c == '\n' || c == ',' || c.is_whitespace())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            )
        } else {
            json!(raw.parse::<i64>().map_err(|_| {
                CoreError::validation(key.as_str(), format!("{key} must be a whole number"))
            })?)
        };
        patch.insert(key.clone(), value);
    }
    Ok(patch)
}

async fn save(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> Result<Response, DashError> {
    let svc = &s.services.settings;
    let result = match patch_from_form(&form) {
        Ok(p) => svc.update(&session.0.actor, &p).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(v) => Ok(render(&page(&session, v, true, None))),
        Err(e) if matches!(e.kind, yanto_core::ErrorKind::Validation(_)) => {
            let v = svc
                .read(&session.0.actor)
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(
                status_for(&e.kind),
                &page(&session, v, false, Some(public_message(&e))),
            ))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}
