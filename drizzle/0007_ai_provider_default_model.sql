ALTER TABLE "ai_providers" ADD COLUMN IF NOT EXISTS "default_model_id" text REFERENCES "ai_models"("id") ON DELETE SET NULL;
