ALTER TABLE "backups" ADD COLUMN IF NOT EXISTS "source_node_id" text REFERENCES "deployment_nodes"("id") ON DELETE SET NULL DEFAULT 'node_master_local';
ALTER TABLE "backups" ADD COLUMN IF NOT EXISTS "checksum" text;

CREATE TABLE IF NOT EXISTS "backup_policies" (
  "id" text PRIMARY KEY,
  "name" text NOT NULL,
  "source_node_id" text NOT NULL REFERENCES "deployment_nodes"("id") ON DELETE CASCADE,
  "target_container_id" text,
  "enabled" boolean NOT NULL DEFAULT true,
  "hourly_at_minute" integer NOT NULL DEFAULT 0,
  "hourly_retention" integer NOT NULL DEFAULT 24,
  "daily_retention" integer NOT NULL DEFAULT 30,
  "last_run_at" timestamptz,
  "next_run_at" timestamptz,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "updated_at" timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE "backups" ADD COLUMN IF NOT EXISTS "policy_id" text REFERENCES "backup_policies"("id") ON DELETE SET NULL;
CREATE TABLE IF NOT EXISTS "backup_policy_destinations" (
  "policy_id" text NOT NULL REFERENCES "backup_policies"("id") ON DELETE CASCADE,
  "destination_node_id" text NOT NULL REFERENCES "deployment_nodes"("id") ON DELETE CASCADE,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("policy_id", "destination_node_id")
);
CREATE TABLE IF NOT EXISTS "backup_replicas" (
  "id" text PRIMARY KEY,
  "backup_id" text NOT NULL REFERENCES "backups"("id") ON DELETE CASCADE,
  "destination_node_id" text NOT NULL REFERENCES "deployment_nodes"("id") ON DELETE CASCADE,
  "status" text NOT NULL DEFAULT 'pending',
  "file_path" text,
  "checksum" text,
  "error" text,
  "attempts" integer NOT NULL DEFAULT 0,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "updated_at" timestamptz NOT NULL DEFAULT now(),
  "finished_at" timestamptz,
  UNIQUE ("backup_id", "destination_node_id")
);

CREATE TABLE IF NOT EXISTS "frp_servers" (
  "id" text PRIMARY KEY,
  "node_id" text NOT NULL UNIQUE REFERENCES "deployment_nodes"("id") ON DELETE CASCADE,
  "name" text NOT NULL,
  "public_host" text NOT NULL,
  "bind_port" integer NOT NULL DEFAULT 7000,
  "port_start" integer NOT NULL DEFAULT 25560,
  "port_end" integer NOT NULL DEFAULT 25600,
  "auth_token" text NOT NULL,
  "status" text NOT NULL DEFAULT 'offline',
  "last_error" text,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "updated_at" timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS "frp_node_assignments" (
  "node_id" text PRIMARY KEY REFERENCES "deployment_nodes"("id") ON DELETE CASCADE,
  "role" text NOT NULL DEFAULT 'disabled',
  "server_id" text REFERENCES "frp_servers"("id") ON DELETE SET NULL,
  "desired_revision" integer NOT NULL DEFAULT 1,
  "applied_revision" integer NOT NULL DEFAULT 0,
  "status" text NOT NULL DEFAULT 'pending',
  "last_error" text,
  "updated_at" timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE "frp_tunnels" ADD COLUMN IF NOT EXISTS "client_node_id" text REFERENCES "deployment_nodes"("id") ON DELETE CASCADE;
ALTER TABLE "frp_tunnels" ADD COLUMN IF NOT EXISTS "server_id" text REFERENCES "frp_servers"("id") ON DELETE CASCADE;
UPDATE "frp_tunnels" SET "client_node_id" = "node_id" WHERE "client_node_id" IS NULL AND "node_id" IS NOT NULL;
DROP INDEX IF EXISTS "frp_tunnels_protocol_remote_port_idx";
CREATE UNIQUE INDEX IF NOT EXISTS "frp_tunnels_legacy_protocol_remote_port_idx" ON "frp_tunnels"("protocol", "remote_port") WHERE "server_id" IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS "frp_tunnels_server_protocol_remote_port_idx" ON "frp_tunnels"("server_id", "protocol", "remote_port") WHERE "server_id" IS NOT NULL;
