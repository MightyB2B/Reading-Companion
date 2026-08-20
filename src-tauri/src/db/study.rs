//! Database access for the study layer.
//!
//! Split out of `db/mod.rs` because that file is already the size of the rest
//! of the crate, not because these tables are separate in any deeper sense.

use rusqlite::{params, OptionalExtension, Row};

use super::Db;
use crate::error::{AppError, Result};
use crate::study::{
    Argument, ArgumentLink, Move, Note, Notebook, Premise, Term, TermRevision, TermStatus,
};

fn note_from_row(r: &Row) -> rusqlite::Result<Note> {
    let mv: Option<String> = r.get("move")?;
    Ok(Note {
        id: r.get("id")?,
        book_id: r.get("book_id")?,
        notebook_id: r.get("notebook_id")?,
        block_id: r.get("block_id")?,
        char_start: r.get("char_start")?,
        char_end: r.get("char_end")?,
        anchor_text: r.get("anchor_text")?,
        r#move: mv.as_deref().and_then(Move::from_str_opt),
        body: r.get("body")?,
        orphaned: r.get::<_, i64>("orphaned")? != 0,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

const NOTE_COLS: &str = "id, book_id, notebook_id, block_id, char_start, char_end,
                         anchor_text, move, body, orphaned, created_at, updated_at";

impl Db {
    // ---- notes and marks -------------------------------------------------

    /// Create a note, a mark, or both.
    ///
    /// A body with no move is a note; a move with no body is a mark. One with
    /// neither is refused: an anchor pointing at nothing is not a record of
    /// anything, and allowing it means the Notes panel fills with blanks.
    #[allow(clippy::too_many_arguments)]
    pub fn add_note(
        &self,
        book_id: i64,
        block_id: Option<i64>,
        span: Option<(i64, i64)>,
        anchor_text: &str,
        mv: Option<Move>,
        body: &str,
        notebook_id: Option<i64>,
    ) -> Result<i64> {
        let body = body.trim();
        if body.is_empty() && mv.is_none() {
            return Err(AppError::Invalid(
                "a note needs something written in it, or a tag".into(),
            ));
        }
        let (start, end) = match span {
            Some((s, e)) if e > s => (Some(s), Some(e)),
            // A span that is empty or inverted is a selection bug upstream;
            // storing it would put a zero-width highlight in the text.
            _ => (None, None),
        };

        let conn = self.lock();
        conn.execute(
            "INSERT INTO notes
                 (book_id, notebook_id, block_id, char_start, char_end,
                  anchor_text, move, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                book_id,
                notebook_id,
                block_id,
                start,
                end,
                anchor_text.trim(),
                mv.map(|m| m.as_str()),
                body,
            ],
        )?;
        let id = conn.last_insert_rowid();

        // The primary anchor is mirrored into `note_anchors` so the two
        // representations never disagree: everything that walks anchors sees
        // all of them, and the reading column can still draw a note without a
        // join.
        if let (Some(block), Some(s), Some(e)) = (block_id, start, end) {
            conn.execute(
                "INSERT OR IGNORE INTO note_anchors
                     (note_id, block_id, char_start, char_end, anchor_text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, block, s, e, anchor_text.trim()],
            )?;
        }
        Ok(id)
    }

    /// Attach more places to a note that already exists.
    ///
    /// Two things want this. A selection dragged across a page boundary covers
    /// several paragraphs and has always *meant* to attach to all of them.
    /// And a note tracking a term through a book belongs everywhere it turns
    /// up, not only where it was first written.
    pub fn add_note_anchors(&self, note_id: i64, anchors: &[NewAnchor]) -> Result<usize> {
        if anchors.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut added = 0;
        for a in anchors {
            added += tx.execute(
                "INSERT OR IGNORE INTO note_anchors
                     (note_id, block_id, char_start, char_end, anchor_text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![note_id, a.block_id, a.char_start, a.char_end, a.anchor_text.trim()],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// Every place a note is attached, in reading order.
    pub fn note_anchors(&self, note_id: i64) -> Result<Vec<StoredAnchor>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.block_id, a.char_start, a.char_end, a.anchor_text,
                    a.orphaned, p.page_no
               FROM note_anchors a
               LEFT JOIN blocks b ON b.id = a.block_id
               LEFT JOIN pages  p ON p.id = b.page_id
              WHERE a.note_id = ?1
              ORDER BY p.page_no, a.char_start",
        )?;
        let rows = stmt.query_map(params![note_id], |r| {
            Ok(StoredAnchor {
                id: r.get(0)?,
                block_id: r.get(1)?,
                char_start: r.get(2)?,
                char_end: r.get(3)?,
                anchor_text: r.get(4)?,
                orphaned: r.get::<_, i64>(5)? != 0,
                page_no: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_note_anchor(&self, anchor_id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM note_anchors WHERE id = ?1", params![anchor_id])?;
        Ok(())
    }

    pub fn update_note(&self, id: i64, body: &str, mv: Option<Move>) -> Result<()> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE notes
                SET body = ?2, move = ?3, updated_at = datetime('now')
              WHERE id = ?1",
            params![id, body.trim(), mv.map(|m| m.as_str())],
        )?;
        if changed == 0 {
            return Err(AppError::Invalid(format!("no note with id {id}")));
        }
        Ok(())
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Every note in a book, newest first.
    pub fn list_notes(&self, book_id: i64, notebook_id: Option<i64>) -> Result<Vec<Note>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT {NOTE_COLS} FROM notes
              WHERE book_id = ?1 AND (?2 IS NULL OR notebook_id = ?2)
              ORDER BY updated_at DESC, id DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![book_id, notebook_id], |r| note_from_row(r))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The notes anchored inside a set of blocks.
    ///
    /// Takes many blocks at once because the reading column renders a window
    /// of them and asking per-block would be one query per paragraph on every
    /// scroll.
    pub fn notes_for_blocks(&self, block_ids: &[i64]) -> Result<Vec<Note>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let placeholders = vec!["?"; block_ids.len()].join(",");
        // Driven from `note_anchors` rather than from `notes.block_id`: a note
        // attached to five paragraphs has to mark all five, not just the one
        // it happened to be written from. The anchor's own offsets are
        // returned in place of the note's, so each occurrence highlights the
        // right words.
        let sql = format!(
            "SELECT n.id, n.book_id, n.notebook_id,
                    a.block_id AS block_id,
                    a.char_start AS char_start,
                    a.char_end AS char_end,
                    a.anchor_text AS anchor_text,
                    n.move, n.body, a.orphaned AS orphaned,
                    n.created_at, n.updated_at
               FROM note_anchors a
               JOIN notes n ON n.id = a.note_id
              WHERE a.block_id IN ({placeholders}) AND a.orphaned = 0
              ORDER BY a.char_start"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(block_ids), |r| note_from_row(r))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- notebooks -------------------------------------------------------

    pub fn list_notebooks(&self, book_id: i64) -> Result<Vec<Notebook>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.book_id, n.name, n.kind,
                    (SELECT COUNT(*) FROM notes WHERE notebook_id = n.id) AS notes
               FROM notebooks n
              WHERE n.book_id = ?1 OR n.book_id IS NULL
              ORDER BY n.kind = 'manual' DESC, n.name",
        )?;
        let rows = stmt.query_map(params![book_id], |r| {
            Ok(Notebook {
                id: r.get("id")?,
                book_id: r.get("book_id")?,
                name: r.get("name")?,
                kind: r.get("kind")?,
                notes: r.get("notes")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn create_notebook(&self, book_id: Option<i64>, name: &str, kind: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Invalid("a notebook needs a name".into()));
        }
        let conn = self.lock();
        conn.execute(
            "INSERT INTO notebooks (book_id, name, kind) VALUES (?1, ?2, ?3)",
            params![book_id, name, kind],
        )?;
        Ok(conn.last_insert_rowid())
    }

    // ---- terms -----------------------------------------------------------

    /// Save a term, keeping the previous gloss when the sense moves.
    ///
    /// A philosopher's term shifting under you is the most interesting thing
    /// that happens while reading one, and discarding the earlier reading
    /// would destroy the evidence that it shifted. So an update that actually
    /// changes the gloss files the old one first.
    pub fn save_term(
        &self,
        book_id: i64,
        term: &str,
        gloss: &str,
        status: TermStatus,
        first_block_id: Option<i64>,
    ) -> Result<i64> {
        let term_text = term.trim();
        if term_text.is_empty() {
            return Err(AppError::Invalid("a term needs a word".into()));
        }
        let conn = self.lock();

        let existing: Option<(i64, String, String)> = conn
            .query_row(
                "SELECT id, my_gloss, status FROM terms WHERE book_id = ?1 AND term = ?2",
                params![book_id, term_text],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        match existing {
            Some((id, old_gloss, old_status)) => {
                if old_gloss != gloss.trim() && !old_gloss.is_empty() {
                    conn.execute(
                        "INSERT INTO term_revisions (term_id, my_gloss, status)
                         VALUES (?1, ?2, ?3)",
                        params![id, old_gloss, old_status],
                    )?;
                }
                conn.execute(
                    "UPDATE terms
                        SET my_gloss = ?2, status = ?3, updated_at = datetime('now')
                      WHERE id = ?1",
                    params![id, gloss.trim(), status.as_str()],
                )?;
                Ok(id)
            }
            None => {
                conn.execute(
                    "INSERT INTO terms (book_id, term, my_gloss, status, first_block_id)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        book_id,
                        term_text,
                        gloss.trim(),
                        status.as_str(),
                        first_block_id
                    ],
                )?;
                Ok(conn.last_insert_rowid())
            }
        }
    }

    pub fn list_terms(&self, book_id: i64) -> Result<Vec<Term>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.book_id, t.term, t.my_gloss, t.status, t.first_block_id,
                    t.created_at, t.updated_at,
                    (SELECT COUNT(*) FROM term_mentions WHERE term_id = t.id) AS mentions
               FROM terms t
              WHERE t.book_id = ?1
              ORDER BY t.term COLLATE NOCASE",
        )?;
        let mut terms: Vec<Term> = stmt
            .query_map(params![book_id], |r| {
                Ok(Term {
                    id: r.get("id")?,
                    book_id: r.get("book_id")?,
                    term: r.get("term")?,
                    my_gloss: r.get("my_gloss")?,
                    status: TermStatus::from_str_lossy(&r.get::<_, String>("status")?),
                    first_block_id: r.get("first_block_id")?,
                    mentions: r.get("mentions")?,
                    revisions: Vec::new(),
                    created_at: r.get("created_at")?,
                    updated_at: r.get("updated_at")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut rev = conn.prepare(
            "SELECT my_gloss, status, created_at FROM term_revisions
              WHERE term_id = ?1 ORDER BY created_at DESC",
        )?;
        for t in &mut terms {
            t.revisions = rev
                .query_map(params![t.id], |r| {
                    Ok(TermRevision {
                        my_gloss: r.get(0)?,
                        status: TermStatus::from_str_lossy(&r.get::<_, String>(1)?),
                        created_at: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        Ok(terms)
    }

    pub fn delete_term(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM terms WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Record every occurrence of a term's word within a book's text.
    ///
    /// Called once when a term is created. Case-insensitive substring match on
    /// a word boundary — deliberately simple, because a missed occurrence
    /// costs an underline and a false one is visible and dismissible.
    pub fn index_term_mentions(&self, term_id: i64, book_id: i64, word: &str) -> Result<i64> {
        let needle = word.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(0);
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT b.id, b.text_norm
               FROM blocks b JOIN pages p ON b.page_id = p.id
              WHERE p.book_id = ?1",
        )?;
        let blocks: Vec<(i64, String)> = stmt
            .query_map(params![book_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut found = 0i64;
        for (block_id, text) in blocks {
            for (start, end) in word_spans(&text, &needle) {
                conn.execute(
                    "INSERT OR IGNORE INTO term_mentions
                         (term_id, block_id, char_start, char_end)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![term_id, block_id, start as i64, end as i64],
                )?;
                found += 1;
            }
        }
        Ok(found)
    }

    /// Term occurrences inside a window of blocks, for underlining the text.
    pub fn term_mentions_for_blocks(&self, block_ids: &[i64]) -> Result<Vec<TermMention>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let placeholders = vec!["?"; block_ids.len()].join(",");
        let sql = format!(
            "SELECT m.block_id, m.char_start, m.char_end, t.id, t.term, t.my_gloss, t.status
               FROM term_mentions m JOIN terms t ON m.term_id = t.id
              WHERE m.block_id IN ({placeholders})
              ORDER BY m.char_start"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(block_ids), |r| {
            Ok(TermMention {
                block_id: r.get(0)?,
                char_start: r.get(1)?,
                char_end: r.get(2)?,
                term_id: r.get(3)?,
                term: r.get(4)?,
                my_gloss: r.get(5)?,
                status: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- arguments -------------------------------------------------------

    pub fn create_argument(
        &self,
        book_id: i64,
        label: &str,
        anchor_block_id: Option<i64>,
    ) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO arguments (book_id, label, anchor_block_id) VALUES (?1, ?2, ?3)",
            params![book_id, label.trim(), anchor_block_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn save_argument(
        &self,
        id: i64,
        label: &str,
        conclusion: &str,
        verdict: &str,
        gap: &str,
        notes: &str,
    ) -> Result<()> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE arguments
                SET label = ?2, conclusion = ?3, verdict = ?4, gap = ?5,
                    notes = ?6, updated_at = datetime('now')
              WHERE id = ?1",
            params![id, label.trim(), conclusion.trim(), verdict, gap, notes],
        )?;
        if changed == 0 {
            return Err(AppError::Invalid(format!("no argument with id {id}")));
        }
        Ok(())
    }

    /// Replace an argument's premises wholesale.
    ///
    /// Reconstruction is iterative — you reorder, merge, and split premises
    /// constantly — so diffing rows would be a great deal of machinery to
    /// preserve ids nothing else refers to.
    pub fn set_premises(&self, argument_id: i64, premises: &[NewPremise]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM premises WHERE argument_id = ?1",
            params![argument_id],
        )?;
        for (i, p) in premises.iter().enumerate() {
            tx.execute(
                "INSERT INTO premises
                     (argument_id, ordinal, text, implicit, source_block_id,
                      char_start, char_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    argument_id,
                    i as i64,
                    p.text.trim(),
                    p.implicit as i64,
                    p.source_block_id,
                    p.char_start,
                    p.char_end,
                ],
            )?;
        }
        tx.execute(
            "UPDATE arguments SET updated_at = datetime('now') WHERE id = ?1",
            params![argument_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_arguments(&self, book_id: i64) -> Result<Vec<Argument>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, book_id, label, conclusion, verdict, gap, anchor_block_id,
                    notes, created_at, updated_at
               FROM arguments WHERE book_id = ?1
              ORDER BY updated_at DESC",
        )?;
        let mut args: Vec<Argument> = stmt
            .query_map(params![book_id], |r| {
                Ok(Argument {
                    id: r.get("id")?,
                    book_id: r.get("book_id")?,
                    label: r.get("label")?,
                    conclusion: r.get("conclusion")?,
                    verdict: r.get("verdict")?,
                    gap: r.get("gap")?,
                    anchor_block_id: r.get("anchor_block_id")?,
                    notes: r.get("notes")?,
                    premises: Vec::new(),
                    links: Vec::new(),
                    created_at: r.get("created_at")?,
                    updated_at: r.get("updated_at")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut prem = conn.prepare(
            "SELECT id, ordinal, text, implicit, source_block_id, char_start, char_end
               FROM premises WHERE argument_id = ?1 ORDER BY ordinal",
        )?;
        let mut link = conn.prepare(
            "SELECT l.id, l.child_id, a.label, a.conclusion, l.role
               FROM argument_links l JOIN arguments a ON a.id = l.child_id
              WHERE l.parent_id = ?1",
        )?;
        for a in &mut args {
            a.premises = prem
                .query_map(params![a.id], |r| {
                    Ok(Premise {
                        id: r.get(0)?,
                        ordinal: r.get(1)?,
                        text: r.get(2)?,
                        implicit: r.get::<_, i64>(3)? != 0,
                        source_block_id: r.get(4)?,
                        char_start: r.get(5)?,
                        char_end: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            a.links = link
                .query_map(params![a.id], |r| {
                    Ok(ArgumentLink {
                        id: r.get(0)?,
                        child_id: r.get(1)?,
                        child_label: r.get(2)?,
                        child_conclusion: r.get(3)?,
                        role: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        Ok(args)
    }

    pub fn delete_argument(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM arguments WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn link_arguments(&self, parent_id: i64, child_id: i64, role: &str) -> Result<()> {
        if parent_id == child_id {
            return Err(AppError::Invalid(
                "an argument cannot support itself".into(),
            ));
        }
        self.lock().execute(
            "INSERT OR REPLACE INTO argument_links (parent_id, child_id, role)
             VALUES (?1, ?2, ?3)",
            params![parent_id, child_id, role],
        )?;
        Ok(())
    }

    pub fn unlink_arguments(&self, link_id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM argument_links WHERE id = ?1", params![link_id])?;
        Ok(())
    }

    // ---- thesis ----------------------------------------------------------

    pub fn set_thesis(&self, book_id: i64, statement: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO theses (book_id, statement) VALUES (?1, ?2)",
            params![book_id, statement.trim()],
        )?;
        Ok(())
    }

    pub fn get_thesis(&self, book_id: i64) -> Result<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT statement FROM theses WHERE book_id = ?1
                  ORDER BY created_at DESC, id DESC LIMIT 1",
                params![book_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// A place to attach a note, as the frontend sends it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NewAnchor {
    pub block_id: i64,
    pub char_start: i64,
    pub char_end: i64,
    pub anchor_text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredAnchor {
    pub id: i64,
    pub block_id: Option<i64>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub orphaned: bool,
    pub page_no: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TermMention {
    pub block_id: i64,
    pub char_start: i64,
    pub char_end: i64,
    pub term_id: i64,
    pub term: String,
    pub my_gloss: String,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NewPremise {
    pub text: String,
    pub implicit: bool,
    pub source_block_id: Option<i64>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
}

/// Character offsets of `needle` in `haystack`, on word boundaries.
///
/// Offsets are in `char`s rather than bytes because the frontend slices these
/// strings with JavaScript's UTF-16-ish indexing and a byte offset into a
/// paragraph containing an em dash would land mid-character.
fn word_spans(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let need: Vec<char> = needle.chars().collect();
    if need.is_empty() || need.len() > hay.len() {
        return Vec::new();
    }
    let boundary = |c: Option<&char>| c.is_none_or(|c| !c.is_alphanumeric());

    let mut out = Vec::new();
    let mut i = 0;
    while i + need.len() <= hay.len() {
        if hay[i..i + need.len()] == need[..]
            && boundary(i.checked_sub(1).and_then(|j| hay.get(j)))
            && boundary(hay.get(i + need.len()))
        {
            out.push((i, i + need.len()));
            i += need.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_spans_respect_boundaries() {
        // "substance" must not match inside "substances" — an underline that
        // covers half a word looks like a rendering bug.
        assert_eq!(word_spans("substance and substances", "substance"), vec![(0, 9)]);
        assert_eq!(word_spans("The Substance.", "substance"), vec![(4, 13)]);
        assert_eq!(word_spans("insubstantial", "substance"), vec![]);
    }

    #[test]
    fn word_spans_count_characters_not_bytes() {
        // The em dash is three bytes; a byte offset would put the highlight in
        // the wrong place for every word after it.
        let spans = word_spans("a\u{2014}substance", "substance");
        assert_eq!(spans, vec![(2, 11)]);
    }

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn book(db: &Db) -> i64 {
        db.create_book("Ethics", Some("Spinoza"), "early_modern").unwrap()
    }

    #[test]
    fn a_note_needs_a_body_or_a_tag() {
        let db = db();
        let b = book(&db);
        assert!(db.add_note(b, None, None, "", None, "   ", None).is_err());
        assert!(db
            .add_note(b, None, None, "", Some(Move::Premise), "", None)
            .is_ok());
    }

    #[test]
    fn a_changed_gloss_files_the_old_one() {
        let db = db();
        let b = book(&db);
        db.save_term(b, "substance", "a thing", TermStatus::Unclear, None)
            .unwrap();
        db.save_term(b, "substance", "that which is in itself", TermStatus::Working, None)
            .unwrap();

        let terms = db.list_terms(b).unwrap();
        assert_eq!(terms.len(), 1, "the term was duplicated rather than updated");
        assert_eq!(terms[0].my_gloss, "that which is in itself");
        assert_eq!(
            terms[0].revisions.len(),
            1,
            "the earlier reading was thrown away"
        );
        assert_eq!(terms[0].revisions[0].my_gloss, "a thing");
    }

    #[test]
    fn resaving_the_same_gloss_files_nothing() {
        let db = db();
        let b = book(&db);
        db.save_term(b, "mode", "an affection", TermStatus::Unclear, None)
            .unwrap();
        db.save_term(b, "mode", "an affection", TermStatus::Settled, None)
            .unwrap();
        assert!(db.list_terms(b).unwrap()[0].revisions.is_empty());
    }

    #[test]
    fn premises_are_replaced_wholesale_and_renumbered() {
        let db = db();
        let b = book(&db);
        let a = db.create_argument(b, "I P5", None).unwrap();
        db.set_premises(
            a,
            &[
                NewPremise { text: "one".into(), implicit: false, source_block_id: None, char_start: None, char_end: None },
                NewPremise { text: "two".into(), implicit: true, source_block_id: None, char_start: None, char_end: None },
            ],
        )
        .unwrap();
        db.set_premises(
            a,
            &[NewPremise { text: "only".into(), implicit: false, source_block_id: None, char_start: None, char_end: None }],
        )
        .unwrap();

        let args = db.list_arguments(b).unwrap();
        assert_eq!(args[0].premises.len(), 1);
        assert_eq!(args[0].premises[0].ordinal, 0);
        assert_eq!(args[0].premises[0].text, "only");
    }

    #[test]
    fn a_note_can_be_attached_in_several_places() {
        // A selection dragged across a page boundary covers two paragraphs and
        // has always meant to attach to both.
        let db = db();
        let b = book(&db);
        let page = db.add_page(b, "h1", "o.jpg", "p.jpg").unwrap();
        db.save_ocr_result(
            page,
            "m",
            "raw",
            &[
                crate::models::DraftBlock {
                    kind: crate::models::BlockKind::Paragraph,
                    text_raw: "the first paragraph".into(),
                    text_norm: "the first paragraph".into(),
                },
                crate::models::DraftBlock {
                    kind: crate::models::BlockKind::Paragraph,
                    text_raw: "the second paragraph".into(),
                    text_norm: "the second paragraph".into(),
                },
            ],
        )
        .unwrap();
        let blocks = db.list_blocks(page).unwrap();

        let note = db
            .add_note(b, Some(blocks[0].id), Some((4, 9)), "first", None, "spans both", None)
            .unwrap();
        db.add_note_anchors(
            note,
            &[NewAnchor {
                block_id: blocks[1].id,
                char_start: 4,
                char_end: 10,
                anchor_text: "second".into(),
            }],
        )
        .unwrap();

        // Both paragraphs must mark it, not only the one it was written from.
        let marked = db
            .notes_for_blocks(&[blocks[0].id, blocks[1].id])
            .unwrap();
        assert_eq!(marked.len(), 2, "{marked:#?}");
        assert!(marked.iter().all(|n| n.id == note));
        assert_eq!(
            marked.iter().map(|n| n.anchor_text.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );

        // But it is still one note in the list, not two.
        assert_eq!(db.list_notes(b, None).unwrap().len(), 1);
        assert_eq!(db.note_anchors(note).unwrap().len(), 2);
    }

    #[test]
    fn attaching_the_same_place_twice_is_harmless() {
        let db = db();
        let b = book(&db);
        let page = db.add_page(b, "h9", "o.jpg", "p.jpg").unwrap();
        db.save_ocr_result(
            page,
            "m",
            "raw",
            &[crate::models::DraftBlock {
                kind: crate::models::BlockKind::Paragraph,
                text_raw: "hello there".into(),
                text_norm: "hello there".into(),
            }],
        )
        .unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;

        let note = db
            .add_note(b, None, None, "", Some(Move::Premise), "", None)
            .unwrap();
        let anchor = NewAnchor {
            block_id: block,
            char_start: 0,
            char_end: 5,
            anchor_text: "hello".into(),
        };
        db.add_note_anchors(note, std::slice::from_ref(&anchor)).unwrap();
        db.add_note_anchors(note, std::slice::from_ref(&anchor)).unwrap();
        assert_eq!(db.note_anchors(note).unwrap().len(), 1);
    }

    #[test]
    fn an_argument_cannot_support_itself() {
        let db = db();
        let b = book(&db);
        let a = db.create_argument(b, "A", None).unwrap();
        assert!(db.link_arguments(a, a, "supports").is_err());
    }
}
