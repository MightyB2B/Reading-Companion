//! Importing text that is already text.
//!
//! A photograph has to go through a vision model to become words. An EPUB, a
//! PDF with an embedded text layer, and a web page do not — the words are
//! already there, and running them through OCR would be slower and strictly
//! worse. These sources therefore bypass the model entirely and produce blocks
//! directly.
//!
//! What they share with the photograph path is everything downstream: the same
//! paragraph blocks, the same study method, the same coach. Only the way the
//! characters arrive differs.

pub mod epub_source;
pub mod html;
pub mod pdf_source;

use serde::{Deserialize, Serialize};

/// Where a document came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Photo,
    Epub,
    Pdf,
    Web,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Photo => "photo",
            SourceKind::Epub => "epub",
            SourceKind::Pdf => "pdf",
            SourceKind::Web => "web",
        }
    }

    /// Guess from a path or URL.
    pub fn from_source(source: &str) -> Self {
        let lower = source.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return SourceKind::Web;
        }
        match lower.rsplit('.').next() {
            Some("epub") => SourceKind::Epub,
            Some("pdf") => SourceKind::Pdf,
            _ => SourceKind::Photo,
        }
    }
}

/// One page's worth of extracted text.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcePage {
    /// The page number where the source has a real one — a PDF's own paging.
    /// `None` where the division into pages is ours, as in an EPUB chapter.
    pub page_no: Option<i64>,
    /// Chapter title, section heading, or similar, when the source offers one.
    pub label: Option<String>,
    /// Paragraphs, already separated.
    pub paragraphs: Vec<String>,
}

/// What a whole document extracted to.
#[derive(Debug, Clone)]
pub struct Document {
    pub title: Option<String>,
    pub author: Option<String>,
    pub pages: Vec<SourcePage>,
}

/// Paragraphs per page when the source has no pagination of its own.
///
/// A chapter can run to thousands of words. The method works on one paragraph
/// at a time and the reader navigates by page, so a chapter delivered as a
/// single enormous page would be unusable; this is a comfortable screenful.
pub const PARAGRAPHS_PER_PAGE: usize = 8;

/// The chapter part of a leading reference like `10:14`.
///
/// Scripture and other referenced texts number every unit `chapter:verse` and
/// carry no headings at all — the divisions are in the numbering. Without
/// reading it, a Bible imports as thousands of identical eight-verse pages
/// with nothing to navigate by.
fn leading_chapter(text: &str) -> Option<u32> {
    let token = text.split_whitespace().next()?;
    let (chapter, verse) = token.split_once(':')?;
    if verse.is_empty() || !verse.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    chapter.parse().ok()
}

/// Share of paragraphs that must open with a reference before the run is
/// paginated by it.
const REFERENCED_SHARE: f32 = 0.6;

/// Split a paragraph that has run several numbered units together.
///
/// Publishers do this freely — one `<p>` holding `3:4 … 3:5 …` — and it leaves
/// blocks of two and a half thousand characters where the text intends short
/// numbered units. That matters here beyond tidiness: the study method works
/// on one block at a time, and a chapter boundary hidden inside a fused
/// paragraph is a chapter the pagination cannot see.
///
/// Only applied to text already established as numbered, so ordinary prose
/// that happens to mention a time or a ratio is untouched.
fn split_references(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for (i, word) in words.iter().enumerate() {
        // A reference opening a new unit, not the one this paragraph began
        // with, and not a bare number inside a sentence.
        if i > 0 && !current.is_empty() && is_reference(word) {
            out.push(current.join(" "));
            current.clear();
        }
        current.push(word);
    }
    if !current.is_empty() {
        out.push(current.join(" "));
    }
    out
}

/// Is this token a `chapter:verse` reference?
fn is_reference(token: &str) -> bool {
    let Some((chapter, verse)) = token.split_once(':') else {
        return false;
    };
    !chapter.is_empty()
        && !verse.is_empty()
        && chapter.chars().all(|c| c.is_ascii_digit())
        && verse.chars().all(|c| c.is_ascii_digit())
}

