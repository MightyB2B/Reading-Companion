//! Finding scripture citations in ordinary prose.
//!
//! The point of this module is not to read a Bible — it is to read *Hodge*.
//! Nineteenth-century theology cites scripture on nearly every page, in Roman
//! numerals, abbreviated a dozen different ways: `Rom. iii. 23`, `1 Cor. xiii.
//! 4-7`, `Ps. cx. 1`. Turning those into live references is what makes a
//! library of theology navigable, and it pays off before a Bible has ever been
//! imported.
//!
//! Hand-rolled rather than regex-driven. The grammar is small, the failure
//! modes are specific (a Roman numeral chapter followed by a Roman numeral
//! verse, a semicolon list that inherits its book), and a scanner that can
//! report *why* it stopped is worth more here than a pattern that cannot.

pub mod books;

use serde::{Deserialize, Serialize};

pub use books::{canonical_name, resolve_book, BOOKS};

/// One citation, located in the text it was found in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// Character offset of the first character of the citation.
    pub char_start: usize,
    /// Character offset one past its last character.
    pub char_end: usize,
    /// Exactly as printed, so a misparse is visible rather than silent.
    pub surface: String,
    /// OSIS abbreviation: `Rom`, `Ps`, `1Cor`.
    pub osis_book: String,
    pub chapter: u32,
    /// None for a whole-chapter citation such as `Gen. i.`
    pub verse_start: Option<u32>,
    /// Equal to `verse_start` for a single verse; larger for a range.
    pub verse_end: Option<u32>,
    /// How sure the scanner is. Chapter-and-verse in a recognised book scores
    /// 1.0; a bare chapter, or a two-letter abbreviation, scores lower.
    pub confidence: f32,
}

impl Reference {
    /// `Rom 3:23`, `Rom 3:23-25`, `Gen 1`.
    pub fn label(&self) -> String {
        let name = canonical_name(&self.osis_book);
        match (self.verse_start, self.verse_end) {
            (Some(a), Some(b)) if b > a => format!("{name} {}:{a}-{b}", self.chapter),
            (Some(a), _) => format!("{name} {}:{a}", self.chapter),
            _ => format!("{name} {}", self.chapter),
        }
    }
}

