import { Router } from "express";
import { requireAuth, requireOwner } from "../auth.js";
import { accessibleProjectIds, assertProjectPermission, hasProjectPermission, isOwner } from "../authorization.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { deploymentInput, envInput, envVariablesInput, projectInput, rollbackInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { readProjectCompose } from "../services/compose.js";
import { previewRollbackForProject, rollbackTargetForProject, startDeployment } from "../services/deployments.js";
import { previewEnvContent, previewProjectEnv, readProjectEnv, readProjectEnvVariables, writeProjectEnv, writeProjectEnvVariables } from "../services/project-env.js";
import type { PendingDeploymentEnv } from "../services/deployment-runner.js";
import { restartProjectCompose, stopProjectCompose } from "../services/project-runtime.js";
import { createProject, deleteProject, getProject, getProjectDeployToken, listProjectsWithContainerCounts, publicProject, updateProject } from "../services/projects.js";
import { config } from "../config.js";

const router = Router();

router.get(
  "/api/projects",
  requireAuth,
  asyncRoute(async (req, res) => {
    const rows = await listProjectsWithContainerCounts();
    const projectIds = accessibleProjectIds(req);
    const folderCounts = new Map<string, number>();
    for (const project of rows) folderCounts.set(project.folderName, (folderCounts.get(project.folderName) ?? 0) + 1);
    res.json(rows.filter((project) => projectIds === null || projectIds.has(project.id)).map((project) => {
      const view = publicProject(project);
      const ambiguousContainerMapping = (folderCounts.get(project.folderName) ?? 0) > 1;
      return {
        ...view,
        ...(!hasProjectPermission(req, project.id, "config") ? { composeContent: null } : {}),
        ...(!hasProjectPermission(req, project.id, "hostnames") ? { cloudflareRoutes: [] } : {}),
        ...(ambiguousContainerMapping && !isOwner(req) ? { containerCount: 0 } : {}),
        ...(ambiguousContainerMapping && isOwner(req)
          ? { containerMappingWarning: "Multiple projects use this folder; delegated container access is disabled until folder names are unique." }
          : {})
      };
    }));
  })
);

router.post(
  "/api/projects",
  requireOwner,
  asyncRoute(async (req, res) => {
    const body = projectInput.parse(req.body);
    const project = await createProject(body);
    await recordAuditLog({ actor: actor(req), action: "project.create", entityType: "project", entityId: project.id, projectId: project.id });
    res.status(201).json({ ...publicProject(project), deployToken: project.deployToken });
  })
);

router.patch(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.partial().parse(req.body);
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "config");
    if (!isOwner(req) && (body.folderName !== undefined || body.targetNodeId !== undefined)) {
      res.status(403).json({ message: "Only the owner can change a project's folder or target node." });
      return;
    }
    const project = await updateProject(projectId, body);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "project.update", entityType: "project", entityId: project.id, projectId: project.id });
    res.json(publicProject(project));
  })
);

router.get(
  "/api/projects/:id/deploy-token",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    assertProjectPermission(req, id, "secrets");
    const deployToken = await getProjectDeployToken(id);
    if (!deployToken) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "project.deploy_token.reveal", entityType: "project", entityId: id, projectId: id });
    res.json({ deployToken });
  })
);

router.delete(
  "/api/projects/:id",
  requireOwner,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await deleteProject(id);
    await recordAuditLog({ actor: actor(req), action: "project.delete", entityType: "project", entityId: id });
    res.status(204).end();
  })
);

