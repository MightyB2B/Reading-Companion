-- Schema v7 — the study layer.
--
-- Everything here anchors to a span of text: a block plus a character range.
-- That single convention is what lets one set of tools work on a philosophy
-- paragraph and on a verse of scripture without either being a special case.
--
-- Kept in its own file so a fresh database and a migrated one are built from
-- exactly the same statements. `init` runs schema.sql then this; the v6->v7
-- migration runs this alone.

-- A notebook groups notes. `book_id` is null for a notebook that spans the
-- library. `kind` distinguishes the ones the reader made from the ones a
-- workflow created for its own answers.
CREATE TABLE IF NOT EXISTS notebooks (
    id         INTEGER PRIMARY KEY,
    book_id    INTEGER REFERENCES books(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL DEFAULT 'manual',  -- manual|workflow|terms
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- A note and a mark are the same row.
--
-- A note has a body and no `move`; a mark has a `move` and usually no body;
-- one with both is a highlighted passage you also wrote about. They share a
-- table because they share the hard part — anchoring — and splitting them
-- would mean maintaining that logic twice.
--
-- `anchor_text` exists so an anchor survives re-transcription. Re-running OCR
-- destroys and rebuilds a page's blocks, taking their ids with them; the
-- stored text is what lets the note find its way home afterwards.
CREATE TABLE IF NOT EXISTS notes (
    id          INTEGER PRIMARY KEY,
    book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    notebook_id INTEGER REFERENCES notebooks(id) ON DELETE SET NULL,
    -- Null for a note about the book as a whole rather than a passage in it.
    block_id    INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    char_start  INTEGER,
    char_end    INTEGER,
    anchor_text TEXT    NOT NULL DEFAULT '',
    -- null | thesis | premise | conclusion | definition | objection | reply |
    -- example | concession | aporia
    move        TEXT,
    body        TEXT    NOT NULL DEFAULT '',
    -- Set when the anchor could not be found after a re-import. The note is
    -- kept and flagged rather than deleted: the reader wrote it, and losing
    -- it silently because a page was re-transcribed is not acceptable.
    orphaned    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The author's own vocabulary — Adler's rule 5.
--
-- Deliberately separate from `lookups`: the dictionary reports what a word
-- meant in 1913, this records what *this author* means by it, which is the
-- meaning you actually need and the one no dictionary can supply.
CREATE TABLE IF NOT EXISTS terms (
    id             INTEGER PRIMARY KEY,
    book_id        INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    term           TEXT    NOT NULL,
    my_gloss       TEXT    NOT NULL DEFAULT '',
    status         TEXT    NOT NULL DEFAULT 'unclear',  -- unclear|working|settled
    first_block_id INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    created_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (book_id, term)
);

-- Superseded glosses, kept.
--
-- A term whose sense moves is the most interesting thing that can happen
-- while reading a philosopher, and throwing away the earlier reading would
-- destroy the evidence that it moved.
CREATE TABLE IF NOT EXISTS term_revisions (
    id         INTEGER PRIMARY KEY,
    term_id    INTEGER NOT NULL REFERENCES terms(id) ON DELETE CASCADE,
    my_gloss   TEXT    NOT NULL,
    status     TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS term_mentions (
    id         INTEGER PRIMARY KEY,
    term_id    INTEGER NOT NULL REFERENCES terms(id)  ON DELETE CASCADE,
    block_id   INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    char_start INTEGER NOT NULL,
    char_end   INTEGER NOT NULL,
    UNIQUE (term_id, block_id, char_start)
);

-- A reconstruction in standard form.
--
-- The conclusion is a column and the premises are rows, which is what lets
-- the map be *rendered* from the data rather than drawn by hand. A drawn map
-- is a second copy of the argument, and second copies drift.
CREATE TABLE IF NOT EXISTS arguments (
    id         INTEGER PRIMARY KEY,
    book_id    INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    label      TEXT    NOT NULL DEFAULT '',
    conclusion TEXT    NOT NULL DEFAULT '',
    -- undecided | sound | valid_unsound | invalid
    verdict    TEXT    NOT NULL DEFAULT 'undecided',
    -- none | missing_premise | equivocation | irrelevance | circularity
    gap        TEXT    NOT NULL DEFAULT 'none',
    -- Where the reader was when they started it, so the argument can be
    -- found again from the text and the text from the argument.
    anchor_block_id INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    notes      TEXT    NOT NULL DEFAULT '',
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- `implicit` marks a premise the reader supplied rather than the author.
-- Keeping the distinction is the whole point of the suppressed-premise step:
-- an argument that needs three unstated premises is telling you something.
CREATE TABLE IF NOT EXISTS premises (
    id              INTEGER PRIMARY KEY,
    argument_id     INTEGER NOT NULL REFERENCES arguments(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL,
    text            TEXT    NOT NULL,
    implicit        INTEGER NOT NULL DEFAULT 0,
    source_block_id INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    char_start      INTEGER,
    char_end        INTEGER
);

CREATE TABLE IF NOT EXISTS argument_links (
    id        INTEGER PRIMARY KEY,
    parent_id INTEGER NOT NULL REFERENCES arguments(id) ON DELETE CASCADE,
    child_id  INTEGER NOT NULL REFERENCES arguments(id) ON DELETE CASCADE,
    role      TEXT    NOT NULL,  -- supports|objects_to|replies_to
    UNIQUE (parent_id, child_id)
);

-- What the reader thinks the book is arguing, so drift is visible.
CREATE TABLE IF NOT EXISTS theses (
    id         INTEGER PRIMARY KEY,
    book_id    INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    statement  TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Verse addressing.
--
-- A Bible is a book like any other: its text lives in `blocks`. This maps a
-- verse onto the span of a block that holds it, so every tool that works on
-- a character range works on scripture unchanged.
CREATE TABLE IF NOT EXISTS verses (
    id         INTEGER PRIMARY KEY,
    book_id    INTEGER NOT NULL REFERENCES books(id)  ON DELETE CASCADE,
    block_id   INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    osis_book  TEXT    NOT NULL,   -- 'Rom', 'Ps' — the OSIS abbreviations
    chapter    INTEGER NOT NULL,
    verse      INTEGER NOT NULL,
    char_start INTEGER NOT NULL DEFAULT 0,
    char_end   INTEGER NOT NULL DEFAULT 0,
    UNIQUE (book_id, osis_book, chapter, verse)
);

-- Every scripture citation found anywhere in the library.
--
-- `book_id` is denormalised off `blocks -> pages -> books` on purpose: the
-- question this table exists to answer is "what in my library cites Romans
-- 3", and making that a single index scan is worth one redundant column.
--
-- `surface` holds the citation exactly as printed, because nineteenth-century
-- theology cites in Roman numerals and the parser will not always be right.
-- Storing what it saw is what makes a misparse correctable instead of silent.
CREATE TABLE IF NOT EXISTS refs (
    id          INTEGER PRIMARY KEY,
    book_id     INTEGER NOT NULL REFERENCES books(id)  ON DELETE CASCADE,
    block_id    INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    char_start  INTEGER NOT NULL,
    char_end    INTEGER NOT NULL,
    surface     TEXT    NOT NULL,
    osis_book   TEXT    NOT NULL,
    chapter     INTEGER NOT NULL,
    verse_start INTEGER,
    verse_end   INTEGER,
    confidence  REAL    NOT NULL DEFAULT 1.0,
    corrected   INTEGER NOT NULL DEFAULT 0,
    UNIQUE (block_id, char_start)
);

-- Workflows become data.
--
-- The five-step summarise method shipped as hardcoded React. Making the steps
-- rows is what lets the argument pass and the inductive study method exist
-- without three bespoke panels, and lets the reader write their own.
CREATE TABLE IF NOT EXISTS workflows (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    purpose    TEXT    NOT NULL DEFAULT '',
    builtin    INTEGER NOT NULL DEFAULT 0,
    spec_json  TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workflow_runs (
    id          INTEGER PRIMARY KEY,
    workflow_id INTEGER NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    book_id     INTEGER NOT NULL REFERENCES books(id)     ON DELETE CASCADE,
    block_id    INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    notebook_id INTEGER REFERENCES notebooks(id) ON DELETE SET NULL,
    state_json  TEXT    NOT NULL DEFAULT '{}',
    finished    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Saved panel arrangements. A property of this machine's screen rather than
-- of the library, but stored here so a layout survives a reinstall.
CREATE TABLE IF NOT EXISTS layouts (
    id        INTEGER PRIMARY KEY,
    name      TEXT    NOT NULL UNIQUE,
    builtin   INTEGER NOT NULL DEFAULT 0,
    spec_json TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_book   ON notes(book_id, updated_at);
CREATE INDEX IF NOT EXISTS idx_notes_block  ON notes(block_id, char_start);
CREATE INDEX IF NOT EXISTS idx_terms_book   ON terms(book_id, term);
CREATE INDEX IF NOT EXISTS idx_mentions     ON term_mentions(block_id);
CREATE INDEX IF NOT EXISTS idx_premises_arg ON premises(argument_id, ordinal);
CREATE INDEX IF NOT EXISTS idx_args_book    ON arguments(book_id, updated_at);
CREATE INDEX IF NOT EXISTS idx_verses_ref   ON verses(osis_book, chapter, verse);
CREATE INDEX IF NOT EXISTS idx_refs_block   ON refs(block_id, char_start);
-- The reverse index: everything in the library that cites a given passage.
CREATE INDEX IF NOT EXISTS idx_refs_target  ON refs(osis_book, chapter, verse_start);
CREATE INDEX IF NOT EXISTS idx_runs_book    ON workflow_runs(book_id, updated_at);
