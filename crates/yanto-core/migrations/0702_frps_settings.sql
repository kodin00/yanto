-- FRP settings singleton (docs/rewrite/02 4.7). No FRP secrets live here; they stay in frps.toml.
CREATE TABLE frps_settings (
    id          INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    public_host TEXT    NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL
) STRICT;

INSERT INTO frps_settings (id, public_host, updated_at) VALUES (1, '', 0);
