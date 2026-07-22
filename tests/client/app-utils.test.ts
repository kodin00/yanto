import { describe, expect, it } from "vitest";
import {
  bytes,
  cloudflareAssignmentTcpPorts,
  cloudflareServiceUrl,
  containerTcpPorts,
  dateTime,
  deploymentChanges,
  durationBetween,
  durationSince,
  endpoint,
  githubRepoNameFromUrl,
  githubWebhookEndpoint,
  isProtectedYantoContainer,
  normalizeEnvRows,
  pageItems,
  pageSize,
  projectNameFromSource,
  slugifyFolderName,
  totalPages,
  usedMemoryMb
} from "../../src/client/app-utils";
import type { CloudflareTunnelAssignment, ContainerInfo, Deployment, Project } from "../../src/shared/types";

describe("bytes", () => {
  it("formats zero bytes", () => {
    expect(bytes(0)).toBe("0 B");
  });

  it("formats bytes under 1 KB", () => {
    expect(bytes(512)).toBe("512 B");
  });

  it("formats kilobytes", () => {
    expect(bytes(1024)).toBe("1.0 KB");
    expect(bytes(1536)).toBe("1.5 KB");
  });

  it("formats megabytes", () => {
    expect(bytes(1024 * 1024)).toBe("1.0 MB");
  });

  it("formats gigabytes", () => {
    expect(bytes(1024 ** 3)).toBe("1.0 GB");
  });

  it("formats terabytes", () => {
    expect(bytes(1024 ** 4)).toBe("1.0 TB");
  });

  it("stays at TB for very large values", () => {
    expect(bytes(1024 ** 5)).toBe("1024.0 TB");
  });
});

describe("dateTime", () => {
  it("returns dash for null", () => {
    expect(dateTime(null)).toBe("-");
  });

  it("returns dash for empty string", () => {
    expect(dateTime("")).toBe("-");
  });

  it("formats a valid date string", () => {
    const result = dateTime("2024-01-15T10:30:00Z");
    expect(result).not.toBe("-");
    expect(result.length).toBeGreaterThan(0);
  });
});

describe("durationSince", () => {
  it("returns dash for null", () => {
    expect(durationSince(null)).toBe("-");
  });

  it("returns dash for invalid date", () => {
    expect(durationSince("not-a-date")).toBe("-");
  });

  it("returns seconds for very recent time", () => {
    const now = new Date().toISOString();
    expect(durationSince(now)).toBe("0s");
  });

  it("returns minutes for a time 5 min ago", () => {
    const fiveMinAgo = new Date(Date.now() - 5 * 60 * 1000).toISOString();
    expect(durationSince(fiveMinAgo)).toBe("5m");
  });

  it("returns hours and minutes", () => {
    const twoHoursAgo = new Date(Date.now() - 2 * 3600 * 1000 - 30 * 60 * 1000).toISOString();
    expect(durationSince(twoHoursAgo)).toBe("2h 30m");
  });
});

describe("durationBetween", () => {
  it("returns dash when startedAt is null", () => {
    expect(durationBetween(null, null)).toBe("-");
  });

  it("returns seconds for short durations", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T00:00:45Z")).toBe("45s");
  });

  it("returns minutes and seconds", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T00:02:30Z")).toBe("2m 30s");
  });

  it("returns just minutes when no remaining seconds", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T00:05:00Z")).toBe("5m");
  });

  it("returns hours and minutes", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T01:30:00Z")).toBe("1h 30m");
  });

  it("returns just hours when no remaining minutes", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T02:00:00Z")).toBe("2h");
  });

  it("returns dash for invalid dates", () => {
    expect(durationBetween("invalid", "2024-01-01T00:00:00Z")).toBe("-");
    expect(durationBetween("2024-01-01T00:00:00Z", "invalid")).toBe("-");
  });

  it("returns 0s when start equals end", () => {
    expect(durationBetween("2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z")).toBe("0s");
  });
});

describe("endpoint", () => {
  const project = { id: "proj-123" } as Project;

  it("builds deploy endpoint URL", () => {
    expect(endpoint(project, "http://localhost:3000")).toBe("http://localhost:3000/deploy?id=proj-123");
  });

  it("strips trailing slash from base URL", () => {
    expect(endpoint(project, "http://localhost:3000/")).toBe("http://localhost:3000/deploy?id=proj-123");
  });
});

describe("githubWebhookEndpoint", () => {
  const project = { id: "proj-456" } as Project;

  it("builds webhook URL", () => {
    expect(githubWebhookEndpoint(project, "https://example.com")).toBe("https://example.com/webhooks/github?id=proj-456");
  });

  it("strips trailing slash", () => {
    expect(githubWebhookEndpoint(project, "https://example.com/")).toBe("https://example.com/webhooks/github?id=proj-456");
  });
});

