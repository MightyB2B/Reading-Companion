//! Did that transcription come out as English?
//!
//! [`crate::ocr::orient`] can tell reliably whether a page is sideways, but
//! not which of the two quarter turns is the right way up — the geometric
//! signal for that was measured against real photographs and got half the
//! cases wrong. So it proposes a turn, and this module checks it.
//!
//! The check is possible because a wrong turn does not degrade the
//! transcription, it destroys it. Upside-down type is not slightly harder to
//! read; it produces strings that are not words at all. The dictionary that
//! already ships with the application can say so in a millisecond, which is a
//! far better arbiter than any amount of pixel arithmetic.

use crate::ocr::segment::WordOracle;

/// Share of words that must be real for a transcription to be trusted.
///
/// A correctly oriented page of English scores far above this even with OCR
/// errors and proper nouns; an upside-down one scores near zero. The gap is
/// wide enough that the exact figure hardly matters.
pub const LEGIBLE_THRESHOLD: f32 = 0.45;

/// Too little text to judge. A page with a few words on it tells us nothing
/// either way, and guessing would be worse than leaving it alone.
const MIN_WORDS: usize = 25;

/// The fraction of words in a transcription that the dictionary recognises.
///
/// Returns `None` when there is not enough text to form a view.
pub fn legibility(text: &str, oracle: &dyn WordOracle) -> Option<f32> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        // Very short tokens are mostly noise either way round, and "a" and
        // "I" would flatter a page of gibberish.
        .filter(|w| w.chars().count() >= 3 && w.chars().all(char::is_alphabetic))
        .collect();

    if words.len() < MIN_WORDS {
        return None;
    }
    let known = words.iter().filter(|w| oracle.is_word(w)).count();
    Some(known as f32 / words.len() as f32)
}

/// Does this transcription read as language?
///
/// `None` — not enough text to tell — counts as legible: the point of this is
/// to catch a page that is definitely wrong, not to reject anything it cannot
/// vouch for.
pub fn reads_as_language(text: &str, oracle: &dyn WordOracle) -> bool {
    legibility(text, oracle).is_none_or(|score| score >= LEGIBLE_THRESHOLD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct Fixture(HashSet<String>);

    impl WordOracle for Fixture {
        fn is_word(&self, w: &str) -> bool {
            self.0.contains(w)
        }
    }

    /// An oracle that knows the words in the sample and nothing else.
    fn oracle_for(text: &str) -> Fixture {
        Fixture(
            text.split_whitespace()
                .map(|w| {
                    w.trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                })
                .collect(),
        )
    }

    const PASSAGE: &str = "In every science there are two factors facts and ideas or facts and \
        the mind Science is more than knowledge Knowledge is the persuasion of what is true on \
        adequate evidence But the facts of astronomy chemistry or history do not constitute the \
        science of those departments of knowledge";

    #[test]
    fn real_prose_reads_as_language() {
        let oracle = oracle_for(PASSAGE);
        let score = legibility(PASSAGE, &oracle).unwrap();
        assert!(score > 0.9, "score was {score}");
        assert!(reads_as_language(PASSAGE, &oracle));
    }

    /// What an upside-down page transcribes to: shapes that are not words.
    #[test]
    fn nonsense_does_not_read_as_language() {
        let oracle = oracle_for(PASSAGE);
        let garbled = "ʎɹǝʌǝ ǝɔuǝᴉɔs ǝɹǝɥʇ ǝɹɐ sɹoʇɔɐɟ sɐǝpᴉ sʇɔɐɟ puᴉɯ ǝɔuǝᴉɔs \
            ǝɹoɯ uɐɥʇ ǝƃpǝlʍouʞ ǝƃpǝlʍouʞ uoᴉsɐnsɹǝd ʇɐɥʍ ǝnɹʇ ǝʇɐnbǝpɐ ǝɔuǝpᴉʌǝ \
            sʇɔɐɟ ʎɯouoɹʇsɐ ʎɹʇsᴉɯǝɥɔ ʎɹoʇsᴉɥ ǝʇnʇᴉʇsuoɔ ǝɔuǝᴉɔs ǝsoɥʇ sʇuǝɯʇɹɐdǝp \
            ǝƃpǝlʍouʞ ɹou sǝop ǝɹǝɯ ʎlɹǝpɹo ʇuǝɯǝƃuɐɹɹɐ sʇɔɐɟ ʇunoɯɐ";
        let score = legibility(garbled, &oracle);
        assert!(score.is_some(), "the sample should be long enough to judge");
        assert!(!reads_as_language(garbled, &oracle), "score {score:?}");
    }

    /// OCR is never perfect. A page with real errors in it must still pass, or
    /// the check would reject good work.
    #[test]
    fn tolerates_ordinary_transcription_errors() {
        let oracle = oracle_for(PASSAGE);
        let with_errors = "In every scienee there are two factors facts and ideas or faots and \
            the mind Science is more than knowledge Knowledge is the persnasion of what is true on \
            adequate evidenee But the facts of astronomy chemistry or histmy do not constitute the \
            science of those departments of knowledge";
        let score = legibility(with_errors, &oracle).unwrap();
        assert!(score >= LEGIBLE_THRESHOLD, "score was {score}");
        assert!(reads_as_language(with_errors, &oracle));
    }

    #[test]
    fn too_little_text_is_not_judged() {
        let oracle = oracle_for(PASSAGE);
        assert_eq!(legibility("In every science", &oracle), None);
        // And is treated as fine, rather than rejected on no evidence.
        assert!(reads_as_language("In every science", &oracle));
    }

    #[test]
    fn short_tokens_do_not_flatter_a_bad_page() {
        // "a", "I", and stray punctuation must not prop up a score.
        let oracle = oracle_for("a i of to the");
        let junk = "a i a i a i a i a i a i a i a i a i a i a i a i a i a i a i a i a i a i a i";
        assert_eq!(legibility(junk, &oracle), None, "all tokens were too short");
    }
}
