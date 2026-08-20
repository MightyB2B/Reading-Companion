//! The prompts behind the study tools.
//!
//! Every one of these keeps the guarantee the coach already has: the response
//! schema has no field an answer could occupy. The model classifies, points,
//! and asks — it never supplies the reconstruction, the gloss, or the summary.
//!
//! That is what makes the Suggest pass safe to leave switched on. A classifier
//! that returns *ordinals and labels* can highlight a whole page without ever
//! putting a sentence into the reader's work that they did not choose.
//!
//! Two of these return quoted text rather than labels, because "which words is
//! this author using oddly" cannot be answered with an ordinal. Those are
//! defended differently: every quoted span is checked against the passage
//! before it is returned, and anything the model invented is dropped. See
//! [`keep_only_quoted`].

use serde::{Deserialize, Serialize};

use crate::coach::split_sentences;
use crate::error::Result;
use crate::ollama::OllamaClient;
use crate::study::Move;

/// What a sentence is doing, as the classifier sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentenceRole {
    /// Index into the passage's sentences, zero-based.
    pub ordinal: usize,
    pub role: Move,
    /// The sentence itself, filled in here from the passage rather than by
    /// the model — so a suggestion can never show text the author did not
    /// write.
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct RawRoles {
    sentences: Vec<RawRole>,
}

#[derive(Debug, Deserialize)]
struct RawRole {
    ordinal: usize,
    role: String,
}

fn roles_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "sentences": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "ordinal": {
                            "type": "integer",
                            "description": "Zero-based index of the sentence, as numbered in the passage."
                        },
                        "role": {
                            "type": "string",
                            "enum": ["thesis", "premise", "conclusion", "definition",
                                     "objection", "reply", "example", "concession", "aporia"]
                        }
                    },
                    "required": ["ordinal", "role"]
                }
            }
        },
        "required": ["sentences"]
    })
}

const ROLES_SYSTEM: &str = "\
You are labelling the moves in a philosophical or theological passage.

The reader has numbered sentences in front of them. For each sentence that is \
clearly doing one of the listed things, return its number and the label. Leave \
out any sentence you are not confident about: a missing label costs nothing, \
and a wrong one sends the reader down the wrong path.

Watch for the objection especially. Authors raise a position in order to answer \
it, and very often give no signal at all that the view is not their own. A \
sentence that states a view the surrounding text then argues against is an \
objection, not a thesis.

An example illustrates and carries no argumentative weight. Do not label an \
illustration as a premise.

Return labels only. Do not quote, summarise, paraphrase, or explain.";

/// Label the moves in a passage — the Suggest pass.
///
/// The sentences are numbered here and the model is asked only for numbers
/// back, so nothing it returns can become text on the reader's page.
pub async fn sentence_roles(
    client: &OllamaClient,
    model: &str,
    passage: &str,
) -> Result<Vec<SentenceRole>> {
    let sentences = split_sentences(passage);
    if sentences.is_empty() {
        return Ok(Vec::new());
    }

    let numbered = sentences
        .iter()
        .enumerate()
        .map(|(i, s)| format!("[{i}] {s}"))
        .collect::<Vec<_>>()
        .join("\n");

    let raw: RawRoles = client
        .generate_structured(model, ROLES_SYSTEM, &numbered, roles_schema())
        .await?;

    Ok(clean_roles(raw.sentences, &sentences))
}

/// Drop labels that point at sentences which do not exist, and fill the text
/// in from the passage.
///
/// A model that returns ordinal 9 for a four-sentence paragraph has lost
/// track; rendering that as a highlight would put the mark on the wrong words
/// or crash the column. Deduplicated by ordinal, first label winning.
fn clean_roles(raw: Vec<RawRole>, sentences: &[String]) -> Vec<SentenceRole> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        if r.ordinal >= sentences.len() || !seen.insert(r.ordinal) {
            continue;
        }
        let Some(role) = Move::from_str_opt(&r.role) else {
            continue;
        };
        out.push(SentenceRole {
            ordinal: r.ordinal,
            role,
            text: sentences[r.ordinal].clone(),
        });
    }
    out.sort_by_key(|r| r.ordinal);
    out
}

// ---- support and charity -------------------------------------------------

/// Whether the premises reach the conclusion, and what is missing if not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportCheck {
    pub reaches_conclusion: bool,
    /// none | missing_premise | equivocation | irrelevance | circularity
    pub gap: String,
    /// A question sending the reader back to the argument. Never a premise.
    pub steering_question: String,
}

fn support_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "reaches_conclusion": { "type": "boolean" },
            "gap": {
                "type": "string",
                "enum": ["none", "missing_premise", "equivocation", "irrelevance", "circularity"]
            },
            "steering_question": {
                "type": "string",
                "description": "One short question about what the argument would need. Never state the missing premise itself."
            }
        },
        "required": ["reaches_conclusion", "gap", "steering_question"]
    })
}

