//! Turning raw OCR output into paragraph blocks.
//!
//! This is the highest-risk component in the application: the study method
//! operates on paragraphs, so if segmentation is wrong everything downstream
//! is wrong. It is kept free of I/O and of any database dependency so it can
//! be exercised directly by the tests at the bottom of this file.
//!
//! The dictionary is injected as a [`WordOracle`] rather than reached for
//! directly. That keeps the module testable, lets it degrade to heuristics
//! when no dictionary is built yet, and makes the one genuinely ambiguous
//! decision — whether a line-ending hyphen is a line break or a real compound
//! — answerable with evidence instead of a guess.

use crate::models::{BlockKind, DraftBlock, Normalization};

/// Something that can say whether a string is a real word.
///
/// Backed by `dict.sqlite` in production, by a fixture set in tests.
pub trait WordOracle {
    fn is_word(&self, word: &str) -> bool;
}

/// Characters that can end a line where a word has been split across it.
const HYPHENS: [char; 4] = ['-', '\u{2010}', '\u{00AD}', '\u{2011}'];

/// Typographic and etymological ligatures, expanded for the reading text.
const LIGATURES: [(char, &str); 12] = [
    ('\u{FB00}', "ff"),
    ('\u{FB01}', "fi"),
    ('\u{FB02}', "fl"),
    ('\u{FB03}', "ffi"),
    ('\u{FB04}', "ffl"),
    ('\u{FB05}', "st"),
    ('\u{FB06}', "st"),
    ('\u{00E6}', "ae"),
    ('\u{00C6}', "Ae"),
    ('\u{0153}', "oe"),
    ('\u{0152}', "Oe"),
    ('\u{1E9E}', "Ss"),
];

/// Split a token into leading punctuation, an alphabetic core, and trailing
/// punctuation, so corrections can be applied to the word without destroying
/// the quotes and commas around it.
fn split_affixes(token: &str) -> (&str, &str, &str) {
    let start = token
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(token.len());
    let end = token
        .rfind(|c: char| c.is_alphabetic())
        .map(|i| i + token[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(start);
    (&token[..start], &token[start..end], &token[end..])
}

/// Restore the capitalisation pattern of `original` onto `replacement`.
fn match_case(original: &str, replacement: &str) -> String {
    let orig: Vec<char> = original.chars().collect();
    replacement
        .chars()
        .enumerate()
        .map(|(i, c)| match orig.get(i) {
            Some(o) if o.is_uppercase() => c.to_uppercase().next().unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Repair the classic early-modern OCR failure: the long s (`ſ`) is shaped
/// like an f, so engines read `ſhall` as `fhall` and `himſelf` as `himfelf`.
///
/// Correcting this needs real word knowledge — `f` and `s` are both legitimate
/// letters, so only the dictionary can tell us that `firft` is wrong while
/// `first` is right. We try every subset of the f positions and accept the
/// result only when the evidence is unambiguous.
pub fn repair_long_s(word: &str, oracle: &dyn WordOracle) -> Option<String> {
    let lower = word.to_lowercase();
    if oracle.is_word(&lower) {
        return None; // already a real word, leave it alone
    }

    let f_positions: Vec<usize> = lower
        .char_indices()
        .filter(|(_, c)| *c == 'f')
        .map(|(i, _)| i)
        .collect();

    // Bail on absurd cases rather than exploring 2^n for large n.
    if f_positions.is_empty() || f_positions.len() > 4 {
        return None;
    }

    let mut candidates: Vec<(u32, String)> = Vec::new();
    // Every non-empty subset of the f positions.
    for mask in 1u32..(1 << f_positions.len()) {
        let mut chars: Vec<char> = lower.chars().collect();
        // char_indices gives byte offsets; for ASCII-f words these coincide,
        // but map through explicitly to stay correct on mixed input.
        let byte_to_char: Vec<usize> = lower
            .char_indices()
            .enumerate()
            .filter(|(_, (b, _))| f_positions.contains(b))
            .map(|(ci, _)| ci)
            .collect();

        for (bit, &ci) in byte_to_char.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                chars[ci] = 's';
            }
        }
        let candidate: String = chars.into_iter().collect();
        if oracle.is_word(&candidate) {
            candidates.push((mask.count_ones(), candidate));
        }
    }

    match candidates.len() {
        0 => None,
        1 => Some(match_case(word, &candidates[0].1)),
        // Ambiguous: prefer the most conservative reading (fewest changes).
        _ => {
            candidates.sort_by_key(|(n, _)| *n);
            Some(match_case(word, &candidates[0].1))
        }
    }
}

/// Decide whether a hyphen at a line break joins a split word or is a genuine
/// compound, and produce the joined text.
///
/// `under-` + `stand` becomes `understand`, but `well-` + `known` must stay
/// `well-known`. The dictionary settles it: if the merged form is a word we
/// merge, and if it is not but both halves are words we keep the hyphen.
/// Without a dictionary we fall back to the fact that a lowercase
/// continuation is far more often a broken word than a compound.
pub fn join_hyphenated(left: &str, right: &str, oracle: Option<&dyn WordOracle>) -> String {
    let left_core = left.trim_end_matches(|c| HYPHENS.contains(&c));

    // `left` is the whole paragraph accumulated so far, so only its final
    // token takes part in the join — everything before it is carried through
    // untouched. Likewise only the first token of `right`.
    let head_end = left_core
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + left_core[i..].chars().next().map_or(1, char::len_utf8));
    let (head, left_tok) = left_core.split_at(head_end);

    let right_trimmed = right.trim_start();
    let tok_end = right_trimmed
        .find(char::is_whitespace)
        .unwrap_or(right_trimmed.len());
    let (right_tok, tail) = right_trimmed.split_at(tok_end);

    let (l_pre, l_word, l_post) = split_affixes(left_tok);
    let (r_pre, r_word, r_post) = split_affixes(right_tok);

    // Punctuation between the two halves means this is not a clean word break;
    // don't try to be clever about it.
    if l_word.is_empty() || r_word.is_empty() || !l_post.is_empty() || !r_pre.is_empty() {
        return format!("{left_core}{right}");
    }

    let merged = format!("{}{}", l_word.to_lowercase(), r_word.to_lowercase());
    let hyphenated = format!("{}-{}", l_word.to_lowercase(), r_word.to_lowercase());

    let merge = match oracle {
        Some(o) => {
            if o.is_word(&merged) {
                true
            } else if o.is_word(&hyphenated) {
                false
            } else if o.is_word(&l_word.to_lowercase()) && o.is_word(&r_word.to_lowercase()) {
                // Both halves stand alone and the merge is not a word:
                // most likely a real compound.
                false
            } else {
                // Neither reading is attested; a broken word is the more
                // common cause of a line-ending hyphen.
                true
            }
        }
        None => r_word.chars().next().is_some_and(char::is_lowercase),
    };

    let joined = if merge {
        format!("{l_word}{r_word}")
    } else {
        format!("{l_word}-{r_word}")
    };
    format!("{head}{l_pre}{joined}{r_post}{tail}")
}

/// Is this line just a page number, a folio marker, or a rule?
fn is_page_furniture(line: &str) -> bool {
    let t = line.trim().trim_matches(|c: char| c == '[' || c == ']');

    // Markdown horizontal rules. Small OCR models emit runs of these when they
    // overrun the end of the page content.
    let rule_chars = ['-', '_', '*'];
    if t.len() >= 3 && t.chars().all(|c| rule_chars.contains(&c)) {
        return true;
    }

    if t.is_empty() || t.chars().count() > 12 {
        return false;
    }
    let stripped: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if stripped.is_empty() {
        return false;
    }
    // Arabic page numbers.
    if stripped.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Roman numerals, as used for front matter.
    let low = stripped.to_lowercase();
    low.chars().all(|c| "ivxlcdm".contains(c))
}

/// Is every cased letter in this string uppercase?
fn is_all_caps(text: &str) -> bool {
    let mut saw_letter = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            saw_letter = true;
            if c.is_lowercase() {
                return false;
            }
        }
    }
    saw_letter
}

