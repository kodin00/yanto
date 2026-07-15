import { Check, ChevronDown, Copy, FolderKanban, KeyRound, Plus, RefreshCw, ShieldCheck, Trash2, UserRoundCog } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ManagedUser, Project, ProjectPermission, UserProjectAccess } from "../../shared/types";
import { api } from "../lib/api";
import { Button, StatusBadge, TextField } from "./ui";

type AccessPreset = "viewer" | "operator" | "manager" | "custom";
type DraftAccess = Record<string, ProjectPermission[]>;

const permissions: Array<{ id: ProjectPermission; label: string }> = [
  { id: "deploy", label: "Deploy" },
  { id: "runtime", label: "Runtime" },
  { id: "config", label: "Config" },
  { id: "secrets", label: "Secrets" },
  { id: "backups", label: "Backups" },
  { id: "hostnames", label: "Hostnames" },
];

const presetPermissions: Record<Exclude<AccessPreset, "custom">, ProjectPermission[]> = {
  viewer: [],
  operator: ["deploy", "runtime", "backups"],
  manager: permissions.map((permission) => permission.id),
};

function accessToDraft(access: UserProjectAccess[]): DraftAccess {
  return Object.fromEntries(access.map((entry) => [entry.projectId, entry.permissions.filter((permission) => permission !== "tasks")]));
}

function draftToAccess(draft: DraftAccess, projects: Project[]): UserProjectAccess[] {
  return projects
    .filter((project) => project.id in draft)
    .map((project) => ({ projectId: project.id, projectName: project.name, permissions: (draft[project.id] ?? []).filter((permission) => permission !== "tasks") }));
}

function detectPreset(values: ProjectPermission[]): AccessPreset {
  const normalized = [...values].sort().join(",");
  for (const preset of ["viewer", "operator", "manager"] as const) {
    if ([...presetPermissions[preset]].sort().join(",") === normalized) return preset;
  }
  return "custom";
}

function permissionLabels(access: UserProjectAccess[]) {
  const granted = new Set(access.flatMap((entry) => entry.permissions));
  return permissions.filter((permission) => granted.has(permission.id)).map((permission) => permission.label);
}

