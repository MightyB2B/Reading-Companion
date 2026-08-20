-- Schema v8 — searching the library, and notes that span more than one place.
--
-- Applied on a fresh install (after schema.sql and v7.sql) and as the v7->v8
-- migration, so every statement is `IF NOT EXISTS` and every backfill is
-- idempotent.

-- A note can be anchored in several places at once.
--
-- Two things want this. A selection dragged across a page boundary covers two
-- paragraphs and has always *meant* to attach to both — resolving it to one
-- was the bug that made highlighting across a page break unusable. And a note
-- tracking a term or a theme belongs at every place it turns up, not only the
-- first.
--
-- `notes.block_id` / `char_start` / `char_end` stay as the primary anchor:
-- everything already written keeps working, the reading column can draw a note
-- without a join, and this table carries the rest.
CREATE TABLE IF NOT EXISTS note_anchors (
    id          INTEGER PRIMARY KEY,
    note_id     INTEGER NOT NULL REFERENCES notes(id)   ON DELETE CASCADE,
    block_id    INTEGER          REFERENCES blocks(id)  ON DELETE SET NULL,
    char_start  INTEGER,
    char_end    INTEGER,
    anchor_text TEXT    NOT NULL DEFAULT '',
    -- Could not be found after a re-transcription. Flagged, never deleted.
    orphaned    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (note_id, block_id, char_start)
);

-- Existing notes become their own first anchor, so the two representations
-- agree from the moment this runs. OR IGNORE makes re-application harmless.
INSERT OR IGNORE INTO note_anchors (note_id, block_id, char_start, char_end, anchor_text, orphaned)
SELECT id, block_id, char_start, char_end, anchor_text, orphaned
  FROM notes
 WHERE block_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_anchors_note  ON note_anchors(note_id);
CREATE INDEX IF NOT EXISTS idx_anchors_block ON note_anchors(block_id, char_start);

-- Full-text search over the reader's own books.
--
-- The one thing a research tool has to be able to do, and the one thing this
-- application could not: find the passage you remember but cannot place. The
-- dictionary has had FTS5 from the start; the library never did.
--
-- An external-content table: the text lives in `blocks` and this holds only
-- the index, so a book is not stored twice. `porter` stemming means a search
-- for "knowing" finds "knew".
--
-- The indexed column MUST be named `text_norm`, matching the column in
-- `blocks`. With `content=`, FTS5 rebuilds a row by issuing
-- `SELECT T.<column> FROM blocks AS T`, so a column named anything else makes
-- every insert fail with "no such column".
--
-- Dropped and rebuilt rather than created if absent: an index is derived data,
-- rebuilding it for a personal library costs milliseconds, and a half-built
-- index left by an interrupted migration would otherwise survive forever
-- behind `IF NOT EXISTS` and quietly return nothing.
DROP TRIGGER IF EXISTS blocks_fts_insert;
DROP TRIGGER IF EXISTS blocks_fts_delete;
DROP TRIGGER IF EXISTS blocks_fts_update;
DROP TABLE IF EXISTS blocks_fts;

CREATE VIRTUAL TABLE blocks_fts USING fts5(
    text_norm,
    content='blocks',
    content_rowid='id',
    tokenize='porter unicode61'
);

-- Keep the index in step with the text. External-content FTS5 tables are not
-- updated automatically, and a stale index is worse than no index: it returns
-- paragraphs that no longer say what it claims.
CREATE TRIGGER blocks_fts_insert AFTER INSERT ON blocks BEGIN
    INSERT INTO blocks_fts (rowid, text_norm) VALUES (new.id, new.text_norm);
END;

CREATE TRIGGER blocks_fts_delete AFTER DELETE ON blocks BEGIN
    INSERT INTO blocks_fts (blocks_fts, rowid, text_norm)
    VALUES ('delete', old.id, old.text_norm);
END;

CREATE TRIGGER blocks_fts_update AFTER UPDATE ON blocks BEGIN
    INSERT INTO blocks_fts (blocks_fts, rowid, text_norm)
    VALUES ('delete', old.id, old.text_norm);
    INSERT INTO blocks_fts (rowid, text_norm) VALUES (new.id, new.text_norm);
END;

-- Index everything already imported.
INSERT INTO blocks_fts (rowid, text_norm) SELECT id, text_norm FROM blocks;
