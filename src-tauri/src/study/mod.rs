//! The study layer: notes, marks, terms, and argument reconstructions.
//!
//! Everything here anchors to a *span* — a block plus a character range —
//! rather than to a page. That is what lets one set of tools work on a
//! philosophy paragraph and on a verse of scripture without either being a
//! special case, and it is why re-transcribing a page cannot silently destroy
//! the reader's work: the anchor carries its own text and can find its way
//! home when block ids change underneath it.

pub mod commands;

use serde::{Deserialize, Serialize};

/// What a passage is *doing*.
///
/// Marking prose with these is the philosophy-reading analogue of a
/// highlighter, except the colour means something the application can act on:
/// tagging a span as a premise makes it selectable when building an argument.
///
/// The set is drawn from what philosophy reading guides actually teach —
/// flagging the dialectic — rather than from anything about the software.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    /// The one claim a chapter or book exists to defend.
    Thesis,
    /// A reason offered in support of something else.
    Premise,
    /// What the premises are meant to establish.
    Conclusion,
    /// The author fixing a term's meaning, often stipulatively.
    Definition,
    /// A position raised in order to be answered. The one most misread as the
    /// author's own view, because it is frequently unflagged.
    Objection,
    /// The answer to an objection.
    Reply,
    /// An illustration. Carries no argumentative weight — mistaking one for a
    /// premise is the commonest reconstruction error.
    Example,
    /// A point granted to the opponent, marking the limits of the claim.
    Concession,
    /// A puzzle deliberately left unresolved.
    Aporia,
}

impl Move {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        Some(match s {
            "thesis" => Move::Thesis,
            "premise" => Move::Premise,
            "conclusion" => Move::Conclusion,
            "definition" => Move::Definition,
            "objection" => Move::Objection,
            "reply" => Move::Reply,
            "example" => Move::Example,
            "concession" => Move::Concession,
            "aporia" => Move::Aporia,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Move::Thesis => "thesis",
            Move::Premise => "premise",
            Move::Conclusion => "conclusion",
            Move::Definition => "definition",
            Move::Objection => "objection",
            Move::Reply => "reply",
            Move::Example => "example",
            Move::Concession => "concession",
            Move::Aporia => "aporia",
        }
    }

    /// Every move, with what it is and the tell that gives it away on the
    /// page.
    ///
    /// This ships to the frontend so the vocabulary is defined where the
    /// reader meets it. Nobody arrives knowing what an aporia is, and a
    /// palette of nine unexplained words is not a tool.
    pub fn catalogue() -> Vec<MoveInfo> {
        use Move::*;
        [
            (Thesis, "The one claim the chapter or book exists to defend.",
             "Stated once, early, often as a promise: \"I shall show that...\""),
            (Premise, "A reason offered in support of something else.",
             "since, because, for, given that"),
            (Conclusion, "What the premises are meant to establish.",
             "therefore, hence, so, it follows that"),
            (Definition, "The author fixing a term's meaning, often stipulatively.",
             "\"By X I understand...\", \"let us call this...\""),
            (Objection, "A position raised in order to be answered.",
             "\"It might be said...\" — and often no flag at all, which is why this is the one most misread as the author's own view."),
            (Reply, "The answer to an objection.",
             "\"But this cannot be, since...\""),
            (Example, "An illustration. Carries no argumentative weight.",
             "\"for instance\", \"consider the case of\" — mistaking one for a premise is the commonest reconstruction error."),
            (Concession, "A point granted to the opponent.",
             "\"I admit that...\", \"of course...\" — marks the limits of the claim."),
            (Aporia, "A puzzle deliberately left unresolved.",
             "The passage stops without settling. Common in Plato — tagging it stops you hunting for an answer the text does not contain."),
        ]
        .into_iter()
        .map(|(m, what, tell)| MoveInfo {
            id: m,
            label: m.as_str().to_string(),
            what: what.to_string(),
            tell: tell.to_string(),
        })
        .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveInfo {
    pub id: Move,
    pub label: String,
    pub what: String,
    /// How to recognise it in the text.
    pub tell: String,
}

/// A note, a mark, or both.
///
/// One with a `body` and no `r#move` is a note; one with a `r#move` and no
/// body is a mark; one with both is a highlighted passage the reader also
/// wrote about. They are one type because they share the hard part —
/// anchoring — and splitting them would mean maintaining that twice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub book_id: i64,
    pub notebook_id: Option<i64>,
    /// Null for a note about the book rather than a passage in it.
    pub block_id: Option<i64>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    /// The words the note was attached to, kept so the anchor survives a
    /// re-transcription that changes block ids.
    pub anchor_text: String,
    pub r#move: Option<Move>,
    pub body: String,
    /// The anchor could not be found after a re-import. The note is kept and
    /// flagged rather than deleted.
    pub orphaned: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub id: i64,
    pub book_id: Option<i64>,
    pub name: String,
    pub kind: String,
    pub notes: i64,
}

/// Where the reader is with a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermStatus {
    /// Met it, do not have it yet. Everything starts here.
    Unclear,
    Working,
    /// Understood well enough to use. Moving one here is the reward.
    Settled,
}

