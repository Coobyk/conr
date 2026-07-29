-- Require a reason when blocking (enforced in app; column stores the text).
ALTER TABLE blocks ADD COLUMN IF NOT EXISTS reason TEXT NOT NULL DEFAULT '';
