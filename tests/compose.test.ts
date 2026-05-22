import { describe, expect, it } from "vitest";
import YAML from "yaml";
import { autoStartOverrideFile, buildAutoStartOverride } from "../src/server/services/compose.js";

describe("compose helpers", () => {
  it("builds a restart override for each service", () => {
    const override = YAML.parse(
      buildAutoStartOverride(`
services:
  web:
    build: .
  worker:
    image: node:22
`)
    );

    expect(autoStartOverrideFile()).toBe(".yanto.restart.override.yml");
    expect(override).toEqual({
      services: {
        web: { restart: "unless-stopped" },
        worker: { restart: "unless-stopped" }
      }
    });
  });

  it("rejects compose files without services", () => {
    expect(() => buildAutoStartOverride("volumes:\n  data:\n")).toThrow("services object");
  });
});