function ProjectAccessEditor({ projects, value, onChange }: { projects: Project[]; value: DraftAccess; onChange: (value: DraftAccess) => void }) {
  const selectedCount = Object.keys(value).length;

  return (
    <div className="access-editor">
      <div className="access-editor-head">
        <strong>Project access</strong>
        <span>{selectedCount} assigned</span>
      </div>
      <div className="access-project-list">
        {projects.map((project) => {
          const assigned = project.id in value;
          const selectedPermissions = value[project.id] ?? [];
          const preset = detectPreset(selectedPermissions);
          return (
            <article className={`access-project-row ${assigned ? "assigned" : ""}`} key={project.id}>
              <label className="access-project-toggle">
                <input
                  type="checkbox"
                  checked={assigned}
                  onChange={(event) => {
                    const next = { ...value };
                    if (event.target.checked) next[project.id] = [];
                    else delete next[project.id];
                    onChange(next);
                  }}
                />
                <span className="task-file-check checked-mark">{assigned ? <Check size={12} /> : null}</span>
                <span><strong>{project.name}</strong><small>{project.folderName}</small></span>
              </label>
              {assigned ? (
                <div className="access-project-controls">
                  <div className="access-presets" aria-label={`Access preset for ${project.name}`}>
                    {(["viewer", "operator", "manager"] as const).map((id) => (
                      <button key={id} type="button" className={preset === id ? "active" : ""} onClick={() => onChange({ ...value, [project.id]: presetPermissions[id] })}>
                        {id[0].toUpperCase() + id.slice(1)}
                      </button>
                    ))}
                    <span className={preset === "custom" ? "active" : ""}>Custom</span>
                  </div>
                  <div className="access-permission-grid">
                    {permissions.map((permission) => (
                      <label key={permission.id}>
                        <input
                          type="checkbox"
                          checked={selectedPermissions.includes(permission.id)}
                          onChange={(event) => {
                            const nextPermissions = event.target.checked
                              ? [...selectedPermissions, permission.id]
                              : selectedPermissions.filter((entry) => entry !== permission.id);
                            onChange({ ...value, [project.id]: nextPermissions });
                          }}
                        />
                        <span>{permission.label}</span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : null}
            </article>
          );
        })}
        {!projects.length ? <p className="muted">Add a project before inviting a delegated user.</p> : null}
      </div>
    </div>
  );
}

export function UserAccessPanel({ projects, currentUserId, copyText }: { projects: Project[]; currentUserId: string; copyText: (value: string) => Promise<void> }) {
  const [users, setUsers] = useState<ManagedUser[]>([]);
  const [username, setUsername] = useState("");
  const [newAccess, setNewAccess] = useState<DraftAccess>({});
  const [drafts, setDrafts] = useState<Record<string, DraftAccess>>({});
  const [secretLink, setSecretLink] = useState<{ title: string; url: string } | null>(null);
  const [expandedUserId, setExpandedUserId] = useState<string | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const rows = await api.users();
      setUsers(rows);
      setDrafts(Object.fromEntries(rows.map((user) => [user.id, accessToDraft(user.projectAccess)])));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to load users.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  const members = useMemo(() => users.filter((user) => user.role === "member"), [users]);
  const owner = useMemo(() => users.find((user) => user.role === "owner"), [users]);

  async function invite() {
    if (!username.trim() || !Object.keys(newAccess).length) return;
    setBusy("invite");
    setError(null);
    try {
      const result = await api.createUser(username.trim(), draftToAccess(newAccess, projects));
      setUsers((current) => [...current, result.user]);
      setDrafts((current) => ({ ...current, [result.user.id]: accessToDraft(result.user.projectAccess) }));
      setSecretLink({ title: `Setup link for ${result.user.username}`, url: result.setupUrl });
      setUsername("");
      setNewAccess({});
      setExpandedUserId(result.user.id);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to create user.");
    } finally {
      setBusy(null);
    }
  }

  async function saveAccess(user: ManagedUser) {
    setBusy(`access:${user.id}`);
    setError(null);
    try {
      const updated = await api.replaceUserAccess(user.id, draftToAccess(drafts[user.id] ?? {}, projects));
      setUsers((current) => current.map((entry) => entry.id === user.id ? updated : entry));
      setDrafts((current) => ({ ...current, [updated.id]: accessToDraft(updated.projectAccess) }));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to update access.");
    } finally {
      setBusy(null);
    }
  }

  async function toggleStatus(user: ManagedUser) {
    const status = user.status === "disabled" ? "active" : "disabled";
    setBusy(`status:${user.id}`);
    setError(null);
    try {
      const updated = await api.setUserStatus(user.id, status);
      setUsers((current) => current.map((entry) => entry.id === user.id ? updated : entry));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to update user status.");
    } finally {
      setBusy(null);
    }
  }

  async function resetLink(user: ManagedUser) {
    setBusy(`reset:${user.id}`);
    setError(null);
    try {
      const result = await api.createUserResetLink(user.id);
      setSecretLink({ title: `Password reset link for ${user.username}`, url: result.resetUrl });
      setUsers((current) => current.map((entry) => entry.id === user.id ? { ...entry, status: "invited" } : entry));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to create reset link.");
    } finally {
      setBusy(null);
    }
  }

  async function removeMember(user: ManagedUser) {
    if (deleteConfirmId !== user.id) {
      setDeleteConfirmId(user.id);
      return;
    }
    setBusy(`delete:${user.id}`);
    setError(null);
    try {
      await api.deleteUser(user.id);
      setUsers((current) => current.filter((entry) => entry.id !== user.id));
      setDrafts((current) => {
        const next = { ...current };
        delete next[user.id];
        return next;
      });
      setExpandedUserId(null);
      setDeleteConfirmId(null);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Unable to remove member.");
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="panel user-access-panel">
      <div className="panel-head">
        <div className="panel-head-title">
          <span className="settings-panel-icon"><UserRoundCog size={16} /></span>
          <div><h2>Users &amp; access</h2><p className="muted">Assign project visibility and capabilities. Unchecked capabilities remain read-only.</p></div>
        </div>
        <Button variant="secondary" disabled={loading} onClick={() => void refresh()} icon={<RefreshCw size={15} className={loading ? "spin" : ""} />}>Refresh</Button>
      </div>

      {secretLink ? (
        <div className="created-secret-box">
          <div><strong>{secretLink.title}</strong><p className="muted">Copy this one-time link now. Generating another link invalidates it.</p></div>
          <div className="token-box"><span>{secretLink.url}</span><button type="button" onClick={() => void copyText(secretLink.url)} aria-label="Copy account link"><Copy size={15} /></button></div>
          <Button variant="secondary" onClick={() => setSecretLink(null)}>I copied it</Button>
        </div>
      ) : null}

      <div className="user-access-create">
        <div className="user-access-create-head"><ShieldCheck size={16} /><div><strong>Invite a member</strong><p className="muted">The member chooses a password from the generated setup link.</p></div></div>
        <TextField label="Username" value={username} onChange={setUsername} autoComplete="off" placeholder="deploy-operator" />
        <ProjectAccessEditor projects={projects} value={newAccess} onChange={setNewAccess} />
        <div className="actions"><Button disabled={!username.trim() || !Object.keys(newAccess).length} loading={busy === "invite"} onClick={() => void invite()} icon={<Plus size={15} />}>Create invite</Button></div>
      </div>

      {error ? <p className="user-access-error" role="alert">{error}</p> : null}
      <div className="managed-user-list">
        {owner ? (
          <article className="managed-user-card owner-card">
            <header><div><div className="managed-user-name"><strong>{owner.username}</strong><span className="user-type-badge">Owner</span></div><span>Access to every project</span></div><StatusBadge status={owner.status} /></header>
            <div className="actions"><Button variant="secondary" loading={busy === `reset:${owner.id}`} onClick={() => void resetLink(owner)} icon={<KeyRound size={14} />}>Owner reset link</Button></div>
          </article>
        ) : null}
        {members.map((user) => {
          const expanded = expandedUserId === user.id;
          const capabilities = permissionLabels(user.projectAccess);
          return (
            <article className={`managed-user-card member-card ${expanded ? "expanded" : ""}`} key={user.id}>
              <button
                className="managed-user-summary"
                type="button"
                aria-expanded={expanded}
                aria-controls={`member-access-${user.id}`}
                onClick={() => {
                  setExpandedUserId(expanded ? null : user.id);
                  setDeleteConfirmId(null);
                }}
              >
                <div className="managed-user-summary-main">
                  <div className="managed-user-name"><strong>{user.username}</strong><span className="user-type-badge">Member</span></div>
                  <div className="member-access-summary">
                    <span><FolderKanban size={12} />{user.projectAccess.length ? user.projectAccess.map((entry) => entry.projectName).join(", ") : "No projects"}</span>
                    <span>{capabilities.length ? capabilities.join(" · ") : "View only"}</span>
                  </div>
                </div>
                <div className="managed-user-summary-state"><StatusBadge status={user.status} /><ChevronDown size={17} className={expanded ? "expanded" : ""} /></div>
              </button>
              {expanded ? (
                <div className="managed-user-editor" id={`member-access-${user.id}`}>
                  <p className="member-last-login">{user.lastLoginAt ? `Last login ${new Date(user.lastLoginAt).toLocaleString()}` : "Never signed in"}</p>
                  <ProjectAccessEditor projects={projects} value={drafts[user.id] ?? {}} onChange={(value) => setDrafts((current) => ({ ...current, [user.id]: value }))} />
                  <div className="actions member-actions">
                    <Button variant="secondary" loading={busy === `reset:${user.id}`} onClick={() => void resetLink(user)} icon={<KeyRound size={14} />}>Reset link</Button>
                    <Button variant="secondary" loading={busy === `access:${user.id}`} onClick={() => void saveAccess(user)}>Save access</Button>
                    <Button variant={user.status === "disabled" ? "secondary" : "danger"} disabled={user.id === currentUserId} loading={busy === `status:${user.id}`} onClick={() => void toggleStatus(user)}>
                      {user.status === "disabled" ? "Enable" : "Disable"}
                    </Button>
                    {deleteConfirmId === user.id ? <Button variant="ghost" onClick={() => setDeleteConfirmId(null)}>Cancel</Button> : null}
                    <Button variant="danger" loading={busy === `delete:${user.id}`} onClick={() => void removeMember(user)} icon={<Trash2 size={14} />}>
                      {deleteConfirmId === user.id ? "Confirm removal" : "Remove member"}
                    </Button>
                  </div>
                  {deleteConfirmId === user.id ? <p className="member-delete-warning">This permanently removes the account and immediately ends its access.</p> : null}
                </div>
              ) : null}
            </article>
          );
        })}
        {!loading && !members.length ? <p className="muted">No delegated users yet.</p> : null}
      </div>
    </section>
  );
}
