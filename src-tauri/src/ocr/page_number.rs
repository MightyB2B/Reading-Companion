//! Reading the page number off the page.
//!
//! Import order is a bad source for this. Photograph pages out of sequence,
//! re-shoot one that came out blurry, or skip a plate, and the numbering is
//! wrong for the rest of the book. The number is printed on the page; the
//! honest thing is to read it.
//!
//! Two layers, for the same reason as everywhere else in this app: a cheap
//! deterministic pass that is right most of the time, and a model call for the
//! cases it cannot settle.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::ollama::OllamaClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberSource {
    Detected,
    Manual,
    Unknown,
}

impl NumberSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NumberSource::Detected => "detected",
            NumberSource::Manual => "manual",
            NumberSource::Unknown => "unknown",
        }
    }
}

/// Roman numerals, used for front matter.
fn roman_to_int(s: &str) -> Option<i64> {
    let s = s.to_lowercase();
    if s.is_empty() || !s.chars().all(|c| "ivxlcdm".contains(c)) {
        return None;
    }
    let value = |c: char| match c {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        'l' => 50,
        'c' => 100,
        'd' => 500,
        'm' => 1000,
        _ => 0,
    };
    let chars: Vec<char> = s.chars().collect();
    let mut total = 0i64;
    for i in 0..chars.len() {
        let v = value(chars[i]);
        let next = chars.get(i + 1).map(|c| value(*c)).unwrap_or(0);
        if v < next {
            total -= v;
        } else {
            total += v;
        }
    }
    (total > 0 && total < 4000).then_some(total)
}

/// Page numbers live in the margins, so only the first and last few lines are
/// worth considering. A bare number in the middle of a page is a footnote
/// marker or a date, not a folio.
const MARGIN_LINES: usize = 3;

/// A page number appears alone on its line, possibly with light decoration
/// such as `[4]` or `- 4 -`.
fn number_on_line(line: &str) -> Option<i64> {
    let t = line
        .trim()
        .trim_matches(|c: char| c == '[' || c == ']' || c == '-' || c == '.' || c.is_whitespace());
    if t.is_empty() || t.chars().count() > 8 {
        return None;
    }
    if let Ok(n) = t.parse::<i64>() {
        // Guard against a stray year or a four-figure footnote.
        return (n > 0 && n < 3000).then_some(n);
    }
    roman_to_int(t)
}

/// Numerals that belong to the book's structure, not to the page.
///
/// `CHAPTER I.` in a running head is the single most tempting wrong answer:
/// qwen3:4b read `INTRODUCTION. [Ch. I. — METHOD]` and reported page 1 for a
/// photograph of pages 2 and 3, despite being told chapter numbers do not
/// count. The instruction was not enough, so the numeral is stripped before
/// the model ever sees the line.
const STRUCTURAL_MARKERS: [&str; 8] = [
    "chapter", "ch.", "book", "part", "section", "sec.", "vol.", "volume",
];

fn strip_structural_numerals(line: &str) -> String {
    let lower = line.to_lowercase();
    if STRUCTURAL_MARKERS.iter().any(|m| lower.contains(m)) {
        // Blank the whole line: a line naming a chapter carries no folio.
        return String::new();
    }
    line.to_string()
}

/// The deterministic pass: a bare number alone on a line near the top or
/// bottom of the transcription.
pub fn detect_heuristically(raw: &str) -> Option<i64> {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }

    let head = lines.iter().take(MARGIN_LINES);
    let tail = lines.iter().rev().take(MARGIN_LINES);

    // The foot of the page is the more common position, so it is tried first.
    tail.chain(head).find_map(|l| number_on_line(l))
}