const SUPPORT_SYSTEM: &str = "\
You are checking a reader's reconstruction of an argument.

They have written a conclusion and the premises they think support it. Say \
whether the premises, as written, actually reach the conclusion, and if not, \
name the kind of gap.

For steering_question, ask one short question about what the argument would \
need in order to work. Never state the missing premise. Never supply the \
premise in the question. The reader is meant to find it.";

pub async fn support_check(
    client: &OllamaClient,
    model: &str,
    conclusion: &str,
    premises: &[String],
) -> Result<SupportCheck> {
    let listed = premises
        .iter()
        .enumerate()
        .map(|(i, p)| format!("P{}. {p}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!("{listed}\nTherefore: {conclusion}");

    let mut check: SupportCheck = client
        .generate_structured(model, SUPPORT_SYSTEM, &prompt, support_schema())
        .await?;
    check.steering_question = crate::coach::scrub_question(&check.steering_question, &prompt);
    Ok(check)
}

/// Which premise is doing the least work, and whether a better one exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharityCheck {
    /// One-based, matching how premises are shown to the reader. Zero when no
    /// premise stands out.
    pub weakest_premise: usize,
    pub stronger_available: bool,
    pub steering_question: String,
}

fn charity_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "weakest_premise": {
                "type": "integer",
                "description": "One-based number of the least plausible premise, or 0 if none stands out."
            },
            "stronger_available": {
                "type": "boolean",
                "description": "Whether a more plausible premise would do the same job."
            },
            "steering_question": {
                "type": "string",
                "description": "One short question. Never state the stronger premise."
            }
        },
        "required": ["weakest_premise", "stronger_available", "steering_question"]
    })
}

const CHARITY_SYSTEM: &str = "\
You are applying the principle of charity to a reader's reconstruction.

A reconstruction should give the author the strongest version of their \
argument, even when the reader intends to reject it. A reconstruction that \
makes the author look foolish is usually a reconstruction error rather than a \
discovery.

Name the premise carrying the least weight, and say whether a more plausible \
one would do the same job. Do not write that premise. Ask a question instead.";

pub async fn charity_check(
    client: &OllamaClient,
    model: &str,
    conclusion: &str,
    premises: &[String],
) -> Result<CharityCheck> {
    let listed = premises
        .iter()
        .enumerate()
        .map(|(i, p)| format!("P{}. {p}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!("{listed}\nTherefore: {conclusion}");

    let mut check: CharityCheck = client
        .generate_structured(model, CHARITY_SYSTEM, &prompt, charity_schema())
        .await?;
    if check.weakest_premise > premises.len() {
        // Pointing at a premise that is not there would highlight nothing.
        check.weakest_premise = 0;
    }
    check.steering_question = crate::coach::scrub_question(&check.steering_question, &prompt);
    Ok(check)
}

// ---- candidate terms -----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateTerm {
    /// Copied from the passage, and verified to be there.
    pub surface_form: String,
    /// The author explicitly fixes its meaning here, rather than merely using
    /// it in a special way.
    pub stipulated: bool,
}

#[derive(Debug, Deserialize)]
struct RawTerms {
    terms: Vec<CandidateTerm>,
}

fn terms_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "terms": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "surface_form": {
                            "type": "string",
                            "description": "The word or phrase exactly as it appears in the passage. Copy it; do not rephrase it."
                        },
                        "stipulated": {
                            "type": "boolean",
                            "description": "True when the author explicitly defines it here."
                        }
                    },
                    "required": ["surface_form", "stipulated"]
                }
            }
        },
        "required": ["terms"]
    })
}

const TERMS_SYSTEM: &str = "\
You are spotting the words that carry the weight in a philosophical or \
theological passage.

Return only words or short phrases that appear in the passage and that this \
author is using in a special, technical, or stipulated sense — the ones a \
reader must pin down before the argument can be followed. Ordinary words used \
ordinarily do not belong here.

Copy each one exactly as it appears. Do not define them. Do not explain them. \
The reader writes the meaning; you are only pointing.";

/// Propose terms worth glossing. Never proposes a meaning.
pub async fn candidate_terms(
    client: &OllamaClient,
    model: &str,
    passage: &str,
) -> Result<Vec<CandidateTerm>> {
    let raw: RawTerms = client
        .generate_structured(model, TERMS_SYSTEM, passage, terms_schema())
        .await?;
    Ok(keep_only_quoted(raw.terms, passage))
}

/// Drop any candidate the passage does not actually contain.
///
/// This is the structural defence for the two prompts that must return text.
/// A model asked to copy will occasionally paraphrase, and a term the author
/// never used has no business in a lexicon of that author's usage. Checking is
/// cheap and makes the guarantee real rather than a matter of prompt wording.
fn keep_only_quoted(terms: Vec<CandidateTerm>, passage: &str) -> Vec<CandidateTerm> {
    let hay = passage.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    terms
        .into_iter()
        .filter(|t| {
            let needle = t.surface_form.trim().to_lowercase();
            !needle.is_empty() && hay.contains(&needle) && seen.insert(needle)
        })
        .collect()
}