/// Find every scripture citation in a passage.
///
/// Offsets are in `char`s, not bytes: the frontend slices these strings in
/// JavaScript, and a byte offset into a paragraph containing an em dash lands
/// mid-character and corrupts every highlight after it.
pub fn find_references(text: &str) -> Vec<Reference> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Reference> = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        // Citations only ever start at a word boundary. Without this, the
        // "Am" in "Amsterdam" becomes the book of Amos.
        //
        // They also never start with a space: the scanner tolerates one while
        // matching, so beginning here would put the leading blank inside the
        // stored surface text and every highlight would sit one char left.
        if chars[i].is_whitespace() || (i > 0 && is_word_char(chars[i - 1])) {
            i += 1;
            continue;
        }

        match scan_citation(&chars, i) {
            Some(first) => {
                let end = first.char_end;
                // A semicolon or comma list continues the same book:
                // `Rom. iii. 23; vi. 23` is two references, not one and a
                // stray number.
                let mut cursor = end;
                let book = first.osis_book.clone();
                out.push(first);
                while let Some(next) = scan_continuation(&chars, cursor, &book) {
                    cursor = next.char_end;
                    out.push(next);
                }
                i = cursor;
            }
            None => i += 1,
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// A book name, then a chapter, then optionally verses.
fn scan_citation(chars: &[char], start: usize) -> Option<Reference> {
    let (osis, mut i, name_confidence) = scan_book_name(chars, start)?;

    // `Rom.` — the abbreviating full stop, if present.
    if chars.get(i) == Some(&'.') {
        i += 1;
    }
    i = skip_space(chars, i);

    let (chapter, after_chapter) = scan_number(chars, i)?;
    if chapter == 0 || chapter > 150 {
        return None;
    }
    i = after_chapter;

    let (verse_start, verse_end, end, confidence) = scan_verse_part(chars, i, name_confidence);

    let surface: String = chars[start..end].iter().collect();
    Some(Reference {
        char_start: start,
        char_end: end,
        surface: surface.trim_end().to_string(),
        osis_book: osis,
        chapter,
        verse_start,
        verse_end,
        confidence,
    })
}

/// The `; vi. 23` in `Rom. iii. 23; vi. 23` — a chapter and verse inheriting
/// the book named before it.
fn scan_continuation(chars: &[char], from: usize, osis: &str) -> Option<Reference> {
    let mut i = from;
    // Only a semicolon carries the book forward. A comma after a verse is a
    // verse list within the same chapter, and a full stop ends the sentence.
    if chars.get(i) != Some(&';') {
        return None;
    }
    let sep_start = i;
    i = skip_space(chars, i + 1);

    // If a book name follows, this is a fresh citation and the outer scan
    // will pick it up on its own terms.
    if scan_book_name(chars, i).is_some() {
        return None;
    }

    let (chapter, after) = scan_number(chars, i)?;
    if chapter == 0 || chapter > 150 {
        return None;
    }
    let (verse_start, verse_end, end, confidence) = scan_verse_part(chars, after, 0.9);

    // A bare number after a semicolon is far more often a page or a footnote
    // than a chapter, so a continuation must carry a verse to be believed.
    verse_start?;

    let surface: String = chars[sep_start..end].iter().collect();
    Some(Reference {
        char_start: sep_start,
        char_end: end,
        surface: surface.trim_start_matches(';').trim().to_string(),
        osis_book: osis.to_string(),
        chapter,
        verse_start,
        verse_end,
        confidence,
    })
}

/// The `. 23`, `:23`, or `:23-25` after a chapter.
///
/// Returns the verses, where the citation ends, and how much to believe it.
fn scan_verse_part(
    chars: &[char],
    from: usize,
    name_confidence: f32,
) -> (Option<u32>, Option<u32>, usize, f32) {
    let chapter_only = (None, None, from, name_confidence * 0.75);

    let mut i = from;
    // Victorian style writes `Rom. iii. 23`; modern writes `Rom 3:23`.
    match chars.get(i) {
        Some('.') => {
            i += 1;
            i = skip_space(chars, i);
        }
        Some(':') => i += 1,
        Some(' ') => i = skip_space(chars, i),
        _ => return chapter_only,
    }

    let Some((first, after)) = scan_number(chars, i) else {
        // `Gen. i.` — a whole chapter. The trailing stop belongs to the
        // citation, so include it.
        let end = if chars.get(from) == Some(&'.') { from + 1 } else { from };
        return (None, None, end, name_confidence * 0.75);
    };
    i = after;

    let mut last = first;
    // A range: `4-7`, `4–7` with an en dash, or `4 ff.`
    if matches!(chars.get(i), Some('-') | Some('\u{2013}') | Some('\u{2014}')) {
        if let Some((second, after_second)) = scan_number(chars, i + 1) {
            if second > first {
                last = second;
                i = after_second;
            }
        }
    } else if let Some(after_ff) = scan_ff(chars, i) {
        // "and following" — an open range. Recording it as a two-verse span
        // is a lie, so it stays a single verse and the surface text carries
        // the ff. for anyone reading it.
        i = after_ff;
    }

    (Some(first), Some(last), i, name_confidence)
}

fn scan_ff(chars: &[char], from: usize) -> Option<usize> {
    let mut i = skip_space(chars, from);
    if chars.get(i) != Some(&'f') {
        return None;
    }
    i += 1;
    if chars.get(i) != Some(&'f') {
        return None;
    }
    i += 1;
    if chars.get(i) == Some(&'.') {
        i += 1;
    }
    Some(i)
}

fn skip_space(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
        i += 1;
    }
    i
}

/// A chapter or verse number, Arabic or Roman.
fn scan_number(chars: &[char], from: usize) -> Option<(u32, usize)> {
    if let Some(hit) = scan_arabic(chars, from) {
        return Some(hit);
    }
    scan_roman(chars, from)
}

fn scan_arabic(chars: &[char], from: usize) -> Option<(u32, usize)> {
    let mut i = from;
    let mut n: u32 = 0;
    while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
        n = n.checked_mul(10)?.checked_add(d)?;
        i += 1;
        if i - from > 3 {
            // Four digits is a year or a page number, not a verse.
            return None;
        }
    }
    (i > from).then_some((n, i))
}