/// Words a heading opens with in a book of this kind.
const HEADING_OPENERS: [&str; 10] = [
    "chapter", "book", "part", "section", "appendix", "preface", "introduction", "volume",
    "article", "canto",
];

/// Does this line read as a heading rather than prose?
///
/// This has to work without markdown. `glm-ocr` transcribes a page as plain
/// text, so the `#` prefixes the original classifier looked for never appear
/// and *every* line came back as a paragraph — which meant the reader was
/// asked to write a one-sentence summary of "CHAPTER I."
///
/// The signals below are drawn from a real page of Hodge's *Systematic
/// Theology*: a running head in full caps, `INTRODUCTION.`, `CHAPTER I.`,
/// `ON METHOD.`, and a section head opening with `§`.
pub fn looks_like_heading(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let words = t.split_whitespace().count();

    // Headings are short. This is the single most reliable signal, and it
    // keeps a full-caps sentence inside a paragraph from being mistaken.
    if words > 12 {
        return false;
    }

    // Section marks: "§ 1. Theology a Science."
    if t.starts_with('§') || t.starts_with("Sec.") || t.starts_with("SEC.") {
        return true;
    }

    // "CHAPTER I.", "BOOK II", "PART FIRST"
    let first_word = t
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if HEADING_OPENERS.contains(&first_word.as_str()) && words <= 6 {
        return true;
    }

    // A short line in full capitals: the running head, "ON METHOD."
    if is_all_caps(t) && words <= 8 {
        return true;
    }

    // A short line with no terminal punctuation is a title, not a sentence.
    if words <= 8
        && !t.ends_with(['.', '!', '?', ',', ';', ':'])
        && t.chars().next().is_some_and(char::is_uppercase)
    {
        return true;
    }

    // A line set in title case, even with a full stop after it.
    //
    // "Necessity for System in Theology." is an italic heading in the book,
    // and the trailing period was enough to hide it from every test above —
    // so it was stored as prose, put into the summarising queue, and left out
    // of the outline entirely.
    if crate::ocr::outline::is_title_case(t) {
        return true;
    }

    false
}

