import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CloudflareRoute, Project, SessionUser } from "../../src/shared/types";
import { api } from "../../src/client/lib/api";
import { HostnamesView } from "../../src/client/views/HostnamesView";
import { ProjectsView } from "../../src/client/views/ProjectsView";
import type { SettingsState } from "../../src/client/views/types";

const project: Project = {
  id: "prj_1",
  name: "Web app",
  gitUrl: "https://github.com/example/web-app.git",
  dockerImage: "",
  branch: "main",
  folderName: "web-app",
  localPath: "/projects/web-app",
  composeFile: "compose.yml",
  composeContent: null,
  envFile: ".env",
  autoStart: true,
  manualDeployEnabled: true,
  githubWebhookEnabled: false,
  targetNodeId: "node_master_local",
  sshPublicKey: null,
  agentImage: "",
  createdAt: "2026-01-01T00:00:00.000Z",
  updatedAt: "2026-01-01T00:00:00.000Z"
};

const session: SessionUser = {
  id: "usr_owner",
  username: "owner",
  role: "owner",
  status: "active",
  projectAccess: [],
  allowedViews: ["projects"],
  appBaseUrl: "http://localhost:3000",
  localNodeId: "node_master_local"
};

const settings = {
  appBaseUrl: "http://localhost:3000",
  cf: { accountId: "", zoneId: "", hasApiToken: false }
} as SettingsState;

afterEach(() => vi.restoreAllMocks());

describe("project interactions", () => {
  it("uses a dedicated Edit button without making the card a nested fake button", async () => {
    const openProject = vi.fn();
    const deploy = vi.fn();
    const { container } = render(
      <ProjectsView
        session={session}
        visibleProjects={[project]}
        projects={[project]}
        nodes={[{ id: "node_master_local", name: "Master", role: "master", status: "online" }]}
        containersByProjectFolder={new Map()}
        cfRoutesByProject={{}}
        routeDiagnosticsByRouteId={{}}
        latestDeploymentByProject={new Map()}
        settings={settings}
        busy={null}
        projectPage={1}
        openProject={openProject}
        openRollback={vi.fn()}
        deploy={deploy}
        copyText={vi.fn()}
        copyDeployToken={vi.fn()}
        setConfirm={vi.fn()}
        refreshProjects={vi.fn()}
        setProjectPage={vi.fn()}
      />
    );

    const card = container.querySelector("article.project-card");
    expect(card).not.toHaveAttribute("role");
    expect(card).not.toHaveAttribute("tabindex");

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Deploy" }));
    expect(deploy).toHaveBeenCalledWith(project);
    expect(openProject).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(openProject).toHaveBeenCalledWith(project);
  });
});

describe("member hostname loading", () => {
  it("loads the aggregate hostname endpoint once and groups routes by project", async () => {
    const route: CloudflareRoute = {
      id: "route_1",
      tunnelId: "tunnel_1",
      projectId: project.id,
      assignmentId: "assignment_1",
      zoneId: "zone_1",
      hostname: "app.example.com",
      serviceTarget: "http://web:3000",
      protocol: "http",
      port: 3000,
      noTlsVerify: false,
      enabled: true,
      syncStatus: "active",
      lastError: null,
      cfDnsRecordId: "dns_1",
      lastPublishedAt: "2026-01-01T00:00:00.000Z",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z"
    };
    const aggregate = vi.spyOn(api, "cloudflareHostnames").mockResolvedValue([route]);
    const perProject = vi.spyOn(api, "projectCfRoutes").mockResolvedValue([]);

    render(<HostnamesView projects={[project]} containers={[]} refreshKey={0} isOwner={false} toast={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("app.example.com")).toBeInTheDocument());
    expect(aggregate).toHaveBeenCalledTimes(1);
    expect(perProject).not.toHaveBeenCalled();
  });
});
