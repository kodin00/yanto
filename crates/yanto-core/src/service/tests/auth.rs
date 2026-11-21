use std::time::Duration;

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::Actor;
use crate::testing::{MEMBER_PASSWORD, OWNER_PASSWORD, TestCore, token_from_link};

const HOUR: Duration = Duration::from_secs(3600);

#[tokio::test]
async fn setup_creates_single_owner_and_consumes_code() {
    let core = TestCore::new().await;
    assert!(core.services.auth.setup_pending().await.unwrap());
    let code = core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap();
    // Lowercase and without dashes is accepted.
    let loose = code.code.expose().replace('-', "").to_lowercase();
    let out = core
        .services
        .auth
        .owner_setup(&core.origin(), &loose, "boss", OWNER_PASSWORD, Some("ua"))
        .await
        .unwrap();
    assert_eq!(out.username, "boss");
    assert!(!core.services.auth.setup_pending().await.unwrap());
    // Code is gone: reuse fails, and so does a second Owner.
    let again = core
        .services
        .auth
        .owner_setup(
            &core.origin(),
            code.code.expose(),
            "boss2",
            OWNER_PASSWORD,
            None,
        )
        .await;
    // The consumed code no longer validates, so a second Owner cannot be created.
    assert_eq!(kind(again), ErrorKind::Unauthenticated);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_settings WHERE key LIKE 'setup.%'")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(audit_count(&core, "auth.setup_owner", "ok").await, 1);
}

#[tokio::test]
async fn setup_code_is_stored_hashed_and_expires() {
    let core = TestCore::new().await;
    let code = core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'setup.code_hash'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(!stored.contains(&code.code.expose().replace('-', "")));
    core.clock.advance(25 * HOUR);
    let r = core
        .services
        .auth
        .owner_setup(
            &core.origin(),
            code.code.expose(),
            "boss",
            OWNER_PASSWORD,
            None,
        )
        .await;
    assert_eq!(kind(r), ErrorKind::Unauthenticated);
}

