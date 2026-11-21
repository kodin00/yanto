-- Hostnames: a public name routed to a Project's published loopback port (or the Hosted Registry)
-- through one Ingress Provider (docs/rewrite/02 4.6, ADR 0008).
--
-- `project_port_id` uses NO ACTION instead of the RESTRICT of 02: both refuse to delete a port
-- that a Hostname uses, but NO ACTION is checked at the end of the statement, so deleting a
-- Project (which cascades to its ports and its Hostnames) is not blocked by the ordering of the
-- two cascades.
CREATE TABLE hostnames (
    id                 TEXT    NOT NULL PRIMARY KEY,
    target_kind        TEXT    NOT NULL DEFAULT 'project' CHECK (target_kind IN ('project', 'registry')),
    project_id         TEXT    REFERENCES projects (id) ON DELETE CASCADE,
    project_port_id    TEXT    REFERENCES project_ports (id) ON DELETE NO ACTION,
    hostname           TEXT    NOT NULL UNIQUE CHECK (hostname = lower(hostname)),
    ingress_provider   TEXT    NOT NULL CHECK (ingress_provider IN ('cloudflare_tunnel', 'traefik')),
    tunnel_id          TEXT    REFERENCES cloudflare_tunnels (id) ON DELETE RESTRICT,
    zone_id            TEXT    REFERENCES cloudflare_zones (id) ON DELETE SET NULL,
    manage_dns         INTEGER NOT NULL DEFAULT 0 CHECK (manage_dns IN (0, 1)),
    dns_record_id      TEXT,
    enabled            INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    sync_status        TEXT    NOT NULL DEFAULT 'pending'
        CHECK (sync_status IN ('pending', 'active', 'error', 'disabled')),
    last_error         TEXT,
    last_published_at  INTEGER,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    CHECK ((ingress_provider = 'cloudflare_tunnel') = (tunnel_id IS NOT NULL)),
    CHECK (manage_dns = 0 OR zone_id IS NOT NULL),
    CHECK ((target_kind = 'project') = (project_id IS NOT NULL AND project_port_id IS NOT NULL))
) STRICT;

CREATE INDEX hostnames_project_idx ON hostnames (project_id);
CREATE INDEX hostnames_port_idx ON hostnames (project_port_id);
CREATE INDEX hostnames_tunnel_idx ON hostnames (tunnel_id);

-- Cross-table invariant: the port belongs to the Hostname's Project.
CREATE TRIGGER hostnames_port_project_insert BEFORE INSERT ON hostnames
WHEN NEW.project_port_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM project_ports WHERE id = NEW.project_port_id AND project_id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'project_port does not belong to the hostname project');
END;

CREATE TRIGGER hostnames_port_project_update BEFORE UPDATE ON hostnames
WHEN NEW.project_port_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM project_ports WHERE id = NEW.project_port_id AND project_id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'project_port does not belong to the hostname project');
END;
