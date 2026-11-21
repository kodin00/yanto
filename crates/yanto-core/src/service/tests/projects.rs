//! Projects: creation rules, secrets handling, ports, deploy token, deletion.

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Permission, Scope};
use crate::service::projects::{DeleteOptions, PortInput, ProjectInput, ProjectPatch, SourceInput};
use crate::testing::{Reply, TestCore};

pub(super) fn image_input(name: &str, image: &str) -> ProjectInput {
    ProjectInput {
        name: name.into(),
        slug: None,
        source: SourceInput::Image {
            image_ref: image.into(),
        },
        compose_path: None,
        compose_content: None,
        env_file: None,
        auto_start: None,
        manual_deploy_enabled: None,
        github_webhook_enabled: None,
        deploy_key_id: None,
        generate_deploy_key: false,
        registry_credential_id: None,
    }
}

/// A Git project with pasted compose, so a fake `git clone` still leaves a compose file.
pub(super) fn git_input(name: &str, url: &str) -> ProjectInput {
    ProjectInput {
        source: SourceInput::Git {
            url: url.into(),
            branch: None,
        },
        compose_content: Some("services:\n  app:\n    image: nginx:alpine\n".into()),
        ..image_input(name, "x")
    }
}

const PORTS_JSON: &str = r#"{"services":{"web":{"image":"nginx:alpine","ports":[{"mode":"ingress","host_ip":"127.0.0.1","target":80,"published":"18080","protocol":"tcp"}]}}}"#;

async fn owner(core: &TestCore) -> Actor {
    let o = core.setup_owner().await;
    core.session_actor(&o).await
}

#[tokio::test]
async fn create_returns_the_token_once_and_never_stores_it_in_the_clear() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let created = core
        .services
        .projects
        .create(&o, &image_input("My App", "nginx:alpine"))
        .await
        .unwrap();
    let token = created.deploy_token.expose().to_owned();
    assert!(token.starts_with("ydp_"));
    assert_eq!(created.project.slug, "my-app");
    assert_eq!(created.project.source_kind, "image");
    let row: (String, String) =
        sqlx::query_as("SELECT deploy_token_hash, deploy_token_enc FROM projects")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert_eq!(row.0, crate::crypto::sha256_hex(&token));
    assert!(!row.1.contains(&token) && row.1.starts_with("enc:v1:"));
    let dto = serde_json::to_string(&created.project).unwrap();
    assert!(!dto.contains(&token) && !dto.contains("deploy_token"));
    assert_eq!(audit_count(&core, "project.create", "ok").await, 1);
    assert!(core.config.paths.data_dir.join("projects/my-app").is_dir());
}

