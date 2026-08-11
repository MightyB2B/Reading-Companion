//! Recovering a book's structure from its headings.
//!
//! Segmentation already tells a heading from a paragraph. This goes one step
//! further and asks what *kind* of heading it is, so the reader can navigate
//! by chapter instead of scrolling a list of page numbers — which stops being
//! usable at about the twentieth page and becomes absurd at the two hundred
//! and eighty-second, which is what one ordinary EPUB produced.
//!
//! Everything here works on the text alone. There is no table of contents to
//! consult: a photographed page has none, and an EPUB's is frequently a
//! navigation document the extractor has already discarded as furniture.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadingLevel {
    /// The largest division: a book, a part, the introduction.
    ///
    /// Distinct from a chapter because they nest. Hodge's page one carries
    /// `INTRODUCTION.` and `CHAPTER I.` together, and treating both as
    /// chapters means the chapter you are actually in gets lost behind the
    /// part containing it.
    Part,
    /// A chapter within a part.
    Chapter,
    /// A numbered division within a chapter: `§ 1`, `Sec. 4`.
    ///
    /// A section is a *span*, not a property of one page. `§ 1` runs until
    /// `§ 2` begins, which may be two pages later — treating it as belonging
    /// only to the page it is printed on leaves every following page looking
    /// as though it has no place in the book.
    Section,
    /// A descriptive heading naming what a passage is about, subordinate to
    /// the numbered section containing it.
    ///
    /// "Necessity for System in Theology." sits inside `§ 1`; it does not end
    /// it. Keeping it below section level is what stops it displacing the
    /// section a page actually belongs to.
    Topic,
    /// A heading with no structural claim — a running head, a caption.
    Minor,
}

/// Words that open a chapter.
const CHAPTER_WORDS: [&str; 6] = ["chapter", "canto", "act", "letter", "epistle", "lecture"];

/// Words that open a division larger than a chapter.
const PART_WORDS: [&str; 3] = ["book", "part", "volume"];

/// Divisions that stand alone, with no number after them. These sit at part
/// level: an introduction contains chapters, it is not one.
const STANDALONE_DIVISIONS: [&str; 10] = [
    "introduction",
    "preface",
    "prologue",
    "epilogue",
    "foreword",
    "afterword",
    "conclusion",
    "appendix",
    "postscript",
    "contents",
];

/// Words a title leaves in lower case.
const MINOR_WORDS: [&str; 20] = [
    "a", "an", "the", "of", "in", "on", "for", "and", "or", "to", "from", "by", "with", "at", "as",
    "but", "nor", "into", "upon", "per",
];

/// Longest a line can be and still be read as a title.
const MAX_TITLE_CHARS: usize = 80;

/// Is this line set in title case?
///
/// The signal that catches a heading a period would otherwise hide.
/// "Necessity for System in Theology." capitalises every word that carries
/// meaning and leaves "for" and "in" alone; a sentence capitalises only its
/// first word. Without this, an italic section heading was stored as a
/// paragraph — put into the summarising queue and left out of the outline.
pub fn is_title_case(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() > MAX_TITLE_CHARS {
        return false;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if !(2..=10).contains(&words.len()) {
        return false;
    }

    let mut carrying = 0usize;
    let mut capitalised = 0usize;

    for (i, word) in words.iter().enumerate() {
        let core = word.trim_matches(|c: char| !c.is_alphanumeric());
        if core.is_empty() {
            continue;
        }
        // A minor word may stay lower case anywhere but the opening.
        if i > 0 && MINOR_WORDS.contains(&core.to_lowercase().as_str()) {
            continue;
        }
        carrying += 1;
        if core.chars().next().is_some_and(char::is_uppercase) {
            capitalised += 1;
        }
    }

    carrying >= 2 && capitalised == carrying
}

/// Is every cased letter uppercase? Running heads are set this way.
fn is_all_caps(text: &str) -> bool {
    let mut saw = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            saw = true;
            if c.is_lowercase() {
                return false;
            }
        }
    }
    saw
}