/// Split a run of paragraphs into pages of a workable size.
///
/// Where the text numbers itself, its own divisions are used; otherwise a
/// fixed count, which is all that can be done for prose.
pub fn paginate(paragraphs: Vec<String>, label: Option<String>) -> Vec<SourcePage> {
    if paragraphs.is_empty() {
        return Vec::new();
    }

    let referenced = paragraphs
        .iter()
        .filter(|p| leading_chapter(p).is_some())
        .count();
    if referenced as f32 >= paragraphs.len() as f32 * REFERENCED_SHARE {
        return paginate_by_reference(paragraphs, label);
    }

    paragraphs
        .chunks(PARAGRAPHS_PER_PAGE)
        .enumerate()
        .map(|(i, chunk)| SourcePage {
            page_no: None,
            // Only the first page of a chapter carries its title; repeating it
            // on every chunk would put a heading in the middle of the prose.
            label: if i == 0 { label.clone() } else { None },
            paragraphs: chunk.to_vec(),
        })
        .collect()
}

/// One page per numbered chapter.
///
/// A chapter is the unit the reader thinks in and the unit the text declares,
/// so it beats any count we could pick. It also makes the page count mean
/// something: a Bible goes from three thousand arbitrary pages to roughly
/// twelve hundred real chapters.
fn paginate_by_reference(paragraphs: Vec<String>, label: Option<String>) -> Vec<SourcePage> {
    let mut pages: Vec<SourcePage> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut current: Option<u32> = None;
    let mut label = label;

    // Separate any units the source ran together first, or a chapter boundary
    // buried inside a fused paragraph goes unseen.
    let paragraphs: Vec<String> = paragraphs.iter().flat_map(|p| split_references(p)).collect();

    for paragraph in paragraphs {
        let chapter = leading_chapter(&paragraph);
        // A paragraph without a reference continues whatever it follows.
        if let Some(chapter) = chapter {
            if current.is_some_and(|c| c != chapter) && !run.is_empty() {
                pages.push(SourcePage {
                    page_no: None,
                    label: label.take(),
                    paragraphs: std::mem::take(&mut run),
                });
            }
            current = Some(chapter);
        }
        run.push(paragraph);
    }

    if !run.is_empty() {
        pages.push(SourcePage {
            page_no: None,
            label: label.take(),
            paragraphs: run,
        });
    }
    pages
}