/// Reconcile the numbers of two facing pages from one photograph.
///
/// The halves of a spread are consecutive — the right page is always the left
/// page plus one — and reading them independently throws that away. On a real
/// import the left came out as 2 and the right as 1, so the right half sorted
/// ahead of its own left half and appeared under the wrong chapter.
///
/// When the two disagree, both readings cannot be right, and there is no
/// reason to trust one over the other on its own. What settles it is the rest
/// of the book: the assignment that yields two plausible numbers not already
/// belonging to other pages is the one to keep.
pub fn reconcile_facing(
    left: Option<i64>,
    right: Option<i64>,
    used_elsewhere: &[i64],
) -> (Option<i64>, Option<i64>) {
    let plausible = |n: i64| n >= 1 && n < 3000;
    let free = |n: i64| !used_elsewhere.contains(&n);
    let score = |l: i64, r: i64| {
        if !plausible(l) || !plausible(r) {
            return -1;
        }
        free(l) as i32 + free(r) as i32
    };

    match (left, right) {
        (None, None) => (None, None),
        // One reading carries the other: they are consecutive by construction.
        (Some(l), None) => (Some(l), plausible(l + 1).then_some(l + 1)),
        (None, Some(r)) => (plausible(r - 1).then_some(r - 1), Some(r)),
        (Some(l), Some(r)) if r == l + 1 => (Some(l), Some(r)),
        (Some(l), Some(r)) => {
            // Trust whichever reading survives contact with the rest of the book.
            let from_left = score(l, l + 1);
            let from_right = score(r - 1, r);
            if from_right > from_left {
                (Some(r - 1), Some(r))
            } else if from_left >= 0 {
                (Some(l), Some(l + 1))
            } else {
                // Neither works; leave them to be asked about.
                (None, None)
            }
        }
    }
}

fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "page_number": {
                "type": "integer",
                "minimum": -1,
                "description": "The number printed on this page, or -1 if the page shows no number."
            },
            "numeral_style": {
                "type": "string",
                "enum": ["arabic", "roman", "none"],
                "description": "Roman numerals indicate front matter."
            }
        },
        "required": ["page_number", "numeral_style"]
    })
}

#[derive(Debug, Clone, Deserialize)]
struct DetectedNumber {
    page_number: i64,
    #[allow(dead_code)]
    numeral_style: String,
}

const SYSTEM: &str = "\
You are given the raw text transcribed from one page of a printed book.

Find the page number printed on that page. It is normally a bare number alone \
on a line at the very top or the very bottom of the text, sometimes in roman \
numerals in front matter.

Report only a number that is actually printed on the page as a folio. Chapter \
numbers, section numbers, verse numbers, footnote markers, years, and numbers \
inside sentences are not page numbers. If the page carries no page number, \
answer -1.";

/// The question that works.
///
/// Phrasing matters more than it should here. "What page number is printed on
/// this page?" made both models transcribe the whole page instead; naming the
/// margin and demanding only the number produced a bare "2" and "3" on the two
/// halves of a photographed spread.
pub const FOLIO_PROMPT: &str = "Read only the page number printed in the top or bottom margin of \
this page. Reply with only that number, nothing else. If there is no page number, reply NONE.";

/// Pull a page number out of a short model reply.
///
/// Kept strict: the reply is expected to be a bare number, so anything longer
/// than a few tokens means the model answered a different question and its
/// output should not be trusted.
pub fn parse_folio_reply(reply: &str) -> Option<i64> {
    let t = reply.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return None;
    }
    // More than a couple of words means it started transcribing the page.
    if t.split_whitespace().count() > 2 {
        return None;
    }
    number_on_line(t)
}

/// Ask a vision model to read the folio off the image itself.
///
/// Used only when the text of the transcription did not contain one, because
/// this costs a model load: the vision model is a third resident and the three
/// together do not fit in 8GB.
pub async fn detect_from_image(
    client: &OllamaClient,
    model: &str,
    image: Vec<u8>,
) -> Option<i64> {
    let reply = client
        .ask_about_image(model, FOLIO_PROMPT, image)
        .await
        .ok()?;
    parse_folio_reply(&reply)
}

