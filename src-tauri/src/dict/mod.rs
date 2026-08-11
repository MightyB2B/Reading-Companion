//! Dictionary lookup, in two layers.
//!
//! **Layer one is local and instant.** A word is resolved against
//! `dict.sqlite` — Webster's Revised Unabridged (1913) — with no model
//! involved. This is the layer that makes the feature trustworthy: a
//! definition either exists in a real dictionary or it does not appear.
//!
//! **Layer two answers "what does it mean *here*".** The model receives the
//! sentence together with the senses layer one already retrieved, and picks
//! among them. It selects and paraphrases; it does not define. Grounding the
//! call in retrieved senses is what makes an invented definition structurally
//! unlikely rather than merely discouraged — the same principle the coach uses.
//!
//! # Why the 1913 edition
//!
//! Because meanings move. Webster's first sense for *nice* is "Foolish; silly;
//! simple", for *awful* it is "Oppressing with fear or horror". Those are the
//! senses Austen and Gibbon were using. Looking such words up in a modern
//! dictionary returns a confident, fluent, wrong answer — the worst possible
//! failure for a tool whose purpose is comprehension.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::ollama::OllamaClient;

pub struct Dictionary {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sense {
    pub gloss: String,
    /// e.g. "obsolete", "archaic" — surfaced so the reader knows a sense was
    /// already out of use in 1913.
    pub labels: Vec<String>,
    pub pos: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Found under the word exactly as printed.
    Direct,
    /// Reached through the dictionary's own cross-reference (shew -> show).
    CrossReference,
    /// Reached through a known early-modern spelling (publick -> public).
    ArchaicSpelling,
    /// Reached through an irregular form (hath -> have).
    Irregular,
    /// Reached by stripping a regular inflection (running -> run).
    Inflection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lookup {
    /// The surface form as it appears on the page.
    pub word: String,
    /// The headword the senses belong to.
    pub lemma: String,
    pub resolution: Resolution,
    pub senses: Vec<Sense>,
}

/// The in-context answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextualSense {
    /// Index into the senses that were handed to the model. `-1` when none of
    /// them fit, which is a legitimate answer and better than a forced choice.
    pub sense_index: i64,
    pub plain_meaning: String,
    pub why_this_sense: String,
}

/// Senses beyond this are not worth the reader's attention or the model's
/// context. Webster's lists up to forty for common words.
const MAX_SENSES: usize = 12;

impl Dictionary {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(AppError::NotFound(format!(
                "dictionary database at {}. Build it with: node scripts/build-dictionary.mjs",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Is this a headword in the dictionary?
    ///
    /// Also the [`WordOracle`](crate::ocr::segment::WordOracle) implementation
    /// used to settle line-break hyphenation and long-s repair.
    pub fn contains(&self, word: &str) -> bool {
        let conn = self.lock();
        conn.query_row(
            "SELECT 1 FROM entries WHERE headword = ?1 LIMIT 1",
            params![word.to_lowercase()],
            |_| Ok(()),
        )
        .optional()
        .map(|r| r.is_some())
        .unwrap_or(false)
    }

    fn senses_for(&self, headword: &str) -> Result<Vec<Sense>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT s.gloss, s.labels, e.pos
             FROM entries e JOIN senses s ON s.entry_id = e.id
             WHERE e.headword = ?1
             ORDER BY e.id, s.ordinal
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![headword, MAX_SENSES as i64], |r| {
                let labels_json: String = r.get(1)?;
                Ok(Sense {
                    gloss: r.get(0)?,
                    labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                    pos: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn variant_of(&self, word: &str) -> Result<Option<(String, Resolution)>> {
        let conn = self.lock();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT lemma, kind FROM variants WHERE variant = ?1",
                params![word],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(lemma, kind)| {
            let res = match kind.as_str() {
                "cross_reference" => Resolution::CrossReference,
                "archaic_spelling" => Resolution::ArchaicSpelling,
                _ => Resolution::Irregular,
            };
            (lemma, res)
        }))
    }

    /// Layer one. Resolve a surface form to senses, entirely offline.
    ///
    /// Resolution is tried in order of confidence: the exact word, then the
    /// variant table (which is data, not guesswork), then regular inflection
    /// rules. Every candidate an inflection rule produces is checked against
    /// the dictionary before it is accepted, so a wrong guess yields no result
    /// rather than the wrong word's definition.
    pub fn lookup(&self, word: &str) -> Result<Option<Lookup>> {
        let cleaned = normalise_surface(word);
        if cleaned.is_empty() {
            return Ok(None);
        }

        // 1. Exactly as printed.
        let senses = self.senses_for(&cleaned)?;
        if !senses.is_empty() {
            return Ok(Some(Lookup {
                word: word.to_string(),
                lemma: cleaned,
                resolution: Resolution::Direct,
                senses,
            }));
        }

        // 2. A known variant.
        if let Some((lemma, resolution)) = self.variant_of(&cleaned)? {
            let senses = self.senses_for(&lemma)?;
            if !senses.is_empty() {
                return Ok(Some(Lookup {
                    word: word.to_string(),
                    lemma,
                    resolution,
                    senses,
                }));
            }
        }

        // 3. Regular inflection, validated against the dictionary.
        for candidate in inflection_candidates(&cleaned) {
            let senses = self.senses_for(&candidate)?;
            if !senses.is_empty() {
                return Ok(Some(Lookup {
                    word: word.to_string(),
                    lemma: candidate,
                    resolution: Resolution::Inflection,
                    senses,
                }));
            }
        }

        Ok(None)
    }
}

impl crate::ocr::segment::WordOracle for Dictionary {
    fn is_word(&self, word: &str) -> bool {
        self.contains(word)
    }
}

/// Strip the punctuation a word carries when lifted out of running prose.
///
/// Internal hyphens and apostrophes survive, because `well-known` and `don't`
/// are words. A selection containing no letters at all is not.
fn normalise_surface(word: &str) -> String {
    let trimmed = word
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'');

    if !trimmed.chars().any(char::is_alphanumeric) {
        return String::new();
    }
    trimmed.to_lowercase()
}

/// Candidate lemmas from regular English inflection, most likely first.
///
/// Deliberately generous: producing a candidate costs one indexed lookup, and
/// every candidate is validated before use, so over-generating is cheap and
/// safe while under-generating means the reader gets nothing.
fn inflection_candidates(word: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if s.len() >= 2 && !out.contains(&s) {
            out.push(s);
        }
    };

    let w = word;

    // plural / third person
    if let Some(stem) = w.strip_suffix("ies") {
        push(format!("{stem}y"));
    }
    if let Some(stem) = w.strip_suffix("es") {
        push(stem.to_string());
    }
    if let Some(stem) = w.strip_suffix('s') {
        push(stem.to_string());
    }
    // past tense
    if let Some(stem) = w.strip_suffix("ied") {
        push(format!("{stem}y"));
    }
    if let Some(stem) = w.strip_suffix("ed") {
        push(stem.to_string());
        push(format!("{stem}e"));
        // doubled consonant: "stopped" -> "stop"
        if let Some(undoubled) = undouble(stem) {
            push(undoubled);
        }
    }
    // present participle
    if let Some(stem) = w.strip_suffix("ing") {
        push(stem.to_string());
        push(format!("{stem}e"));
        if let Some(undoubled) = undouble(stem) {
            push(undoubled);
        }
    }
    // comparative / superlative
    if let Some(stem) = w.strip_suffix("est") {
        push(stem.to_string());
        push(format!("{stem}e"));
    }
    if let Some(stem) = w.strip_suffix("er") {
        push(stem.to_string());
        push(format!("{stem}e"));
    }
    // adverbial
    if let Some(stem) = w.strip_suffix("ly") {
        push(stem.to_string());
    }
    // archaic verb endings, common in the books this app is for
    if let Some(stem) = w.strip_suffix("eth") {
        push(stem.to_string());
        push(format!("{stem}e"));
    }
    if let Some(stem) = w.strip_suffix("est") {
        push(stem.to_string());
    }

    out
}

/// "stopp" -> "stop", when the last two letters are the same consonant.
fn undouble(stem: &str) -> Option<String> {
    let mut chars = stem.chars().rev();
    let last = chars.next()?;
    let prev = chars.next()?;
    if last == prev && last.is_alphabetic() && !"aeiou".contains(last) {
        Some(stem[..stem.len() - last.len_utf8()].to_string())
    } else {
        None
    }
}

fn contextual_schema(sense_count: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "sense_index": {
                "type": "integer",
                "minimum": -1,
                "maximum": sense_count.saturating_sub(1) as i64,
                "description": "Index of the sense that applies in this sentence, or -1 if none of them do."
            },
            // maxLength is a guardrail, not styling. Without it qwen3:4b was
            // observed looping inside `plain_meaning` — restating the same
            // explanation until it ran past num_predict, leaving the JSON
            // unterminated and unparseable. A bounded string ends cleanly.
            // The limits are generous enough that a real answer is never cut.
            "plain_meaning": {
                "type": "string",
                "maxLength": 400,
                "description": "What the word means in this sentence, in plain modern English. One short sentence."
            },
            "why_this_sense": {
                "type": "string",
                "maxLength": 400,
                "description": "The clue in the sentence that settles it. One short sentence."
            }
        },
        "required": ["sense_index", "plain_meaning", "why_this_sense"]
    })
}

