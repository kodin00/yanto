import { Cloud, Copy, Edit3, Plus, RefreshCw, Trash2 } from "lucide-react";
import { memo, useMemo, useState } from "react";
import type { FormEvent } from "react";
import type { CloudflareClient, CloudflareDnsRecord, CloudflareDnsRecordType, CloudflareRouteDiagnostic } from "../../shared/types";
import { Pagination } from "../components/Pagination";
import { Button, CustomSelect, IconButton, StatusBadge, TextField, ToggleField } from "../components/ui";
import type { CloudflareDnsRecordPayload } from "../lib/api";
import type { ConfirmState } from "./types";

const editableTypes: CloudflareDnsRecordType[] = ["A", "AAAA", "CNAME", "TXT", "MX", "NS"];
const proxiedTypes = new Set<CloudflareDnsRecordType>(["A", "AAAA", "CNAME"]);

type DnsFormState = {
  id: string | null;
  type: CloudflareDnsRecordType;
  name: string;
  content: string;
  ttl: string;
  proxied: boolean;
  priority: string;
  comment: string;
};

const emptyForm: DnsFormState = {
  id: null,
  type: "A",
  name: "",
  content: "",
  ttl: "1",
  proxied: false,
  priority: "",
  comment: ""
};

type Props = {
  clients: CloudflareClient[];
  selectedClientId: string;
  records: CloudflareDnsRecord[];
  visibleRecords: CloudflareDnsRecord[];
  diagnostics: CloudflareRouteDiagnostic[];
  busy: string | null;
  loading?: boolean;
  page: number;
  selectClient: (clientId: string) => void;
  createRecord: (payload: CloudflareDnsRecordPayload) => Promise<void>;
  updateRecord: (id: string, payload: CloudflareDnsRecordPayload) => Promise<void>;
  deleteRecord: (record: CloudflareDnsRecord) => Promise<void>;
  copyText: (value: string) => Promise<void>;
  setConfirm: (state: ConfirmState) => void;
  setPage: (page: number) => void;
  openClients: () => void;
};

function editableType(type: string): type is CloudflareDnsRecordType {
  return editableTypes.includes(type as CloudflareDnsRecordType);
}

function recordForm(record: CloudflareDnsRecord): DnsFormState {
  return {
    id: record.id,
    type: editableType(record.type) ? record.type : "A",
    name: record.name,
    content: record.content,
    ttl: String(record.ttl || 1),
    proxied: record.proxied,
    priority: record.priority == null ? "" : String(record.priority),
    comment: record.comment ?? ""
  };
}

function payloadFromForm(form: DnsFormState): CloudflareDnsRecordPayload {
  return {
    type: form.type,
    name: form.name.trim(),
    content: form.content.trim(),
    ttl: Number(form.ttl || 1),
    proxied: proxiedTypes.has(form.type) ? form.proxied : false,
    priority: form.type === "MX" && form.priority ? Number(form.priority) : null,
    comment: form.comment.trim() || null
  };
}

