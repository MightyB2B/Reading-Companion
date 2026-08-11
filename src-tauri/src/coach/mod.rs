//! The summarisation coach.
//!
//! The design constraint that matters most is what the coach must *not* do. A
//! model that writes the one-sentence summary for you destroys the point of
//! the exercise: the value is in the reader doing the compression, and an
//! exemplar on screen turns the task into copying.
//!
//! # Why this is a rubric and not a critique
//!
//! The first design asked the model for prose feedback under a schema with no
//! "answer" field, on the theory that the missing field made leaking
//! impossible. Testing against qwen3:4b disproved it twice over:
//!
//! 1. The model stated the paragraph's main point inside the prose fields —
//!    "the difference was not intelligence but method" — which is precisely
//!    the sentence the reader was supposed to write.
//! 2. Hardening the instructions made it worse. With terse rules and
//!    worked BAD/GOOD examples, the 4B model began pattern-matching the
//!    examples and inverted the fields, reporting the paragraph's point as
//!    what the reader had "captured".
//!
//! A 4B model will not reliably withhold content while writing free prose
//! about it. So it is not asked to. The model returns *categorical* judgments
//! — booleans and an enum — and the application composes the wording from
//! them in [`render_feedback`]. Content it never writes is content it cannot
//! leak, and the guarantee no longer depends on instruction-following.
//!
//! One free-text field survives: the steering question. Questions leak far
//! less than statements, and [`scrub_question`] catches the case where it
//! quotes the paragraph back anyway.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::models::Era;
use crate::ollama::OllamaClient;

/// What the reader's sentence does and does not do.
///
/// Every field is categorical. There is nowhere here to write a summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    pub verdict: Verdict,
    pub covers: Coverage,
    pub problem: Problem,
    /// The only free text, and the only field needing a leak guard.
    pub steering_question: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    OnTarget,
    Partial,
    OffTarget,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::OnTarget => "on_target",
            Verdict::Partial => "partial",
            Verdict::OffTarget => "off_target",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Coverage {
    /// Does the sentence name who or what the paragraph is about? This is
    /// step 4 of the method, checked by the model as a backstop to the
    /// reader's own self-assessment.
    pub subject: bool,
    /// Does it state what happens, or what is claimed?
    pub main_action: bool,
    /// Does it carry the why or the so-what, where the paragraph has one?
    pub reason_or_result: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    #[serde(alias = "")]
    None,
    /// True as far as it goes, but says nothing specific.
    TooVague,
    /// Fastened onto a supporting detail instead of the point.
    DetailNotPoint,
    /// About something the paragraph does not actually say.
    WrongFocus,
    /// Right direction, stops short.
    Incomplete,
}

impl Problem {
    pub fn as_str(self) -> &'static str {
        match self {
            Problem::None => "none",
            Problem::TooVague => "too_vague",
            Problem::DetailNotPoint => "detail_not_point",
            Problem::WrongFocus => "wrong_focus",
            Problem::Incomplete => "incomplete",
        }
    }
}

pub fn rubric_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "verdict": {
                "type": "string",
                "enum": ["on_target", "partial", "off_target"]
            },
            "covers": {
                "type": "object",
                "properties": {
                    "subject":          { "type": "boolean" },
                    "main_action":      { "type": "boolean" },
                    "reason_or_result": { "type": "boolean" }
                },
                "required": ["subject", "main_action", "reason_or_result"]
            },
            "problem": {
                "type": "string",
                "enum": ["none", "too_vague", "detail_not_point", "wrong_focus", "incomplete"]
            },
            "steering_question": {
                "type": "string",
                "description": "One short question sending the reader back to the right part of the paragraph. Ask about what they should look for. Never state what they will find."
            }
        },
        "required": ["verdict", "covers", "problem", "steering_question"]
    })
}

const COACH_SYSTEM: &str = "\
You are a reading coach. A reader is working through a book one paragraph at a \
time, writing one sentence that captures each paragraph's main point.

You are grading their sentence against a rubric. Answer only with the rubric \
fields. Judge whether their sentence captures the paragraph's main point or \
action. Ignore style, grammar, and word choice entirely: a blunt, plain \
sentence that captures the point is a complete success.

For steering_question, ask one short question that sends them back to the right \
part of the paragraph. Ask what to look for. Never say what they will find, and \
never quote the paragraph.";

fn user_prompt(paragraph: &str, sentence: &str, era: Era) -> String {
    format!(
        "{era_note}\n\n\
         PARAGRAPH:\n{paragraph}\n\n\
         THE READER'S ONE-SENTENCE SUMMARY:\n{sentence}",
        era_note = era.prose_guidance(),
        paragraph = paragraph.trim(),
        sentence = sentence.trim(),
    )
}

