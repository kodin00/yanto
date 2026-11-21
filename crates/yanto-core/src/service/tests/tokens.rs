use std::time::Duration;

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Channel, Origin, Permission, Scope};
use crate::testing::TestCore;

fn rest(ip: &str) -> Origin {
    Origin::new(Channel::Rest, Some(ip.into()))
}

#[tokio::test]
async fn token_roundtrip_is_hashed_and_acts_as_its_user() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let o = core.session_actor(&owner).await;
    let created = core
        .services
        .tokens
        .create(&o, "ci", Scope::Write, Some(30))
        .await
        .unwrap();
    let bearer = created.token.expose().to_owned();
    assert!(bearer.starts_with("yat_"));
    assert_eq!(created.info.hint, &bearer[4..12]);
    assert_eq!(created.info.id, format!("tok_{}", &bearer[4..28]));

    // Only the hash is stored.
    let hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens WHERE token_hash = ? OR id = ?")
            .bind(&bearer)
            .bind(&bearer)
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert_eq!(hits, 0);

    let actor = core
        .services
        .tokens
        .authenticate(&rest("1.1.1.1"), &bearer)
        .await
        .unwrap();
    assert!(actor.is_owner());
    assert_eq!(actor.token_scope(), Some(Scope::Write));
    assert_eq!(actor.channel, Channel::Rest);
    let who = core.services.tokens.whoami(&actor).await.unwrap();
    assert_eq!(who.username, "owner");
    assert_eq!(who.token.unwrap().name, "ci");
    assert_eq!(audit_count(&core, "api_token.create", "ok").await, 1);
}

#[tokio::test]
async fn members_inherit_only_their_own_grants_through_a_token() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let alice = core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    let (_, actor) = core.token_actor(&alice, Scope::Admin).await;
    assert!(!actor.is_owner());
    assert_eq!(actor.grants.len(), 1);
    assert!(actor.grants["prj_1"].contains(&Permission::Deploy));
}

#[tokio::test]
async fn tokens_cannot_manage_tokens_or_sessions() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let (_, admin) = core.token_actor(&owner, Scope::Admin).await;
    assert_eq!(
        kind(
            core.services
                .tokens
                .create(&admin, "x", Scope::Read, None)
                .await
        ),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(core.services.tokens.list(&admin).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(core.services.tokens.revoke(&admin, "tok_x").await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(core.services.auth.session_list(&admin, "s").await),
        ErrorKind::Forbidden
    );
}

#[tokio::test]
async fn revoked_expired_and_disabled_owner_tokens_stop_working() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let o = core.session_actor(&owner).await;

    let a = core.session_actor(&alice).await;
    let t1 = core
        .services
        .tokens
        .create(&a, "one", Scope::Read, None)
        .await
        .unwrap();
    core.services.tokens.revoke(&o, &t1.info.id).await.unwrap(); // Owner revokes a Member's token
    assert_eq!(
        kind(
            core.services
                .tokens
                .authenticate(&rest("1.1.1.1"), t1.token.expose())
                .await
        ),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(core.services.tokens.revoke(&o, &t1.info.id).await),
        ErrorKind::NotFound
    );

    let t2 = core
        .services
        .tokens
        .create(&a, "two", Scope::Read, Some(1))
        .await
        .unwrap();
    assert!(
        core.services
            .tokens
            .authenticate(&rest("1.1.1.1"), t2.token.expose())
            .await
            .is_ok()
    );
    core.clock.advance(Duration::from_secs(25 * 3600));
    assert!(
        core.services
            .tokens
            .authenticate(&rest("1.1.1.1"), t2.token.expose())
            .await
            .is_err()
    );

    let a = core.session_actor(&alice).await;
    let t3 = core
        .services
        .tokens
        .create(&a, "three", Scope::Read, None)
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
            .tokens
            .authenticate(&rest("1.1.1.1"), t3.token.expose())
            .await
            .is_err()
    );
    assert_eq!(audit_count(&core, "api_token.revoke", "ok").await, 1);
}

#[tokio::test]
async fn listing_and_revoking_visibility() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let bob = core.create_member("bob", &[]).await;
    let (o, a, b) = (
        core.session_actor(&owner).await,
        core.session_actor(&alice).await,
        core.session_actor(&bob).await,
    );
    core.services
        .tokens
        .create(&o, "o1", Scope::Read, None)
        .await
        .unwrap();
    let at = core
        .services
        .tokens
        .create(&a, "a1", Scope::Read, None)
        .await
        .unwrap();
    core.services
        .tokens
        .create(&b, "b1", Scope::Read, None)
        .await
        .unwrap();

    assert_eq!(core.services.tokens.list(&o).await.unwrap().len(), 3);
    let own = core.services.tokens.list(&a).await.unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].name, "a1");
    // Bob cannot see or revoke Alice's token.
    assert_eq!(
        kind(core.services.tokens.revoke(&b, &at.info.id).await),
        ErrorKind::NotFound
    );
    core.services.tokens.revoke(&a, &at.info.id).await.unwrap();
}

#[tokio::test]
async fn names_unique_per_user_among_active_tokens() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core.create_member("alice", &[]).await;
    let (o, a) = (
        core.session_actor(&owner).await,
        core.session_actor(&alice).await,
    );
    let t = core
        .services
        .tokens
        .create(&o, "ci", Scope::Read, None)
        .await
        .unwrap();
    assert_eq!(
        kind(
            core.services
                .tokens
                .create(&o, "ci", Scope::Read, None)
                .await
        ),
        ErrorKind::Conflict
    );
    core.services
        .tokens
        .create(&a, "ci", Scope::Read, None)
        .await
        .unwrap();
    core.services.tokens.revoke(&o, &t.info.id).await.unwrap();
    core.services
        .tokens
        .create(&o, "ci", Scope::Read, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn malformed_bearer_values_fail_and_are_rate_limited_per_ip() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let ip = rest("192.0.2.50");
    for bad in [
        "",
        "Bearer x",
        "yat_",
        "yat_abc_def",
        &format!("yat_{}_{}", "g".repeat(24), "0".repeat(64)),
        &format!("yat_{}_{}", "0".repeat(24), "0".repeat(64)),
    ] {
        assert_eq!(
            kind(core.services.tokens.authenticate(&ip, bad).await),
            ErrorKind::Unauthenticated,
            "{bad}"
        );
    }
    // 6 failures so far; push to the 20 limit.
    for _ in 0..14 {
        let _ = core.services.tokens.authenticate(&ip, "nope").await;
    }
    assert_eq!(
        kind(core.services.tokens.authenticate(&ip, "nope").await),
        ErrorKind::RateLimited
    );
    assert_eq!(
        audit_count(&core, "api_token.auth_failed", "denied").await,
        1
    );
    // Another IP is unaffected.
    assert_eq!(
        kind(
            core.services
                .tokens
                .authenticate(&rest("192.0.2.51"), "nope")
                .await
        ),
        ErrorKind::Unauthenticated
    );
}

#[tokio::test]
async fn create_validates_name_and_expiry() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let o = core.session_actor(&owner).await;
    for name in ["", "   ", &"n".repeat(101)] {
        assert!(matches!(
            kind(
                core.services
                    .tokens
                    .create(&o, name, Scope::Read, None)
                    .await
            ),
            ErrorKind::Validation(_)
        ));
    }
    assert!(matches!(
        kind(
            core.services
                .tokens
                .create(&o, "x", Scope::Read, Some(0))
                .await
        ),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(
            core.services
                .tokens
                .create(&Actor::cli(), "x", Scope::Read, None)
                .await
        ),
        ErrorKind::Validation(_)
    ));
}