export const DnsView = memo(function DnsView(props: Props) {
  const { clients, selectedClientId, records, visibleRecords, diagnostics, busy, loading, page, selectClient, createRecord, updateRecord, deleteRecord, copyText, setConfirm, setPage, openClients } = props;
  const [form, setForm] = useState<DnsFormState>(emptyForm);
  const selectedClient = clients.find((client) => client.id === selectedClientId);
  const ready = Boolean(selectedClient?.zoneId && selectedClient.hasApiToken);
  const canProxy = proxiedTypes.has(form.type);
  const saving = busy === "dns-save";
  const typeOptions = useMemo(() => editableTypes.map((type) => ({ label: type, value: type })), []);
  const diagnosticsByHostname = useMemo(() => Object.fromEntries(diagnostics.map((diagnostic) => [diagnostic.hostname.toLowerCase(), diagnostic])), [diagnostics]);
  const yantoRecordKeys = useMemo(() => new Set(diagnostics.flatMap((diagnostic) => {
    if (!diagnostic.expectedDnsTarget) return [];
    return [`${diagnostic.hostname.toLowerCase()}|cname|${diagnostic.expectedDnsTarget.toLowerCase()}`];
  })), [diagnostics]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const payload = payloadFromForm(form);
    if (form.id) {
      await updateRecord(form.id, payload);
    } else {
      await createRecord(payload);
    }
    setForm(emptyForm);
  }

  return (
    <section className="dns-layout">
      <section className="panel dns-client-panel">
        <div className="panel-head">
          <h2>Client DNS</h2>
          <span className="count">{clients.length ? `${clients.length} clients` : "No clients"}</span>
        </div>
        {clients.length ? (
          <div className="cloudflare-tabs">
            {clients.map((client) => (
              <button
                type="button"
                className={client.id === selectedClientId ? "active" : ""}
                key={client.id}
                onClick={() => {
                  setForm(emptyForm);
                  selectClient(client.id);
                }}
              >
                {client.name}
              </button>
            ))}
          </div>
        ) : (
          <div className="dns-not-ready">
            <p className="muted">Add a Cloudflare client before managing DNS records.</p>
            <Button variant="secondary" onClick={openClients}>
              Open clients
            </Button>
          </div>
        )}
      </section>

      <section className="panel dns-editor-panel">
        <div className="panel-head">
          <h2>{form.id ? "Edit DNS record" : "Add DNS record"}</h2>
          <Cloud size={19} />
        </div>
        {!ready ? (
          <div className="dns-not-ready">
            <p className="muted">{selectedClient ? "This client needs a Zone ID and API token before DNS records can be managed." : "Choose a Cloudflare client before managing DNS records."}</p>
            <Button variant="secondary" onClick={openClients}>
              Open clients
            </Button>
          </div>
        ) : null}
        <form className="form-grid dns-record-form" onSubmit={submit} autoComplete="off">
          <div className="dns-form-grid">
            <CustomSelect label="Type" value={form.type} options={typeOptions} disabled={!ready || saving} onChange={(type) => setForm((current) => ({ ...current, type, proxied: proxiedTypes.has(type) ? current.proxied : false }))} />
            <TextField label="Name" value={form.name} onChange={(name) => setForm((current) => ({ ...current, name }))} placeholder="www or example.com" required disabled={!ready || saving} />
            <TextField label="Content" value={form.content} onChange={(content) => setForm((current) => ({ ...current, content }))} placeholder="Target value" required disabled={!ready || saving} />
            <TextField label="TTL" value={form.ttl} onChange={(ttl) => setForm((current) => ({ ...current, ttl: ttl.replace(/\D/g, "") || "1" }))} placeholder="1" required disabled={!ready || saving} />
            {form.type === "MX" ? (
              <TextField label="Priority" value={form.priority} onChange={(priority) => setForm((current) => ({ ...current, priority: priority.replace(/\D/g, "") }))} placeholder="10" disabled={!ready || saving} />
            ) : null}
          </div>
          <TextField label="Comment" value={form.comment} onChange={(comment) => setForm((current) => ({ ...current, comment }))} placeholder="Optional note" disabled={!ready || saving} />
          <ToggleField label="Proxied" value={canProxy && form.proxied} onChange={(proxied) => setForm((current) => ({ ...current, proxied }))} disabled={!ready || saving || !canProxy} description={canProxy ? "Routes traffic through Cloudflare." : "Only A, AAAA, and CNAME records can be proxied."} />
          <div className="actions">
            {form.id ? (
              <Button type="button" variant="ghost" disabled={saving} onClick={() => setForm(emptyForm)}>
                Cancel
              </Button>
            ) : null}
            <Button type="submit" disabled={!ready || saving || !form.name.trim() || !form.content.trim()} loading={saving} icon={<Plus size={16} />}>
              {form.id ? "Save record" : "Create record"}
            </Button>
          </div>
        </form>
      </section>

      <section className="panel dns-records-panel">
        <div className="panel-head">
          <h2>{selectedClient ? `${selectedClient.name} DNS records` : "DNS records"}</h2>
          <span className="count">{loading ? "Loading" : `${records.length} records`}</span>
        </div>
        <div className="table-wrap dns-table-wrap">
          <table>
            <thead>
              <tr>
                <th>Type</th>
                <th>Name</th>
                <th>Content</th>
                <th>TTL</th>
                <th>Proxy</th>
                <th>Updated</th>
                <th className="action-cell">Actions</th>
              </tr>
            </thead>
            <tbody>
              {visibleRecords.map((record) => {
                const diagnostic = diagnosticsByHostname[record.name.toLowerCase()];
                const recordKey = `${record.name.toLowerCase()}|${record.type.toLowerCase()}|${record.content.toLowerCase()}`;
                const yantoManaged = yantoRecordKeys.has(recordKey);
                const conflict = Boolean(diagnostic && !yantoManaged && ["conflict", "mismatch"].includes(diagnostic.dnsStatus));
                return (
                <tr key={record.id} className={conflict ? "dns-record-conflict" : yantoManaged ? "dns-record-yanto" : ""}>
                  <td><StatusBadge status={record.type} /></td>
                  <td className="dns-name-cell">
                    <div className="dns-name-cell-content">
                      <span>{record.name}</span>
                      {yantoManaged ? <StatusBadge status="yanto" label="Yanto route" /> : null}
                      {conflict ? <StatusBadge status="conflict" label="Conflict" /> : null}
                    </div>
                  </td>
                  <td className="dns-content-cell">{record.content}</td>
                  <td>{record.ttl === 1 ? "Auto" : record.ttl}</td>
                  <td>{record.proxiable ? <StatusBadge status={record.proxied ? "proxied" : "dns-only"} label={record.proxied ? "Proxied" : "DNS only"} /> : <span className="muted">N/A</span>}</td>
                  <td>{record.modifiedOn ? new Date(record.modifiedOn).toLocaleString() : "Unknown"}</td>
                  <td className="action-cell">
                    <div className="table-actions icon-actions">
                      <IconButton label="Copy content" onClick={() => void copyText(record.content)}><Copy size={14} /></IconButton>
                      <IconButton label="Edit record" disabled={!editableType(record.type)} onClick={() => setForm(recordForm(record))}><Edit3 size={14} /></IconButton>
                      <IconButton
                        label="Delete record"
                        variant="danger"
                        onClick={() => setConfirm({
                          title: "Delete DNS record",
                          body: `Delete ${record.type} record for ${record.name}?`,
                          label: "Delete",
                          danger: true,
                          loadingMessage: "Deleting DNS record...",
                          successMessage: "DNS record deleted.",
                          action: () => deleteRecord(record)
                        })}
                      >
                        {busy === `dns-delete:${record.id}` ? <RefreshCw size={14} className="spin" /> : <Trash2 size={14} />}
                      </IconButton>
                    </div>
                  </td>
                </tr>
              );
              })}
              {!visibleRecords.length ? (
                <tr>
                  <td colSpan={7}>{loading ? "Loading DNS records..." : ready ? "No DNS records found." : "Choose a Cloudflare client first."}</td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </div>
        <Pagination label="DNS records" page={page} totalItems={records.length} onPageChange={setPage} />
      </section>
    </section>
  );
});