#[tokio::test]
async fn create_validates_source_and_slug() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let s = &core.services.projects;
    for bad in [
        "file:///etc/passwd",
        "ext::sh -c id",
        "http://insecure/x",
        "-oProxyCommand=x",
    ] {
        let r = s.create(&o, &git_input("g", bad)).await;
        assert!(matches!(kind(r), ErrorKind::Validation(_)), "{bad}");
    }
    let mut dup = image_input("Dup", "nginx");
    dup.slug = Some("dup".into());
    s.create(&o, &dup).await.unwrap();
    assert_eq!(kind(s.create(&o, &dup).await), ErrorKind::Conflict);
    let mut auto = image_input("Auto", "x");
    auto.source = SourceInput::Auto("git@github.com:org/repo.git".into());
    assert_eq!(
        s.create(&o, &auto).await.unwrap().project.source_kind,
        "git"
    );
    auto.source = SourceInput::Auto("docker run nginx".into());
    assert!(matches!(
        kind(s.create(&o, &auto).await),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
async fn git_project_can_generate_a_deploy_key() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let mut input = git_input("Repo", "git@github.com:org/repo.git");
    input.generate_deploy_key = true;
    let created = core.services.projects.create(&o, &input).await.unwrap();
    let key = created.deploy_key.expect("key");
    assert!(key.public_key.starts_with("ssh-ed25519 "));
    assert_eq!(
        created.project.deploy_key_id.as_deref(),
        Some(key.id.as_str())
    );
    let enc: String = sqlx::query_scalar("SELECT private_key_enc FROM deploy_keys")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert!(enc.starts_with("enc:v1:") && !enc.contains("OPENSSH"));
    // The key cannot be deleted while referenced.
    assert_eq!(
        kind(core.services.deploy_keys.delete(&o, &key.id).await),
        ErrorKind::Conflict
    );
    let mut image = image_input("Img", "nginx");
    image.generate_deploy_key = true;
    assert!(core.services.projects.create(&o, &image).await.is_err());
}

#[tokio::test]
async fn compose_ports_are_derived_loopback_only_and_collisions_are_409() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.commands
        .on("config --format json", Reply::stdout(PORTS_JSON));
    let mut input = image_input("Web", "nginx");
    input.compose_content = Some("services:\n  web:\n    image: nginx\n".into());
    let created = core.services.projects.create(&o, &input).await.unwrap();
    assert_eq!(created.project.ports.len(), 1);
    assert_eq!(created.project.ports[0].host_port, 18080);

    // A second Project declaring the same port collides, naming the owner.
    let mut other = image_input("Other", "nginx");
    other.compose_content = input.compose_content.clone();
    let err = core.services.projects.create(&o, &other).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("project Web"), "{}", err.message);

    // A host listener collides too, but the Project's own declared port never does.
    core.probe.bind(19090, "tcp", Some("nginx"));
    let p = &created.project;
    let set = |host| PortInput {
        label: "web".into(),
        compose_service: Some("web".into()),
        host_port: host,
        container_port: 80,
        bind_address: "127.0.0.1".into(),
        protocol: "tcp".into(),
    };
    let e = core
        .services
        .projects
        .ports_set(&o, &p.id, &[set(19090)])
        .await
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert!(e.message.contains("host process nginx"));
    core.services
        .projects
        .ports_set(&o, &p.id, &[set(18080)])
        .await
        .unwrap();
    let mut public = set(18081);
    public.bind_address = "0.0.0.0".into();
    assert!(matches!(
        kind(core.services.projects.ports_set(&o, &p.id, &[public]).await),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
async fn public_publish_in_compose_is_rejected() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.commands.on(
        "config --format json",
        Reply::stdout(
            r#"{"services":{"web":{"ports":[{"target":80,"published":"8080","protocol":"tcp"}]}}}"#,
        ),
    );
    let mut input = image_input("Web", "nginx");
    input.compose_content = Some("services: {}".into());
    let err = core.services.projects.create(&o, &input).await.unwrap_err();
    assert!(matches!(err.kind, ErrorKind::Validation(_)));
    assert!(err.message.contains("127.0.0.1"));
}

#[tokio::test]
async fn env_is_masked_patched_and_reveal_is_audited() {
    let core = TestCore::new().await;
    let ou = core.setup_owner().await;
    let o = core.session_actor(&ou).await;
    let p = core
        .services
        .projects
        .create(&o, &image_input("E", "nginx"))
        .await
        .unwrap()
        .project;
    let s = &core.services.projects;
    s.env_write(&o, &p.id, "# db\nDB_PASSWORD=hunter2hunter2\nMODE=prod\n")
        .await
        .unwrap();
    let view = s.env_read(&o, &p.id).await.unwrap();
    assert_eq!(view.variables.len(), 2);
    assert!(
        serde_json::to_string(&view)
            .unwrap()
            .find("hunter2")
            .is_none()
    );
    let masked = s.env_content_read(&o, &p.id, false).await.unwrap();
    assert!(!masked.contains("hunter2") && masked.contains("# db"));
    assert_eq!(
        audit_count(&core, "project.env_read", "ok").await,
        0,
        "masked reads are not audited"
    );

    // Saving the masked text back keeps the stored value.
    s.env_write(&o, &p.id, &masked.replace("MODE=********", "MODE=dev"))
        .await
        .unwrap();
    let revealed = s.env_content_read(&o, &p.id, true).await.unwrap();
    assert!(revealed.contains("DB_PASSWORD=hunter2hunter2") && revealed.contains("MODE=dev"));
    assert_eq!(audit_count(&core, "project.env_read", "ok").await, 1);
    let meta: String = sqlx::query_scalar(
        "SELECT group_concat(metadata) FROM audit_logs WHERE action LIKE 'project.env_%'",
    )
    .fetch_one(core.db.reader())
    .await
    .unwrap();
    assert!(
        !meta.contains("hunter2"),
        "audit holds key names only: {meta}"
    );

    use crate::deploy::env::VarChange;
    s.env_variables_write(
        &o,
        &p.id,
        &[
            VarChange {
                key: "NEW".into(),
                value: Some("1".into()),
            },
            VarChange {
                key: "MODE".into(),
                value: None,
            },
        ],
    )
    .await
    .unwrap();
    let keys: Vec<String> = s
        .env_read(&o, &p.id)
        .await
        .unwrap()
        .variables
        .into_iter()
        .map(|v| v.key)
        .collect();
    assert_eq!(keys, ["DB_PASSWORD", "NEW"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(core.config.paths.data_dir.join("projects/e/.env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // A read-scope token cannot reveal; an admin token can.
    let (_, read_tok) = core.token_actor(&ou, Scope::Read).await;
    assert_eq!(
        kind(s.env_content_read(&read_tok, &p.id, true).await),
        ErrorKind::Forbidden
    );
    let (_, admin_tok) = core.token_actor(&ou, Scope::Admin).await;
    assert!(s.env_content_read(&admin_tok, &p.id, true).await.is_ok());
}

#[tokio::test]
async fn deploy_token_reveal_and_rotate() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let created = core
        .services
        .projects
        .create(&o, &image_input("T", "nginx"))
        .await
        .unwrap();
    let id = &created.project.id;
    let revealed = core
        .services
        .projects
        .deploy_token_reveal(&o, id)
        .await
        .unwrap();
    assert_eq!(revealed.expose(), created.deploy_token.expose());
    let rotated = core
        .services
        .projects
        .deploy_token_rotate(&o, id)
        .await
        .unwrap();
    assert_ne!(rotated.expose(), created.deploy_token.expose());
    assert_eq!(
        core.services
            .projects
            .deploy_token_reveal(&o, id)
            .await
            .unwrap()
            .expose(),
        rotated.expose()
    );
    assert_eq!(
        audit_count(&core, "project.deploy_token_reveal", "ok").await,
        2
    );
    assert_eq!(
        audit_count(&core, "project.deploy_token_rotate", "ok").await,
        1
    );
}

#[tokio::test]
async fn update_rules_and_member_permissions() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let p = core
        .services
        .projects
        .create(&o, &image_input("U", "nginx"))
        .await
        .unwrap()
        .project;
    let m = core
        .create_member("m", &[(&p.id, &[Permission::Config])])
        .await;
    let ma = core.session_actor(&m).await;
    let patch = ProjectPatch {
        name: Some("Renamed".into()),
        image_ref: Some("nginx:1.27".into()),
        auto_start: Some(false),
        ..Default::default()
    };
    let up = core
        .services
        .projects
        .update(&ma, &p.id, &patch)
        .await
        .unwrap();
    assert_eq!(
        (up.name.as_str(), up.image_ref.as_deref(), up.auto_start),
        ("Renamed", Some("nginx:1.27"), false)
    );
    // Wrong source kind.
    let bad = ProjectPatch {
        git_url: Some("git@h:r.git".into()),
        ..Default::default()
    };
    assert!(
        core.services
            .projects
            .update(&ma, &p.id, &bad)
            .await
            .is_err()
    );
    // Unknown deploy key.
    let bad = ProjectPatch {
        deploy_key_id: Some(Some("dky_nope".into())),
        ..Default::default()
    };
    assert!(matches!(
        kind(core.services.projects.update(&ma, &p.id, &bad).await),
        ErrorKind::Validation(_)
    ));
    // A Member never creates or deletes Projects.
    assert_eq!(
        kind(
            core.services
                .projects
                .create(&ma, &image_input("N", "x"))
                .await
        ),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(
            core.services
                .projects
                .delete(&ma, &p.id, &DeleteOptions::default())
                .await
        ),
        ErrorKind::Forbidden
    );
    // Listing is scoped to the grants.
    core.services
        .projects
        .create(&o, &image_input("Hidden", "x"))
        .await
        .unwrap();
    assert_eq!(core.services.projects.list(&ma).await.unwrap().len(), 1);
    assert_eq!(core.services.projects.list(&o).await.unwrap().len(), 2);
}

#[tokio::test]
async fn delete_stops_containers_removes_files_and_detaches_audit() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let p = core
        .services
        .projects
        .create(&o, &image_input("Gone", "nginx"))
        .await
        .unwrap()
        .project;
    let dir = core.config.paths.data_dir.join("projects/gone");
    assert!(dir.exists());
    let vol = core
        .services
        .projects
        .delete(
            &o,
            &p.id,
            &DeleteOptions {
                stop_containers: true,
                delete_volumes: true,
                confirm_slug: None,
            },
        )
        .await;
    assert!(
        matches!(kind(vol), ErrorKind::Validation(_)),
        "volumes need the typed slug"
    );
    core.services
        .projects
        .delete(
            &o,
            &p.id,
            &DeleteOptions {
                stop_containers: true,
                delete_volumes: false,
                confirm_slug: None,
            },
        )
        .await
        .unwrap();
    assert!(!dir.exists());
    assert!(
        core.commands
            .calls()
            .iter()
            .any(|c| c == "docker compose -p gone down --remove-orphans")
    );
    let attached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE project_id = ?")
        .bind(&p.id)
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(attached, 0, "audit rows are detached, not deleted");
    assert!(audit_count(&core, "project.create", "ok").await == 1);
}