// ---- interlocutor --------------------------------------------------------

/// Who a passage is arguing against, in the passage's own words.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interlocutor {
    /// The words naming the opponent, quoted. Empty when nobody is named.
    pub addressee_quoted: String,
    /// The words stating the position being opposed, quoted.
    pub position_quoted: String,
}

fn interlocutor_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "addressee_quoted": {
                "type": "string",
                "description": "The exact words from the passage naming who is being answered. Empty string if nobody is named."
            },
            "position_quoted": {
                "type": "string",
                "description": "The exact words from the passage stating the position being opposed. Empty string if none."
            }
        },
        "required": ["addressee_quoted", "position_quoted"]
    })
}

const INTERLOCUTOR_SYSTEM: &str = "\
You are identifying who a passage is answering.

Quote from the passage only. If the passage names an opponent, quote the words \
that name them. If it states a position it is arguing against, quote the words \
that state it. If it does neither, return empty strings.

Do not infer. Do not name a philosopher the passage does not name. Do not \
describe the position in your own words. Quote, or return nothing.";

/// Who the passage is answering — strictly from its own words.
///
/// "Who is this replying to" is the question where a model most wants to
/// invent a plausible name, so both fields are checked against the passage
/// and cleared if they are not in it.
pub async fn interlocutor(
    client: &OllamaClient,
    model: &str,
    passage: &str,
) -> Result<Interlocutor> {
    let mut found: Interlocutor = client
        .generate_structured(model, INTERLOCUTOR_SYSTEM, passage, interlocutor_schema())
        .await?;

    let hay = passage.to_lowercase();
    if !hay.contains(&found.addressee_quoted.trim().to_lowercase())
        || found.addressee_quoted.trim().is_empty()
    {
        found.addressee_quoted = String::new();
    }
    if !hay.contains(&found.position_quoted.trim().to_lowercase())
        || found.position_quoted.trim().is_empty()
    {
        found.position_quoted = String::new();
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ordinal: usize, role: &str) -> RawRole {
        RawRole {
            ordinal,
            role: role.into(),
        }
    }

    fn sentences() -> Vec<String> {
        vec!["One.".into(), "Two.".into(), "Three.".into()]
    }

    #[test]
    fn a_label_for_a_sentence_that_does_not_exist_is_dropped() {
        // Rendering this would put the highlight on the wrong words.
        let out = clean_roles(vec![raw(0, "premise"), raw(9, "thesis")], &sentences());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].ordinal, 0);
    }

    #[test]
    fn an_unknown_role_is_dropped_rather_than_defaulted() {
        let out = clean_roles(vec![raw(0, "vibes")], &sentences());
        assert!(out.is_empty());
    }

    #[test]
    fn the_first_label_for_a_sentence_wins() {
        let out = clean_roles(vec![raw(1, "premise"), raw(1, "objection")], &sentences());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Move::Premise);
    }

    #[test]
    fn labelled_text_comes_from_the_passage_not_the_model() {
        // The whole safety property: a suggestion can only ever show words
        // the author wrote.
        let out = clean_roles(vec![raw(2, "conclusion")], &sentences());
        assert_eq!(out[0].text, "Three.");
    }

    #[test]
    fn roles_come_back_in_reading_order() {
        let out = clean_roles(
            vec![raw(2, "conclusion"), raw(0, "premise"), raw(1, "premise")],
            &sentences(),
        );
        assert_eq!(
            out.iter().map(|r| r.ordinal).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn a_term_the_author_never_used_is_dropped() {
        // The structural guarantee for the one prompt that returns text.
        let passage = "By substance I understand that which is in itself.";
        let kept = keep_only_quoted(
            vec![
                CandidateTerm { surface_form: "substance".into(), stipulated: true },
                CandidateTerm { surface_form: "monad".into(), stipulated: false },
            ],
            passage,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].surface_form, "substance");
    }

    #[test]
    fn candidate_terms_match_regardless_of_case() {
        let kept = keep_only_quoted(
            vec![CandidateTerm { surface_form: "Substance".into(), stipulated: false }],
            "the substance of the matter",
        );
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn duplicate_candidates_are_collapsed() {
        let kept = keep_only_quoted(
            vec![
                CandidateTerm { surface_form: "mode".into(), stipulated: false },
                CandidateTerm { surface_form: "Mode".into(), stipulated: true },
            ],
            "a mode is an affection",
        );
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn an_empty_candidate_is_dropped() {
        // An empty needle is contained by every string, so without this every
        // blank the model emits becomes a term.
        let kept = keep_only_quoted(
            vec![CandidateTerm { surface_form: "   ".into(), stipulated: false }],
            "any passage at all",
        );
        assert!(kept.is_empty());
    }
}
