//! Searching the reader's own library.
//!
//! The one thing a research tool has to be able to do, and the one thing this
//! application could not: find the passage you remember but cannot place. The
//! dictionary has had FTS5 from the start; the library never did.
//!
//! Notes and terms are searched in the same call as the text. A reader looking
//! for "substance" wants the paragraphs, the gloss they wrote, and the note
//! they left — asking which box to type it in is the application's problem
//! leaking into the reading.

use rusqlite::params;

use super::Db;
use crate::error::Result;

/// Where a hit came from, so the result list can group and label them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HitKind {
    /// The book's own text.
    Text,
    Note,
    Term,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub kind: HitKind,
    pub book_id: i64,
    pub book_title: String,
    /// Null for a book-level note or a term with no first occurrence.
    pub block_id: Option<i64>,
    pub page_no: Option<i64>,
    /// The matching text with the hit wrapped in «...», for the UI to mark up.
    pub snippet: String,
    /// FTS relevance, lower is better. Zero for notes and terms, which are
    /// matched by LIKE rather than ranked.
    pub rank: f64,
}

/// How many text hits to return. A search that floods the panel with four
/// hundred rows has not answered the question any better than twenty.
const TEXT_LIMIT: usize = 60;

impl Db {
    /// Search the library: text, notes, and terms together.
    ///
    /// `book_id` narrows to one book; None searches everything, which is the
    /// point — "where else have I read this" is the question worth asking.
    pub fn search_library(&self, query: &str, book_id: Option<i64>) -> Result<Vec<SearchHit>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.lock();
        let mut hits = Vec::new();

        // ---- the text -----------------------------------------------------
        //
        // The query is passed to FTS5 as a quoted phrase rather than raw. Bare
        // input is a query *language*: an unbalanced quote or a stray `NEAR`
        // is a syntax error, and typing `don't` should search for the word,
        // not fail. Quoting makes every input a literal phrase search, with
        // an internal quote doubled to escape it.
        let phrase = format!("\"{}\"", trimmed.replace('"', "\"\""));

        let mut stmt = conn.prepare(
            "SELECT b.id, p.book_id, bk.title, p.page_no,
                    snippet(blocks_fts, 0, '«', '»', '…', 24) AS snip,
                    bm25(blocks_fts) AS rank
               FROM blocks_fts
               JOIN blocks b  ON b.id = blocks_fts.rowid
               JOIN pages  p  ON p.id = b.page_id
               JOIN books  bk ON bk.id = p.book_id
              WHERE blocks_fts MATCH ?1
                AND (?2 IS NULL OR p.book_id = ?2)
              ORDER BY rank
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![phrase, book_id, TEXT_LIMIT as i64], |r| {
            Ok(SearchHit {
                kind: HitKind::Text,
                block_id: Some(r.get(0)?),
                book_id: r.get(1)?,
                book_title: r.get(2)?,
                page_no: r.get(3)?,
                snippet: r.get(4)?,
                rank: r.get(5)?,
            })
        })?;
        hits.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        drop(stmt);

        // ---- what the reader wrote ---------------------------------------
        //
        // Notes and terms are matched with LIKE rather than indexed. There are
        // hundreds of them, not hundreds of thousands, and an index that has
        // to be kept in step with every keystroke of an edit costs more than
        // the scan it saves.
        let like = format!("%{}%", trimmed.replace('%', "\\%").replace('_', "\\_"));

        let mut stmt = conn.prepare(
            "SELECT n.book_id, bk.title, n.block_id, p.page_no, n.body, n.anchor_text
               FROM notes n
               JOIN books bk ON bk.id = n.book_id
               LEFT JOIN blocks b ON b.id = n.block_id
               LEFT JOIN pages  p ON p.id = b.page_id
              WHERE (n.body LIKE ?1 ESCAPE '\\' OR n.anchor_text LIKE ?1 ESCAPE '\\')
                AND (?2 IS NULL OR n.book_id = ?2)
              ORDER BY n.updated_at DESC
              LIMIT 40",
        )?;
        let rows = stmt.query_map(params![like, book_id], |r| {
            let body: String = r.get(4)?;
            let anchor: String = r.get(5)?;
            Ok(SearchHit {
                kind: HitKind::Note,
                book_id: r.get(0)?,
                book_title: r.get(1)?,
                block_id: r.get(2)?,
                page_no: r.get(3)?,
                snippet: if body.trim().is_empty() {
                    format!("“{anchor}”")
                } else {
                    body
                },
                rank: 0.0,
            })
        })?;
        hits.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        drop(stmt);