/// Read the page number, preferring the deterministic pass and falling back to
/// the model.
///
/// The heuristic is not merely an optimisation: it costs nothing, it is right
/// on the great majority of pages, and it cannot hallucinate. The model is
/// there for pages where the number sits somewhere unusual or is fused with
/// other text, which is exactly what happened on the first real page imported
/// (`1. Theology a Science.`).
/// Three passes, cheapest first:
///
/// 1. a bare number in the margins of the transcription — free, cannot invent;
/// 2. the text model reading the transcription — cheap, but it mistook `Ch. I.`
///    for page 1 on a real page, which is why chapter lines are stripped first;
/// 3. a vision model reading the folio off the image — reliable, but costs a
///    model load, so it is last.
///
/// When all three decline, the number stays unknown and the reader is asked.
/// That fallback is not a failure mode to be embarrassed about: some pages
/// genuinely carry no number.
pub async fn detect(
    client: &OllamaClient,
    text_model: &str,
    vision_model: &str,
    raw: &str,
    image: Option<Vec<u8>>,
) -> (Option<i64>, NumberSource) {
    if let Some(n) = detect_heuristically(raw) {
        return (Some(n), NumberSource::Detected);
    }

    if let Ok(Some(n)) = detect_with_model(client, text_model, raw).await {
        return (Some(n), NumberSource::Detected);
    }

    if let Some(image) = image {
        if let Some(n) = detect_from_image(client, vision_model, image).await {
            return (Some(n), NumberSource::Detected);
        }
    }

    (None, NumberSource::Unknown)
}

