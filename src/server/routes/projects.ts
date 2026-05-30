import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { deploymentInput, envInput, envVariablesInput, projectInput, rollbackInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { readProjectCompose } from "../services/compose.js";
import { rollbackTargetForProject, startDeployment } from "../services/deployments.js";
import { previewEnvContent, previewProjectEnv, readProjectEnv, readProjectEnvVariables, writeProjectEnv, writeProjectEnvVariables } from "../services/project-env.js";
import { restartProjectCompose, stopProjectCompose } from "../services/project-runtime.js";
import { createProject, deleteProject, getProject, listProjectsWithRoutes, publicProject, updateProject } from "../services/projects.js";
import { config } from "../config.js";

const router = Router();

router.get(
  "/api/projects",
  requireAuth,
  asyncRoute(async (_req, res) => {
    const rows = await listProjectsWithRoutes();
    res.json(rows.map(publicProject));
  })
);

router.post(
  "/api/projects",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.parse(req.body);
    const project = await createProject(body);
    await recordAuditLog({ actor: actor(req), action: "project.create", entityType: "project", entityId: project.id, projectId: project.id });
    res.status(201).json(project);
  })
);

router.patch(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.partial().parse(req.body);
    const project = await updateProject(routeParam(req, "id"), body);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "project.update", entityType: "project", entityId: project.id, projectId: project.id });
    res.json(publicProject(project));
  })
);

router.delete(
  "/api/projects/:id",
  requireAuth,
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
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (!project.manualDeployEnabled) {
      res.status(403).json({ message: "Manual deployments are disabled for this project." });
      return;
    }
    const result = await startDeployment(projectId, "manual", body);
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
  "/api/projects/:id/rollback",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = rollbackInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
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
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
    const body = envInput.parse(req.body);
    res.json(previewEnvContent(body.content, body.envFile ?? ".env"));
  })
);

router.put(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
    const project = await getProject(routeParam(req, "id"));
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