        let mut stmt = conn.prepare(
            "SELECT t.book_id, bk.title, t.first_block_id, p.page_no, t.term, t.my_gloss
               FROM terms t
               JOIN books bk ON bk.id = t.book_id
               LEFT JOIN blocks b ON b.id = t.first_block_id
               LEFT JOIN pages  p ON p.id = b.page_id
              WHERE (t.term LIKE ?1 ESCAPE '\\' OR t.my_gloss LIKE ?1 ESCAPE '\\')
                AND (?2 IS NULL OR t.book_id = ?2)
              ORDER BY t.term
              LIMIT 40",
        )?;
        let rows = stmt.query_map(params![like, book_id], |r| {
            let term: String = r.get(4)?;
            let gloss: String = r.get(5)?;
            Ok(SearchHit {
                kind: HitKind::Term,
                book_id: r.get(0)?,
                book_title: r.get(1)?,
                block_id: r.get(2)?,
                page_no: r.get(3)?,
                snippet: format!("{term} — {gloss}"),
                rank: 0.0,
            })
        })?;
        hits.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);

        Ok(hits)
    }

    /// Every place in the library a word occurs, for looking one up in another
    /// book without leaving the passage you are reading.
    pub fn concordance(&self, word: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut hits = self.search_library(word, None)?;
        hits.retain(|h| h.kind == HitKind::Text);
        hits.truncate(limit);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BlockKind, DraftBlock};

    fn seeded() -> (Db, i64, i64) {
        let db = Db::open_in_memory().unwrap();
        let hodge = db
            .create_book("Systematic Theology", Some("Charles Hodge"), "victorian")
            .unwrap();
        let lewis = db
            .create_book("The Abolition of Man", Some("C. S. Lewis"), "early_20c")
            .unwrap();

        for (book, hash, text) in [
            (hodge, "h1", "In every science there are two factors: facts and ideas."),
            (hodge, "h2", "The Bible is no more a system of theology than nature is."),
            (lewis, "l1", "The task of the modern educator is not to cut down jungles."),
        ] {
            let page = db.add_page(book, hash, "o.jpg", "p.jpg").unwrap();
            db.set_page_number(page, Some(1), "detected").unwrap();
            db.save_ocr_result(
                page,
                "m",
                text,
                &[DraftBlock {
                    kind: BlockKind::Paragraph,
                    text_raw: text.into(),
                    text_norm: text.into(),
                }],
            )
            .unwrap();
        }
        (db, hodge, lewis)
    }

    #[test]
    fn finds_a_phrase_in_the_right_book() {
        let (db, hodge, _) = seeded();
        let hits = db.search_library("system of theology", None).unwrap();
        assert_eq!(hits.len(), 1, "{hits:#?}");
        assert_eq!(hits[0].book_id, hodge);
        assert_eq!(hits[0].kind, HitKind::Text);
        assert!(hits[0].snippet.contains('«'), "no hit marker: {}", hits[0].snippet);
    }

    #[test]
    fn searches_across_books_by_default_and_narrows_on_request() {
        // "the" is in one Hodge paragraph and the Lewis one, so an unscoped
        // search crosses books and a scoped one does not.
        let (db, hodge, lewis) = seeded();

        let everywhere = db.search_library("the", None).unwrap();
        assert_eq!(everywhere.len(), 2, "{everywhere:#?}");
        assert!(everywhere.iter().any(|h| h.book_id == hodge));
        assert!(everywhere.iter().any(|h| h.book_id == lewis));

        let scoped = db.search_library("the", Some(hodge)).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].book_id, hodge);
    }

    #[test]
    fn stemming_finds_a_different_inflection() {
        // The porter tokenizer is the reason to bother with FTS5 rather than
        // LIKE: "educators" has to find "educator".
        let (db, _, _) = seeded();
        assert_eq!(db.search_library("educators", None).unwrap().len(), 1);
    }

    #[test]
    fn a_stray_quote_is_a_search_not_a_syntax_error() {
        // Raw input is an FTS5 query language; unbalanced punctuation used to
        // be a hard error rather than a search that found nothing.
        let (db, _, _) = seeded();
        for query in ["\"", "don't", "facts AND", "NEAR(", "*", "^ideas"] {
            assert!(
                db.search_library(query, None).is_ok(),
                "{query:?} was treated as query syntax"
            );
        }
    }

    #[test]
    fn an_empty_query_returns_nothing_rather_than_everything() {
        let (db, _, _) = seeded();
        assert!(db.search_library("   ", None).unwrap().is_empty());
    }

    #[test]
    fn the_index_follows_an_edit() {
        let (db, hodge, _) = seeded();
        let block = db.search_library("jungles", None).unwrap();
        assert!(block.is_empty() || block[0].book_id != hodge);

        // Editing a paragraph must move the hit with it, or search returns
        // text the page no longer contains.
        let hit = db.search_library("two factors", None).unwrap();
        let id = hit[0].block_id.unwrap();
        db.edit_block(id, "In every science there are two elements.").unwrap();

        assert!(db.search_library("two factors", None).unwrap().is_empty());
        assert_eq!(db.search_library("two elements", None).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_block_removes_it_from_the_index() {
        let (db, _, _) = seeded();
        let id = db.search_library("jungles", None).unwrap()[0].block_id.unwrap();
        db.delete_block(id).unwrap();
        assert!(db.search_library("jungles", None).unwrap().is_empty());
    }

    #[test]
    fn notes_and_terms_are_found_alongside_the_text() {
        let (db, hodge, _) = seeded();
        let block = db.search_library("two factors", None).unwrap()[0]
            .block_id
            .unwrap();
        db.add_note(hodge, Some(block), Some((0, 2)), "In", None, "a note about induction", None)
            .unwrap();
        db.save_term(hodge, "science", "ordered knowledge", crate::study::TermStatus::Working, Some(block))
            .unwrap();

        let hits = db.search_library("induction", None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, HitKind::Note);

        let hits = db.search_library("ordered knowledge", None).unwrap();
        assert!(hits.iter().any(|h| h.kind == HitKind::Term));
    }

    #[test]
    fn the_concordance_only_returns_text() {
        let (db, hodge, _) = seeded();
        let block = db.search_library("two factors", None).unwrap()[0]
            .block_id
            .unwrap();
        db.add_note(hodge, Some(block), None, "", None, "science science", None)
            .unwrap();

        let hits = db.concordance("science", 10).unwrap();
        assert!(hits.iter().all(|h| h.kind == HitKind::Text));
    }
}
