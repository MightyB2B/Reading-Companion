//! EPUB import.
//!
//! An EPUB is a zip of XHTML documents plus a spine giving their reading
//! order, so extraction is a matter of walking the spine and running each
//! chapter through the HTML extractor. No OCR is involved and none is wanted:
//! the words are already words.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{AppError, Result};

use super::{html, paginate, Document, SourcePage};

/// Flatten the table of contents into anchor id -> label.
///
/// Two things make this necessary. The entries nest, and only the top level is
/// handed over directly — a real Bible listed sixty-six books as children of
/// two testaments, so reading the top level alone found three entries out of
/// sixty-seven. And the targets carry fragments (`chapter.html#pgepubid00003`)
/// pointing *into* a file rather than at it, because a publisher's file
/// boundaries follow byte size rather than chapters: in that book the second
/// file opened midway through Genesis.
fn toc_anchors(points: &[epub::doc::NavPoint], into: &mut HashMap<String, String>) {
    for point in points {
        let target = point.content.to_string_lossy();
        if let Some((_, fragment)) = target.rsplit_once('#') {
            if !fragment.is_empty() {
                into.insert(fragment.to_string(), point.label.trim().to_string());
            }
        }
        toc_anchors(&point.children, into);
    }
}

/// Read an EPUB into pages.
///
/// Chapters are paginated into workable chunks rather than handed over whole,
/// because a chapter can run to thousands of words and the reader navigates by
/// page.
pub fn read(path: &Path) -> Result<Document> {
    let mut doc = epub::doc::EpubDoc::new(path)
        .map_err(|e| AppError::Invalid(format!("could not open the EPUB: {e}")))?;

    // `mdata` hands back the metadata entry, not the string inside it.
    let title = doc.mdata("title").map(|m| m.value.clone());
    let author = doc.mdata("creator").map(|m| m.value.clone());

    let mut anchors = HashMap::new();
    toc_anchors(&doc.toc, &mut anchors);

    // "Chapters" rather than "pages": a spine entry is a document, and an EPUB
    // has no pages at all until a reader chooses a font size.
    let count = doc.get_num_chapters();

    // Text is accumulated across the whole spine and only broken where the
    // book names a new division — never at a file boundary.
    //
    // Publishers split by file size rather than by content: in one real book
    // the second file opened midway through Genesis, so paginating each file
    // separately cut every chapter that straddled a boundary in two and
    // reported Genesis as having fifty-three chapters instead of fifty.
    //
    // The document's own <title> is deliberately unused as a fallback. Those
    // same size-split files all carry the same <title>, which labelled a
    // hundred and twenty pages of a real import with one line of front matter.
    let mut pages: Vec<SourcePage> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut label: Option<String> = None;

    for index in 0..count {
        if !doc.set_current_chapter(index) {
            continue;
        }
        // Some spine entries are covers or navigation documents with no prose;
        // those simply produce nothing and are skipped.
        let Some((content, _mime)) = doc.get_current_str() else {
            continue;
        };

        for item in html::extract(&content, &anchors) {
            if item.is_heading {
                pages.extend(paginate(std::mem::take(&mut run), label.take()));
                label = Some(item.text);
            } else {
                run.push(item.text);
            }
        }
    }
    pages.extend(paginate(run, label));

    if pages.is_empty() {
        return Err(AppError::Invalid(
            "no readable text was found in that EPUB".into(),
        ));
    }

    Ok(Document {
        title,
        author,
        pages,
    })
}