fn normalise(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', ':', '—', '-'])
        .trim()
        .to_lowercase()
}

/// Is this token a roman numeral or a plain number?
fn is_numeral(token: &str) -> bool {
    let t = token.trim_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() {
        return false;
    }
    t.chars().all(|c| c.is_ascii_digit())
        || t.chars().all(|c| "ivxlcdmIVXLCDM".contains(c))
}

/// What kind of heading is this?
pub fn heading_level(text: &str) -> HeadingLevel {
    let normalised = normalise(text);
    if normalised.is_empty() {
        return HeadingLevel::Minor;
    }
    let words: Vec<&str> = normalised.split_whitespace().collect();
    let first = words[0].trim_matches(|c: char| !c.is_alphanumeric());

    if PART_WORDS.contains(&first) && words.len() <= 4 {
        return HeadingLevel::Part;
    }
    if STANDALONE_DIVISIONS.contains(&first) && words.len() <= 3 {
        return HeadingLevel::Part;
    }
    if CHAPTER_WORDS.contains(&first) && words.len() <= 4 {
        return HeadingLevel::Chapter;
    }

    // "§ 1. Theology a Science." — the section mark is unambiguous.
    if normalised.starts_with('§') || first == "sec" || first == "section" {
        return HeadingLevel::Section;
    }

    // A bare numeral standing as a heading: "XIV", "12."
    if words.len() == 1 && is_numeral(words[0]) {
        return HeadingLevel::Chapter;
    }

    // A descriptive heading naming what the passage is about.
    //
    // Case does the work here. A running head is set in full capitals and
    // repeats the book or chapter, carrying no information about the page it
    // sits on; a topic heading is set in title case and says what follows.
    // Only the second is worth navigating by.
    if is_title_case(text) && !is_all_caps(text) && !is_running_head(text) {
        return HeadingLevel::Topic;
    }

    HeadingLevel::Minor
}

/// Is this the line printed across the top of every page?
///
/// A running head cites where you are rather than naming what follows —
/// "INTRODUCTION. [Ch. I. — METHOD". The bracketed back-reference is the
/// giveaway, and it is set in mixed case often enough that the capitals test
/// alone lets it through and into the outline.
fn is_running_head(text: &str) -> bool {
    if text.contains('[') || text.contains(']') {
        return true;
    }
    // A chapter or book cited partway through a line is a reference to one,
    // not the start of one.
    let lower = text.to_lowercase();
    let mut words = lower.split_whitespace().skip(1);
    words.any(|w| {
        let core = w.trim_matches(|c: char| !c.is_alphanumeric());
        matches!(core, "ch" | "chap" | "chapter" | "bk" | "book" | "sec" | "vol")
    })
}

/// Is this heading only a division marker, with no title of its own?
///
/// `CHAPTER I.` names nothing; `CHAPTER I. ON METHOD.` does. Books very often
/// set the two on separate lines, which is why the distinction matters.
pub fn is_bare_marker(text: &str) -> bool {
    let normalised = normalise(text);
    let words: Vec<&str> = normalised.split_whitespace().collect();
    match words.len() {
        // "XIV", "12."
        1 => is_numeral(words[0]) || STANDALONE_DIVISIONS.contains(&words[0]),
        // "CHAPTER I.", "BOOK II"
        2 => {
            let first = words[0].trim_matches(|c: char| !c.is_alphanumeric());
            (CHAPTER_WORDS.contains(&first) || PART_WORDS.contains(&first)) && is_numeral(words[1])
        }
        _ => false,
    }
}

/// A heading with its level, after titles have been folded into markers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heading {
    pub block_id: i64,
    pub text: String,
    pub level: HeadingLevel,
}

/// Longest a following heading may be and still be read as a chapter's title.
const MAX_TITLE_WORDS: usize = 8;

