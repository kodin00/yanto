//! Setup, login, logout and invite/reset acceptance.

use askama::Template;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::service::auth::LoginOutcome;

use super::session::{Client, Session, clear_cookie, set_cookie, user_agent};
use super::view::{DashError, PageCtx, escape, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

/// Routes reachable without a session. Adding a route here makes it public: keep this list short.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/setup", get(setup_page).post(setup_submit))
        .route("/login", get(login_page).post(login_submit))
        .route("/account/setup", get(account_page).post(account_submit))
        .route("/account/setup/preview", post(account_preview))
}

pub fn session_routes() -> Router<AppState> {
    Router::new().route("/logout", post(logout))
}

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage {
    ctx: PageCtx,
    error: Option<String>,
    username: String,
}

#[derive(Template)]
#[template(path = "auth/setup.html")]
struct SetupPage {
    ctx: PageCtx,
    error: Option<String>,
    username: String,
}

#[derive(Template)]
#[template(path = "auth/account_setup.html")]
struct AccountPage {
    ctx: PageCtx,
    error: Option<String>,
    token: String,
}

fn signed_in(state: &AppState, out: &LoginOutcome) -> Response {
    (
        AppendHeaders([(SET_COOKIE, set_cookie(state, &out.session))]),
        Redirect::to("/"),
    )
        .into_response()
}

fn form_status(err: &CoreError) -> StatusCode {
    status_for(&err.kind)
}

// ---- login ---------------------------------------------------------------

async fn login_page(State(s): State<AppState>) -> Response {
    if matches!(s.services.auth.setup_pending().await, Ok(true)) {
        return Redirect::to("/setup").into_response();
    }
    render(&LoginPage {
        ctx: PageCtx::public("Sign in"),
        error: None,
        username: String::new(),
    })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_submit(
    State(s): State<AppState>,
    Client(origin): Client,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    match s
        .services
        .auth
        .login(
            &origin,
            &f.username,
            &f.password,
            user_agent(&headers).as_deref(),
        )
        .await
    {
        Ok(out) => signed_in(&s, &out),
        Err(e) => render_status(
            form_status(&e),
            &LoginPage {
                ctx: PageCtx::public("Sign in"),
                error: Some(public_message(&e)),
                username: f.username,
            },
        ),
    }
}

async fn logout(State(s): State<AppState>, session: Session) -> Response {
    let res = s
        .services
        .auth
        .logout(&session.0.actor, &session.0.session_id)
        .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "logout failed");
    }
    (
        AppendHeaders([(SET_COOKIE, clear_cookie(&s))]),
        Redirect::to("/login"),
    )
        .into_response()
}

// ---- first Owner -----------------------------------------------------------

async fn setup_page(State(s): State<AppState>) -> Response {
    if !matches!(s.services.auth.setup_pending().await, Ok(true)) {
        return Redirect::to("/login").into_response();
    }
    render(&SetupPage {
        ctx: PageCtx::public("Set up Yanto"),
        error: None,
        username: String::new(),
    })
}

#[derive(Deserialize)]
struct SetupForm {
    code: String,
    username: String,
    password: String,
    confirm: String,
}

async fn setup_submit(
    State(s): State<AppState>,
    Client(origin): Client,
    headers: HeaderMap,
    Form(f): Form<SetupForm>,
) -> Response {
    if !matches!(s.services.auth.setup_pending().await, Ok(true)) {
        return Redirect::to("/login").into_response();
    }
    let page = |status: StatusCode, error: String, username: String| {
        render_status(
            status,
            &SetupPage {
                ctx: PageCtx::public("Set up Yanto"),
                error: Some(error),
                username,
            },
        )
    };
    if f.password != f.confirm {
        return page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Passwords do not match".into(),
            f.username,
        );
    }
    match s
        .services
        .auth
        .owner_setup(
            &origin,
            &f.code,
            &f.username,
            &f.password,
            user_agent(&headers).as_deref(),
        )
        .await
    {
        Ok(out) => signed_in(&s, &out),
        Err(e) => page(form_status(&e), public_message(&e), f.username),
    }
}

// ---- invite / reset acceptance ------------------------------------------------

async fn account_page() -> Response {
    render(&AccountPage {
        ctx: PageCtx::public("Set your password"),
        error: None,
        token: String::new(),
    })
}

#[derive(Deserialize)]
struct PreviewForm {
    token: String,
}

/// htmx fragment: who the link is for. POST so the token stays out of URLs and logs.
async fn account_preview(
    State(s): State<AppState>,
    Client(origin): Client,
    headers: HeaderMap,
    Form(f): Form<PreviewForm>,
) -> Response {
    match s.services.auth.account_preview(&origin, &f.token).await {
        Ok(p) => Html(format!(
            "<p class=\"muted\">{} for <strong>{}</strong></p>",
            if p.purpose == "reset" {
                "Password reset"
            } else {
                "Invitation"
            },
            escape(&p.username)
        ))
        .into_response(),
        Err(e) => DashError::new(e, &headers).into_response(),
    }
}

#[derive(Deserialize)]
struct AccountForm {
    token: String,
    password: String,
    confirm: String,
}

async fn account_submit(
    State(s): State<AppState>,
    Client(origin): Client,
    headers: HeaderMap,
    Form(f): Form<AccountForm>,
) -> Response {
    let page = |status: StatusCode, error: String, token: String| {
        render_status(
            status,
            &AccountPage {
                ctx: PageCtx::public("Set your password"),
                error: Some(error),
                token,
            },
        )
    };
    if f.password != f.confirm {
        return page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Passwords do not match".into(),
            f.token,
        );
    }
    match s
        .services
        .auth
        .account_setup(
            &origin,
            &f.token,
            &f.password,
            user_agent(&headers).as_deref(),
        )
        .await
    {
        Ok(out) => signed_in(&s, &out),
        Err(e) => page(form_status(&e), public_message(&e), f.token),
    }
}