fn classify(line: &str) -> (BlockKind, String) {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("#### ").or_else(|| t.strip_prefix("### ")) {
        return (BlockKind::Heading, rest.trim().to_string());
    }
    if let Some(rest) = t.strip_prefix("## ").or_else(|| t.strip_prefix("# ")) {
        return (BlockKind::Heading, rest.trim().to_string());
    }
    if let Some(rest) = t.strip_prefix("> ") {
        return (BlockKind::Quote, rest.trim().to_string());
    }
    // Heading detection deliberately does NOT happen here. In printed-layout
    // mode a line is a fragment of a paragraph, and the opening fragment of a
    // paragraph is short and unpunctuated — indistinguishable from a title.
    // It is applied to the assembled block instead, at flush time.
    (BlockKind::Paragraph, t.to_string())
}

/// Expand ligatures and repair long-s, according to how old the book is.
pub fn normalize(text: &str, level: Normalization, oracle: Option<&dyn WordOracle>) -> String {
    if level == Normalization::Off {
        return text.to_string();
    }

    // Ligature expansion: safe at every level above Off.
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match LIGATURES.iter().find(|(from, _)| *from == ch) {
            Some((_, to)) => out.push_str(to),
            None => out.push(ch),
        }
    }

    if level != Normalization::Full {
        return out;
    }

    // Long-s proper is unambiguous — it is only ever an s.
    out = out.replace('\u{017F}', "s");

    // The f-for-long-s misreading is ambiguous and needs the dictionary.
    let Some(oracle) = oracle else { return out };

    out.split_inclusive(char::is_whitespace)
        .map(|tok| {
            let trailing_ws: String = tok
                .chars()
                .rev()
                .take_while(|c| c.is_whitespace())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let body = &tok[..tok.len() - trailing_ws.len()];
            let (pre, core, post) = split_affixes(body);
            if core.is_empty() || !core.to_lowercase().contains('f') {
                return tok.to_string();
            }
            match repair_long_s(core, oracle) {
                Some(fixed) => format!("{pre}{fixed}{post}{trailing_ws}"),
                None => tok.to_string(),
            }
        })
        .collect()
}

/// How the model laid out its transcription.
///
/// This is not cosmetic. `glm-ocr` unwraps the page itself and emits one long
/// line per paragraph, whereas a model asked to preserve the printed layout
/// emits one line per *printed* line. Unwrapping the first kind fuses adjacent
/// paragraphs together; not unwrapping the second kind leaves every printed
/// line as its own "paragraph". Either mistake destroys the unit the whole
/// study method operates on, so we detect which we are looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    /// One line per paragraph; the model already joined the printed lines.
    Prewrapped,
    /// One line per printed line; we must join them ourselves.
    Printed,
}

/// A printed line of prose is bounded by the page measure — roughly 50-80
/// characters, and essentially never past 120. A single line longer than this
/// means the model has already joined the printed lines for us.
///
/// The median is *not* usable here: real output mixes a 34-character running
/// head with 200-character paragraphs, dragging the median below any workable
/// threshold. The maximum is the signal that actually separates the two cases.
const PREWRAPPED_MAX_LINE: usize = 120;

/// Share of lines ending a sentence above which a page reads as prewrapped.
/// In printed text only a minority of lines close a sentence; when the model
/// has unwrapped the page, nearly every line does.
const PREWRAPPED_TERMINAL_PCT: usize = 80;

pub fn detect_line_mode(raw: &str) -> LineMode {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !is_page_furniture(l))
        .collect();

    if lines.is_empty() {
        return LineMode::Printed;
    }

    let max_len = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    if max_len > PREWRAPPED_MAX_LINE {
        return LineMode::Prewrapped;
    }

    // Fallback for a page made up of unusually short paragraphs, where no
    // single line is long enough to trip the length test.
    let terminal = lines
        .iter()
        .filter(|l| l.ends_with(['.', '!', '?', '"', '\u{201D}', '\u{2019}']))
        .count();
    if lines.len() >= 3 && terminal * 100 >= lines.len() * PREWRAPPED_TERMINAL_PCT {
        LineMode::Prewrapped
    } else {
        LineMode::Printed
    }
}

/// Turn one page of raw OCR output into ordered paragraph blocks.
///
/// Blank lines always separate blocks. Whether consecutive non-blank lines are
/// also joined depends on the detected [`LineMode`].
pub fn segment_page(
    raw: &str,
    level: Normalization,
    oracle: Option<&dyn WordOracle>,
) -> Vec<DraftBlock> {
    let mode = detect_line_mode(raw);
    segment_page_with_mode(raw, mode, level, oracle)
}

