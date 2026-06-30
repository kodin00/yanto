CREATE TABLE IF NOT EXISTS "mcp_access_tokens" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"token_hash" text NOT NULL,
	"access_level" text NOT NULL,
	"last_used_at" timestamp with time zone,
	"revoked_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "mcp_access_tokens_token_hash_idx" ON "mcp_access_tokens" USING btree ("token_hash");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "mcp_access_tokens_revoked_idx" ON "mcp_access_tokens" USING btree ("revoked_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "mcp_access_tokens_created_at_idx" ON "mcp_access_tokens" USING btree ("created_at");