async fn detect_with_model(
    client: &OllamaClient,
    model: &str,
    raw: &str,
) -> Result<Option<i64>> {
    // Only the margins are relevant, and sending less is faster and less
    // distracting to the model.
    let lines: Vec<String> = raw
        .lines()
        .map(str::trim)
        // Lines naming a chapter or book are removed outright, so the model
        // cannot mistake their numeral for a folio.
        .map(strip_structural_numerals)
        .filter(|l| !l.is_empty())
        .collect();
    let n = lines.len();
    let excerpt = if n <= 12 {
        lines.join("\n")
    } else {
        format!(
            "{}\n...\n{}",
            lines[..6].join("\n"),
            lines[n - 6..].join("\n")
        )
    };

    let out: DetectedNumber = client
        .generate_structured(model, SYSTEM, &format!("PAGE TEXT:\n{excerpt}"), schema())
        .await?;

    Ok((out.page_number > 0 && out.page_number < 3000).then_some(out.page_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_number_at_the_foot_of_the_page() {
        let raw = "Some prose on the page.\n\nMore prose here.\n\n47";
        assert_eq!(detect_heuristically(raw), Some(47));
    }

    #[test]
    fn reads_a_number_at_the_head_of_the_page() {
        let raw = "112\n\nSome prose on the page.\n\nMore prose.";
        assert_eq!(detect_heuristically(raw), Some(112));
    }

    #[test]
    fn reads_roman_numerals_from_front_matter() {
        let raw = "Some prose.\n\nMore prose.\n\nxiv";
        assert_eq!(detect_heuristically(raw), Some(14));
        assert_eq!(roman_to_int("iv"), Some(4));
        assert_eq!(roman_to_int("xlii"), Some(42));
    }

    #[test]
    fn ignores_decoration_around_the_number() {
        assert_eq!(number_on_line("[4]"), Some(4));
        assert_eq!(number_on_line("- 12 -"), Some(12));
        assert_eq!(number_on_line("  9.  "), Some(9));
    }

    #[test]
    fn ignores_numbers_inside_prose() {
        let raw = "There were 47 reasons for this, and the author lists them all in turn.";
        assert_eq!(detect_heuristically(raw), None);
    }

    /// A bare number in the body is a footnote marker, not a folio.
    #[test]
    fn ignores_a_bare_number_in_the_middle_of_the_page() {
        let raw = "Line one of prose.\nLine two of prose.\nLine three.\n7\nLine five.\nLine six.\nLine seven of prose.";
        assert_eq!(detect_heuristically(raw), None);
    }

    #[test]
    fn ignores_a_year() {
        assert_eq!(number_on_line("1913"), Some(1913));
        // Guarded at the top end so a four-figure date is not taken as a folio
        // on a book that cannot have 3000 pages.
        assert_eq!(number_on_line("12000"), None);
    }

    /// The real page that motivated the model fallback: the number is fused
    /// with a repeat of the section head, so no line is a bare number.
    /// The real misdetection: a photograph of pages 2 and 3 was recorded as
    /// page 1, because the running head read `INTRODUCTION. [Ch. I. — METHOD]`
    /// and the model took the chapter numeral for a folio.
    #[test]
    fn a_chapter_numeral_is_never_offered_as_a_page_number() {
        assert_eq!(strip_structural_numerals("INTRODUCTION. [Ch. I. — METHOD]"), "");
        assert_eq!(strip_structural_numerals("CHAPTER I."), "");
        assert_eq!(strip_structural_numerals("BOOK II"), "");
        assert_eq!(strip_structural_numerals("Part First"), "");
        // Ordinary prose and a bare folio are untouched.
        assert_eq!(strip_structural_numerals("47"), "47");
        assert_eq!(
            strip_structural_numerals("In every science there are two factors."),
            "In every science there are two factors."
        );
    }

    #[test]
    fn hodge_page_defeats_the_heuristic_and_needs_the_model() {
        let raw = "SYSTEMATIC THEOLOGY\n\nINTRODUCTION.\n\nCHAPTER I.\n\n\
                   In every science there are two factors: facts and ideas.\n\n\
                   1. Theology a Science.";
        // "1. Theology a Science." is not a bare number, so the deterministic
        // pass correctly declines rather than guessing 1 for the wrong reason.
        assert_eq!(detect_heuristically(raw), None);
    }

    #[test]
    fn accepts_a_bare_number_from_the_vision_model() {
        // What qwen3-vl:4b actually returned for the two halves of the spread.
        assert_eq!(parse_folio_reply("2"), Some(2));
        assert_eq!(parse_folio_reply("3"), Some(3));
        assert_eq!(parse_folio_reply(" 47 \n"), Some(47));
        assert_eq!(parse_folio_reply("xiv"), Some(14));
    }

    #[test]
    fn rejects_a_reply_that_transcribed_the_page_instead() {
        // What glm-ocr returned to the same question: it ignored the question
        // and began transcribing, with the folio buried at the front. Parsing
        // a number out of that would be guessing.
        assert_eq!(
            parse_folio_reply("2 INTRODUCTION. [Ch. I. — METHOD the facts of Scripture."),
            None
        );
        assert_eq!(parse_folio_reply("NONE"), None);
        assert_eq!(parse_folio_reply(""), None);
    }

    /// The real misreading: a spread whose left half read as 2 and right half
    /// as 1. Page 1 already belonged to another page in the book, which is
    /// what gives the left reading away as the sound one.
    #[test]
    fn a_facing_page_that_collides_defers_to_its_partner() {
        assert_eq!(reconcile_facing(Some(2), Some(1), &[1]), (Some(2), Some(3)));
    }

    #[test]
    fn consecutive_readings_are_left_alone() {
        assert_eq!(reconcile_facing(Some(4), Some(5), &[]), (Some(4), Some(5)));
    }

    #[test]
    fn one_reading_supplies_the_other() {
        assert_eq!(reconcile_facing(Some(8), None, &[]), (Some(8), Some(9)));
        assert_eq!(reconcile_facing(None, Some(9), &[]), (Some(8), Some(9)));
    }

    #[test]
    fn a_right_hand_page_one_has_no_left_hand_partner() {
        // Page 0 does not exist, so the left half stays unknown and is asked
        // about rather than invented.
        assert_eq!(reconcile_facing(None, Some(1), &[]), (None, Some(1)));
    }

    #[test]
    fn neither_reading_is_forced_when_both_are_impossible() {
        assert_eq!(reconcile_facing(Some(0), Some(-5), &[]), (None, None));
    }

    #[test]
    fn the_partner_wins_when_its_reading_is_the_free_one() {
        // Left read 7, right read 20; 7 and 8 are both taken, 19 and 20 free.
        assert_eq!(
            reconcile_facing(Some(7), Some(20), &[7, 8]),
            (Some(19), Some(20))
        );
    }

    #[test]
    fn rejects_nonsense_roman_numerals() {
        assert_eq!(roman_to_int("hello"), None);
        assert_eq!(roman_to_int(""), None);
    }
}