pub fn segment_page_with_mode(
    raw: &str,
    mode: LineMode,
    level: Normalization,
    oracle: Option<&dyn WordOracle>,
) -> Vec<DraftBlock> {
    let mut blocks: Vec<DraftBlock> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_kind = BlockKind::Paragraph;

    let flush = |current: &mut Vec<String>, kind: BlockKind, blocks: &mut Vec<DraftBlock>| {
        if current.is_empty() {
            return;
        }
        // Unwrap the lines into a single run of prose.
        let mut text = String::new();
        for line in current.iter() {
            if text.is_empty() {
                text.push_str(line);
                continue;
            }
            if text.ends_with(|c| HYPHENS.contains(&c)) {
                let joined = join_hyphenated(&text, line, oracle);
                text = joined;
            } else {
                text.push(' ');
                text.push_str(line);
            }
        }
        let text_raw = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text_raw.is_empty() {
            current.clear();
            return;
        }
        let text_norm = normalize(&text_raw, level, oracle);

        // Now that the whole block is assembled, decide whether it is prose.
        let kind = if kind == BlockKind::Paragraph && looks_like_heading(&text_raw) {
            BlockKind::Heading
        } else {
            kind
        };

        blocks.push(DraftBlock {
            kind,
            text_raw,
            text_norm,
        });
        current.clear();
    };

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            flush(&mut current, current_kind, &mut blocks);
            current_kind = BlockKind::Paragraph;
            continue;
        }
        if is_page_furniture(trimmed) {
            flush(&mut current, current_kind, &mut blocks);
            current_kind = BlockKind::Paragraph;
            continue;
        }

        let (kind, content) = classify(trimmed);
        if content.is_empty() {
            continue;
        }

        // A heading always stands alone.
        if kind == BlockKind::Heading {
            flush(&mut current, current_kind, &mut blocks);
            current.push(content);
            flush(&mut current, BlockKind::Heading, &mut blocks);
            current_kind = BlockKind::Paragraph;
            continue;
        }

        // A change of kind (prose -> quote) also starts a new block.
        if !current.is_empty() && kind != current_kind {
            flush(&mut current, current_kind, &mut blocks);
        }
        current_kind = kind;
        current.push(content);

        // The model already unwrapped this page, so a line ending is a
        // paragraph ending. Joining further would fuse distinct paragraphs.
        if mode == LineMode::Prewrapped {
            flush(&mut current, current_kind, &mut blocks);
            current_kind = BlockKind::Paragraph;
        }
    }
    flush(&mut current, current_kind, &mut blocks);

    blocks
}

/// Drop a wholesale repetition of the page.
///
/// Small OCR models are prone to transcribing a page, reaching the end, and —
/// with no stop token to tell them the turn is over — starting again from the
/// top. `glm-ocr` does this reproducibly on some pages and not on others, so
/// it has to be handled rather than prompted away. Working on blocks rather
/// than raw text makes this a clean semantic comparison instead of string
/// surgery.
///
/// Deduplication is by content rather than by position, because the repeat is
/// not always a clean doubling: on the early-modern fixture the model emitted
/// the running head *between* the two copies, which defeats any positional
/// scheme. A paragraph that has already appeared on the same page is a
/// repetition wherever it turns up.
///
/// Only substantial blocks are considered. Short lines — a line of dialogue,
/// a refrain, a repeated exclamation — can legitimately recur, so they are
/// left alone.
const MIN_DEDUP_CHARS: usize = 40;

pub fn dedup_repeated_pass(blocks: &mut Vec<DraftBlock>) {
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    blocks.retain(|b| {
        if b.text_raw.chars().count() < MIN_DEDUP_CHARS {
            return true;
        }
        // `insert` returns false when the text has been seen before.
        seen.insert(b.text_raw.to_lowercase())
    });
}

/// Does this block break off mid-sentence?
///
/// A page ends where the paper ends, not where the argument does. Hodge's
/// page 2 stops at "...into systematic order" and page 3 opens with "and
/// mutual relation" — one sentence, one paragraph, two photographs.
pub fn ends_mid_sentence(text: &str) -> bool {
    let t = text.trim_end();
    if t.is_empty() {
        return false;
    }
    !t.ends_with(['.', '!', '?', '"', '\u{201D}', '\u{2019}'])
}

/// Does this block pick up mid-sentence?
///
/// A paragraph that opens in lower case did not begin here.
pub fn starts_mid_sentence(text: &str) -> bool {
    text.trim_start()
        .chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(char::is_lowercase)
}

/// Join a fragment ending one page to the fragment opening the next.
///
/// The break falls between words, so a single space is the whole of it —
/// except where the first page ends on a hyphen, which is the same split-word
/// case that happens at every line break and is settled the same way.
pub fn stitch_across_pages(
    tail: &str,
    head: &str,
    oracle: Option<&dyn WordOracle>,
) -> String {
    let tail = tail.trim_end();
    let head = head.trim_start();
    if tail.is_empty() {
        return head.to_string();
    }
    if head.is_empty() {
        return tail.to_string();
    }
    if tail.ends_with(|c| HYPHENS.contains(&c)) {
        return join_hyphenated(tail, head, oracle);
    }
    format!("{tail} {head}")
}

/// Phrases an OCR model uses when it talks about its own work instead of
/// transcribing.
///
/// These are not stylistic slips — they end up as blocks the reader is asked
/// to write a one-sentence summary of. Observed verbatim from glm-ocr on a
/// two-page spread that overran its token budget:
///
/// > The text is cut off before it can be fully transcribed. The continuation
/// > is missing, so no complete transcription can be provided.
///
/// The list is deliberately narrow. A false positive deletes a paragraph of
/// the reader's book, which is far worse than leaving one stray line they can
/// remove by hand — so anything a real author might plausibly write is not
/// here. `"the image shows"` and `"is illegible"` were both tried and dropped
/// for exactly that reason: they occur naturally in printed prose.
const MODEL_COMMENTARY: [&str; 12] = [
    "no complete transcription",
    "the continuation is missing",
    "cut off before it can be",
    "unable to transcribe",
    "the transcription is incomplete",
    "no transcription can be",
    "i cannot provide",
    "i'm unable to",
    "as an ai",
    "no text is visible",
    "the image is too blurry",
    "appears to be a scan",
];

