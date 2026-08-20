//! Reading a book as one continuous column, and keeping anchors attached
//! when the text underneath them is rebuilt.

use rusqlite::params;

use super::Db;
use crate::error::Result;

/// A block in book order, carrying the page it came from.
///
/// `starts_page` is what the reading column draws the page marker from. It is
/// computed here rather than in the frontend because "first block of a page"
/// depends on the ordering, and the ordering is this query's business.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowBlock {
    pub id: i64,
    pub page_id: i64,
    pub page_no: Option<i64>,
    pub ordinal: i64,
    pub kind: String,
    pub text_norm: String,
    pub user_edited: bool,
    /// This block is the first on its page, so the marker goes above it.
    pub starts_page: bool,
    /// The page has not been transcribed yet, so the text stops here.
    pub pending: bool,
}

impl Db {
    /// Every block of a book, in reading order.
    ///
    /// Pages are ordered by their printed number, with un-numbered pages
    /// after the numbered ones in import order — an un-numbered page has no
    /// claim to a position, and guessing one would silently reorder the book.
    ///
    /// Returned whole rather than paged: the largest book in a personal
    /// library is a few thousand blocks, the frontend windows the rendering
    /// anyway, and a cursor API would mean re-deriving `starts_page` across
    /// every boundary.
    pub fn book_blocks(&self, book_id: i64) -> Result<Vec<FlowBlock>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT b.id, b.page_id, p.page_no, b.ordinal, b.kind, b.text_norm,
                    b.user_edited, p.ocr_status
               FROM blocks b JOIN pages p ON b.page_id = p.id
              WHERE p.book_id = ?1
              ORDER BY p.page_no IS NULL, p.page_no, p.id, b.ordinal",
        )?;
        let mut rows: Vec<FlowBlock> = stmt
            .query_map(params![book_id], |r| {
                let status: String = r.get("ocr_status")?;
                Ok(FlowBlock {
                    id: r.get("id")?,
                    page_id: r.get("page_id")?,
                    page_no: r.get("page_no")?,
                    ordinal: r.get("ordinal")?,
                    kind: r.get("kind")?,
                    text_norm: r.get("text_norm")?,
                    user_edited: r.get::<_, i64>("user_edited")? != 0,
                    starts_page: false,
                    pending: status != "done",
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut previous: Option<i64> = None;
        for b in &mut rows {
            b.starts_page = previous != Some(b.page_id);
            previous = Some(b.page_id);
        }
        Ok(rows)
    }

    /// Pages with no transcribed text, in reading order.
    ///
    /// The continuous column has to say where a gap is. A paragraph that runs
    /// off the bottom of page 47 into a page 48 that was never photographed
    /// should say so, rather than appearing to end mid-sentence.
    pub fn empty_pages(&self, book_id: i64) -> Result<Vec<(i64, Option<i64>, String)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.page_no, p.ocr_status
               FROM pages p
              WHERE p.book_id = ?1
                AND NOT EXISTS (SELECT 1 FROM blocks b WHERE b.page_id = p.id)
              ORDER BY p.page_no IS NULL, p.page_no, p.id",
        )?;
        let rows = stmt.query_map(params![book_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Re-attach notes whose block was destroyed by a re-transcription.
    ///
    /// Re-running OCR rebuilds a page's blocks with new ids, and the foreign
    /// key sets each anchor's `block_id` to null rather than deleting the
    /// note. This finds the text again.
    ///
    /// Works per *anchor*, not per note: a note attached to five paragraphs
    /// can lose one of them to a re-read of a single page while the other four
    /// are untouched, and repairing it wholesale would move anchors that were
    /// never broken.
    ///
    /// An anchor is only re-attached when its text appears **exactly once** in
    /// the book. Two matches means it is ambiguous, and guessing would put the
    /// reader's note on the wrong paragraph — worse than telling them it came
    /// loose. Returns (reattached, orphaned).
    pub fn reattach_notes(&self, book_id: i64) -> Result<(usize, usize)> {
        let loose: Vec<(i64, i64, String)> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT a.id, a.note_id, a.anchor_text
                   FROM note_anchors a JOIN notes n ON n.id = a.note_id
                  WHERE n.book_id = ?1 AND a.block_id IS NULL AND a.anchor_text <> ''",
            )?;
            let rows = stmt.query_map(params![book_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if loose.is_empty() {
            return Ok((0, 0));
        }

        let blocks: Vec<(i64, String)> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT b.id, b.text_norm FROM blocks b JOIN pages p ON b.page_id = p.id
                  WHERE p.book_id = ?1",
            )?;
            let rows = stmt.query_map(params![book_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut reattached = 0;
        let mut orphaned = 0;
        let mut touched: Vec<i64> = Vec::new();
        {
            let conn = self.lock();
            for (anchor_id, note_id, anchor) in loose {
                let hits: Vec<(i64, usize)> = blocks
                    .iter()
                    .filter_map(|(bid, text)| char_index_of(text, &anchor).map(|at| (*bid, at)))
                    .collect();

                match hits.as_slice() {
                    [(block_id, at)] => {
                        conn.execute(
                            "UPDATE note_anchors
                                SET block_id = ?2, char_start = ?3, char_end = ?4,
                                    orphaned = 0
                              WHERE id = ?1",
                            params![
                                anchor_id,
                                block_id,
                                *at as i64,
                                (*at + anchor.chars().count()) as i64
                            ],
                        )?;
                        reattached += 1;
                    }
                    _ => {
                        conn.execute(
                            "UPDATE note_anchors SET orphaned = 1 WHERE id = ?1",
                            params![anchor_id],
                        )?;
                        orphaned += 1;
                    }
                }
                touched.push(note_id);
            }

            // Keep the note's own copy of its primary anchor in step, so the
            // Notes list and the reading column agree about where it points.
            touched.sort_unstable();
            touched.dedup();
            for note_id in touched {
                conn.execute(
                    "UPDATE notes
                        SET block_id = (SELECT block_id FROM note_anchors
                                         WHERE note_id = ?1 ORDER BY id LIMIT 1),
                            char_start = (SELECT char_start FROM note_anchors
                                           WHERE note_id = ?1 ORDER BY id LIMIT 1),
                            char_end = (SELECT char_end FROM note_anchors
                                         WHERE note_id = ?1 ORDER BY id LIMIT 1),
                            orphaned = (SELECT orphaned FROM note_anchors
                                         WHERE note_id = ?1 ORDER BY id LIMIT 1)
                      WHERE id = ?1",
                    params![note_id],
                )?;
            }
        }
        Ok((reattached, orphaned))
    }
}

/// Character offset of `needle` in `haystack`, or None.
///
/// Characters rather than bytes, to match every other offset in the app.
fn char_index_of(haystack: &str, needle: &str) -> Option<usize> {
    let byte_at = haystack.find(needle)?;
    Some(haystack[..byte_at].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BlockKind, DraftBlock};

    fn draft(text: &str) -> DraftBlock {
        DraftBlock {
            kind: BlockKind::Paragraph,
            text_raw: text.into(),
            text_norm: text.into(),
        }
    }

    fn page_with(db: &Db, book: i64, hash: &str, page_no: Option<i64>, texts: &[&str]) -> i64 {
        let page = db.add_page(book, hash, "orig.jpg", "proc.jpg").unwrap();
        if let Some(n) = page_no {
            db.set_page_number(page, Some(n), "detected").unwrap();
        }
        let drafts: Vec<DraftBlock> = texts.iter().map(|t| draft(t)).collect();
        db.save_ocr_result(page, "m", "raw", &drafts).unwrap();
        page
    }

    #[test]
    fn blocks_come_back_in_printed_page_order_not_import_order() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        // Imported out of order, as photographing a book often goes.
        page_with(&db, book, "h2", Some(2), &["second page"]);
        page_with(&db, book, "h1", Some(1), &["first page"]);

        let flow = db.book_blocks(book).unwrap();
        assert_eq!(
            flow.iter().map(|b| b.text_norm.as_str()).collect::<Vec<_>>(),
            ["first page", "second page"]
        );
    }

    #[test]
    fn unnumbered_pages_go_last_rather_than_first() {
        // Sorting nulls first would silently move an un-numbered photo to the
        // front of the book.
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        page_with(&db, book, "hx", None, &["unknown page"]);
        page_with(&db, book, "h5", Some(5), &["page five"]);

        let flow = db.book_blocks(book).unwrap();
        assert_eq!(flow[0].text_norm, "page five");
        assert_eq!(flow[1].text_norm, "unknown page");
    }

    #[test]
    fn the_first_block_of_each_page_is_marked() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        page_with(&db, book, "h1", Some(1), &["one", "two"]);
        page_with(&db, book, "h2", Some(2), &["three"]);

        let flow = db.book_blocks(book).unwrap();
        assert_eq!(
            flow.iter().map(|b| b.starts_page).collect::<Vec<_>>(),
            [true, false, true]
        );
    }

    #[test]
    fn a_note_finds_its_text_again_after_a_re_transcription() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let page = page_with(&db, book, "h1", Some(1), &["The cat sat on the mat."]);
        let block = db.list_blocks(page).unwrap()[0].id;

        db.add_note(book, Some(block), Some((4, 7)), "cat", None, "why a cat?", None)
            .unwrap();

        // Re-running OCR destroys the blocks and their ids.
        db.save_ocr_result(page, "m", "raw", &[draft("The cat sat on the mat.")])
            .unwrap();
        let (reattached, orphaned) = db.reattach_notes(book).unwrap();
        assert_eq!((reattached, orphaned), (1, 0));

        let new_block = db.list_blocks(page).unwrap()[0].id;
        let notes = db.notes_for_blocks(&[new_block]).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].char_start, Some(4));
    }

    #[test]
    fn an_ambiguous_anchor_is_orphaned_rather_than_guessed() {
        // Putting the reader's note on the wrong paragraph is worse than
        // telling them it came loose.
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let page = page_with(&db, book, "h1", Some(1), &["the same words here"]);
        let block = db.list_blocks(page).unwrap()[0].id;
        db.add_note(book, Some(block), Some((0, 3)), "the same words", None, "n", None)
            .unwrap();

        db.save_ocr_result(
            page,
            "m",
            "raw",
            &[draft("the same words here"), draft("the same words again")],
        )
        .unwrap();

        let (reattached, orphaned) = db.reattach_notes(book).unwrap();
        assert_eq!((reattached, orphaned), (0, 1));
        // The note still exists — it was flagged, not deleted.
        assert_eq!(db.list_notes(book, None).unwrap().len(), 1);
    }

    #[test]
    fn a_page_with_no_text_is_reported_as_a_gap() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        page_with(&db, book, "h1", Some(1), &["text"]);
        let empty = db.add_page(book, "h2", "o.jpg", "p.jpg").unwrap();
        db.set_page_number(empty, Some(2), "manual").unwrap();

        let gaps = db.empty_pages(book).unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].1, Some(2));
    }
}
