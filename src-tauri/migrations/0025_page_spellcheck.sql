-- Per-page spell check. Notes full of identifiers, symbols and jargon (CS lecture notes are the
-- motivating case) draw a red squiggle under almost every word, which makes the real typos
-- invisible. The preference belongs to the PAGE, not the app: one vault holds both prose that
-- wants checking and code notes that don't, so a global switch would be wrong in half the vault.
--
-- Modelled like 0024's is_folder: a plain column on the already-synced `notes` table, so 0015's
-- change-capture carries it to paired devices unchanged. `DEFAULT 1` keeps spell check ON for
-- every note that predates this migration, which is the behaviour those notes already had.
ALTER TABLE notes ADD COLUMN spellcheck INTEGER NOT NULL DEFAULT 1;
