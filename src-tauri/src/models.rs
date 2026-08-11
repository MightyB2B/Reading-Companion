use serde::{Deserialize, Serialize};

/// The period a book was written in.
///
/// This is the single knob that drives all three era-sensitive behaviours:
/// which dictionary corpus is authoritative, how aggressively archaic
/// typography is normalised, and how the coach calibrates its feedback.
/// Reading Gibbon against a modern dictionary gives you the wrong meanings —
/// *nice*, *awful*, and *want* all shifted — so this is a correctness concern,
/// not a preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Era {
    /// 1950 onwards.
    Modern,
    /// 1900–1950.
    Early20c,
    /// 1800–1900.
    Victorian,
    /// 1500–1800: long-s, ligatures, unsettled orthography.
    EarlyModern,
}

impl Era {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "early_20c" => Era::Early20c,
            "victorian" => Era::Victorian,
            "early_modern" => Era::EarlyModern,
            _ => Era::Modern,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Era::Modern => "modern",
            Era::Early20c => "early_20c",
            Era::Victorian => "victorian",
            Era::EarlyModern => "early_modern",
        }
    }

    /// How hard to work at modernising the printed text.
    pub fn normalization(self) -> Normalization {
        match self {
            Era::Modern => Normalization::Off,
            Era::Early20c | Era::Victorian => Normalization::Light,
            Era::EarlyModern => Normalization::Full,
        }
    }

    /// Which dictionary to treat as authoritative when `dict_corpus` is 'auto'.
    ///
    /// Webster's 1913 is the period-correct choice for anything nineteenth
    /// century or earlier: it records the senses those authors were actually
    /// using.
    pub fn default_corpus(self) -> Corpus {
        match self {
            Era::Modern => Corpus::Wordnet,
            Era::Early20c => Corpus::Both,
            Era::Victorian | Era::EarlyModern => Corpus::Webster1913,
        }
    }

    /// Prose conventions the coach should expect, so it does not mark a reader
    /// down for faithfully summarising a periodic eighteenth-century sentence.
    pub fn prose_guidance(self) -> &'static str {
        match self {
            Era::Modern => {
                "This is modern prose. Expect short paragraphs with an explicit topic sentence."
            }
            Era::Early20c => {
                "This is early-twentieth-century prose. Paragraphs may be longer than modern ones \
                 and the main point is sometimes carried by the final sentence."
            }
            Era::Victorian => {
                "This is Victorian prose. Expect long periodic sentences with subordinate clauses, \
                 and paragraphs that build to their point rather than opening with it. Do not treat \
                 length or indirection as a fault in the reader's summary."
            }
            Era::EarlyModern => {
                "This is early modern prose (1500-1800). Expect elaborate periodic syntax, classical \
                 allusion, capitalised common nouns, and rhetorical structures foreign to modern \
                 writing. Judge the reader's summary on whether it captures the argument, never on \
                 whether it reads like modern English."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normalization {
    /// Leave the text as transcribed.
    Off,
    /// Ligature expansion and whitespace repair only.
    Light,
    /// Adds long-s repair and archaic spelling modernisation.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corpus {
    Webster1913,
    Wordnet,
    Both,
}

impl Corpus {
    pub fn from_book(dict_corpus: &str, era: Era) -> Self {
        match dict_corpus {
            "webster1913" => Corpus::Webster1913,
            "wordnet" => Corpus::Wordnet,
            "both" => Corpus::Both,
            _ => era.default_corpus(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub author: Option<String>,
    pub era: String,
    pub dict_corpus: String,
    /// The page the reader was last on, so closing the app does not lose
    /// their place. Null before they have opened the book, or if that page
    /// has since been deleted.
    pub last_page_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: i64,
    pub book_id: i64,
    /// The number printed on the page, once the model has read it. Null while
    /// the page is still awaiting transcription.
    pub page_no: Option<i64>,
    /// detected | manual | unknown
    pub page_no_source: String,
    /// photo | epub | pdf | web
    pub source_kind: String,
    /// The division this page belongs to, as the source declared it — a
    /// chapter name from an EPUB's contents, or which half of a spread a
    /// photograph came from.
    pub source_ref: Option<String>,
    pub image_orig: String,
    pub image_proc: Option<String>,
    pub ocr_status: String,
    pub ocr_model: Option<String>,
    pub ocr_error: Option<String>,
}

/// A paragraph. The atomic unit the whole study method operates on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: i64,
    pub page_id: i64,
    pub ordinal: i64,
    pub kind: String,
    pub text_raw: String,
    pub text_norm: String,
    pub user_edited: bool,
}

/// A block as it comes out of segmentation, before it has an id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DraftBlock {
    pub kind: BlockKind,
    pub text_raw: String,
    pub text_norm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Paragraph,
    Heading,
    Quote,
    Footnote,
    Caption,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Paragraph => "paragraph",
            BlockKind::Heading => "heading",
            BlockKind::Quote => "quote",
            BlockKind::Footnote => "footnote",
            BlockKind::Caption => "caption",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: i64,
    pub block_id: i64,
    pub sentence: String,
    pub revision: i64,
    pub self_checked: bool,
    pub created_at: String,
}

// The coach's response types live in `crate::coach`: what the model may emit
// is inseparable from how the no-leak guarantee is enforced, so they are
// defined next to that reasoning rather than here.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sense {
    pub id: i64,
    pub headword: String,
    pub pos: Option<String>,
    pub gloss: String,
    pub source: String,
    /// e.g. "archaic", "obsolete" — surfaced so the reader knows a sense is
    /// period-specific.
    pub labels: Vec<String>,
}

/// Result of a dictionary lookup. `senses` always comes from the local
/// database; `llm_gloss` is only populated when the reader explicitly asks
/// what the word means *in this sentence*.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupResult {
    pub word: String,
    pub lemma: String,
    /// True when the surface form was resolved through the archaic-variant
    /// table (e.g. "publick" -> "public"), which is worth telling the reader.
    pub via_variant: bool,
    pub senses: Vec<Sense>,
    pub corpus: Corpus,
}

/// The in-context answer, grounded in a sense that actually exists in the
/// dictionary rather than invented by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextualSense {
    /// Index into the `senses` list that was supplied to the model.
    pub sense_index: usize,
    pub plain_meaning: String,
    pub why_this_sense: String,
}
