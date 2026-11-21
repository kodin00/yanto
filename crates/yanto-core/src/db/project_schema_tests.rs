//! Schema-level tests for migrations 0200 and 0201.

use sqlx::{Connection, SqliteConnection};

use super::Db;

async fn temp_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("yanto.db")).await.unwrap();
    (dir, db)
}

async fn exec(db: &Db, sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(db.writer())
        .await
        .map(|_| ())
}

const PROJECT: &str = "INSERT INTO projects (id, name, slug, source_kind, image_ref, deploy_token_hash, deploy_token_enc, created_at, updated_at) VALUES ('{id}', 'p', '{slug}', 'image', 'nginx', 'h', 'e', 0, 0)";

fn project(id: &str, slug: &str) -> String {
    PROJECT.replace("{id}", id).replace("{slug}", slug)
}

#[tokio::test]
async fn project_constraints() {
    let (_d, db) = temp_db().await;
    exec(&db, &project("prj_1", "app")).await.unwrap();
    assert!(
        exec(&db, &project("prj_2", "app")).await.is_err(),
        "slug is unique"
    );
    assert!(
        exec(&db, &project("prj_3", "Bad Slug")).await.is_err(),
        "slug charset"
    );
    assert!(
        exec(&db, "INSERT INTO projects (id, name, slug, source_kind, git_url, deploy_token_hash, deploy_token_enc, created_at, updated_at) VALUES ('prj_4', 'p', 'g', 'git', 'git@h:r', 'h', 'e', 0, 0)")
            .await
            .is_err(),
        "a git project needs a branch"
    );
    assert!(
        exec(&db, "INSERT INTO projects (id, name, slug, source_kind, git_url, branch, image_ref, deploy_token_hash, deploy_token_enc, created_at, updated_at) VALUES ('prj_5', 'p', 'b', 'git', 'git@h:r', 'master', 'nginx', 'h', 'e', 0, 0)")
            .await
            .is_err(),
        "exactly one source"
    );
    assert!(
        exec(
            &db,
            "UPDATE projects SET compose_path = '../x.yml' WHERE id = 'prj_1'"
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn ports_are_unique_across_projects_and_loopback_only() {
    let (_d, db) = temp_db().await;
    exec(&db, &project("prj_1", "a")).await.unwrap();
    exec(&db, &project("prj_2", "b")).await.unwrap();
    let port = |id: &str, prj: &str, label: &str, host: u16, bind: &str| {
        format!(
            "INSERT INTO project_ports (id, project_id, label, host_port, container_port, bind_address, created_at) VALUES ('{id}', '{prj}', '{label}', {host}, 80, '{bind}', 0)"
        )
    };
    exec(&db, &port("ppt_1", "prj_1", "web", 8080, "127.0.0.1"))
        .await
        .unwrap();
    assert!(
        exec(&db, &port("ppt_2", "prj_2", "web", 8080, "127.0.0.1"))
            .await
            .is_err()
    );
    assert!(
        exec(&db, &port("ppt_3", "prj_1", "web", 8081, "127.0.0.1"))
            .await
            .is_err(),
        "label unique per project"
    );
    assert!(
        exec(&db, &port("ppt_4", "prj_1", "x", 8082, "0.0.0.0"))
            .await
            .is_err()
    );
    exec(&db, "DELETE FROM projects WHERE id = 'prj_1'")
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_ports")
        .fetch_one(db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0, "ports cascade with the project");
}

#[tokio::test]
async fn one_running_and_one_queued_deployment_per_project() {
    let (_d, db) = temp_db().await;
    exec(&db, &project("prj_1", "a")).await.unwrap();
    let dep = |id: &str, status: &str| {
        format!(
            "INSERT INTO deployments (id, project_id, status, \"trigger\", queued_at) VALUES ('{id}', 'prj_1', '{status}', 'manual', 0)"
        )
    };
    exec(&db, &dep("dep_1", "running")).await.unwrap();
    exec(&db, &dep("dep_2", "queued")).await.unwrap();
    assert!(exec(&db, &dep("dep_3", "running")).await.is_err());
    assert!(exec(&db, &dep("dep_4", "queued")).await.is_err());
    exec(&db, &dep("dep_5", "success")).await.unwrap();
    exec(&db, &dep("dep_6", "success")).await.unwrap();
}

#[tokio::test]
async fn deleting_a_project_detaches_audit_rows_and_cascades_grants() {
    let (_d, db) = temp_db().await;
    exec(&db, &project("prj_1", "a")).await.unwrap();
    exec(&db, "INSERT INTO users (id, username, role, status, created_at, updated_at) VALUES ('usr_m', 'm', 'member', 'invited', 0, 0)").await.unwrap();
    exec(&db, "INSERT INTO user_project_access (user_id, project_id, created_at, updated_at) VALUES ('usr_m', 'prj_1', 0, 0)").await.unwrap();
    assert!(
        exec(&db, "INSERT INTO user_project_access (user_id, project_id, created_at, updated_at) VALUES ('usr_m', 'prj_x', 0, 0)").await.is_err(),
        "unknown Project is rejected by the FK"
    );
    exec(&db, "INSERT INTO audit_logs (id, actor_type, actor_label, action, entity_type, project_id, channel, created_at) VALUES ('aud_1', 'system', 's', 'x.y', 'x', 'prj_1', 'system', 0)").await.unwrap();
    exec(&db, "DELETE FROM projects WHERE id = 'prj_1'")
        .await
        .unwrap();
    let pid: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM audit_logs WHERE id = 'aud_1'")
            .fetch_one(db.reader())
            .await
            .unwrap();
    assert_eq!(pid, None);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_project_access")
        .fetch_one(db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// The rebuild keeps valid grants and drops grants for Projects that never existed.
#[tokio::test]
async fn migration_0201_copies_valid_grants_and_drops_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = SqliteConnection::connect(&format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("t.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut conn)
        .await
        .unwrap();
    for sql in [
        include_str!("../../migrations/0001_app_settings.sql"),
        include_str!("../../migrations/0100_users.sql"),
        include_str!("../../migrations/0101_sessions.sql"),
        include_str!("../../migrations/0102_account_tokens.sql"),
        include_str!("../../migrations/0103_user_project_access.sql"),
        include_str!("../../migrations/0104_api_tokens.sql"),
        include_str!("../../migrations/0105_audit_logs.sql"),
        include_str!("../../migrations/0200_projects.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    }
    for sql in [
        "INSERT INTO users (id, username, role, status, created_at, updated_at) VALUES ('usr_m', 'm', 'member', 'invited', 0, 0)",
        &project("prj_real", "real"),
        "INSERT INTO user_project_access VALUES ('usr_m', 'prj_real', 1, 1), ('usr_m', 'prj_ghost', 1, 1)",
        "INSERT INTO user_project_permissions VALUES ('usr_m', 'prj_real', 'deploy'), ('usr_m', 'prj_ghost', 'deploy')",
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut conn)
            .await
            .unwrap();
    }
    let mut tx = conn.begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0201_project_foreign_keys.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let access: Vec<String> = sqlx::query_scalar("SELECT project_id FROM user_project_access")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert_eq!(access, ["prj_real"]);
    let perms: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_project_permissions")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(perms, 1);
    let triggers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name IN ('user_project_access_no_owner','audit_project_detach')")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(triggers, 2);
    let fk: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert!(fk.is_empty());
}

#[tokio::test]
async fn hosted_registry_singleton_and_user_constraints() {
    let (_d, db) = temp_db().await;
    exec(
        &db,
        "UPDATE hosted_registry_settings SET listen_port = 5055 WHERE id = 1",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &db,
            "INSERT INTO hosted_registry_settings (id, updated_at) VALUES (2, 0)"
        )
        .await
        .is_err(),
        "settings are a singleton"
    );
    let user = |id: &str, name: &str| {
        format!(
            "INSERT INTO registry_users (id, username, password_hash, created_at, updated_at) VALUES ('{id}', '{name}', 'h', 0, 0)"
        )
    };
    exec(&db, &user("rus_1", "Alice")).await.unwrap();
    assert!(
        exec(&db, &user("rus_2", "alice")).await.is_err(),
        "NOCASE unique"
    );
    assert!(exec(&db, &user("rus_3", "a:b")).await.is_err(), "no colon");
}

/// 0550 keeps the run history of a Cron Entry (the DROP of the parent must not cascade), keeps
/// valid references, nulls dangling ones, and the new foreign keys work afterwards.
#[tokio::test]
async fn migration_0550_rebuilds_cron_and_backups_keeping_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = SqliteConnection::connect(&format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("t.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut conn)
        .await
        .unwrap();
    for sql in [
        include_str!("../../migrations/0001_app_settings.sql"),
        include_str!("../../migrations/0100_users.sql"),
        include_str!("../../migrations/0101_sessions.sql"),
        include_str!("../../migrations/0102_account_tokens.sql"),
        include_str!("../../migrations/0103_user_project_access.sql"),
        include_str!("../../migrations/0104_api_tokens.sql"),
        include_str!("../../migrations/0105_audit_logs.sql"),
        include_str!("../../migrations/0200_projects.sql"),
        include_str!("../../migrations/0201_project_foreign_keys.sql"),
        include_str!("../../migrations/0250_project_databases.sql"),
        include_str!("../../migrations/0500_cron.sql"),
        include_str!("../../migrations/0501_authorized_keys.sql"),
        include_str!("../../migrations/0502_backups.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    }
    for sql in [
        &project("prj_real", "real"),
        "INSERT INTO project_databases (id, project_id, engine, compose_service, created_at, updated_at) VALUES ('pdb_1', 'prj_real', 'postgres', 'db', 0, 0)",
        "INSERT INTO cron_entries (id, name, schedule, run_as_user, command, project_id, created_at, updated_at) VALUES ('crn_1', 'a', '* * * * *', 'root', 'true', 'prj_real', 0, 0), ('crn_2', 'b', '* * * * *', 'root', 'true', 'prj_ghost', 0, 0)",
        "INSERT INTO cron_runs (id, entry_id, status, started_at) VALUES ('crr_1', 'crn_1', 'success', 1), ('crr_2', 'crn_2', 'failed', 2)",
        "INSERT INTO backups (id, project_id, kind, database_id, status, trigger, filename, file_path, created_at) VALUES ('bak_1', 'prj_real', 'project_postgres', 'pdb_1', 'success', 'manual', 'f', '/p', 1), ('bak_2', 'prj_ghost', 'project_postgres', 'pdb_ghost', 'success', 'manual', 'g', '/q', 2)",
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut conn)
            .await
            .unwrap();
    }
    let mut tx = conn.begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0550_project_foreign_keys.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cron_runs")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(runs, 2, "run history survives the parent rebuild");
    let cron: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, project_id FROM cron_entries ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert_eq!(
        cron,
        [
            ("crn_1".to_owned(), Some("prj_real".to_owned())),
            ("crn_2".to_owned(), None)
        ]
    );
    let backups: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, project_id, database_id FROM backups ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert_eq!(
        backups,
        [
            (
                "bak_1".to_owned(),
                Some("prj_real".to_owned()),
                Some("pdb_1".to_owned())
            ),
            ("bak_2".to_owned(), None, None)
        ]
    );
    // The foreign keys are live: deleting the Project or the declaration detaches, not deletes.
    sqlx::raw_sql("DELETE FROM project_databases WHERE id = 'pdb_1'")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::raw_sql("DELETE FROM projects WHERE id = 'prj_real'")
        .execute(&mut conn)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backups WHERE project_id IS NULL AND database_id IS NULL",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(left, 2);
    assert!(
        sqlx::raw_sql("INSERT INTO backups (id, project_id, kind, status, trigger, filename, file_path, created_at) VALUES ('bak_3', 'nope', 'project_volumes', 'running', 'manual', 'f', '/p', 3)")
            .execute(&mut conn)
            .await
            .is_err(),
        "dangling project ids are refused"
    );
    let idx: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN ('backups_created_idx','backups_project_idx','backups_status_idx','cron_runs_entry_idx')")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(idx, 4);
    let fk: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert!(fk.is_empty());
}