/// Longest a block can be and still be dismissed as commentary.
///
/// Model asides are brief — the one observed ran 131 characters. Anything
/// substantially longer is prose even if it happens to trip a phrase, which is
/// the second guard against deleting real text.
const MAX_COMMENTARY_CHARS: usize = 300;

/// Vocabulary that is only suspicious in a short block at the very end of a
/// page.
///
/// The phrase list above cannot keep up: a second import produced an entirely
/// new formulation — "The transcription is complete and follows the
/// instructions exactly as they are printed. No additional text is present."
/// Enumerating every way a model might congratulate itself is a losing game.
///
/// Position is the sturdier signal. A model appends its aside *after* the
/// text; a book does not end a page with a short remark about images and
/// instructions. Requiring last-position, brevity, and this vocabulary
/// together makes a false positive very unlikely.
const TRAILING_META_VOCAB: [&str; 9] = [
    "transcription",
    "transcribed",
    "the image",
    "this image",
    "the instructions",
    "no additional text",
    "text is present",
    "the page content",
    "as requested",
];

/// Is this block the model talking about the transcription rather than
/// transcribing?
pub fn is_model_commentary(text: &str) -> bool {
    if text.chars().count() > MAX_COMMENTARY_CHARS {
        return false;
    }
    let lower = text.to_lowercase();
    MODEL_COMMENTARY.iter().any(|p| lower.contains(p))
}

/// Drop blocks in which the model is commenting rather than transcribing.
///
/// Two passes: known phrases anywhere, then a stricter positional check on the
/// final block, where models put their asides.
pub fn strip_model_commentary(blocks: &mut Vec<DraftBlock>) {
    blocks.retain(|b| !is_model_commentary(&b.text_raw));

    while let Some(last) = blocks.last() {
        let text = &last.text_raw;
        let lower = text.to_lowercase();

        let short = text.chars().count() <= MAX_COMMENTARY_CHARS;
        let meta = TRAILING_META_VOCAB.iter().any(|w| lower.contains(w));
        if short && meta {
            blocks.pop();
            // A model can append more than one such line, so keep going.
            continue;
        }
        break;
    }
}

