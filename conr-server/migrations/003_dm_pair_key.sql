-- Canonical 1:1 conversation key (sorted user ids) to prevent duplicate DM threads.
-- NULL pair_key rows are legacy conversations; multiple NULLs are allowed under UNIQUE.
ALTER TABLE dm_conversations ADD COLUMN IF NOT EXISTS pair_key TEXT;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'dm_conversations_pair_key_key'
    ) THEN
        ALTER TABLE dm_conversations ADD CONSTRAINT dm_conversations_pair_key_key UNIQUE (pair_key);
    END IF;
END $$;
