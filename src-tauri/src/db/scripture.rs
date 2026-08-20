//! Storing what the citation scanner found, and answering the question that
//! makes it worth storing: *what in my library bears on this passage?*

use rusqlite::{params, OptionalExtension};

use super::Db;
use crate::error::Result;
use crate::scripture::{find_references, Reference};

/// A citation, with enough context to show it in a list without a second
/// query per row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredRef {
    pub id: i64,
    pub book_id: i64,
    pub block_id: i64,
    pub char_start: i64,
    pub char_end: i64,
    pub surface: String,
    pub osis_book: String,
    pub chapter: i64,
    pub verse_start: Option<i64>,
    pub verse_end: Option<i64>,
    pub confidence: f64,
    pub corrected: bool,
    /// Display label — `Romans 3:23`.
    pub label: String,
}

/// One entry in the reverse lookup: somewhere in the library that cites a
/// passage, with the sentence it was cited in.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Citation {
    pub book_id: i64,
    pub book_title: String,
    pub block_id: i64,
    pub page_no: Option<i64>,
    pub surface: String,
    pub label: String,
    /// The words around the citation, so the list is readable on its own.
    pub context: String,
}

fn label_of(osis: &str, chapter: i64, v_start: Option<i64>, v_end: Option<i64>) -> String {
    let name = crate::scripture::canonical_name(osis);
    match (v_start, v_end) {
        (Some(a), Some(b)) if b > a => format!("{name} {chapter}:{a}-{b}"),
        (Some(a), _) => format!("{name} {chapter}:{a}"),
        _ => format!("{name} {chapter}"),
    }
}