#[tokio::test]
async fn reissuing_a_code_revokes_the_old_one_and_owner_blocks_reissue() {
    let core = TestCore::new().await;
    let old = core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap();
    let new = core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap();
    let r = core
        .services
        .auth
        .owner_setup(
            &core.origin(),
            old.code.expose(),
            "boss",
            OWNER_PASSWORD,
            None,
        )
        .await;
    assert_eq!(kind(r), ErrorKind::Unauthenticated);
    core.services
        .auth
        .owner_setup(
            &core.origin(),
            new.code.expose(),
            "boss",
            OWNER_PASSWORD,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        kind(core.services.auth.issue_setup_code(&Actor::cli()).await),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn setup_validates_input_without_consuming_code_and_rate_limits_guesses() {
    let core = TestCore::new().await;
    let code = core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap();
    let weak = core
        .services
        .auth
        .owner_setup(&core.origin(), code.code.expose(), "boss", "short", None)
        .await;
    assert!(matches!(kind(weak), ErrorKind::Validation(_)));
    for _ in 0..10 {
        let _ = core
            .services
            .auth
            .owner_setup(
                &core.origin(),
                "AAAA-AAAA-AAAA-AAAA",
                "boss",
                OWNER_PASSWORD,
                None,
            )
            .await;
    }
    // The right code is now blocked for this IP.
    let r = core
        .services
        .auth
        .owner_setup(
            &core.origin(),
            code.code.expose(),
            "boss",
            OWNER_PASSWORD,
            None,
        )
        .await;
    assert_eq!(kind(r), ErrorKind::RateLimited);
    assert_eq!(audit_count(&core, "auth.rate_limited", "denied").await, 1);
}

#[tokio::test]
async fn login_succeeds_and_failures_are_indistinguishable() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let ok = core
        .services
        .auth
        .login(&core.origin(), "OWNER", &owner.password, Some("curl"))
        .await
        .unwrap();
    assert_eq!(ok.username, owner.username);

    let wrong = core
        .services
        .auth
        .login(&core.origin(), "owner", "wrong-password-1", None)
        .await;
    let unknown = core
        .services
        .auth
        .login(&core.origin(), "nobody", "wrong-password-1", None)
        .await;
    let (w, u) = (wrong.unwrap_err(), unknown.unwrap_err());
    assert_eq!(w.kind, ErrorKind::Unauthenticated);
    assert_eq!(w.message, u.message);
    assert_eq!(audit_count(&core, "auth.login", "ok").await, 1);
    assert_eq!(audit_count(&core, "auth.login_failed", "denied").await, 2);
    let label: String = sqlx::query_scalar(
        "SELECT actor_label FROM audit_logs WHERE action='auth.login_failed' AND actor_id IS NULL",
    )
    .fetch_one(core.db.reader())
    .await
    .unwrap();
    assert_eq!(label, "<unknown>");
}

#[tokio::test]
async fn login_is_rate_limited_per_username_and_recovers() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    for _ in 0..5 {
        let _ = core
            .services
            .auth
            .login(&core.origin(), "owner", "wrong-password-1", None)
            .await;
    }
    let blocked = core
        .services
        .auth
        .login(&core.origin(), "owner", &owner.password, None)
        .await
        .unwrap_err();
    assert_eq!(blocked.kind, ErrorKind::RateLimited);
    assert!(blocked.retry_after_secs.unwrap() > 0);
    // A different IP is also blocked for this username (either bucket trips).
    let other = crate::actor::Origin::new(
        crate::actor::Channel::Dashboard,
        Some("198.51.100.9".into()),
    );
    assert_eq!(
        kind(
            core.services
                .auth
                .login(&other, "owner", &owner.password, None)
                .await
        ),
        ErrorKind::RateLimited
    );
    core.clock.advance(Duration::from_secs(16 * 60));
    core.services
        .auth
        .login(&core.origin(), "owner", &owner.password, None)
        .await
        .unwrap();
    assert!(audit_count(&core, "auth.rate_limited", "denied").await >= 1);
}

#[tokio::test]
async fn sessions_expire_after_24h_idle_and_7d_absolute() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let login = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let cookie = login.session.cookie.expose().to_owned();
    let auth = |c: String| {
        let core = &core;
        async move {
            core.services
                .auth
                .authenticate_session(&core.origin(), &c)
                .await
        }
    };
    assert!(auth(cookie.clone()).await.is_ok());

    // Active use keeps it alive past 24h total, but not past 7 days absolute.
    for _ in 0..6 {
        core.clock.advance(20 * HOUR);
        assert!(
            auth(cookie.clone()).await.is_ok(),
            "should survive while active"
        );
    }
    core.clock.advance(20 * HOUR); // 140h < 168h
    assert!(auth(cookie.clone()).await.is_ok());
    core.clock.advance(30 * HOUR); // > 168h absolute
    assert_eq!(kind(auth(cookie.clone()).await), ErrorKind::Unauthenticated);

    // Idle timeout on a fresh session.
    let login = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    core.clock.advance(25 * HOUR);
    assert_eq!(
        kind(auth(login.session.cookie.expose().to_owned()).await),
        ErrorKind::Unauthenticated
    );
}

#[tokio::test]
async fn cookie_is_stored_only_as_hash_and_garbage_is_rejected() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let login = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let cookie = login.session.cookie.expose();
    let hits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?")
        .bind(cookie)
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(hits, 0);
    for bad in ["", "nope", &"x".repeat(300)] {
        assert_eq!(
            kind(
                core.services
                    .auth
                    .authenticate_session(&core.origin(), bad)
                    .await
            ),
            ErrorKind::Unauthenticated
        );
    }
}

