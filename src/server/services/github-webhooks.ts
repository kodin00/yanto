import crypto from "node:crypto";
import { z } from "zod";
import type { ProjectRow } from "../db/schema.js";
import { constantTimeEqual } from "./tokens.js";

export const githubWebhookPayloadInput = z
  .object({
    ref: z.string().optional(),
    deleted: z.boolean().optional(),
    after: z.string().optional(),
    repository: z
      .object({
        name: z.string().optional(),
        full_name: z.string().optional(),
        default_branch: z.string().optional(),
        clone_url: z.string().optional(),
        ssh_url: z.string().optional()
      })
      .passthrough()
      .optional()
  })
  .passthrough();

export type GithubWebhookPayload = z.infer<typeof githubWebhookPayloadInput>;

export function githubSignature(secret: string, rawBody: Buffer) {
  return `sha256=${crypto.createHmac("sha256", secret).update(rawBody).digest("hex")}`;
}

export function verifyGithubSignature(secret: string, rawBody: Buffer | undefined, signature: string | undefined) {
  if (!rawBody || !signature?.startsWith("sha256=")) {
    return false;
  }
  return constantTimeEqual(githubSignature(secret, rawBody), signature);
}

export function githubBranchFromRef(ref: string | undefined) {
  const prefix = "refs/heads/";
  if (!ref?.startsWith(prefix)) {
    return null;
  }
  return ref.slice(prefix.length);
}

export function projectDeployBranch(project: Pick<ProjectRow, "branch">) {
  return project.branch.trim() || "master";
}

export function githubPayloadFromRequestBody(body: unknown) {
  if (body && typeof body === "object" && "payload" in body && typeof body.payload === "string") {
    try {
      return JSON.parse(body.payload) as unknown;
    } catch {
      throw new Error("Invalid JSON in webhook payload field");
    }
  }
  return body;
}
