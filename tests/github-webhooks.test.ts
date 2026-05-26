import { describe, expect, it } from "vitest";
import { githubBranchFromRef, githubPayloadFromRequestBody, githubSignature, projectDeployBranch, verifyGithubSignature } from "../src/server/services/github-webhooks.js";

describe("github webhooks", () => {
  it("verifies GitHub HMAC signatures", () => {
    const body = Buffer.from(JSON.stringify({ ref: "refs/heads/master" }));
    const signature = githubSignature("secret", body);

    expect(verifyGithubSignature("secret", body, signature)).toBe(true);
    expect(verifyGithubSignature("wrong", body, signature)).toBe(false);
    expect(verifyGithubSignature("secret", body, undefined)).toBe(false);
  });

  it("extracts branch names from push refs", () => {
    expect(githubBranchFromRef("refs/heads/main")).toBe("main");
    expect(githubBranchFromRef("refs/heads/feature/one")).toBe("feature/one");
    expect(githubBranchFromRef("refs/tags/v1.0.0")).toBeNull();
  });

  it("falls back to master when the project branch is blank", () => {
    expect(projectDeployBranch({ branch: "main" })).toBe("main");
    expect(projectDeployBranch({ branch: "   " })).toBe("master");
  });

  it("reads GitHub form-encoded payload bodies", () => {
    expect(githubPayloadFromRequestBody({ payload: "{\"ref\":\"refs/heads/master\"}" })).toEqual({ ref: "refs/heads/master" });
    expect(githubPayloadFromRequestBody({ ref: "refs/heads/master" })).toEqual({ ref: "refs/heads/master" });
  });
});