describe("githubRepoNameFromUrl", () => {
  it("extracts repo name from GitHub SSH URL", () => {
    expect(githubRepoNameFromUrl("git@github.com:kodin00/envchecker.git")).toBe("envchecker");
  });

  it("extracts repo name from GitHub HTTPS URL", () => {
    expect(githubRepoNameFromUrl("https://github.com/kodin00/envchecker.git")).toBe("envchecker");
  });

  it("returns empty for unsupported input", () => {
    expect(githubRepoNameFromUrl("")).toBe("");
    expect(githubRepoNameFromUrl("https://gitlab.com/kodin00/envchecker.git")).toBe("");
    expect(githubRepoNameFromUrl("not a url")).toBe("");
  });
});

describe("projectNameFromSource", () => {
  it("extracts a project name from a Docker pull command", () => {
    expect(projectNameFromSource("docker pull ghcr.io/hedypamungkas/koboi-agent:0.18.7")).toBe("koboi-agent");
  });

  it("accepts a bare Docker image reference", () => {
    expect(projectNameFromSource("ghcr.io/hedypamungkas/koboi-agent:0.18.7")).toBe("koboi-agent");
  });

  it("rejects commands with extra shell input", () => {
    expect(projectNameFromSource("docker pull nginx:latest && touch /tmp/untrusted")).toBe("");
  });
});

describe("slugifyFolderName", () => {
  it("lowercases and strips special chars", () => {
    expect(slugifyFolderName("My Project")).toBe("my-project");
  });

  it("handles leading/trailing dashes", () => {
    expect(slugifyFolderName("--hello--")).toBe("hello");
  });

  it("handles empty string", () => {
    expect(slugifyFolderName("")).toBe("");
  });

  it("preserves dots and underscores", () => {
    expect(slugifyFolderName("my.project_name")).toBe("my.project_name");
  });

  it("truncates to 64 characters", () => {
    const long = "a".repeat(100);
    expect(slugifyFolderName(long)).toHaveLength(64);
  });

  it("trims whitespace", () => {
    expect(slugifyFolderName("  hello  ")).toBe("hello");
  });
});

describe("pageItems", () => {
  const items = Array.from({ length: 25 }, (_, i) => i);

  it("returns first page", () => {
    const result = pageItems(items, 1);
    expect(result).toHaveLength(pageSize);
    expect(result[0]).toBe(0);
  });

  it("returns second page", () => {
    const result = pageItems(items, 2);
    expect(result).toHaveLength(pageSize);
    expect(result[0]).toBe(10);
  });

  it("returns partial last page", () => {
    const result = pageItems(items, 3);
    expect(result).toHaveLength(5);
    expect(result[0]).toBe(20);
  });

  it("returns empty for out-of-range page", () => {
    expect(pageItems(items, 10)).toHaveLength(0);
  });
});

describe("totalPages", () => {
  it("returns 1 for empty array", () => {
    expect(totalPages([])).toBe(1);
  });

  it("returns 1 for items within one page", () => {
    expect(totalPages(new Array(5))).toBe(1);
  });

  it("returns correct pages for exact multiple", () => {
    expect(totalPages(new Array(20))).toBe(2);
  });

  it("rounds up for partial page", () => {
    expect(totalPages(new Array(11))).toBe(2);
  });
});

describe("usedMemoryMb", () => {
  it("returns dash for dash value", () => {
    expect(usedMemoryMb("- / 512MB")).toBe("-");
  });

  it("parses MB value", () => {
    expect(usedMemoryMb("256MB / 512MB")).toBe("256 MB");
  });

  it("converts GB to MB", () => {
    expect(usedMemoryMb("2GB / 4GB")).toBe("2048 MB");
  });

  it("converts KB to MB", () => {
    expect(usedMemoryMb("512KB / 1GB")).toBe("0.5 MB");
  });

  it("handles MiB notation", () => {
    expect(usedMemoryMb("128MiB / 512MiB")).toBe("128 MB");
  });
});

describe("isProtectedYantoContainer", () => {
  it("matches yanto-app container", () => {
    expect(isProtectedYantoContainer({ name: "yanto-app-1" } as ContainerInfo)).toBe(true);
  });

  it("matches yanto-postgres container", () => {
    expect(isProtectedYantoContainer({ name: "yanto-postgres-1" } as ContainerInfo)).toBe(true);
  });

  it("does not match other containers", () => {
    expect(isProtectedYantoContainer({ name: "my-app-1" } as ContainerInfo)).toBe(false);
  });

  it("does not match partial names", () => {
    expect(isProtectedYantoContainer({ name: "yanto-redis-1" } as ContainerInfo)).toBe(false);
  });
});