/// A fallback used when the model's question quotes the paragraph back.
const GENERIC_QUESTION: &str =
    "Read the paragraph again. What is the one thing it is trying to tell you?";

/// Longest run of words the question may share verbatim with the paragraph.
///
/// A question naturally reuses a noun or two from the text; reproducing a
/// whole clause is how the answer gets handed over.
const MAX_SHARED_RUN: usize = 6;

fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Replace the question if it quotes a long span of the paragraph.
///
/// The one remaining leak path, closed with a check rather than a plea to the
/// model.
pub fn scrub_question(question: &str, paragraph: &str) -> String {
    let q = words(question);
    let p = words(paragraph);

    if q.len() > MAX_SHARED_RUN && p.len() >= MAX_SHARED_RUN {
        for window in q.windows(MAX_SHARED_RUN) {
            if p.windows(MAX_SHARED_RUN).any(|pw| pw == window) {
                return GENERIC_QUESTION.to_string();
            }
        }
    }
    if question.trim().is_empty() {
        return GENERIC_QUESTION.to_string();
    }
    question.trim().to_string()
}

/// Compose the reader-facing feedback from the rubric.
///
/// Written here, in ordinary Rust, rather than by the model. This is what
/// makes the no-leak guarantee hold: the sentences below are the only prose
/// the reader ever sees about their gap, and none of them can contain the
/// paragraph's content.
pub fn render_feedback(r: &Rubric) -> Feedback {
    let headline = match r.verdict {
        Verdict::OnTarget => "That captures it.",
        Verdict::Partial => "Close — but something is missing.",
        Verdict::OffTarget => "That is not the main point.",
    };

    let mut notes: Vec<&'static str> = Vec::new();
    if !r.covers.subject {
        notes.push("Your sentence does not make clear who or what this is about.");
    }
    if !r.covers.main_action {
        notes.push("It does not say what actually happens or what is being claimed.");
    }
    if !r.covers.reason_or_result {
        notes.push("It stops at what happened and never reaches the why or the so-what.");
    }

    let problem = match r.problem {
        Problem::None => None,
        Problem::TooVague => Some("It is true, but too general to be useful to you later."),
        Problem::DetailNotPoint => {
            Some("You have hold of a supporting detail rather than the point it supports.")
        }
        Problem::WrongFocus => Some("This is about something the paragraph does not actually claim."),
        Problem::Incomplete => Some("You are heading the right way, but stopped short."),
    };

    Feedback {
        headline: headline.to_string(),
        notes: notes.into_iter().map(str::to_string).collect(),
        problem: problem.map(str::to_string),
        steering_question: r.steering_question.clone(),
        verdict: r.verdict,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub verdict: Verdict,
    pub headline: String,
    pub notes: Vec<String>,
    pub problem: Option<String>,
    pub steering_question: String,
}

/// The rubric the model returned plus the wording the reader sees.
///
/// Both are carried so the rubric can be stored as data rather than
/// reconstructed by parsing the rendered prose back apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub rubric: Rubric,
    pub feedback: Feedback,
}

/// Assess a reader's summary. Never returns the paragraph's content.
pub async fn critique(
    client: &OllamaClient,
    model: &str,
    paragraph: &str,
    sentence: &str,
    era: Era,
) -> Result<Assessment> {
    let mut rubric: Rubric = client
        .generate_structured(
            model,
            COACH_SYSTEM,
            &user_prompt(paragraph, sentence, era),
            rubric_schema(),
        )
        .await?;

    rubric.steering_question = scrub_question(&rubric.steering_question, paragraph);
    let feedback = render_feedback(&rubric);
    Ok(Assessment { rubric, feedback })
}

/// A worked example, produced only when the reader explicitly asks for one
/// after trying themselves.
///
/// A separate type and a separate call, so it can never arrive as a side
/// effect of asking for feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exemplar {
    pub sentence: String,
    /// Why this sentence works, so the example teaches the method rather than
    /// just handing over an answer.
    pub reasoning: String,
}

fn exemplar_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "sentence": {
                "type": "string",
                "description": "One plain sentence capturing the paragraph's main point."
            },
            "reasoning": {
                "type": "string",
                "description": "Why this sentence captures the point, in terms of the method."
            }
        },
        "required": ["sentence", "reasoning"]
    })
}

const EXEMPLAR_SYSTEM: &str = "\
You are a reading coach. The reader has attempted a one-sentence summary and \
has explicitly asked to see one possible example. Give exactly one plain \
sentence capturing the paragraph's main point or action, then explain briefly \
why it works. Keep the sentence simple: it demonstrates the method, it is not a \
display of style.";

