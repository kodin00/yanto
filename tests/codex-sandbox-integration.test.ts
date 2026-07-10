import { describe, it } from "vitest";
import { probeCodexSandbox } from "../src/server/services/codex-sandbox-probe.js";

describe("Codex runner image credential isolation", () => {
  const integration = process.env.YANTO_RUN_CODEX_SANDBOX_INTEGRATION === "1" ? it : it.skip;

  integration("blocks credential/session reads, modification, symlink bypasses, subprocesses, scripts, and network exfiltration", async () => {
    await probeCodexSandbox(process.env.AGENT_DEFAULT_IMAGE);
  }, 120_000);
});