/// Roman numerals, lower or upper case, up to `cl` — Psalm 150 is the
/// largest chapter number in the canon.
fn scan_roman(chars: &[char], from: usize) -> Option<(u32, usize)> {
    fn value(c: char) -> Option<u32> {
        Some(match c.to_ascii_lowercase() {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            _ => return None,
        })
    }

    let mut i = from;
    let mut digits = Vec::new();
    while let Some(v) = chars.get(i).copied().and_then(value) {
        digits.push(v);
        i += 1;
    }
    if digits.is_empty() {
        return None;
    }
    // A letter immediately after is a word, not a numeral: "Civil" must not
    // parse as 100.
    if matches!(chars.get(i), Some(c) if c.is_alphanumeric()) {
        return None;
    }

    // Subtractive pairs (iv, ix, xl) are the only reason this is not a sum:
    // a digit smaller than the one after it is subtracted instead.
    let mut total: i64 = 0;
    for (idx, &v) in digits.iter().enumerate() {
        let next = digits.get(idx + 1).copied().unwrap_or(0);
        if v < next {
            total -= v as i64;
        } else {
            total += v as i64;
        }
    }
    (total > 0 && total <= 150).then_some((total as u32, i))
}

/// A book name at `start`, returning its OSIS id, where it ends, and how much
/// the name alone is worth.
fn scan_book_name(chars: &[char], start: usize) -> Option<(String, usize, f32)> {
    // An ordinal prefix: "1", "I", "First", "Ist".
    let (ordinal, after_ordinal) = scan_ordinal(chars, start);
    let i = skip_space(chars, after_ordinal);

    let mut word_end = i;
    while matches!(chars.get(word_end), Some(c) if c.is_alphabetic()) {
        word_end += 1;
    }
    if word_end == i {
        return None;
    }
    let word: String = chars[i..word_end].iter().collect();

    // "Song of Solomon", "Song of Songs" — the only multi-word names that
    // matter, and worth handling because they are cited by name in full.
    let (word, word_end) = match extend_multiword(chars, &word, word_end) {
        Some(pair) => pair,
        None => (word, word_end),
    };

    let (osis, confidence) = resolve_book(&word, ordinal)?;
    Some((osis, word_end, confidence))
}

fn extend_multiword(chars: &[char], first: &str, end: usize) -> Option<(String, usize)> {
    if !first.eq_ignore_ascii_case("song") {
        return None;
    }
    let mut i = skip_space(chars, end);
    let rest: String = chars[i..].iter().take(12).collect::<String>().to_lowercase();
    for (suffix, len) in [("of solomon", 10), ("of songs", 8)] {
        if rest.starts_with(suffix) {
            i += len;
            return Some((format!("Song {suffix}"), i));
        }
    }
    None
}

/// `1`, `I`, `First`, `1st` before a book name.
fn scan_ordinal(chars: &[char], start: usize) -> (Option<u8>, usize) {
    if let Some((n, after)) = scan_arabic(chars, start) {
        if (1..=3).contains(&n) {
            let after = skip_ordinal_suffix(chars, after);
            // Must be followed by a space and a letter, or it is just a number.
            let probe = skip_space(chars, after);
            if matches!(chars.get(probe), Some(c) if c.is_alphabetic()) && probe > after {
                return (Some(n as u8), after);
            }
        }
    }

    // Roman I/II/III, but only when a capital and followed by a space then a
    // capital letter — otherwise every "I" in the prose is a book prefix.
    let mut i = start;
    let mut count = 0;
    while chars.get(i) == Some(&'I') && count < 3 {
        i += 1;
        count += 1;
    }
    if count > 0 && chars.get(i) == Some(&' ') {
        let probe = skip_space(chars, i);
        if matches!(chars.get(probe), Some(c) if c.is_uppercase()) {
            return (Some(count as u8), i);
        }
    }

    for (word, n) in [("First", 1u8), ("Second", 2), ("Third", 3)] {
        let candidate: String = chars[start..].iter().take(word.len()).collect();
        if candidate.eq_ignore_ascii_case(word) {
            return (Some(n), start + word.len());
        }
    }
    (None, start)
}