describe("cloudflareServiceUrl", () => {
  const makeContainer = (overrides: Partial<ContainerInfo>): ContainerInfo =>
    ({
      name: "test-app-1",
      composeProject: "myproject",
      composeService: "app",
      state: "running",
      ports: "0.0.0.0:3000->3000/tcp",
      isPostgresCandidate: false,
      ...overrides
    }) as ContainerInfo;

  it("returns empty string when no matching containers", () => {
    const project = { folderName: "myproject" } as Project;
    expect(cloudflareServiceUrl(project, [])).toBe("");
  });

  it("builds URL from running container port", () => {
    const project = { folderName: "myproject" } as Project;
    const containers = [makeContainer({})];
    expect(cloudflareServiceUrl(project, containers)).toBe("http://test-app-1:3000");
  });

  it("ignores containers from other projects", () => {
    const project = { folderName: "other" } as Project;
    const containers = [makeContainer({})];
    expect(cloudflareServiceUrl(project, containers)).toBe("");
  });

  it("ignores non-running containers", () => {
    const project = { folderName: "myproject" } as Project;
    const containers = [makeContainer({ state: "exited" })];
    expect(cloudflareServiceUrl(project, containers)).toBe("");
  });

  it("ignores postgres candidates", () => {
    const project = { folderName: "myproject" } as Project;
    const containers = [makeContainer({ isPostgresCandidate: true })];
    expect(cloudflareServiceUrl(project, containers)).toBe("");
  });

  it("prefers app service over others", () => {
    const project = { folderName: "myproject" } as Project;
    const containers = [
      makeContainer({ name: "worker-1", composeService: "worker", ports: "0.0.0.0:4000->4000/tcp" }),
      makeContainer({ name: "app-1", composeService: "app", ports: "0.0.0.0:3000->3000/tcp" })
    ];
    expect(cloudflareServiceUrl(project, containers)).toBe("http://app-1:3000");
  });
});

describe("Cloudflare target ports", () => {
  const composeAssignment = {
    targetType: "compose_service",
    composeProject: "shop",
    composeService: "web",
    containerName: null
  } as CloudflareTunnelAssignment;

  it("extracts unique internal TCP ports from Docker port output", () => {
    expect(containerTcpPorts("0.0.0.0:8080->3000/tcp, [::]:8080->3000/tcp, 3001/tcp, 53/udp")).toEqual([3000, 3001]);
  });

  it("expands bounded TCP port ranges", () => {
    expect(containerTcpPorts("4000-4002/tcp")).toEqual([4000, 4001, 4002]);
  });

  it("finds ports for every replica of an assigned Compose service", () => {
    const containers = [
      { name: "shop-web-1", composeProject: "shop", composeService: "web", ports: "3000/tcp" },
      { name: "shop-web-2", composeProject: "shop", composeService: "web", ports: "3000/tcp, 3001/tcp" },
      { name: "shop-worker-1", composeProject: "shop", composeService: "worker", ports: "9000/tcp" }
    ] as ContainerInfo[];
    expect(cloudflareAssignmentTcpPorts(composeAssignment, containers)).toEqual([3000, 3001]);
  });

  it("finds ports for a directly assigned container", () => {
    const assignment = { targetType: "container", containerName: "standalone" } as CloudflareTunnelAssignment;
    const containers = [
      { name: "standalone", ports: "8080/tcp" },
      { name: "another", ports: "9000/tcp" }
    ] as ContainerInfo[];
    expect(cloudflareAssignmentTcpPorts(assignment, containers)).toEqual([8080]);
  });
});

describe("normalizeEnvRows", () => {
  it("sorts by key and normalizes values", () => {
    const rows = [
      { key: "B_VAR", value: "hello", masked: true },
      { key: "A_VAR", value: null, masked: false }
    ];
    const result = normalizeEnvRows(rows);
    expect(result).toEqual([
      { key: "A_VAR", value: "" },
      { key: "B_VAR", value: "hello" }
    ]);
  });

  it("returns empty array for empty input", () => {
    expect(normalizeEnvRows([])).toEqual([]);
  });
});

describe("deploymentChanges", () => {
  it("joins array changes", () => {
    const d = { changes: ["fix auth", "update deps"] } as unknown as Deployment;
    expect(deploymentChanges(d)).toBe("fix auth, update deps");
  });

  it("returns string changes directly", () => {
    const d = { changes: "hotfix" } as unknown as Deployment;
    expect(deploymentChanges(d)).toBe("hotfix");
  });

  it("returns truncated commitSha", () => {
    const d = { commitSha: "abcdef1234567890abcdef" } as unknown as Deployment;
    expect(deploymentChanges(d)).toBe("abcdef123456");
  });

  it("returns Running when exitCode is null", () => {
    const d = { exitCode: null } as unknown as Deployment;
    expect(deploymentChanges(d)).toBe("Running");
  });

  it("returns Exit code when exitCode is set", () => {
    const d = { exitCode: 1 } as unknown as Deployment;
    expect(deploymentChanges(d)).toBe("Exit 1");
  });
});