/// Reduce a line to comparable form: lowercase, no section marks, no leading
/// numbering, no punctuation.
///
/// This is what lets `"1. Theology a Science."` be recognised as the same
/// thing as `"§ 1. Theology a Science."`.
fn furniture_key(text: &str) -> String {
    let t = text
        .trim()
        .trim_start_matches(|c: char| {
            c == '§' || c == '#' || c.is_ascii_digit() || c == '.' || c.is_whitespace()
        })
        .to_lowercase();
    t.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove the running head and any footer echo from a single page.
///
/// [`strip_running_heads`] can only spot furniture by seeing it repeat across
/// pages, which is no help on the very first import — and the first import is
/// exactly when a reader judges whether the app works. These two rules need
/// only the page in front of them:
///
/// * the running head repeats the book's title, which we already know;
/// * a short trailing block that echoes a heading from higher up the page is a
///   footer or catchword, not prose.
///
/// Observed on Hodge's *Systematic Theology* page 1: a `SYSTEMATIC THEOLOGY`
/// running head, and a trailing `1. Theology a Science.` echoing the `§ 1.`
/// section head with the page number fused onto it.
pub fn strip_single_page_furniture(blocks: &mut Vec<DraftBlock>, book_title: &str) {
    let title_key = furniture_key(book_title);

    // Running head: the first block repeating the book's title.
    if !title_key.is_empty() {
        if let Some(first) = blocks.first() {
            if furniture_key(&first.text_raw) == title_key {
                blocks.remove(0);
            }
        }
    }

    // Footer echo: a short trailing block repeating something above it.
    if blocks.len() >= 2 {
        let last_key = furniture_key(&blocks[blocks.len() - 1].text_raw);
        let last_is_short = blocks[blocks.len() - 1].text_raw.chars().count() <= 60;
        if last_is_short && !last_key.is_empty() {
            let echoes_earlier = blocks[..blocks.len() - 1]
                .iter()
                .any(|b| furniture_key(&b.text_raw) == last_key);
            if echoes_earlier {
                blocks.pop();
            }
        }
    }
}

/// Remove headers and footers that repeat across pages.
///
/// A running head ("THE DECLINE AND FALL", the author's name) is not part of
/// the argument and would otherwise become a paragraph the reader is asked to
/// summarise. We only strip a line when it recurs on at least `min_repeats`
/// pages, so a genuine sentence that happens to appear twice survives.
pub fn strip_running_heads(pages: &mut [Vec<DraftBlock>], min_repeats: usize) {
    use std::collections::HashMap;

    let mut first_counts: HashMap<String, usize> = HashMap::new();
    let mut last_counts: HashMap<String, usize> = HashMap::new();

    for page in pages.iter() {
        if let Some(b) = page.first() {
            *first_counts.entry(b.text_raw.to_lowercase()).or_default() += 1;
        }
        if let Some(b) = page.last() {
            *last_counts.entry(b.text_raw.to_lowercase()).or_default() += 1;
        }
    }

    // Only short lines are plausible running heads; a repeated full paragraph
    // is much more likely to be real text.
    let is_headish = |s: &str| s.split_whitespace().count() <= 8;

    for page in pages.iter_mut() {
        if let Some(b) = page.first() {
            let key = b.text_raw.to_lowercase();
            if is_headish(&key) && first_counts.get(&key).copied().unwrap_or(0) >= min_repeats {
                page.remove(0);
            }
        }
        if let Some(b) = page.last() {
            let key = b.text_raw.to_lowercase();
            if is_headish(&key) && last_counts.get(&key).copied().unwrap_or(0) >= min_repeats {
                page.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct Fixture(HashSet<&'static str>);

    impl Fixture {
        fn new(words: &[&'static str]) -> Self {
            Fixture(words.iter().copied().collect())
        }
    }

    impl WordOracle for Fixture {
        fn is_word(&self, w: &str) -> bool {
            self.0.contains(w.to_lowercase().as_str())
        }
    }

    fn dict() -> Fixture {
        Fixture::new(&[
            "understand", "under", "stand", "well", "known", "well-known", "shall", "himself",
            "first", "most", "justice", "self", "his", "the", "man", "fall", "off", "so", "if",
            "of", "for",
        ])
    }

    #[test]
    fn merges_a_word_broken_across_a_line() {
        let joined = join_hyphenated("under-", "stand", Some(&dict()));
        assert_eq!(joined, "understand");
    }

    #[test]
    fn keeps_a_real_compound_hyphenated() {
        // The critical counter-case: both halves are words and the merged form
        // is not, so the hyphen is real and must survive.
        let joined = join_hyphenated("well-", "known", Some(&dict()));
        assert_eq!(joined, "well-known");
    }

    #[test]
    fn falls_back_to_case_heuristic_without_a_dictionary() {
        assert_eq!(join_hyphenated("under-", "stand", None), "understand");
        // A capitalised continuation is treated as a real compound.
        assert_eq!(join_hyphenated("Anglo-", "Saxon", None), "Anglo-Saxon");
    }

    #[test]
    fn repairs_long_s_misread_as_f() {
        let d = dict();
        assert_eq!(repair_long_s("fhall", &d).as_deref(), Some("shall"));
        assert_eq!(repair_long_s("himfelf", &d).as_deref(), Some("himself"));
        // Two f's, only the second of which is a long s.
        assert_eq!(repair_long_s("firft", &d).as_deref(), Some("first"));
        assert_eq!(repair_long_s("moft", &d).as_deref(), Some("most"));
    }

    #[test]
    fn leaves_genuine_f_words_alone() {
        let d = dict();
        // "fall" and "off" are real words; nothing to repair.
        assert_eq!(repair_long_s("fall", &d), None);
        assert_eq!(repair_long_s("off", &d), None);
    }

    #[test]
    fn preserves_capitalisation_when_repairing() {
        let d = dict();
        assert_eq!(repair_long_s("Fhall", &d).as_deref(), Some("Shall"));
    }

    #[test]
    fn unwraps_lines_into_one_paragraph() {
        let raw = "The quick brown fox\njumped over the lazy dog.";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].text_raw,
            "The quick brown fox jumped over the lazy dog."
        );
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        let raw = "First paragraph here.\n\nSecond paragraph here.";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].text_raw, "Second paragraph here.");
    }

    /// From the real Hodge import: an italic heading that a full stop hid.
    #[test]
    fn a_title_cased_line_is_a_heading_despite_its_full_stop() {
        let raw = "Necessity for System in Theology.\n\n\
                   It may naturally be asked, why not take the truths as God has seen fit to \
                   reveal them, and thus save ourselves the trouble of showing their relation.";
        let blocks = segment_page(raw, Normalization::Off, None);

        assert_eq!(blocks.len(), 2, "got {blocks:#?}");
        assert_eq!(blocks[0].kind, BlockKind::Heading);
        assert_eq!(blocks[0].text_raw, "Necessity for System in Theology.");
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn a_short_sentence_is_still_prose() {
        // Only its first word is capitalised, so it is not a title.
        let raw = "The office of the latter is to state.";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn headings_become_their_own_block() {
        let raw = "# Chapter I\n\nIt was a bright cold day.";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Heading);
        assert_eq!(blocks[0].text_raw, "Chapter I");
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn drops_standalone_page_numbers() {
        let raw = "Some real prose here.\n\n42";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text_raw, "Some real prose here.");

        let roman = "Real prose.\n\nxiv";
        assert_eq!(segment_page(roman, Normalization::Off, None).len(), 1);
    }

    #[test]
    fn hyphenation_across_a_line_break_inside_a_paragraph() {
        let raw = "It is hard to under-\nstand the well-\nknown case.";
        let blocks = segment_page(raw, Normalization::Off, Some(&dict()));
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].text_raw,
            "It is hard to understand the well-known case."
        );
    }

    #[test]
    fn expands_ligatures_at_light_normalization() {
        let raw = "The \u{FB01}rst \u{FB02}ight";
        let blocks = segment_page(raw, Normalization::Light, None);
        assert_eq!(blocks[0].text_norm, "The first flight");
        // The diplomatic text keeps the ligatures as printed.
        assert_eq!(blocks[0].text_raw, "The \u{FB01}rst \u{FB02}ight");
    }

    #[test]
    fn full_normalization_repairs_a_long_s_page() {
        let d = dict();
        let raw = "He fhall do himfelf juftice.";
        let blocks = segment_page(raw, Normalization::Full, Some(&d));
        assert_eq!(blocks[0].text_norm, "He shall do himself justice.");
        // Diplomatic transcription is untouched.
        assert_eq!(blocks[0].text_raw, "He fhall do himfelf juftice.");
    }

    #[test]
    fn normalization_off_leaves_text_identical() {
        let raw = "The \u{FB01}rst case.";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks[0].text_raw, blocks[0].text_norm);
    }

    #[test]
    fn strips_a_running_head_repeated_across_pages() {
        let mk = |s: &str| DraftBlock {
            kind: BlockKind::Paragraph,
            text_raw: s.to_string(),
            text_norm: s.to_string(),
        };
        let mut pages = vec![
            vec![mk("THE DECLINE AND FALL"), mk("Body of page one.")],
            vec![mk("THE DECLINE AND FALL"), mk("Body of page two.")],
            vec![mk("THE DECLINE AND FALL"), mk("Body of page three.")],
        ];
        strip_running_heads(&mut pages, 2);
        assert_eq!(pages[0].len(), 1);
        assert_eq!(pages[0][0].text_raw, "Body of page one.");
        assert_eq!(pages[2][0].text_raw, "Body of page three.");
    }

    #[test]
    fn does_not_strip_a_long_repeated_line() {
        // Long lines are real prose even if they recur; only short ones are
        // plausible running heads.
        let long = "This is a genuinely long sentence that happens to repeat across pages somehow.";
        let mk = |s: &str| DraftBlock {
            kind: BlockKind::Paragraph,
            text_raw: s.to_string(),
            text_norm: s.to_string(),
        };
        let mut pages = vec![
            vec![mk(long), mk("One.")],
            vec![mk(long), mk("Two.")],
        ];
        strip_running_heads(&mut pages, 2);
        assert_eq!(pages[0].len(), 2);
    }

    fn mk(s: &str) -> DraftBlock {
        DraftBlock {
            kind: BlockKind::Paragraph,
            text_raw: s.to_string(),
            text_norm: s.to_string(),
        }
    }

    const P1: &str = "The first paragraph of this page, long enough to be treated as substantial prose.";
    const P2: &str = "The second paragraph of this page, also comfortably past the deduplication floor.";

    /// Taken from the reader's own library: page 2 of Hodge breaks off
    /// mid-sentence and page 3 resumes it.
    #[test]
    fn recognises_a_paragraph_running_across_a_page_break() {
        let tail = "endeavor to bring all the facts of revelation into systematic order";
        let head = "and mutual relation. It is only thus that we can satisfactorily exhibit their truth.";

        assert!(ends_mid_sentence(tail));
        assert!(starts_mid_sentence(head));

        let joined = stitch_across_pages(tail, head, None);
        assert!(joined.starts_with("endeavor to bring"));
        assert!(joined.contains("systematic order and mutual relation"));
    }

    #[test]
    fn a_completed_paragraph_does_not_run_on() {
        assert!(!ends_mid_sentence("This is a finished thought."));
        assert!(!ends_mid_sentence("Is it finished?"));
        assert!(!ends_mid_sentence("\u{201C}Finished,\u{201D} he said.\u{201D}"));
        assert!(!starts_mid_sentence("A new paragraph opens here."));
    }

    #[test]
    fn stitching_repairs_a_word_split_by_the_page_break() {
        // The same hyphenation question as at a line break, one page apart.
        let joined = stitch_across_pages("it is hard to under-", "stand the case", Some(&dict()));
        assert_eq!(joined, "it is hard to understand the case");
    }

    #[test]
    fn stitching_tolerates_an_empty_side() {
        assert_eq!(stitch_across_pages("", "head text", None), "head text");
        assert_eq!(stitch_across_pages("tail text", "", None), "tail text");
    }

    #[test]
    fn removes_a_doubled_transcription() {
        let mut blocks = vec![mk(P1), mk(P2), mk(P1), mk(P2)];
        dedup_repeated_pass(&mut blocks);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text_raw, P1);
        assert_eq!(blocks[1].text_raw, P2);
    }

    /// The case that defeated positional deduplication: on the early-modern
    /// fixture glm-ocr emitted the running head between the two copies.
    #[test]
    fn removes_a_repeat_interrupted_by_a_running_head() {
        let mut blocks = vec![
            mk(P1),
            mk(P2),
            mk("Of the Conduct of the Understanding"),
            mk(P1),
            mk(P2),
        ];
        dedup_repeated_pass(&mut blocks);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].text_raw, P1);
        assert_eq!(blocks[1].text_raw, P2);
        assert_eq!(blocks[2].text_raw, "Of the Conduct of the Understanding");
    }

    #[test]
    fn leaves_a_genuine_short_refrain_alone() {
        // Short lines can legitimately recur, so they are below the floor.
        let mut blocks = vec![mk("A."), mk("B."), mk("A."), mk("C.")];
        dedup_repeated_pass(&mut blocks);
        assert_eq!(blocks.len(), 4);
    }

    #[test]
    fn drops_markdown_horizontal_rules() {
        // Observed tail behaviour of glm-ocr running past the end of a page.
        let raw = "Real prose here.\n\n---\n---\n---";
        let blocks = segment_page(raw, Normalization::Off, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text_raw, "Real prose here.");
    }

    // Verbatim glm-ocr output for the `modern.png` fixture. Note that the
    // second copy begins on the line directly after the first ends, with no
    // blank line between them.
    const OBSERVED_LOOPED_OUTPUT: &str = "\
The question of how a reader comes to understand a difficult book has occupied teachers for a very long time. It is not enough to move the eyes across the page. One must understand each part before the whole can be assembled, and this is a discipline that rewards patience more than speed.