fn skip_ordinal_suffix(chars: &[char], from: usize) -> usize {
    let two: String = chars[from..].iter().take(2).collect::<String>().to_lowercase();
    if matches!(two.as_str(), "st" | "nd" | "rd") {
        from + 2
    } else {
        from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Reference {
        let refs = find_references(text);
        assert_eq!(refs.len(), 1, "expected one reference in {text:?}, got {refs:#?}");
        refs.into_iter().next().unwrap()
    }

    #[test]
    fn victorian_roman_numerals() {
        // The format Hodge actually prints, and the reason this module exists.
        let r = one("as the apostle says, Rom. iii. 23, and again");
        assert_eq!(r.osis_book, "Rom");
        assert_eq!(r.chapter, 3);
        assert_eq!(r.verse_start, Some(23));
        assert_eq!(r.surface, "Rom. iii. 23");
    }

    #[test]
    fn modern_colon_form() {
        let r = one("see John 3:16 for this");
        assert_eq!(r.osis_book, "John");
        assert_eq!((r.chapter, r.verse_start), (3, Some(16)));
    }

    #[test]
    fn numbered_books_in_both_styles() {
        let a = one("1 Cor. xiii. 4-7");
        assert_eq!(a.osis_book, "1Cor");
        assert_eq!((a.chapter, a.verse_start, a.verse_end), (13, Some(4), Some(7)));

        let b = one("II Timothy 3:16");
        assert_eq!(b.osis_book, "2Tim");
    }

    #[test]
    fn psalms_reach_one_hundred_and_fifty() {
        let r = one("Ps. cx. 1");
        assert_eq!((r.osis_book.as_str(), r.chapter, r.verse_start), ("Ps", 110, Some(1)));
        let r = one("Ps. cl. 6");
        assert_eq!(r.chapter, 150);
    }

    #[test]
    fn semicolon_lists_inherit_the_book() {
        let refs = find_references("Rom. iii. 23; vi. 23");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[1].osis_book, "Rom");
        assert_eq!((refs[1].chapter, refs[1].verse_start), (6, Some(23)));
    }

    #[test]
    fn a_semicolon_followed_by_a_new_book_is_a_new_citation() {
        let refs = find_references("Rom. iii. 23; John 3:16");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[1].osis_book, "John");
    }

    #[test]
    fn chapter_only_citations_are_kept_but_doubted() {
        let r = one("as in Gen. i. the account begins");
        assert_eq!(r.chapter, 1);
        assert_eq!(r.verse_start, None);
        assert!(r.confidence < 1.0, "a bare chapter should not be fully trusted");
    }

    #[test]
    fn following_verses() {
        let r = one("Rom. viii. 28 ff.");
        assert_eq!(r.verse_start, Some(28));
        assert!(r.surface.contains("ff"));
    }

    #[test]
    fn ordinary_prose_yields_nothing() {
        // The traps: a book abbreviation inside a longer word, a roman
        // numeral that is really a pronoun, a bare number.
        for text in [
            "Amsterdam in 1620",
            "I have read chapter 3 twice",
            "the Romans were a people",
            "see page 23 above",
            "Job security is not the issue here",
        ] {
            assert!(
                find_references(text).is_empty(),
                "false positive in {text:?}: {:#?}",
                find_references(text)
            );
        }
    }

    #[test]
    fn a_reference_inside_a_word_is_not_a_reference() {
        assert!(find_references("Amos").is_empty());
        assert!(find_references("Judea 3:1").is_empty());
    }

    #[test]
    fn labels_read_the_way_people_write_them() {
        assert_eq!(one("Rom. iii. 23").label(), "Romans 3:23");
        assert_eq!(one("1 Cor. xiii. 4-7").label(), "1 Corinthians 13:4-7");
        assert_eq!(one("Gen. i.").label(), "Genesis 1");
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        // The em dash is three bytes. A byte offset would slice the citation
        // in the wrong place when the frontend highlights it.
        let text = "he wrote\u{2014}Rom. iii. 23\u{2014}plainly";
        let r = one(text);
        let sliced: String = text.chars().skip(r.char_start).take(r.char_end - r.char_start).collect();
        assert_eq!(sliced, "Rom. iii. 23");
    }

    #[test]
    fn several_citations_in_one_paragraph() {
        let text = "Compare Rom. iii. 23 with Ps. cx. 1 and 1 Cor. xiii. 4-7.";
        let refs = find_references(text);
        assert_eq!(refs.len(), 3);
        assert_eq!(
            refs.iter().map(|r| r.osis_book.as_str()).collect::<Vec<_>>(),
            ["Rom", "Ps", "1Cor"]
        );
    }
}