/// Fold a chapter's title into the marker that introduces it.
///
/// `CHAPTER I.` followed by `ON METHOD.` is one chapter called "On Method",
/// not a chapter and an unrelated heading — and reading it as two throws the
/// title away, which is the part a reader actually navigates by.
///
/// Only a bare marker absorbs a title, and only from the heading immediately
/// after it. A marker that already names itself keeps what follows separate.
pub fn fold_titles(headings: Vec<Heading>) -> Vec<Heading> {
    let mut out: Vec<Heading> = Vec::with_capacity(headings.len());
    let mut iter = headings.into_iter().peekable();

    while let Some(mut heading) = iter.next() {
        let structural =
            matches!(heading.level, HeadingLevel::Part | HeadingLevel::Chapter);

        if structural && is_bare_marker(&heading.text) {
            let takes_title = iter.peek().is_some_and(|next| {
                matches!(next.level, HeadingLevel::Minor | HeadingLevel::Topic)
                    && next.text.split_whitespace().count() <= MAX_TITLE_WORDS
            });
            if takes_title {
                let title = iter.next().expect("peeked");
                heading.text = format!("{} {}", heading.text.trim_end(), title.text.trim());
            }
        }
        out.push(heading);
    }
    out
}

impl HeadingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            HeadingLevel::Part => "part",
            HeadingLevel::Chapter => "chapter",
            HeadingLevel::Section => "section",
            HeadingLevel::Topic => "topic",
            HeadingLevel::Minor => "minor",
        }
    }
}