impl Db {
    /// Replace the citations recorded for one block.
    ///
    /// Corrections the reader made by hand are preserved: a rescan happens
    /// whenever a block is edited or re-transcribed, and silently discarding
    /// a fix because the paragraph was touched afterwards would make the
    /// correction feature pointless.
    pub fn replace_refs_for_block(
        &self,
        book_id: i64,
        block_id: i64,
        found: &[Reference],
    ) -> Result<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM refs WHERE block_id = ?1 AND corrected = 0",
            params![block_id],
        )?;
        let mut written = 0;
        for r in found {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO refs
                     (book_id, block_id, char_start, char_end, surface,
                      osis_book, chapter, verse_start, verse_end, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    book_id,
                    block_id,
                    r.char_start as i64,
                    r.char_end as i64,
                    r.surface,
                    r.osis_book,
                    r.chapter as i64,
                    r.verse_start.map(|v| v as i64),
                    r.verse_end.map(|v| v as i64),
                    r.confidence as f64,
                ],
            )?;
            written += changed;
        }
        tx.commit()?;
        Ok(written)
    }

    /// Scan a whole book for scripture citations.
    ///
    /// Runs over `text_norm` because that is what the reader sees and what
    /// the offsets have to line up with.
    pub fn scan_book_references(&self, book_id: i64) -> Result<usize> {
        let blocks: Vec<(i64, String)> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT b.id, b.text_norm
                   FROM blocks b JOIN pages p ON b.page_id = p.id
                  WHERE p.book_id = ?1",
            )?;
            let rows = stmt.query_map(params![book_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut total = 0;
        for (block_id, text) in blocks {
            let found = find_references(&text);
            if found.is_empty() {
                // Still clear stale rows: an edit can remove a citation.
                self.replace_refs_for_block(book_id, block_id, &[])?;
                continue;
            }
            total += self.replace_refs_for_block(book_id, block_id, &found)?;
        }
        Ok(total)
    }

    /// Citations inside a window of blocks, for rendering them as links.
    pub fn refs_for_blocks(&self, block_ids: &[i64]) -> Result<Vec<StoredRef>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let placeholders = vec!["?"; block_ids.len()].join(",");
        let sql = format!(
            "SELECT id, book_id, block_id, char_start, char_end, surface,
                    osis_book, chapter, verse_start, verse_end, confidence, corrected
               FROM refs WHERE block_id IN ({placeholders})
              ORDER BY block_id, char_start"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(block_ids), |r| {
            let osis: String = r.get("osis_book")?;
            let chapter: i64 = r.get("chapter")?;
            let vs: Option<i64> = r.get("verse_start")?;
            let ve: Option<i64> = r.get("verse_end")?;
            Ok(StoredRef {
                id: r.get("id")?,
                book_id: r.get("book_id")?,
                block_id: r.get("block_id")?,
                char_start: r.get("char_start")?,
                char_end: r.get("char_end")?,
                surface: r.get("surface")?,
                label: label_of(&osis, chapter, vs, ve),
                osis_book: osis,
                chapter,
                verse_start: vs,
                verse_end: ve,
                confidence: r.get("confidence")?,
                corrected: r.get::<_, i64>("corrected")? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Correct a misparsed citation by hand.
    pub fn correct_ref(
        &self,
        id: i64,
        osis_book: &str,
        chapter: i64,
        verse_start: Option<i64>,
        verse_end: Option<i64>,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE refs
                SET osis_book = ?2, chapter = ?3, verse_start = ?4, verse_end = ?5,
                    confidence = 1.0, corrected = 1
              WHERE id = ?1",
            params![id, osis_book, chapter, verse_start, verse_end],
        )?;
        Ok(())
    }

    pub fn delete_ref(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM refs WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Everything in the library that cites a passage.
    ///
    /// This is the payoff of storing references at all, and the thing no
    /// commercial Bible software can do: it searches *the reader's own books*.
    /// A verse of null matches any citation of the chapter.
    pub fn citations_of(
        &self,
        osis_book: &str,
        chapter: i64,
        verse: Option<i64>,
    ) -> Result<Vec<Citation>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT r.book_id, bk.title, r.block_id, p.page_no, r.surface,
                    r.osis_book, r.chapter, r.verse_start, r.verse_end,
                    b.text_norm, r.char_start
               FROM refs r
               JOIN blocks b ON b.id = r.block_id
               JOIN pages  p ON p.id = b.page_id
               JOIN books  bk ON bk.id = r.book_id
              WHERE r.osis_book = ?1 AND r.chapter = ?2
                AND (?3 IS NULL
                     OR r.verse_start IS NULL
                     OR (?3 >= r.verse_start AND ?3 <= COALESCE(r.verse_end, r.verse_start)))
              ORDER BY bk.title, p.page_no, r.char_start",
        )?;
        let rows = stmt.query_map(params![osis_book, chapter, verse], |r| {
            let text: String = r.get("text_norm")?;
            let at: i64 = r.get("char_start")?;
            let osis: String = r.get("osis_book")?;
            let ch: i64 = r.get("chapter")?;
            let vs: Option<i64> = r.get("verse_start")?;
            let ve: Option<i64> = r.get("verse_end")?;
            Ok(Citation {
                book_id: r.get("book_id")?,
                book_title: r.get("title")?,
                block_id: r.get("block_id")?,
                page_no: r.get("page_no")?,
                surface: r.get("surface")?,
                label: label_of(&osis, ch, vs, ve),
                context: context_around(&text, at as usize),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Where a passage lives inside one particular book.
    ///
    /// This is what lets two panes follow each other: given a reference the
    /// reader just touched, find the place in the *other* book to move to.
    /// A Bible answers from its verses; anything else answers from the
    /// citations found in it, so a commentary follows the text and the text
    /// follows the commentary.
    pub fn locate_reference(
        &self,
        book_id: i64,
        osis_book: &str,
        chapter: i64,
        verse: Option<i64>,
    ) -> Result<Option<i64>> {
        let conn = self.lock();

        // The verse itself, if this book is a Bible.
        let direct: Option<i64> = conn
            .query_row(
                "SELECT block_id FROM verses
                  WHERE book_id = ?1 AND osis_book = ?2 AND chapter = ?3
                    AND (?4 IS NULL OR verse = ?4)
                  ORDER BY verse LIMIT 1",
                params![book_id, osis_book, chapter, verse],
                |r| r.get(0),
            )
            .optional()?;
        if direct.is_some() {
            return Ok(direct);
        }

        // Otherwise the nearest place this book cites it.
        Ok(conn
            .query_row(
                "SELECT r.block_id FROM refs r
                  WHERE r.book_id = ?1 AND r.osis_book = ?2 AND r.chapter = ?3
                    AND (?4 IS NULL
                         OR r.verse_start IS NULL
                         OR (?4 >= r.verse_start
                             AND ?4 <= COALESCE(r.verse_end, r.verse_start)))
                  ORDER BY r.char_start LIMIT 1",
                params![book_id, osis_book, chapter, verse],
                |r| r.get(0),
            )
            .optional()?)
    }

    // ---- verse addressing -------------------------------------------------

    /// Map a verse onto the block that holds it.
    pub fn add_verse(
        &self,
        book_id: i64,
        block_id: i64,
        osis_book: &str,
        chapter: i64,
        verse: i64,
        span: (i64, i64),
    ) -> Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO verses
                 (book_id, block_id, osis_book, chapter, verse, char_start, char_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![book_id, block_id, osis_book, chapter, verse, span.0, span.1],
        )?;
        Ok(())
    }

    /// The text of a verse from any Bible in the library.
    ///
    /// Returns None when no Bible has been imported, which is the normal
    /// state for a library of theology and not an error.
    pub fn verse_text(&self, osis_book: &str, chapter: i64, verse: i64) -> Result<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT SUBSTR(b.text_norm, v.char_start + 1, v.char_end - v.char_start)
                   FROM verses v JOIN blocks b ON b.id = v.block_id
                  WHERE v.osis_book = ?1 AND v.chapter = ?2 AND v.verse = ?3
                  LIMIT 1",
                params![osis_book, chapter, verse],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// A readable window of text around an offset.
///
/// Cut on word boundaries: a snippet that starts mid-word looks like a bug
/// rather than an excerpt.
fn context_around(text: &str, at: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = at.saturating_sub(90);
    let end = (at + 90).min(chars.len());

    let mut s = start;
    if s > 0 {
        while s < end && chars[s].is_alphanumeric() {
            s += 1;
        }
    }
    let mut e = end;
    if e < chars.len() {
        while e > s && chars[e - 1].is_alphanumeric() {
            e -= 1;
        }
    }

    let body: String = chars[s..e].iter().collect();
    let body = body.trim();
    format!(
        "{}{}{}",
        if s > 0 { "..." } else { "" },
        body,
        if e < chars.len() { "..." } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> (Db, i64, i64) {
        let db = Db::open_in_memory().unwrap();
        let book = db
            .create_book("Systematic Theology", Some("Charles Hodge"), "victorian")
            .unwrap();
        let page = db.add_page(book, "hash", "orig.jpg", "proc.jpg").unwrap();
        let text = "All have sinned, Rom. iii. 23; vi. 23, as Paul says.";
        db.save_ocr_result(
            page,
            "model",
            text,
            &[crate::models::DraftBlock {
                kind: crate::models::BlockKind::Paragraph,
                text_raw: text.into(),
                text_norm: text.into(),
            }],
        )
        .unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;
        (db, book, block)
    }

    #[test]
    fn scanning_a_book_records_every_citation() {
        let (db, book, block) = seeded();
        let n = db.scan_book_references(book).unwrap();
        assert_eq!(n, 2, "the semicolon continuation was missed");

        let stored = db.refs_for_blocks(&[block]).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].label, "Romans 3:23");
        assert_eq!(stored[1].label, "Romans 6:23");
    }

    #[test]
    fn rescanning_does_not_duplicate() {
        let (db, book, block) = seeded();
        db.scan_book_references(book).unwrap();
        db.scan_book_references(book).unwrap();
        assert_eq!(db.refs_for_blocks(&[block]).unwrap().len(), 2);
    }

    #[test]
    fn a_hand_correction_survives_a_rescan() {
        // Otherwise fixing a misparse is pointless: the next edit to the
        // paragraph would undo it.
        let (db, book, block) = seeded();
        db.scan_book_references(book).unwrap();
        let first = db.refs_for_blocks(&[block]).unwrap()[0].id;
        db.correct_ref(first, "Gen", 1, Some(1), Some(1)).unwrap();

        db.scan_book_references(book).unwrap();
        let after = db.refs_for_blocks(&[block]).unwrap();
        assert!(
            after.iter().any(|r| r.corrected && r.osis_book == "Gen"),
            "the correction was overwritten by the rescan"
        );
    }

    #[test]
    fn the_reverse_lookup_finds_the_citing_book() {
        let (db, book, _) = seeded();
        db.scan_book_references(book).unwrap();

        let hits = db.citations_of("Rom", 3, Some(23)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].book_title, "Systematic Theology");
        assert!(hits[0].context.contains("All have sinned"));
    }

    #[test]
    fn a_verse_outside_the_cited_range_does_not_match() {
        let (db, book, _) = seeded();
        db.scan_book_references(book).unwrap();
        assert!(db.citations_of("Rom", 3, Some(24)).unwrap().is_empty());
        assert!(db.citations_of("Rom", 9, Some(1)).unwrap().is_empty());
    }

    #[test]
    fn asking_for_a_chapter_matches_every_verse_in_it() {
        let (db, book, _) = seeded();
        db.scan_book_references(book).unwrap();
        assert_eq!(db.citations_of("Rom", 3, None).unwrap().len(), 1);
    }

    #[test]
    fn context_is_cut_on_word_boundaries() {
        let text = "a".repeat(200) + " middle words here " + &"b".repeat(200);
        let snippet = context_around(&text, 205);
        assert!(snippet.starts_with("..."), "{snippet}");
        assert!(snippet.ends_with("..."), "{snippet}");
        assert!(snippet.contains("middle words here"));
    }

    #[test]
    fn a_reference_is_located_by_the_citation_when_the_book_is_not_a_bible() {
        // How a commentary follows the text: Hodge has no verses, but it does
        // cite Romans 3, and that citation is where the other pane should go.
        let (db, book, block) = seeded();
        db.scan_book_references(book).unwrap();

        assert_eq!(
            db.locate_reference(book, "Rom", 3, Some(23)).unwrap(),
            Some(block)
        );
        assert_eq!(db.locate_reference(book, "Rom", 9, Some(1)).unwrap(), None);
    }

    #[test]
    fn a_bible_answers_from_its_verses_first() {
        let (db, book, block) = seeded();
        db.add_verse(book, block, "Rom", 3, 23, (0, 10)).unwrap();
        assert_eq!(
            db.locate_reference(book, "Rom", 3, Some(23)).unwrap(),
            Some(block)
        );
    }

    #[test]
    fn verse_lookup_is_empty_without_a_bible() {
        let (db, _, _) = seeded();
        assert_eq!(db.verse_text("Rom", 3, 23).unwrap(), None);
    }
}