const CONTEXT_SYSTEM: &str = "\
You help a reader understand an unfamiliar word in the sentence they met it in.

You will be given a word, the sentence, and a numbered list of senses taken from \
a dictionary. Choose the sense that applies in this sentence and restate it in \
plain modern English.

Choose only from the numbered senses. Do not invent a meaning that is not in the \
list. If none of them fit, answer with sense_index -1 and say so plainly. Older \
books often use a word in a sense that has since died out, so do not assume the \
modern meaning is the right one.

Answer with one short sentence per field. Do not restate yourself.";

/// Layer two. Which of the retrieved senses applies here?
pub async fn sense_in_context(
    client: &OllamaClient,
    model: &str,
    word: &str,
    sentence: &str,
    senses: &[Sense],
) -> Result<ContextualSense> {
    if senses.is_empty() {
        return Err(AppError::Invalid(
            "no dictionary senses to choose between".into(),
        ));
    }

    let listed = senses
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let labels = if s.labels.is_empty() {
                String::new()
            } else {
                format!(" [{}]", s.labels.join(", "))
            };
            format!("{i}.{labels} {}", s.gloss)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "WORD: {word}\n\nSENTENCE:\n{sentence}\n\nSENSES:\n{listed}",
        word = word,
        sentence = sentence.trim(),
        listed = listed,
    );

    let mut result: ContextualSense = client
        .generate_structured(model, CONTEXT_SYSTEM, &prompt, contextual_schema(senses.len()))
        .await?;

    // A model that indexes past the end would otherwise crash the UI.
    if result.sense_index >= senses.len() as i64 {
        result.sense_index = -1;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_punctuation_from_a_selected_word() {
        assert_eq!(normalise_surface("  \"Nice,\"  "), "nice");
        assert_eq!(normalise_surface("well-known."), "well-known");
        assert_eq!(normalise_surface("don't"), "don't");
        assert_eq!(normalise_surface("---"), "");
    }

    #[test]
    fn generates_plural_candidates() {
        let c = inflection_candidates("cities");
        assert!(c.contains(&"city".to_string()), "got {c:?}");

        let c = inflection_candidates("books");
        assert!(c.contains(&"book".to_string()));
    }

    #[test]
    fn generates_past_tense_candidates() {
        let c = inflection_candidates("walked");
        assert!(c.contains(&"walk".to_string()), "got {c:?}");

        // Doubled consonant.
        let c = inflection_candidates("stopped");
        assert!(c.contains(&"stop".to_string()), "got {c:?}");

        // Silent e restored.
        let c = inflection_candidates("hoped");
        assert!(c.contains(&"hope".to_string()), "got {c:?}");
    }

    #[test]
    fn generates_participle_candidates() {
        let c = inflection_candidates("running");
        assert!(c.contains(&"run".to_string()), "got {c:?}");

        let c = inflection_candidates("writing");
        assert!(c.contains(&"write".to_string()), "got {c:?}");
    }

    /// The archaic third-person ending is everywhere in the books this app is
    /// meant for.
    #[test]
    fn generates_archaic_verb_candidates() {
        let c = inflection_candidates("speaketh");
        assert!(c.contains(&"speak".to_string()), "got {c:?}");
    }

    #[test]
    fn undoubles_only_repeated_consonants() {
        assert_eq!(undouble("stopp").as_deref(), Some("stop"));
        assert_eq!(undouble("runn").as_deref(), Some("run"));
        // Vowels are not undoubled: "agree" must stay intact.
        assert_eq!(undouble("agree"), None);
        assert_eq!(undouble("walk"), None);
    }

    #[test]
    fn contextual_schema_bounds_the_index() {
        let s = contextual_schema(3);
        assert_eq!(s["properties"]["sense_index"]["maximum"], 2);
        assert_eq!(s["properties"]["sense_index"]["minimum"], -1);
    }

    #[test]
    fn context_prompt_warns_against_the_modern_meaning() {
        // The single most likely failure for an old book is confidently
        // returning today's sense.
        assert!(CONTEXT_SYSTEM.contains("do not assume the"));
        assert!(CONTEXT_SYSTEM.contains("Do not invent"));
    }

    // --- tests against the real dictionary ------------------------------
    // Skipped when it has not been built, so a fresh checkout still passes
    // `cargo test` before `node scripts/build-dictionary.mjs` has been run.

    fn real_dict() -> Option<Dictionary> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("dict.sqlite");
        Dictionary::open(path).ok()
    }

    #[test]
    fn resolves_a_word_directly() {
        let Some(d) = real_dict() else { return };
        let r = d.lookup("reading").unwrap().unwrap();
        assert!(!r.senses.is_empty());
    }

    /// The reason this dictionary was chosen. A modern dictionary gives the
    /// wrong answer for these words in an old book.
    #[test]
    fn gives_the_period_sense_of_a_word_that_shifted() {
        let Some(d) = real_dict() else { return };

        let nice = d.lookup("nice").unwrap().unwrap();
        let first = &nice.senses[0].gloss.to_lowercase();
        assert!(
            first.contains("foolish") || first.contains("silly"),
            "expected the 1913 sense first, got: {first}"
        );
        assert!(nice.senses[0].labels.contains(&"obsolete".to_string()));

        let awful = d.lookup("awful").unwrap().unwrap();
        assert!(
            awful.senses[0].gloss.to_lowercase().contains("fear")
                || awful.senses[0].gloss.to_lowercase().contains("terrible"),
            "got: {}",
            awful.senses[0].gloss
        );
    }

    #[test]
    fn resolves_an_archaic_spelling() {
        let Some(d) = real_dict() else { return };

        // From the curated list.
        let r = d.lookup("publick").unwrap().unwrap();
        assert_eq!(r.lemma, "public");
        assert_eq!(r.resolution, Resolution::ArchaicSpelling);

        // From the dictionary's own cross-references.
        let r = d.lookup("shew").unwrap().unwrap();
        assert!(!r.senses.is_empty());
    }

    #[test]
    fn resolves_an_archaic_verb_form() {
        let Some(d) = real_dict() else { return };
        let r = d.lookup("hath").unwrap().unwrap();
        assert!(!r.senses.is_empty());
    }

    #[test]
    fn resolves_through_inflection() {
        let Some(d) = real_dict() else { return };
        let r = d.lookup("cities").unwrap().unwrap();
        assert_eq!(r.lemma, "city");
        assert_eq!(r.resolution, Resolution::Inflection);
    }

    #[test]
    fn handles_a_word_lifted_out_of_prose_with_punctuation() {
        let Some(d) = real_dict() else { return };
        let r = d.lookup("\"justice,\"").unwrap().unwrap();
        assert_eq!(r.lemma, "justice");
    }

    #[test]
    fn returns_nothing_for_a_nonsense_word() {
        let Some(d) = real_dict() else { return };
        assert!(d.lookup("zzzqqxnotaword").unwrap().is_none());
    }

    /// The dictionary also settles hyphenation and long-s repair for the OCR
    /// pipeline, so the oracle wiring is worth pinning down.
    #[test]
    fn works_as_a_word_oracle_for_segmentation() {
        use crate::ocr::segment::{join_hyphenated, repair_long_s, WordOracle};
        let Some(d) = real_dict() else { return };

        assert!(d.is_word("understand"));
        assert!(!d.is_word("zzzqqxnotaword"));

        // The real payoff: with a dictionary these two now resolve correctly
        // rather than falling back to a case heuristic.
        assert_eq!(join_hyphenated("under-", "stand", Some(&d)), "understand");
        assert_eq!(join_hyphenated("well-", "known", Some(&d)), "well-known");
        assert_eq!(repair_long_s("himfelf", &d).as_deref(), Some("himself"));
    }
}
