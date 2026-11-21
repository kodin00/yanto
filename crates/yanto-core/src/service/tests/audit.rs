use super::kind;
use crate::ErrorKind;
use crate::actor::{Actor, Permission};
use crate::service::audit::{self, AuditActor, AuditEvent, AuditQuery};
use crate::testing::TestCore;

async fn seed(core: &TestCore, project: Option<&str>, n: usize) {
    for i in 0..n {
        let mut ev = AuditEvent::new("project.update", "project").entity(format!("e{i}"));
        if let Some(p) = project {
            ev = ev.project(p);
        }
        audit::write_standalone(
            core.services.ctx(),
            &AuditActor::system(&core.origin()),
            &ev,
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn members_see_only_granted_projects_and_owner_sees_all() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let alice = core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    seed(&core, Some("prj_1"), 2).await;
    seed(&core, Some("prj_2"), 3).await;
    seed(&core, None, 1).await;
    let (o, a) = (
        core.session_actor(&owner).await,
        core.session_actor(&alice).await,
    );

    let all = core
        .services
        .audit
        .list(&o, &AuditQuery::default())
        .await
        .unwrap();
    assert!(all.entries.len() >= 6);
    let mine = core
        .services
        .audit
        .list(&a, &AuditQuery::default())
        .await
        .unwrap();
    assert_eq!(mine.entries.len(), 2);
    assert!(
        mine.entries
            .iter()
            .all(|e| e.project_id.as_deref() == Some("prj_1"))
    );
    let q = AuditQuery {
        project_id: Some("prj_2".into()),
        ..Default::default()
    };
    assert_eq!(
        kind(core.services.audit.list(&a, &q).await),
        ErrorKind::NotFound
    );
    let q = AuditQuery {
        project_id: Some("prj_2".into()),
        ..Default::default()
    };
    assert_eq!(
        core.services
            .audit
            .list(&o, &q)
            .await
            .unwrap()
            .entries
            .len(),
        3
    );
}

#[tokio::test]
async fn cursor_pagination_walks_newest_first_without_gaps() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    seed(&core, Some("prj_1"), 7).await;
    let cli = Actor::cli();
    let mut q = AuditQuery {
        project_id: Some("prj_1".into()),
        limit: Some(3),
        cursor: None,
    };
    let mut seen = Vec::new();
    loop {
        let page = core.services.audit.list(&cli, &q).await.unwrap();
        seen.extend(page.entries.iter().map(|e| e.id.clone()));
        match page.next_cursor {
            Some(c) => q.cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 7);
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 7);
    assert!(matches!(
        kind(
            core.services
                .audit
                .list(
                    &cli,
                    &AuditQuery {
                        cursor: Some("junk".into()),
                        ..Default::default()
                    }
                )
                .await
        ),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
async fn audit_rows_are_append_only() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    seed(&core, Some("prj_1"), 1).await;
    let upd = sqlx::query("UPDATE audit_logs SET action = 'tampered'")
        .execute(core.db.writer())
        .await;
    assert!(upd.is_err());
    let upd = sqlx::query("UPDATE audit_logs SET actor_label = 'x'")
        .execute(core.db.writer())
        .await;
    assert!(upd.is_err());
    // The one permitted mutation: detaching a deleted Project.
    sqlx::query("UPDATE audit_logs SET project_id = NULL WHERE project_id = 'prj_1'")
        .execute(core.db.writer())
        .await
        .unwrap();
}

#[tokio::test]
async fn metadata_never_holds_secret_material() {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let o = core.session_actor(&owner).await;
    let created = core.services.users.create(&o, "alice", &[]).await.unwrap();
    let t = core
        .services
        .tokens
        .create(&o, "ci", crate::actor::Scope::Read, None)
        .await
        .unwrap();
    let dump: Vec<String> =
        sqlx::query_scalar("SELECT metadata || actor_label || action FROM audit_logs")
            .fetch_all(core.db.reader())
            .await
            .unwrap();
    let joined = dump.join("\n");
    let invite_token = crate::testing::token_from_link(created.invite.url.expose());
    assert!(!joined.contains(&invite_token));
    assert!(!joined.contains(t.token.expose()));
    assert!(!joined.contains(crate::testing::OWNER_PASSWORD));
}
