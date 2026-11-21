//! Deploy, rollback and Deployment history. Logs are polled (`deployment_logs`), not streamed.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use yanto_core::actor::Scope;
use yanto_core::service::deployments::{DeployRequest, DeploymentQuery, RollbackRequest};

use crate::mcp::{Class, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(
        ToolDef::new(
            "yanto_project_deploy",
            "project_deploy",
            "Queue a Deployment of a Project (optionally of a specific git ref). Returns at once; poll yanto_deployment_get. Queued Deployments run when `yanto serve` is running.",
            Scope::Write,
            Class::Write,
            |s, a, args: DeployArgs| async move {
                let out = s
                    .deployments
                    .deploy(
                        &a,
                        &args.id,
                        &DeployRequest {
                            target_ref: args.target_ref,
                            env: None,
                        },
                    )
                    .await?;
                to_json(&out)
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_rollback_preview",
            "project_rollback_preview",
            "Preview a rollback to an earlier Deployment or git ref. Changes nothing.",
            Scope::Read,
            Class::Read,
            |s, a, args: RollbackArgs| async move {
                let req = RollbackRequest {
                    deployment_id: args.deployment_id,
                    git_ref: args.git_ref,
                };
                to_json(&s.deployments.rollback_preview(&a, &args.id, &req).await?)
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_rollback",
            "project_rollback",
            "Queue a rollback Deployment to an earlier Deployment or git ref.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: RollbackArgs| async move {
                let req = RollbackRequest {
                    deployment_id: args.deployment_id,
                    git_ref: args.git_ref,
                };
                to_json(&s.deployments.rollback(&a, &args.id, &req).await?)
            },
        )
        .confirm()
        .open_world(),
    );
    set.add(ToolDef::new(
        "yanto_deployments_list",
        "deployments_list",
        "List Deployments, newest first, optionally by Project or status.",
        Scope::Read,
        Class::Read,
        |s, a, args: ListArgs| async move {
            let q = DeploymentQuery {
                project_id: args.project_id,
                status: args.status,
                limit: args.limit,
            };
            to_json(&s.deployments.list(&a, &q).await?)
        },
    ));
    set.add(ToolDef::new(
        "yanto_deployment_get",
        "deployment_get",
        "Get one Deployment (status, commit, timing).",
        Scope::Read,
        Class::Read,
        |s, a, args: DeploymentIdArgs| async move {
            to_json(&s.deployments.get(&a, &args.id).await?)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_deployment_logs",
            "deployment_logs",
            "Deployment log text (last `tail` lines). Poll while the Deployment runs.",
            Scope::Read,
            Class::Read,
            |s, a, args: LogsArgs| async move {
                let text = s.deployments.logs(&a, &args.id, args.tail).await?;
                Ok(json!({ "logs": text }))
            },
        )
        .untrusted(),
    );
    set.add(ToolDef::new(
        "yanto_deployment_cancel",
        "deployment_cancel",
        "Cancel a queued or running Deployment.",
        Scope::Write,
        Class::Write,
        |s, a, args: DeploymentIdArgs| async move {
            to_json(&s.deployments.cancel(&a, &args.id).await?)
        },
    ));
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeployArgs {
    /// Project id.
    id: String,
    /// Git ref (branch, tag or commit) to deploy instead of the configured branch.
    target_ref: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RollbackArgs {
    /// Project id.
    id: String,
    /// An earlier successful Deployment to restore (code or image and config).
    deployment_id: Option<String>,
    /// Git Projects only: an explicit ref.
    git_ref: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListArgs {
    project_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeploymentIdArgs {
    /// Deployment id.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogsArgs {
    /// Deployment id.
    id: String,
    tail: Option<usize>,
}
