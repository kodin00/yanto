import fs from "node:fs";
import { describe, expect, it } from "vitest";
import YAML from "yaml";

describe("deployment workflow", () => {
  const source = fs.readFileSync(".github/workflows/deploy.yml", "utf8");
  const workflow = YAML.parse(source);

  it("uses a finite test command and serializes remote deployments", () => {
    const testStep = workflow.jobs.verify.steps.find((step: { name?: string }) => step.name === "Test");
    expect(testStep.run).toBe("npm run test:run");
    expect(workflow.concurrency["cancel-in-progress"]).toBe(false);
  });

  it("deploys the verified commit and waits for service health", () => {
    const deployStep = workflow.jobs.deploy.steps.find((step: { name?: string }) => step.name === "Deploy");
    expect(deployStep.env.DEPLOY_SHA).toContain("github.sha");
    expect(deployStep.run).toContain("git checkout --detach");
    expect(deployStep.run).toContain("--wait --wait-timeout 180");
  });
});
