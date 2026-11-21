//! Schema-level tests for migrations 0700-0702.

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

async fn seed(db: &Db) {
    for (id, slug) in [("prj_1", "one"), ("prj_2", "two")] {
        exec(db, &format!("INSERT INTO projects (id, name, slug, source_kind, image_ref, deploy_token_hash, deploy_token_enc, created_at, updated_at) VALUES ('{id}', 'p', '{slug}', 'image', 'nginx', 'h', 'e', 0, 0)")).await.unwrap();
    }
    exec(db, "INSERT INTO project_ports (id, project_id, label, host_port, container_port, created_at) VALUES ('ppt_1', 'prj_1', 'web', 8080, 80, 0)").await.unwrap();
    exec(db, "INSERT INTO cloudflare_clients (id, name, account_id, api_token_enc, created_at, updated_at) VALUES ('cfc_1', 'c', 'acc', 'enc', 0, 0)").await.unwrap();
    exec(db, "INSERT INTO cloudflare_tunnels (id, client_id, cf_tunnel_id, name, created_at, updated_at) VALUES ('cft_1', 'cfc_1', 'uuid-1', 't', 0, 0)").await.unwrap();
}

fn hostname(
    id: &str,
    host: &str,
    port: &str,
    project: &str,
    provider: &str,
    tunnel: &str,
) -> String {
    format!(
        "INSERT INTO hostnames (id, project_id, project_port_id, hostname, ingress_provider, tunnel_id, created_at, updated_at) VALUES ('{id}', '{project}', '{port}', '{host}', '{provider}', {tunnel}, 0, 0)"
    )
}

#[tokio::test]
async fn hostname_constraints() {
    let (_d, db) = temp_db().await;
    seed(&db).await;
    exec(
        &db,
        &hostname(
            "hst_1",
            "a.example.com",
            "ppt_1",
            "prj_1",
            "traefik",
            "NULL",
        ),
    )
    .await
    .unwrap();
    assert!(
        exec(
            &db,
            &hostname(
                "hst_2",
                "a.example.com",
                "ppt_1",
                "prj_1",
                "traefik",
                "NULL"
            )
        )
        .await
        .is_err(),
        "hostname is unique across providers"
    );
    assert!(
        exec(
            &db,
            &hostname(
                "hst_3",
                "B.example.com",
                "ppt_1",
                "prj_1",
                "traefik",
                "NULL"
            )
        )
        .await
        .is_err(),
        "hostname must be lowercase"
    );
    assert!(
        exec(
            &db,
            &hostname(
                "hst_4",
                "c.example.com",
                "ppt_1",
                "prj_1",
                "cloudflare_tunnel",
                "NULL"
            )
        )
        .await
        .is_err(),
        "a tunnel hostname needs a tunnel"
    );
    assert!(
        exec(
            &db,
            &hostname(
                "hst_5",
                "d.example.com",
                "ppt_1",
                "prj_1",
                "traefik",
                "'cft_1'"
            )
        )
        .await
        .is_err(),
        "a traefik hostname has no tunnel"
    );
    exec(
        &db,
        &hostname(
            "hst_6",
            "e.example.com",
            "ppt_1",
            "prj_1",
            "cloudflare_tunnel",
            "'cft_1'",
        ),
    )
    .await
    .unwrap();
    assert!(
        exec(
            &db,
            &hostname(
                "hst_7",
                "f.example.com",
                "ppt_1",
                "prj_2",
                "traefik",
                "NULL"
            )
        )
        .await
        .is_err(),
        "the port must belong to the hostname's project"
    );
    assert!(
        exec(&db, "INSERT INTO hostnames (id, target_kind, hostname, ingress_provider, created_at, updated_at) VALUES ('hst_8', 'project', 'g.example.com', 'traefik', 0, 0)").await.is_err(),
        "project target needs a port"
    );
    exec(&db, "INSERT INTO hostnames (id, target_kind, hostname, ingress_provider, created_at, updated_at) VALUES ('hst_9', 'registry', 'reg.example.com', 'traefik', 0, 0)").await.unwrap();
    assert!(
        exec(
            &db,
            "UPDATE hostnames SET manage_dns = 1 WHERE id = 'hst_1'"
        )
        .await
        .is_err(),
        "managed DNS needs a zone"
    );
}

#[tokio::test]
async fn port_in_use_cannot_be_removed_but_project_delete_cascades() {
    let (_d, db) = temp_db().await;
    seed(&db).await;
    exec(
        &db,
        &hostname(
            "hst_1",
            "a.example.com",
            "ppt_1",
            "prj_1",
            "traefik",
            "NULL",
        ),
    )
    .await
    .unwrap();
    assert!(
        exec(&db, "DELETE FROM project_ports WHERE id = 'ppt_1'")
            .await
            .is_err(),
        "a port used by a Hostname is protected"
    );
    exec(&db, "DELETE FROM projects WHERE id = 'prj_1'")
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hostnames")
        .fetch_one(db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn one_active_tunnel_and_tunnel_in_use_is_protected() {
    let (_d, db) = temp_db().await;
    seed(&db).await;
    exec(&db, "INSERT INTO cloudflare_tunnels (id, client_id, cf_tunnel_id, name, active, created_at, updated_at) VALUES ('cft_2', 'cfc_1', 'uuid-2', 't2', 1, 0, 0)").await.unwrap();
    assert!(
        exec(
            &db,
            "UPDATE cloudflare_tunnels SET active = 1 WHERE id = 'cft_1'"
        )
        .await
        .is_err(),
        "only one active tunnel"
    );
    exec(
        &db,
        &hostname(
            "hst_1",
            "a.example.com",
            "ppt_1",
            "prj_1",
            "cloudflare_tunnel",
            "'cft_1'",
        ),
    )
    .await
    .unwrap();
    assert!(
        exec(&db, "DELETE FROM cloudflare_tunnels WHERE id = 'cft_1'")
            .await
            .is_err()
    );
    assert!(
        exec(&db, "DELETE FROM cloudflare_clients WHERE id = 'cfc_1'")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn frps_settings_singleton_exists() {
    let (_d, db) = temp_db().await;
    let host: String = sqlx::query_scalar("SELECT public_host FROM frps_settings WHERE id = 1")
        .fetch_one(db.reader())
        .await
        .unwrap();
    assert_eq!(host, "");
    assert!(
        exec(
            &db,
            "INSERT INTO frps_settings (id, updated_at) VALUES (2, 0)"
        )
        .await
        .is_err()
    );
}