router.post(
  "/api/projects/:id/deploy",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = deploymentInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "deploy");
    if (body.envContent !== undefined || body.envVariables !== undefined) {
      assertProjectPermission(req, projectId, "secrets");
    }
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (!project.manualDeployEnabled) {
      res.status(403).json({ message: "Manual deployments are disabled for this project." });
      return;
    }
    const pendingEnv: PendingDeploymentEnv | undefined =
      body.envContent !== undefined
        ? { mode: "text", content: body.envContent, envFile: body.envFile }
        : body.envVariables
          ? { mode: "variables", variables: body.envVariables, envFile: body.envFile }
          : undefined;
    const result = await startDeployment(projectId, "manual", { targetRef: body.targetRef, pendingEnv });
    await recordAuditLog({
      actor: actor(req),
      action: "deployment.start",
      entityType: "deployment",
      entityId: result.deployment.id,
      projectId,
      metadata: { trigger: "manual", targetRef: body.targetRef ?? null, reused: result.reused }
    });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

router.post(
  "/api/projects/:id/rollback/preview",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = rollbackInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "deploy");
    const preview = await previewRollbackForProject(projectId, body.targetRef ?? "");
    res.json(preview);
  })
);

router.post(
  "/api/projects/:id/rollback",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = rollbackInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "deploy");
    const target = await rollbackTargetForProject(projectId, body.deploymentId, body.targetRef);
    const result = await startDeployment(projectId, "rollback", {
      targetRef: target.targetRef,
      rollbackFromDeploymentId: target.rollbackFromDeploymentId ?? undefined
    });
    await recordAuditLog({
      actor: actor(req),
      action: "deployment.rollback",
      entityType: "deployment",
      entityId: result.deployment.id,
      projectId,
      metadata: { targetRef: target.targetRef, rollbackFromDeploymentId: target.rollbackFromDeploymentId, reused: result.reused }
    });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

router.get(
  "/api/projects/:id/compose/content",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "config");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await readProjectCompose(project));
  })
);

router.get(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "secrets");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await readProjectEnvVariables(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

router.get(
  "/api/projects/:id/env/content",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "secrets");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await readProjectEnv(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

router.get(
  "/api/projects/:id/env/preview",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "secrets");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await previewProjectEnv(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

router.post(
  "/api/projects/:id/env/preview",
  requireAuth,
  asyncRoute(async (req, res) => {
    assertProjectPermission(req, routeParam(req, "id"), "secrets");
    const body = envInput.parse(req.body);
    res.json(previewEnvContent(body.content, body.envFile ?? ".env"));
  })
);

router.put(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "secrets");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    const body = envInput.parse(req.body);
    const preview = await writeProjectEnv(project, body.content, body.envFile);
    await recordAuditLog({
      actor: actor(req),
      action: "project.env.write",
      entityType: "project",
      entityId: project.id,
      projectId: project.id,
      metadata: { envFile: preview.envFile, entryCount: preview.entryCount }
    });
    res.json(preview);
  })
);

router.patch(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "secrets");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    const body = envVariablesInput.parse(req.body);
    const preview = await writeProjectEnvVariables(project, body.variables, body.envFile);
    await recordAuditLog({
      actor: actor(req),
      action: "project.env.write",
      entityType: "project",
      entityId: project.id,
      projectId: project.id,
      metadata: { envFile: preview.envFile, entryCount: preview.entryCount }
    });
    res.json({ ok: true, preview });
  })
);

router.post(
  "/api/projects/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "runtime");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (project.targetNodeId !== config.localNodeId) {
      res.status(400).json({ message: "Stopping projects on worker nodes is not supported yet." });
      return;
    }
    const logs = await stopProjectCompose(project);
    await recordAuditLog({ actor: actor(req), action: "project.compose.stop", entityType: "project", entityId: project.id, projectId: project.id });
    res.json({ ok: true, logs });
  })
);

router.post(
  "/api/projects/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const projectId = routeParam(req, "id");
    assertProjectPermission(req, projectId, "runtime");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (project.targetNodeId !== config.localNodeId) {
      res.status(400).json({ message: "Restarting projects on worker nodes is not supported yet." });
      return;
    }
    const logs = await restartProjectCompose(project);
    await recordAuditLog({ actor: actor(req), action: "project.compose.restart", entityType: "project", entityId: project.id, projectId: project.id });
    res.json({ ok: true, logs });
  })
);

export default router;