/// Shorten a paragraph to something that fits on one line of a menu.
///
/// Cut at a word boundary: a preview ending mid-word looks like a bug.
pub fn preview(text: &str, limit: usize) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit {
        return clean;
    }
    let mut out = String::new();
    for word in clean.split(' ') {
        if out.chars().count() + word.chars().count() + 1 > limit {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = clean.chars().take(limit).collect();
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(id: i64, text: &str) -> Heading {
        Heading {
            block_id: id,
            text: text.to_string(),
            level: heading_level(text),
        }
    }

    /// Headings taken from the real Hodge import.
    #[test]
    fn recognises_the_divisions_of_a_real_book() {
        // An introduction contains chapters; it is not one.
        assert_eq!(heading_level("INTRODUCTION."), HeadingLevel::Part);
        assert_eq!(heading_level("CHAPTER I."), HeadingLevel::Chapter);
        assert_eq!(heading_level("§ 1. Theology a Science."), HeadingLevel::Section);
        // A running head is not a division.
        assert_eq!(heading_level("SYSTEMATIC THEOLOGY"), HeadingLevel::Minor);
    }

    #[test]
    fn recognises_other_common_division_headings() {
        assert_eq!(heading_level("BOOK II"), HeadingLevel::Part);
        assert_eq!(heading_level("Part First"), HeadingLevel::Part);
        assert_eq!(heading_level("Preface"), HeadingLevel::Part);
        assert_eq!(heading_level("APPENDIX"), HeadingLevel::Part);
        assert_eq!(heading_level("Letter III"), HeadingLevel::Chapter);
    }

    /// The case that prompted this: the chapter number and its title are set
    /// on separate lines, and reading them as two headings throws the title
    /// away.
    #[test]
    fn a_chapter_takes_the_title_on_the_line_below() {
        let folded = fold_titles(vec![
            heading(1, "INTRODUCTION."),
            heading(2, "CHAPTER I."),
            heading(3, "ON METHOD."),
            heading(4, "§ 1. Theology a Science."),
        ]);

        let texts: Vec<&str> = folded.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "INTRODUCTION.",
                "CHAPTER I. ON METHOD.",
                "§ 1. Theology a Science.",
            ]
        );
        assert_eq!(folded[0].level, HeadingLevel::Part);
        assert_eq!(folded[1].level, HeadingLevel::Chapter);
        assert_eq!(folded[2].level, HeadingLevel::Section);
    }

    #[test]
    fn a_chapter_that_names_itself_keeps_the_next_heading_separate() {
        // "CHAPTER I. ON METHOD." is already titled, so what follows is its
        // own heading rather than more of the title.
        let folded = fold_titles(vec![
            heading(1, "CHAPTER I. ON METHOD."),
            heading(2, "Some other heading"),
        ]);
        assert_eq!(folded.len(), 2);
    }

    #[test]
    fn a_marker_does_not_swallow_a_section() {
        // A section heading is structure of its own, not a chapter's title.
        let folded = fold_titles(vec![
            heading(1, "CHAPTER I."),
            heading(2, "§ 1. Theology a Science."),
        ]);
        assert_eq!(folded.len(), 2);
        assert_eq!(folded[0].text, "CHAPTER I.");
    }

    #[test]
    fn a_marker_does_not_swallow_a_long_line() {
        let folded = fold_titles(vec![
            heading(1, "CHAPTER I."),
            heading(
                2,
                "a heading so long that it could not possibly be the title of this chapter",
            ),
        ]);
        assert_eq!(folded.len(), 2);
    }

    #[test]
    fn recognises_a_bare_marker() {
        assert!(is_bare_marker("CHAPTER I."));
        assert!(is_bare_marker("BOOK II"));
        assert!(is_bare_marker("XIV"));
        assert!(!is_bare_marker("CHAPTER I. ON METHOD."));
        assert!(!is_bare_marker("Necessity for System in Theology"));
    }

    #[test]
    fn a_bare_numeral_heads_a_chapter() {
        assert_eq!(heading_level("XIV"), HeadingLevel::Chapter);
        assert_eq!(heading_level("12."), HeadingLevel::Chapter);
    }

    /// The heading the navigator was missing: an italic topic heading on the
    /// left half of a photographed spread.
    ///
    /// Subordinate to the numbered section, not equal to it — this sits
    /// inside `§ 1` and must not displace it, because `§ 1` runs on until
    /// `§ 2` starts a page later.
    #[test]
    fn a_descriptive_title_is_a_topic_below_its_section() {
        assert_eq!(
            heading_level("Necessity for System in Theology."),
            HeadingLevel::Topic
        );
        assert!(HeadingLevel::Section < HeadingLevel::Topic);
    }

    /// A running head repeats the book or chapter and says nothing about the
    /// page it sits on, so it stays out of the outline.
    #[test]
    fn a_running_head_is_minor() {
        assert_eq!(heading_level("THEOLOGICAL METHOD."), HeadingLevel::Minor);
        assert_eq!(heading_level("SYSTEMATIC THEOLOGY"), HeadingLevel::Minor);
        // Verbatim from the import: mixed case, so the capitals test alone
        // let it through into the outline as though it named the passage.
        assert_eq!(
            heading_level("INTRODUCTION. [Ch. I. — METHOD"),
            HeadingLevel::Minor
        );
    }

    #[test]
    fn a_back_reference_is_not_a_topic() {
        assert!(is_running_head("INTRODUCTION. [Ch. I. — METHOD"));
        assert!(is_running_head("Systematic Theology Ch. II"));
        // A genuine topic heading cites nothing.
        assert!(!is_running_head("Necessity for System in Theology."));
    }

    #[test]
    fn title_case_is_told_from_a_sentence() {
        assert!(is_title_case("Necessity for System in Theology."));
        assert!(is_title_case("The Art of Reading"));
        // A sentence capitalises only its opening word.
        assert!(!is_title_case("The office of the latter is to take those facts."));
        assert!(!is_title_case("It may naturally be asked, why not take the truths."));
        // One word is a fragment, not a title.
        assert!(!is_title_case("Theology"));
    }

    #[test]
    fn a_long_line_starting_with_chapter_is_not_a_chapter_heading() {
        // Prose can open with the word; a heading is short.
        assert_eq!(
            heading_level("Chapter divisions were introduced by later editors for convenience"),
            HeadingLevel::Minor
        );
    }

    #[test]
    fn previews_cut_at_a_word_boundary() {
        let text = "In every science there are two factors: facts and ideas.";
        let p = preview(text, 25);
        assert!(p.chars().count() <= 26, "got {p:?}");
        assert!(p.ends_with('…'));
        assert!(!p.contains("scien…"), "cut mid-word: {p:?}");
    }

    #[test]
    fn short_text_is_left_whole() {
        assert_eq!(preview("A short line.", 40), "A short line.");
    }
}