impl TermStatus {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "working" => TermStatus::Working,
            "settled" => TermStatus::Settled,
            _ => TermStatus::Unclear,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TermStatus::Unclear => "unclear",
            TermStatus::Working => "working",
            TermStatus::Settled => "settled",
        }
    }
}

/// A word this author uses in a special sense.
///
/// Adler's rule 5, and the gap no dictionary can fill: Webster's 1913 reports
/// what *substance* meant in 1913, which is not what Spinoza meant by it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Term {
    pub id: i64,
    pub book_id: i64,
    pub term: String,
    pub my_gloss: String,
    pub status: TermStatus,
    pub first_block_id: Option<i64>,
    /// How many times it has been found in the book.
    pub mentions: i64,
    /// Superseded glosses, newest first. Empty until the sense moves.
    pub revisions: Vec<TermRevision>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermRevision {
    pub my_gloss: String,
    pub status: TermStatus,
    pub created_at: String,
}

/// A reconstruction in standard form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Argument {
    pub id: i64,
    pub book_id: i64,
    pub label: String,
    pub conclusion: String,
    pub verdict: String,
    pub gap: String,
    pub anchor_block_id: Option<i64>,
    pub notes: String,
    pub premises: Vec<Premise>,
    /// Arguments that support, object to, or reply to this one.
    pub links: Vec<ArgumentLink>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Premise {
    pub id: i64,
    pub ordinal: i64,
    pub text: String,
    /// True when the reader supplied it rather than the author. Keeping the
    /// distinction is the point of the suppressed-premise step.
    pub implicit: bool,
    pub source_block_id: Option<i64>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgumentLink {
    pub id: i64,
    pub child_id: i64,
    pub child_label: String,
    pub child_conclusion: String,
    pub role: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_move_round_trips_through_its_string() {
        for info in Move::catalogue() {
            assert_eq!(
                Move::from_str_opt(info.id.as_str()),
                Some(info.id),
                "{} did not round-trip",
                info.label
            );
        }
    }

    #[test]
    fn the_catalogue_covers_every_variant() {
        // A move without an explanation is a word in a palette, which is the
        // thing this catalogue exists to prevent.
        assert_eq!(Move::catalogue().len(), 9);
        for info in Move::catalogue() {
            assert!(!info.what.is_empty());
            assert!(!info.tell.is_empty());
        }
    }

    #[test]
    fn unknown_move_is_none_rather_than_a_default() {
        // Defaulting would silently turn a typo into a thesis.
        assert_eq!(Move::from_str_opt("premis"), None);
        assert_eq!(Move::from_str_opt(""), None);
    }
}
