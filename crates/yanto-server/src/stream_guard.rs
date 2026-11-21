//! Long-lived SSE streams re-check their caller (docs/rewrite/03 a.3): every 15 s the session
//! cookie or API Token is authenticated again and the stream-specific permission re-evaluated.
//! On loss the stream sends a terminal `authorization_revoked` event and ends, so logout, a
//! revoked token, a disabled account or a removed grant also cut off open log streams.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, header};
use axum::response::sse::Event;
use futures_util::future::BoxFuture;
use futures_util::{Stream, StreamExt};
use yanto_core::actor::{Actor, Channel, Origin};
use yanto_core::service::Services;
use yanto_core::{CoreError, ErrorKind};

/// 03 a.3: streams re-check every 15 s.
pub const RECHECK: Duration = Duration::from_secs(15);

/// `true` while the caller may keep the stream.
pub type Check = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

/// Whether an error from a stream-specific re-check means access is gone (as opposed to, say,
/// Docker being briefly unreachable, which must not end the stream).
pub fn lost_access(e: &CoreError) -> bool {
    matches!(
        e.kind,
        ErrorKind::Unauthenticated | ErrorKind::Forbidden | ErrorKind::NotFound
    )
}

fn check_with<A, F, Fut>(services: Arc<Services>, auth: A, still: F) -> Check
where
    A: Fn(Arc<Services>) -> BoxFuture<'static, Option<Actor>> + Send + Sync + 'static,
    F: Fn(Arc<Services>, Actor) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let auth = Arc::new(auth);
    let still = Arc::new(still);
    Arc::new(move || {
        let (services, auth, still) = (services.clone(), auth.clone(), still.clone());
        Box::pin(async move {
            match auth(services.clone()).await {
                Some(actor) => still(services, actor).await,
                None => false,
            }
        })
    })
}

/// Re-authenticate the dashboard session cookie of this request, then run `still`.
pub fn session_check<F, Fut>(
    services: Arc<Services>,
    cookie: String,
    ip: Option<String>,
    still: F,
) -> Check
where
    F: Fn(Arc<Services>, Actor) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let origin = Origin::new(Channel::Dashboard, ip);
    check_with(
        services,
        move |s| {
            let (cookie, origin) = (cookie.clone(), origin.clone());
            Box::pin(async move {
                s.auth
                    .authenticate_session(&origin, &cookie)
                    .await
                    .ok()
                    .map(|a| a.actor)
            })
        },
        still,
    )
}

/// Re-authenticate the request's `Authorization: Bearer` API Token, then run `still`.
pub fn token_check<F, Fut>(
    services: Arc<Services>,
    headers: &HeaderMap,
    ip: Option<String>,
    still: F,
) -> Check
where
    F: Fn(Arc<Services>, Actor) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then(|| rest.trim().to_owned())
        })
        .unwrap_or_default();
    let origin = Origin::new(Channel::Rest, ip);
    check_with(
        services,
        move |s| {
            let (token, origin) = (token.clone(), origin.clone());
            Box::pin(async move { s.tokens.authenticate(&origin, &token).await.ok() })
        },
        still,
    )
}

/// Pass `inner` through, running `check` every `every`; when it fails, emit
/// `event: authorization_revoked` and end.
pub fn revocable<S>(
    inner: S,
    check: Check,
    every: Duration,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let start = tokio::time::Instant::now() + every;
    let state = (
        Box::pin(inner),
        tokio::time::interval_at(start, every),
        check,
        false,
    );
    futures_util::stream::unfold(state, |(mut inner, mut tick, check, done)| async move {
        if done {
            return None;
        }
        loop {
            tokio::select! {
                item = inner.next() => {
                    return item.map(|i| (i, (inner, tick, check, false)));
                }
                _ = tick.tick() => {
                    if !check().await {
                        let ev = Event::default().event("authorization_revoked").data("revoked");
                        return Some((Ok(ev), (inner, tick, check, true)));
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn the_stream_ends_with_a_revoked_event_once_the_check_fails() {
        let allowed = Arc::new(AtomicBool::new(true));
        let flag = allowed.clone();
        let check: Check = Arc::new(move || {
            let ok = flag.load(Ordering::SeqCst);
            Box::pin(async move { ok })
        });
        let inner = futures_util::stream::pending::<Result<Event, Infallible>>();
        let mut s = Box::pin(revocable(inner, check, Duration::from_millis(20)));
        // Passing checks keep an idle stream open.
        let idle = tokio::time::timeout(Duration::from_millis(100), s.next()).await;
        assert!(idle.is_err());
        allowed.store(false, Ordering::SeqCst);
        let last = tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("revoked event")
            .expect("an event");
        assert!(last.is_ok());
        assert!(s.next().await.is_none(), "the stream ends after the event");
    }

    #[tokio::test]
    async fn items_flow_through_unchanged() {
        let check: Check = Arc::new(|| Box::pin(async { true }));
        let inner = futures_util::stream::iter([Ok(Event::default().data("a"))]);
        let out: Vec<_> = revocable(inner, check, RECHECK).collect().await;
        assert_eq!(out.len(), 1);
    }
}
