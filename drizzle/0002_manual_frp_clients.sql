ALTER TABLE "frp_tunnels" ALTER COLUMN "node_id" DROP NOT NULL;
--> statement-breakpoint
DROP TABLE IF EXISTS "frp_worker_states";
