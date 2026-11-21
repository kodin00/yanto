use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Permission};
use crate::db::repo::users::UserStatus;
use crate::service::users::GrantInput;
use crate::testing::TestCore;

fn grant(p: &str, perms: &[Permission]) -> GrantInput {
    GrantInput {
        project_id: p.into(),
        permissions: perms.to_vec(),
    }
}

#[tokio::test]
async fn create_lists_updates_and_deletes_a_member() {
    let core = TestCore::new().await;
    for p in ["prj_1", "prj_2", "prj_3"] {
        core.ensure_project(p).await;
    }
    let owner = core.setup_owner().await;
    let o = core.session_actor(&owner).await;
    let created = core
        .services
        .users
        .create(
            &o,
            "alice",
            &[grant("prj_1", &[Permission::Deploy, Permission::Secrets])],
        )
        .await
        .unwrap();
    assert_eq!(created.user.status, "invited");
    assert_eq!(created.user.grants[0].permissions, ["deploy", "secrets"]);
    assert_eq!(
        kind(core.services.users.create(&o, "ALICE", &[]).await),
        ErrorKind::Conflict
    );
    let list = core.services.users.list(&o).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].role, "owner");

    let updated = core
        .services
        .users
        .update_access(
            &o,
            &created.user.id,
            &[grant("prj_2", &[]), grant("prj_3", &[Permission::Backups])],
        )
        .await
        .unwrap();
    assert_eq!(updated.grants.len(), 2);
    assert!(matches!(
        kind(
            core.services
                .users
                .update_access(&o, &created.user.id, &[grant("prj_nope", &[])])
                .await
        ),
        ErrorKind::Validation(_)
    ));
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_project_permissions WHERE project_id = 'prj_1'",
    )
    .fetch_one(core.db.reader())
    .await
    .unwrap();
    assert_eq!(n, 0, "replace removes old permissions");

    core.services
        .users
        .delete(&o, &created.user.id)
        .await
        .unwrap();
    assert_eq!(
        kind(core.services.users.get(&o, &created.user.id).await),
        ErrorKind::NotFound
    );
    for (action, n) in [
        ("user.create", 1),
        ("user.access_update", 1),
        ("user.delete", 1),
    ] {
        assert_eq!(audit_count(&core, action, "ok").await, n, "{action}");
    }
}

#[tokio::test]
async fn the_owner_cannot_be_targeted_by_member_operations() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let cli = Actor::cli();
    assert!(matches!(
        kind(core.services.users.delete(&cli, &owner.id).await),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(
            core.services
                .users
                .set_status(&cli, &owner.id, UserStatus::Disabled)
                .await
        ),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(core.services.users.reset_link(&cli, &owner.id).await),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(
            core.services
                .users
                .update_access(&cli, &owner.id, &[])
                .await
        ),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
async fn status_update_rules() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let cli = Actor::cli();
    let invited = core
        .services
        .users
        .create(&cli, "carol", &[])
        .await
        .unwrap()
        .user;
    // Invited users cannot be force-activated; status must be active or disabled.
    assert!(matches!(
        kind(
            core.services
                .users
                .set_status(&cli, &invited.id, UserStatus::Invited)
                .await
        ),
        ErrorKind::Validation(_)
    ));
    // Disable then re-enable: a user who never set a password goes back to invited.
    let d = core
        .services
        .users
        .set_status(&cli, &invited.id, UserStatus::Disabled)
        .await
        .unwrap();
    assert_eq!(d.status, "disabled");
    let e = core
        .services
        .users
        .set_status(&cli, &invited.id, UserStatus::Active)
        .await
        .unwrap();
    assert_eq!(e.status, "invited");
    // Disabled users cannot get a link.
    core.services
        .users
        .set_status(&cli, &invited.id, UserStatus::Disabled)
        .await
        .unwrap();
    assert_eq!(
        kind(core.services.users.reset_link(&cli, &invited.id).await),
        ErrorKind::Conflict
    );
    assert_eq!(
        kind(
            core.services
                .users
                .set_status(&cli, "usr_missing", UserStatus::Disabled)
                .await
        ),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn deleting_a_member_revokes_everything_and_keeps_audit_history() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    let (bearer, _) = core.token_actor(&alice, crate::actor::Scope::Write).await;
    let o = core.session_actor(&owner).await;
    core.services.users.delete(&o, &alice.id).await.unwrap();
    let origin = crate::actor::Origin::new(crate::actor::Channel::Rest, None);
    assert!(
        core.services
            .tokens
            .authenticate(&origin, &bearer)
            .await
            .is_err()
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE actor_label = 'alice'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(rows >= 1, "alice's audit history survives deletion");
    let access: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_project_access")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(access, 0);
}

#[tokio::test]
async fn input_validation() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let cli = Actor::cli();
    for bad in ["", "has space", "emoji😀"] {
        assert!(
            matches!(
                kind(core.services.users.create(&cli, bad, &[]).await),
                ErrorKind::Validation(_)
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        kind(
            core.services
                .users
                .create(&cli, "dave", &[grant("../etc", &[])])
                .await
        ),
        ErrorKind::Validation(_)
    ));
}