#[tokio::test]
async fn logout_revokes_the_session() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let login = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let s = core
        .services
        .auth
        .authenticate_session(&core.origin(), login.session.cookie.expose())
        .await
        .unwrap();
    core.services
        .auth
        .logout(&s.actor, &s.session_id)
        .await
        .unwrap();
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), login.session.cookie.expose())
            .await
            .is_err()
    );
    assert_eq!(audit_count(&core, "auth.logout", "ok").await, 1);
}

#[tokio::test]
async fn password_change_requires_current_and_revokes_other_sessions() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let a = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let b = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let sa = core
        .services
        .auth
        .authenticate_session(&core.origin(), a.session.cookie.expose())
        .await
        .unwrap();
    let wrong = core
        .services
        .auth
        .password_change(
            &sa.actor,
            &sa.session_id,
            "nope-nope-nope",
            "brand-new-password",
        )
        .await;
    assert!(matches!(kind(wrong), ErrorKind::Validation(_)));
    core.services
        .auth
        .password_change(
            &sa.actor,
            &sa.session_id,
            &owner.password,
            "brand-new-password",
        )
        .await
        .unwrap();
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), a.session.cookie.expose())
            .await
            .is_ok()
    );
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), b.session.cookie.expose())
            .await
            .is_err()
    );
    assert!(
        core.services
            .auth
            .login(&core.origin(), &owner.username, &owner.password, None)
            .await
            .is_err()
    );
    core.services
        .auth
        .login(&core.origin(), &owner.username, "brand-new-password", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn session_list_and_revoke_are_scoped_to_the_caller() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let o = core.session_actor(&owner).await;
    let a_login = core
        .services
        .auth
        .login(&core.origin(), "alice", &alice.password, None)
        .await
        .unwrap();
    let a = core
        .services
        .auth
        .authenticate_session(&core.origin(), a_login.session.cookie.expose())
        .await
        .unwrap();
    let mine = core
        .services
        .auth
        .session_list(&a.actor, &a.session_id)
        .await
        .unwrap();
    assert!(mine.iter().all(|s| s.id == a.session_id || !s.current));
    assert!(mine.iter().any(|s| s.current));
    // The Owner cannot revoke Alice's session through the self-service op.
    assert_eq!(
        kind(core.services.auth.session_revoke(&o, &a.session_id).await),
        ErrorKind::NotFound
    );
    core.services
        .auth
        .session_revoke(&a.actor, &a.session_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn invite_is_single_use_expires_and_preview_does_not_consume() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let created = core
        .services
        .users
        .create(&Actor::cli(), "alice", &[])
        .await
        .unwrap();
    let token = token_from_link(created.invite.url.expose());
    // The link points at the fragment form and the token is hashed at rest.
    assert!(
        created
            .invite
            .url
            .expose()
            .contains("/account/setup#token=")
    );
    let stored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_tokens WHERE token_hash = ?")
            .bind(&token)
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert_eq!(stored, 0);

    let p = core
        .services
        .auth
        .account_preview(&core.origin(), &token)
        .await
        .unwrap();
    assert_eq!(
        (p.username.as_str(), p.purpose.as_str()),
        ("alice", "invite")
    );
    core.services
        .auth
        .account_preview(&core.origin(), &token)
        .await
        .unwrap();

    let weak = core
        .services
        .auth
        .account_setup(&core.origin(), &token, "short", None)
        .await;
    assert!(matches!(kind(weak), ErrorKind::Validation(_)));
    core.services
        .auth
        .account_setup(&core.origin(), &token, MEMBER_PASSWORD, None)
        .await
        .unwrap();
    // Second use fails.
    assert_eq!(
        kind(
            core.services
                .auth
                .account_setup(&core.origin(), &token, MEMBER_PASSWORD, None)
                .await
        ),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(
            core.services
                .auth
                .account_preview(&core.origin(), &token)
                .await
        ),
        ErrorKind::Unauthenticated
    );

    // Expiry after 24h.
    let bob = core
        .services
        .users
        .create(&Actor::cli(), "bob", &[])
        .await
        .unwrap();
    let btok = token_from_link(bob.invite.url.expose());
    core.clock.advance(25 * HOUR);
    assert_eq!(
        kind(
            core.services
                .auth
                .account_setup(&core.origin(), &btok, MEMBER_PASSWORD, None)
                .await
        ),
        ErrorKind::Unauthenticated
    );
}

#[tokio::test]
async fn invited_user_cannot_login_and_reset_replaces_password_and_sessions() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let login = core
        .services
        .auth
        .login(&core.origin(), "alice", &alice.password, None)
        .await
        .unwrap();
    let link = core
        .services
        .users
        .reset_link(&Actor::cli(), &alice.id)
        .await
        .unwrap();
    // Sessions died and the old password no longer works.
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), login.session.cookie.expose())
            .await
            .is_err()
    );
    assert_eq!(
        kind(
            core.services
                .auth
                .login(&core.origin(), "alice", &alice.password, None)
                .await
        ),
        ErrorKind::Unauthenticated
    );
    let token = token_from_link(link.url.expose());
    core.services
        .auth
        .account_setup(&core.origin(), &token, "a-brand-new-password", None)
        .await
        .unwrap();
    core.services
        .auth
        .login(&core.origin(), "alice", "a-brand-new-password", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn newer_link_supersedes_older_one() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let first = core
        .services
        .users
        .reset_link(&Actor::cli(), &alice.id)
        .await
        .unwrap();
    let second = core
        .services
        .users
        .reset_link(&Actor::cli(), &alice.id)
        .await
        .unwrap();
    let (t1, t2) = (
        token_from_link(first.url.expose()),
        token_from_link(second.url.expose()),
    );
    assert!(
        core.services
            .auth
            .account_preview(&core.origin(), &t1)
            .await
            .is_err()
    );
    assert!(
        core.services
            .auth
            .account_preview(&core.origin(), &t2)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn owner_reset_revokes_sessions_and_issues_a_24h_link() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let login = core
        .services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    let link = core.services.auth.owner_reset(&Actor::cli()).await.unwrap();
    assert_eq!(link.username, "owner");
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), login.session.cookie.expose())
            .await
            .is_err()
    );
    assert!(
        core.services
            .auth
            .login(&core.origin(), &owner.username, &owner.password, None)
            .await
            .is_err()
    );
    let token = token_from_link(link.url.expose());
    core.services
        .auth
        .account_setup(&core.origin(), &token, "recovered-password-1", None)
        .await
        .unwrap();
    core.services
        .auth
        .login(&core.origin(), "owner", "recovered-password-1", None)
        .await
        .unwrap();
    assert_eq!(audit_count(&core, "auth.owner_reset", "ok").await, 1);
    // Owner still singular.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='owner'")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn owner_reset_without_owner_is_not_found() {
    let core = TestCore::new().await;
    assert_eq!(
        kind(core.services.auth.owner_reset(&Actor::cli()).await),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn disabling_a_user_blocks_login_and_kills_sessions() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let login = core
        .services
        .auth
        .login(&core.origin(), "alice", &alice.password, None)
        .await
        .unwrap();
    core.services
        .users
        .set_status(
            &Actor::cli(),
            &alice.id,
            crate::db::repo::users::UserStatus::Disabled,
        )
        .await
        .unwrap();
    assert!(
        core.services
            .auth
            .authenticate_session(&core.origin(), login.session.cookie.expose())
            .await
            .is_err()
    );
    assert!(
        core.services
            .auth
            .login(&core.origin(), "alice", &alice.password, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn purge_removes_expired_rows() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    core.services
        .auth
        .login(&core.origin(), &owner.username, &owner.password, None)
        .await
        .unwrap();
    core.services
        .users
        .create(&Actor::cli(), "alice", &[])
        .await
        .unwrap();
    core.clock.advance(8 * 24 * HOUR);
    assert!(core.services.auth.purge_expired().await.unwrap() >= 3);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
}
