//! PDF import.
//!
//! Two quite different kinds of file share this extension. One carries a text
//! layer and can be read directly; the other is a bag of page images from a
//! scanner, where the "text" is a picture of text and OCR is the only route.
//!
//! This module handles the first and detects the second, so a scanned PDF
//! reports something a reader can act on instead of importing as a book of
//! blank pages.

use std::path::Path;

use crate::error::{AppError, Result};

use super::{clean_paragraph, Document, SourcePage};

/// Below this many characters, a page is treated as having no text layer.
///
/// A scanned page usually extracts to nothing at all, but a stray header or a
/// page number can leak through, so the threshold is not zero.
const MIN_CHARS_PER_PAGE: usize = 40;

/// Share of pages that must have real text for the file to count as readable.
const MIN_TEXT_PAGE_RATIO: f32 = 0.5;

/// Split extracted page text into paragraphs.
///
/// PDF text extraction preserves the printed line breaks and nothing else, so
/// a blank line is the only paragraph signal available. Where a file has none,
/// the whole page becomes one block, which the reader can split by hand.
fn paragraphs_from_page(text: &str) -> Vec<String> {
    let by_blank: Vec<String> = text
        .split("\n\n")
        .filter_map(clean_paragraph)
        .collect();

    if !by_blank.is_empty() {
        return by_blank;
    }
    clean_paragraph(text).into_iter().collect()
}

pub fn read(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path)?;

    let pages_text = pdf_extract::extract_text_from_mem_by_pages(&bytes)
        .map_err(|e| AppError::Invalid(format!("could not read the PDF: {e}")))?;

    if pages_text.is_empty() {
        return Err(AppError::Invalid("that PDF has no pages".into()));
    }

    let with_text = pages_text
        .iter()
        .filter(|t| t.chars().filter(|c| c.is_alphabetic()).count() >= MIN_CHARS_PER_PAGE)
        .count();

    if (with_text as f32) < pages_text.len() as f32 * MIN_TEXT_PAGE_RATIO {
        return Err(AppError::Invalid(format!(
            "this PDF has no text layer — {with_text} of {} pages contain text. \
             It is probably a scan, so its pages are pictures rather than words. \
             Export the pages as images and import them as photographs instead.",
            pages_text.len()
        )));
    }

    let pages: Vec<SourcePage> = pages_text
        .into_iter()
        .enumerate()
        .filter_map(|(i, text)| {
            let paragraphs = paragraphs_from_page(&text);
            (!paragraphs.is_empty()).then(|| SourcePage {
                // A PDF paginates itself, so its own numbering is used rather
                // than invented.
                page_no: Some(i as i64 + 1),
                label: None,
                paragraphs,
            })
        })
        .collect();

    if pages.is_empty() {
        return Err(AppError::Invalid(
            "no readable text was found in that PDF".into(),
        ));
    }

    Ok(Document {
        title: path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
        author: None,
        pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_page_on_blank_lines() {
        let text = "The first paragraph of the page.\n\nThe second paragraph of the page.";
        let paras = paragraphs_from_page(text);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0], "The first paragraph of the page.");
    }

    #[test]
    fn keeps_a_page_with_no_blank_lines_as_one_block() {
        // Plenty of PDFs extract without any paragraph signal at all.
        let text = "A single run of text\nwith line breaks\nbut no blank lines.";
        let paras = paragraphs_from_page(text);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0], "A single run of text with line breaks but no blank lines.");
    }

    #[test]
    fn a_blank_page_yields_nothing() {
        assert!(paragraphs_from_page("   \n\n  ").is_empty());
    }
}