/// Tidy a paragraph pulled out of markup.
///
/// Collapses the whitespace that XHTML and PDF layout both leave behind, and
/// drops anything too short to be prose — a stray navigation word, a page
/// artefact, a lone bullet.
pub fn clean_paragraph(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.chars().filter(|c| c.is_alphabetic()).count() < 3 {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_each_kind_of_source() {
        assert_eq!(SourceKind::from_source("https://example.com/a"), SourceKind::Web);
        assert_eq!(SourceKind::from_source("http://example.com"), SourceKind::Web);
        assert_eq!(SourceKind::from_source("C:/books/moby.EPUB"), SourceKind::Epub);
        assert_eq!(SourceKind::from_source("/home/a/paper.pdf"), SourceKind::Pdf);
        assert_eq!(SourceKind::from_source("/home/a/page.jpg"), SourceKind::Photo);
    }

    #[test]
    fn collapses_whitespace_left_by_markup() {
        assert_eq!(
            clean_paragraph("  The quick\n   brown\tfox  ").as_deref(),
            Some("The quick brown fox")
        );
    }

    #[test]
    fn drops_fragments_that_are_not_prose() {
        assert_eq!(clean_paragraph("   "), None);
        assert_eq!(clean_paragraph("•"), None);
        assert_eq!(clean_paragraph("12"), None);
    }

    #[test]
    fn paginates_a_long_chapter_into_workable_pages() {
        let paras: Vec<String> = (0..20).map(|i| format!("Paragraph number {i}.")).collect();
        let pages = paginate(paras, Some("Chapter I".into()));

        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].paragraphs.len(), PARAGRAPHS_PER_PAGE);
        assert_eq!(pages[2].paragraphs.len(), 4);
        // The chapter title appears once, not on every page of the chapter.
        assert_eq!(pages[0].label.as_deref(), Some("Chapter I"));
        assert_eq!(pages[1].label, None);
    }

    #[test]
    fn reads_a_leading_reference() {
        assert_eq!(leading_chapter("10:14 Now the weight of gold."), Some(10));
        assert_eq!(leading_chapter("3:1 In the beginning."), Some(3));
        // Ordinary prose carries no such reference.
        assert_eq!(leading_chapter("The office of the latter is to state."), None);
        assert_eq!(leading_chapter("At 10:14 he departed."), None);
    }

    /// A referenced text declares its own divisions, and they beat any count
    /// we could pick: pages become chapters rather than arbitrary runs.
    #[test]
    fn a_referenced_text_paginates_by_its_own_chapters() {
        let paragraphs: Vec<String> = vec![
            "9:1 First verse of nine.",
            "9:2 Second verse of nine.",
            "9:3 Third verse of nine.",
            "10:1 First verse of ten.",
            "10:2 Second verse of ten.",
            "11:1 The eleventh opens.",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        let pages = paginate(paragraphs, Some("The Book of Joshua".into()));

        assert_eq!(pages.len(), 3, "one page per chapter, got {pages:#?}");
        assert_eq!(pages[0].paragraphs.len(), 3);
        assert_eq!(pages[1].paragraphs.len(), 2);
        // The division's name belongs to where it starts, not to every page.
        assert_eq!(pages[0].label.as_deref(), Some("The Book of Joshua"));
        assert_eq!(pages[1].label, None);
    }

    /// Publishers run several numbered units into one paragraph. Left fused,
    /// a block reaches thousands of characters and any chapter boundary
    /// inside it is invisible to pagination.
    #[test]
    fn units_the_source_ran_together_are_separated() {
        let fused = "3:4 The first unit of text here. 3:5 The second unit of text here.";
        let parts = split_references(fused);

        assert_eq!(parts.len(), 2, "got {parts:#?}");
        assert!(parts[0].starts_with("3:4"));
        assert!(parts[1].starts_with("3:5"));
    }

    /// The case that made chapter counts wrong: a boundary hidden mid-block.
    #[test]
    fn a_chapter_boundary_inside_a_fused_paragraph_is_found() {
        let paragraphs = vec![
            "26:64 The last unit of one chapter. 27:1 The first unit of the next.".to_string(),
            "27:2 And it continues.".to_string(),
        ];
        let pages = paginate(paragraphs, None);

        assert_eq!(pages.len(), 2, "got {pages:#?}");
        assert_eq!(pages[0].paragraphs.len(), 1);
        assert_eq!(pages[1].paragraphs.len(), 2);
    }

    #[test]
    fn a_reference_is_told_from_a_time_or_a_ratio() {
        assert!(is_reference("3:16"));
        assert!(!is_reference("half:past"));
        assert!(!is_reference("3:"));
        assert!(!is_reference(":16"));
    }

    #[test]
    fn splitting_loses_nothing() {
        let text = "1:1 One two three. 1:2 Four five six. 1:3 Seven eight.";
        assert_eq!(split_references(text).join(" "), text);
    }

    #[test]
    fn a_long_chapter_is_not_split_by_count() {
        // Twenty verses of one chapter stay together: the text says they
        // belong together, and that outranks a tidy page length.
        let paragraphs: Vec<String> = (1..=20)
            .map(|v| format!("7:{v} A verse of the seventh chapter."))
            .collect();
        let pages = paginate(paragraphs, None);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].paragraphs.len(), 20);
    }

    #[test]
    fn prose_still_paginates_by_count() {
        // Only a mostly-referenced run switches strategy.
        let paragraphs: Vec<String> = (0..20)
            .map(|i| format!("An ordinary paragraph, number {i}."))
            .collect();
        assert_eq!(paginate(paragraphs, None).len(), 3);
    }

    #[test]
    fn an_unreferenced_line_continues_the_chapter_it_follows() {
        let paragraphs: Vec<String> = vec![
            "4:1 The chapter opens.",
            "A heading or note carrying no reference.",
            "4:2 And continues.",
            "5:1 A new chapter.",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        let pages = paginate(paragraphs, None);
        assert_eq!(pages.len(), 2, "got {pages:#?}");
        assert_eq!(pages[0].paragraphs.len(), 3);
    }

    #[test]
    fn an_empty_chapter_yields_no_pages() {
        assert!(paginate(Vec::new(), Some("Empty".into())).is_empty());
    }
}