pub async fn exemplar(
    client: &OllamaClient,
    model: &str,
    paragraph: &str,
    era: Era,
) -> Result<Exemplar> {
    let prompt = format!(
        "{}\n\nPARAGRAPH:\n{}",
        era.prose_guidance(),
        paragraph.trim()
    );
    client
        .generate_structured(model, EXEMPLAR_SYSTEM, &prompt, exemplar_schema())
        .await
}

const ABBREVIATIONS: [&str; 16] = [
    "mr.", "mrs.", "ms.", "dr.", "st.", "prof.", "e.g.", "i.e.", "etc.", "vs.", "jr.", "sr.",
    "rev.", "cf.", "viz.", "ibid.",
];

/// Split a paragraph into its sentences.
///
/// Used for the sentence tier of the method: before compressing a whole
/// paragraph into one sentence, the reader works through it a sentence at a
/// time. That is the step where a long periodic sentence in an older book
/// actually gets understood rather than skimmed.
///
/// A full sentence tokeniser is not warranted here. A terminator ends a
/// sentence when it is not a known abbreviation and the next word opens a new
/// one; getting an unusual case slightly wrong costs the reader one extra
/// split, not a wrong answer.
pub fn split_sentences(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for (i, word) in words.iter().enumerate() {
        current.push(word);

        let lower = word.to_lowercase();
        if ABBREVIATIONS.contains(&lower.as_str()) || !word.ends_with(['.', '!', '?']) {
            continue;
        }
        // A terminator on the last word simply closes the final sentence.
        let Some(next) = words.get(i + 1) else { continue };

        if next
            .chars()
            .find(|c| c.is_alphanumeric() || *c == '"' || *c == '\u{201C}')
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit() || c == '"' || c == '\u{201C}')
        {
            out.push(current.join(" "));
            current.clear();
        }
    }
    if !current.is_empty() {
        out.push(current.join(" "));
    }
    out
}

