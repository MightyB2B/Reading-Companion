-- Book Reading Companion — library schema (v1)
--
-- The paragraph (a row in `blocks`) is the atomic unit of this application.
-- The study method operates on paragraphs, so everything upstream (OCR) exists
-- to produce clean blocks and everything downstream (summaries, critiques,
-- lookups) hangs off them.

CREATE TABLE books (
    id          INTEGER PRIMARY KEY,
    title       TEXT    NOT NULL,
    author      TEXT,
    -- Drives dictionary corpus, archaic normalisation, and coach calibration.
    -- One of: modern | early_20c | victorian | early_modern
    era         TEXT    NOT NULL DEFAULT 'modern',
    -- 'auto' derives the corpus from `era`; an explicit value overrides it.
    dict_corpus TEXT    NOT NULL DEFAULT 'auto',
    -- Where the reader stopped. SET NULL rather than CASCADE: deleting the
    -- page they happened to be on must not delete the book.
    last_page_id INTEGER REFERENCES pages(id) ON DELETE SET NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- `page_no` is what the model read off the page, not the order you happened to
-- import in. It stays null until the page has been transcribed, and carries no
-- uniqueness constraint: two photographs genuinely can show the same numbered
-- page, and the honest way to catch that is the image hash below.
CREATE TABLE pages (
    id             INTEGER PRIMARY KEY,
    book_id        INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    page_no        INTEGER,
    page_no_source TEXT    NOT NULL DEFAULT 'unknown',  -- detected|manual|unknown
    image_hash     TEXT,                  -- SHA-256 of the archival image
    -- photo | epub | pdf | web. Text sources skip OCR entirely: their words
    -- are already words, and a vision model would be slower and worse.
    source_kind    TEXT    NOT NULL DEFAULT 'photo',
    source_ref     TEXT,                  -- chapter label, URL, or similar
    -- For a photograph, the viewable full-resolution copy. For a text source,
    -- the file or address the page was taken from.
    image_orig     TEXT    NOT NULL,
    image_proc     TEXT,                  -- bounded copy actually sent to the model
    ocr_status     TEXT    NOT NULL DEFAULT 'pending',  -- pending|running|done|failed
    ocr_model      TEXT,                  -- which model produced ocr_raw
    ocr_raw        TEXT,                  -- verbatim model output, kept for re-segmentation
    ocr_error      TEXT,
    created_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- `ocr_raw` is retained deliberately: segmentation rules will improve, and we
-- want to re-derive blocks without re-running the model over the image.
CREATE TABLE blocks (
    id          INTEGER PRIMARY KEY,
    page_id     INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    kind        TEXT    NOT NULL DEFAULT 'paragraph',  -- paragraph|heading|quote|footnote|caption
    -- text_raw  = diplomatic transcription, as printed (long-s, ligatures intact)
    -- text_norm = modernised reading text
    -- Both are stored so old books can be toggled without re-running OCR.
    text_raw    TEXT    NOT NULL,
    text_norm   TEXT    NOT NULL,
    user_edited INTEGER NOT NULL DEFAULT 0,
    UNIQUE (page_id, ordinal)
);

CREATE TABLE summaries (
    id           INTEGER PRIMARY KEY,
    block_id     INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    -- Which sentence of the paragraph this summarises. Null means the summary
    -- is of the paragraph as a whole. The reader works sentence by sentence
    -- first, then compresses the paragraph.
    sentence_ordinal INTEGER,
    sentence     TEXT    NOT NULL,
    revision     INTEGER NOT NULL DEFAULT 1,
    -- the user's own who/what gate, passed before the model is ever consulted
    self_checked INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The coach's assessment, stored as the rubric the model actually returns.
--
-- Note the absence of any free-text column holding the paragraph's meaning.
-- The model emits categorical judgements only; the reader-facing wording is
-- composed by the application from these flags. See src/coach/mod.rs for why
-- prose feedback had to be abandoned.
CREATE TABLE critiques (
    id                INTEGER PRIMARY KEY,
    summary_id        INTEGER NOT NULL REFERENCES summaries(id) ON DELETE CASCADE,
    verdict           TEXT    NOT NULL,  -- on_target|partial|off_target
    covers_subject    INTEGER NOT NULL DEFAULT 0,
    covers_action     INTEGER NOT NULL DEFAULT 0,
    covers_reason     INTEGER NOT NULL DEFAULT 0,
    problem           TEXT    NOT NULL DEFAULT 'none',
    steering_question TEXT,               -- scrubbed before storage
    model             TEXT    NOT NULL,
    created_at        TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The "build up" step: paragraph sentences roll into sections, sections into
-- chapters. source_ids_json holds the ordered ids of whatever was rolled up.
CREATE TABLE rollups (
    id              INTEGER PRIMARY KEY,
    book_id         INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    scope           TEXT    NOT NULL,  -- section|chapter
    label           TEXT,
    source_ids_json TEXT    NOT NULL,
    sentence        TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE lookups (
    id         INTEGER PRIMARY KEY,
    book_id    INTEGER REFERENCES books(id)  ON DELETE CASCADE,
    block_id   INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    word       TEXT    NOT NULL,   -- surface form as printed
    lemma      TEXT,               -- resolved headword
    sentence   TEXT,               -- the sentence it was found in
    sense_json TEXT,               -- candidate senses retrieved from dict.sqlite
    llm_gloss  TEXT,               -- in-context paraphrase, if requested
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE vocab (
    id              INTEGER PRIMARY KEY,
    book_id         INTEGER REFERENCES books(id)   ON DELETE CASCADE,
    word            TEXT    NOT NULL,
    lemma           TEXT,
    count           INTEGER NOT NULL DEFAULT 1,
    first_lookup_id INTEGER REFERENCES lookups(id) ON DELETE SET NULL,
    UNIQUE (book_id, word)
);

CREATE INDEX idx_pages_book       ON pages(book_id, page_no);
-- Re-importing the same photograph is caught here rather than by page number.
CREATE UNIQUE INDEX idx_pages_hash ON pages(book_id, image_hash);
CREATE INDEX idx_blocks_page     ON blocks(page_id, ordinal);
CREATE INDEX idx_summaries_block ON summaries(block_id, revision);
CREATE INDEX idx_critiques_summ  ON critiques(summary_id);
CREATE INDEX idx_lookups_book    ON lookups(book_id, created_at);
CREATE INDEX idx_rollups_book    ON rollups(book_id, scope);
