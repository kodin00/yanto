-- Cloudflare clients, zone cache, remote-managed tunnels and DNS record cache (docs/rewrite/02 4.6).
-- The API token is stored sealed (`api_token_enc`); the tunnel connector token is never stored.
CREATE TABLE cloudflare_clients (
    id            TEXT    NOT NULL PRIMARY KEY,
    name          TEXT    NOT NULL UNIQUE,
    account_id    TEXT    NOT NULL UNIQUE,
    api_token_enc TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;

CREATE TABLE cloudflare_zones (
    id        TEXT    NOT NULL PRIMARY KEY,
    client_id TEXT    NOT NULL REFERENCES cloudflare_clients (id) ON DELETE CASCADE,
    name      TEXT    NOT NULL COLLATE NOCASE,
    status    TEXT,
    synced_at INTEGER NOT NULL
) STRICT;

CREATE INDEX cloudflare_zones_client_idx ON cloudflare_zones (client_id);

CREATE TABLE cloudflare_tunnels (
    id             TEXT    NOT NULL PRIMARY KEY,
    client_id      TEXT    NOT NULL REFERENCES cloudflare_clients (id) ON DELETE RESTRICT,
    cf_tunnel_id   TEXT    NOT NULL UNIQUE,
    name           TEXT    NOT NULL,
    active         INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0, 1)),
    status         TEXT    NOT NULL DEFAULT 'unknown',
    last_health_at INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
) STRICT;

-- Exactly one tunnel is bound to the Host cloudflared at a time (ASSUMPTION, 04 9.2).
CREATE UNIQUE INDEX cloudflare_tunnels_one_active ON cloudflare_tunnels (active) WHERE active = 1;

CREATE TABLE cloudflare_dns_records (
    id        TEXT    NOT NULL PRIMARY KEY,
    zone_id   TEXT    NOT NULL REFERENCES cloudflare_zones (id) ON DELETE CASCADE,
    type      TEXT    NOT NULL CHECK (type IN ('A', 'AAAA', 'CNAME', 'TXT', 'MX', 'NS')),
    name      TEXT    NOT NULL,
    content   TEXT    NOT NULL,
    ttl       INTEGER NOT NULL DEFAULT 1,
    proxied   INTEGER NOT NULL DEFAULT 0 CHECK (proxied IN (0, 1)),
    priority  INTEGER CHECK (priority IS NULL OR priority BETWEEN 0 AND 65535),
    comment   TEXT,
    synced_at INTEGER NOT NULL
) STRICT;

CREATE INDEX cloudflare_dns_zone_idx ON cloudflare_dns_records (zone_id, type, name);