/// Does a piece of text contain more than one sentence?
///
/// Step 3 of the method is "write *one* sentence", so the UI constrains the
/// input. Abbreviations are excluded so "Dr. Johnson argued X" is not counted
/// as two.
pub fn counts_as_multiple_sentences(text: &str) -> bool {
    split_sentences(text).len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAGRAPH: &str = "A well-known experiment makes the point. Readers were asked to \
        summarise a passage in one sentence. Those who worked paragraph by paragraph retained far \
        more than those who read the chapter straight through and then tried to recall it. The \
        difference was not intelligence but method.";

    /// The structural guarantee, now enforceable: every field the model fills
    /// is categorical except the question, which is scrubbed separately.
    #[test]
    fn rubric_schema_has_no_free_text_except_the_question() {
        let schema = rubric_schema();
        let props = schema["properties"].as_object().unwrap();

        for (name, spec) in props {
            if name == "steering_question" {
                continue;
            }
            let ty = spec["type"].as_str().unwrap();
            assert!(
                ty == "boolean" || ty == "object" || spec.get("enum").is_some(),
                "field `{name}` is unconstrained free text; the model could write \
                 the paragraph's point into it"
            );
        }
    }

    #[test]
    fn rendered_feedback_never_contains_paragraph_content() {
        // Every combination of rubric outcomes, checked against the giveaway
        // phrases from the source paragraph.
        let giveaways = [
            "not intelligence",
            "but method",
            "retained far more",
            "paragraph by paragraph",
        ];

        for verdict in [Verdict::OnTarget, Verdict::Partial, Verdict::OffTarget] {
            for problem in [
                Problem::None,
                Problem::TooVague,
                Problem::DetailNotPoint,
                Problem::WrongFocus,
                Problem::Incomplete,
            ] {
                for bits in 0..8u8 {
                    let r = Rubric {
                        verdict,
                        covers: Coverage {
                            subject: bits & 1 != 0,
                            main_action: bits & 2 != 0,
                            reason_or_result: bits & 4 != 0,
                        },
                        problem,
                        steering_question: String::new(),
                    };
                    let f = render_feedback(&r);
                    let text =
                        format!("{} {} {}", f.headline, f.notes.join(" "), f.problem.unwrap_or_default())
                            .to_lowercase();
                    for g in giveaways {
                        assert!(!text.contains(g), "rendered feedback leaked `{g}`");
                    }
                }
            }
        }
    }

    /// Verbatim output from qwen3:4b during evaluation. The rubric stopped the
    /// model writing prose about the paragraph, but it still smuggled the
    /// point into the one remaining free-text field, phrased as a question.
    #[test]
    fn scrubs_the_real_observed_question_leak() {
        let leaky =
            "What specific detail in the paragraph shows that the difference was not intelligence but method?";
        assert_eq!(scrub_question(leaky, PARAGRAPH), GENERIC_QUESTION);
    }

    #[test]
    fn scrubs_a_question_that_quotes_a_clause_back() {
        let leaky =
            "Why did those who worked paragraph by paragraph retained far more than those who read?";
        assert_eq!(scrub_question(leaky, PARAGRAPH), GENERIC_QUESTION);
    }

    /// Also from evaluation, and correctly left alone: it names the topic
    /// without disclosing the finding.
    #[test]
    fn keeps_a_real_question_that_only_names_the_topic() {
        let good = "What part of the paragraph explains why the method matters for memory retention?";
        assert_eq!(scrub_question(good, PARAGRAPH), good);
    }

    #[test]
    fn keeps_a_genuinely_socratic_question() {
        let good = "What does the last line add that the rest does not?";
        assert_eq!(scrub_question(good, PARAGRAPH), good);
    }

    #[test]
    fn replaces_an_empty_question() {
        assert_eq!(scrub_question("   ", PARAGRAPH), GENERIC_QUESTION);
    }

    #[test]
    fn missing_coverage_produces_a_specific_note() {
        let r = Rubric {
            verdict: Verdict::Partial,
            covers: Coverage {
                subject: true,
                main_action: true,
                reason_or_result: false,
            },
            problem: Problem::Incomplete,
            steering_question: "What does the final sentence add?".into(),
        };
        let f = render_feedback(&r);
        assert_eq!(f.notes.len(), 1);
        assert!(f.notes[0].contains("why"));
        assert!(f.problem.unwrap().contains("stopped short"));
    }

    #[test]
    fn full_coverage_on_target_produces_no_notes() {
        let r = Rubric {
            verdict: Verdict::OnTarget,
            covers: Coverage {
                subject: true,
                main_action: true,
                reason_or_result: true,
            },
            problem: Problem::None,
            steering_question: String::new(),
        };
        let f = render_feedback(&r);
        assert!(f.notes.is_empty());
        assert!(f.problem.is_none());
        assert_eq!(f.headline, "That captures it.");
    }

    #[test]
    fn era_guidance_reaches_the_prompt() {
        let p = user_prompt("Some paragraph.", "My sentence.", Era::Victorian);
        assert!(p.contains("Victorian"));
        assert!(p.contains("Some paragraph."));
        assert!(p.contains("My sentence."));
    }

    #[test]
    fn detects_two_sentences() {
        assert!(counts_as_multiple_sentences(
            "The author argues X. He then proves it."
        ));
    }

    #[test]
    fn accepts_a_single_sentence() {
        assert!(!counts_as_multiple_sentences(
            "The author argues that method beats speed."
        ));
        assert!(!counts_as_multiple_sentences(
            "The author argues that method beats speed"
        ));
    }

    #[test]
    fn splits_a_paragraph_into_sentences() {
        let s = split_sentences(
            "Science is more than knowledge. Knowledge is the persuasion of what is true. \
             But the facts of astronomy do not constitute the science.",
        );
        assert_eq!(s.len(), 3, "got {s:#?}");
        assert_eq!(s[0], "Science is more than knowledge.");
        assert!(s[2].starts_with("But the facts"));
    }

    #[test]
    fn keeps_an_abbreviation_inside_its_sentence() {
        let s = split_sentences("Dr. Johnson argued the point. He was right.");
        assert_eq!(s.len(), 2, "got {s:#?}");
        assert_eq!(s[0], "Dr. Johnson argued the point.");
    }

    #[test]
    fn a_single_sentence_splits_into_one() {
        let s = split_sentences("Only one sentence here.");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn keeps_a_trailing_fragment() {
        // Pages end mid-sentence; the fragment must not be dropped.
        let s = split_sentences("A complete sentence. And a fragment that runs");
        assert_eq!(s.len(), 2, "got {s:#?}");
        assert_eq!(s[1], "And a fragment that runs");
    }

    #[test]
    fn splitting_loses_no_words() {
        let text = "First one. Second one! Third one? A trailing fragment";
        let joined = split_sentences(text).join(" ");
        assert_eq!(joined, text);
    }

    #[test]
    fn does_not_trip_on_abbreviations() {
        assert!(!counts_as_multiple_sentences(
            "Dr. Johnson argued that method beats speed."
        ));
        assert!(!counts_as_multiple_sentences(
            "Readers retain more, e.g. when working paragraph by paragraph."
        ));
    }
}