A well-known experiment makes the point. Readers were asked to summarise a passage in one sentence. Those who worked paragraph by paragraph retained far more than those who read the chapter straight through and then tried to recall it. The difference was not intelligence but method.
The question of how a reader comes to understand a difficult book has occupied teachers for a very long time. It is not enough to move the eyes across the page. One must understand each part before the whole can be assembled, and this is a discipline that rewards patience more than speed.

A well-known experiment makes the point. Readers were asked to summarise a passage in one sentence. Those who worked paragraph by paragraph retained far more than those who read the chapter straight through and then tried to recall it. The difference was not intelligence but method.
47
---
---
---";

    #[test]
    fn detects_prewrapped_output() {
        assert_eq!(
            detect_line_mode(OBSERVED_LOOPED_OUTPUT),
            LineMode::Prewrapped
        );
    }

    #[test]
    fn detects_printed_line_output() {
        let printed = "The question of how a reader comes to understand a diffi-\n\
                       cult book has occupied teachers for a very long time. It is\n\
                       not enough to move the eyes across the page.";
        assert_eq!(detect_line_mode(printed), LineMode::Printed);
    }

    /// The end-to-end recovery: real looped model output in, two clean
    /// paragraphs out.
    #[test]
    fn recovers_a_clean_page_from_real_looped_output() {
        let mut blocks = segment_page(OBSERVED_LOOPED_OUTPUT, Normalization::Off, None);
        dedup_repeated_pass(&mut blocks);

        assert_eq!(blocks.len(), 2, "expected two paragraphs, got {blocks:#?}");
        assert!(blocks[0].text_raw.starts_with("The question of how a reader"));
        assert!(blocks[0].text_raw.ends_with("more than speed."));
        assert!(blocks[1].text_raw.starts_with("A well-known experiment"));
        // The page number and the rules must not survive as readable blocks.
        assert!(blocks.iter().all(|b| b.text_raw != "47"));
        assert!(blocks.iter().all(|b| !b.text_raw.contains("---")));
    }

    /// Guards the failure this mode detection exists to prevent: without it,
    /// the last paragraph of the first pass and the first of the second get
    /// fused into a single block.
    #[test]
    fn prewrapped_mode_does_not_fuse_adjacent_paragraphs() {
        let blocks = segment_page(OBSERVED_LOOPED_OUTPUT, Normalization::Off, None);
        assert_eq!(blocks.len(), 4, "expected the doubled pair, got {blocks:#?}");
        assert_eq!(blocks[0].text_raw, blocks[2].text_raw);
        assert_eq!(blocks[1].text_raw, blocks[3].text_raw);
    }

    /// Verbatim glm-ocr output for `early_modern.png` under the guarded
    /// production config: a correct transcription, the running head, the
    /// transcription again, then the folio number degenerating into a run.
    ///
    /// This is the hardest real case observed, and the reason deduplication
    /// works on content rather than position.
    const OBSERVED_EARLY_MODERN: &str = "\
It shall be granted that the first duty of a reasonable man is to understand himself, and that justice must begin at home. The most learned of the anti-ents were of this opinion, and did flourish accordingly.

Yet it is a strange thing that so many should profess a love of wisdom, and so few should practise it in the ordinary course of their lives.
Of the Conduct of the Understanding
It shall be granted that the first duty of a reasonable man is to understand himself, and that justice must begin at home. The most learned of the anti-ents were of this opinion, and did flourish accordingly.

Yet it is a strange thing that so many should profess a love of wisdom, and so few should practise it in the ordinary course of their lives.
xiv
xiv
xiv
xiv
xiv";

    #[test]
    fn recovers_an_early_modern_page_from_real_output() {
        assert_eq!(detect_line_mode(OBSERVED_EARLY_MODERN), LineMode::Prewrapped);

        let mut blocks = segment_page(OBSERVED_EARLY_MODERN, Normalization::Off, None);
        dedup_repeated_pass(&mut blocks);

        // Two paragraphs plus the running head, which only cross-page
        // analysis can identify as furniture.
        assert_eq!(blocks.len(), 3, "got {blocks:#?}");
        assert!(blocks[0].text_raw.starts_with("It shall be granted"));
        assert!(blocks[1].text_raw.starts_with("Yet it is a strange thing"));
        // The repeated folio number must not survive as readable prose.
        assert!(blocks.iter().all(|b| b.text_raw != "xiv"));
    }
}
