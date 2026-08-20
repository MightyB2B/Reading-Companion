//! Getting your work out of the application.
//!
//! Everything a reader writes here — the summary spine, the glosses, the
//! reconstructions — is theirs, and a study tool that can only be read inside
//! itself is a place work goes to be forgotten. Three formats, one document
//! model: Markdown for anything text-shaped, `.docx` for Word, and `.odt` for
//! LibreOffice.

pub mod docx;
pub mod markdown;
pub mod odt;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, Result};

/// The shape of an exported document, independent of any file format.
///
/// Built once and handed to each writer, so the three formats cannot drift
/// into saying different things about the same book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub title: String,
    pub author: Option<String>,
    pub exported: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub heading: String,
    /// Shown under the heading when the section needs explaining.
    pub blurb: Option<String>,
    pub entries: Vec<Entry>,
}

/// One item: a heading of its own, some body paragraphs, and optional detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub title: Option<String>,
    pub body: Vec<String>,
    /// Small print — a page number, a status, a source.
    pub note: Option<String>,
    /// Rendered as a list rather than as paragraphs.
    pub bullets: Vec<String>,
}

impl Entry {
    fn plain(body: impl Into<String>) -> Self {
        Entry {
            title: None,
            body: vec![body.into()],
            note: None,
            bullets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Markdown,
    Docx,
    Odt,
}

impl Format {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Format::Markdown,
            "docx" | "word" => Format::Docx,
            "odt" | "libreoffice" => Format::Odt,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Docx => "docx",
            Format::Odt => "odt",
        }
    }
}

/// Gather everything a reader has written about a book.
pub fn build(db: &Db, book_id: i64) -> Result<Document> {
    let book = db.get_book(book_id)?;
    let mut sections = Vec::new();

    // ---- the spine ------------------------------------------------------
    let spine = db.summary_spine(book_id)?;
    if !spine.is_empty() {
        sections.push(Section {
            heading: "The argument in one sentence at a time".into(),
            blurb: Some(
                "Each paragraph reduced to its main point, in the order you read them."
                    .into(),
            ),
            entries: spine
                .iter()
                .map(|s| Entry {
                    title: None,
                    body: vec![s.sentence.clone()],
                    // A spine entry always has a page; the type does not
                    // know that, and "p. —" is better than refusing to export.
                    note: Some(match s.page_no {
                        Some(n) => format!("p. {n}"),
                        None => "unnumbered page".into(),
                    }),
                    bullets: Vec::new(),
                })
                .collect(),
        });
    }

    // ---- terms ----------------------------------------------------------
    let terms = db.list_terms(book_id)?;
    if !terms.is_empty() {
        sections.push(Section {
            heading: "Terms".into(),
            blurb: Some(
                "Words this author uses in a special sense, in your words.".into(),
            ),
            entries: terms
                .iter()
                .map(|t| Entry {
                    title: Some(t.term.clone()),
                    body: if t.my_gloss.is_empty() {
                        vec!["(no gloss written)".into()]
                    } else {
                        vec![t.my_gloss.clone()]
                    },
                    note: Some(format!(
                        "{} · {} mention{}",
                        t.status.as_str(),
                        t.mentions,
                        if t.mentions == 1 { "" } else { "s" }
                    )),
                    // Superseded readings are kept, because a term whose sense
                    // moved is the most interesting thing in the notebook.
                    bullets: t
                        .revisions
                        .iter()
                        .map(|r| format!("earlier: {}", r.my_gloss))
                        .collect(),
                })
                .collect(),
        });
    }

    // ---- arguments ------------------------------------------------------
    let arguments = db.list_arguments(book_id)?;
    if !arguments.is_empty() {
        sections.push(Section {
            heading: "Arguments".into(),
            blurb: Some("Reconstructions in standard form.".into()),
            entries: arguments
                .iter()
                .map(|a| {
                    let mut bullets: Vec<String> = a
                        .premises
                        .iter()
                        .map(|p| {
                            if p.implicit {
                                format!("{}. {} (unstated)", p.ordinal + 1, p.text)
                            } else {
                                format!("{}. {}", p.ordinal + 1, p.text)
                            }
                        })
                        .collect();
                    bullets.push(format!("Therefore: {}", a.conclusion));

                    Entry {
                        title: Some(if a.label.is_empty() {
                            "Untitled argument".into()
                        } else {
                            a.label.clone()
                        }),
                        body: if a.notes.trim().is_empty() {
                            Vec::new()
                        } else {
                            vec![a.notes.clone()]
                        },
                        note: Some(format!("verdict: {}", a.verdict)),
                        bullets,
                    }
                })
                .collect(),
        });
    }

    // ---- notes ----------------------------------------------------------
    let notes = db.list_notes(book_id, None)?;
    if !notes.is_empty() {
        sections.push(Section {
            heading: "Notes".into(),
            blurb: None,
            entries: notes
                .iter()
                .map(|n| Entry {
                    title: None,
                    body: if n.body.trim().is_empty() {
                        vec!["(tagged, nothing written)".into()]
                    } else {
                        vec![n.body.clone()]
                    },
                    note: Some(match (&n.r#move, n.anchor_text.is_empty()) {
                        (Some(m), false) => {
                            format!("{} · on “{}”", m.as_str(), n.anchor_text)
                        }
                        (Some(m), true) => m.as_str().to_string(),
                        (None, false) => format!("on “{}”", n.anchor_text),
                        (None, true) => "about the book".into(),
                    }),
                    bullets: Vec::new(),
                })
                .collect(),
        });
    }

    if sections.is_empty() {
        sections.push(Section {
            heading: "Nothing written yet".into(),
            blurb: None,
            entries: vec![Entry::plain(
                "This book has no summaries, terms, arguments, or notes yet.",
            )],
        });
    }

    Ok(Document {
        title: book.title,
        author: book.author,
        exported: chrono::Local::now().format("%-d %B %Y").to_string(),
        sections,
    })
}

/// Render a document to bytes in the requested format.
pub fn render(doc: &Document, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Markdown => Ok(markdown::render(doc).into_bytes()),
        Format::Docx => docx::render(doc),
        Format::Odt => odt::render(doc),
    }
}

/// Escape the five characters that are markup in XML.
///
/// Shared by the `.docx` and `.odt` writers, which are both zipped XML. A
/// reader's note containing `<` or `&` would otherwise produce a file the
/// word processor refuses to open, and losing an export to an ampersand is
/// exactly the kind of failure that makes people stop trusting a tool.
pub(crate) fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Write a rendered document to disk.
pub fn write(doc: &Document, format: Format, path: &std::path::Path) -> Result<()> {
    let bytes = render(doc, format)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes).map_err(|e| {
        AppError::Invalid(format!("could not write {}: {e}", path.display()))
    })
}

#[cfg(test)]
pub(crate) fn sample() -> Document {
    Document {
        title: "Ethics".into(),
        author: Some("Spinoza".into()),
        exported: "19 August 2026".into(),
        sections: vec![Section {
            heading: "Terms".into(),
            blurb: Some("Words used in a special sense.".into()),
            entries: vec![Entry {
                title: Some("substance".into()),
                body: vec!["That which is in itself <and> conceived through itself.".into()],
                note: Some("working · 34 mentions".into()),
                bullets: vec!["earlier: a thing & its parts".into()],
            }],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip_through_their_names() {
        for (name, format) in [
            ("markdown", Format::Markdown),
            ("md", Format::Markdown),
            ("docx", Format::Docx),
            ("word", Format::Docx),
            ("odt", Format::Odt),
            ("libreoffice", Format::Odt),
        ] {
            assert_eq!(Format::from_str_opt(name), Some(format));
        }
        assert_eq!(Format::from_str_opt("pdf"), None);
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        // An ampersand in a note used to be enough to produce a file Word
        // refuses to open.
        assert_eq!(
            escape_xml("Tom & Jerry <b>\"x\"</b>"),
            "Tom &amp; Jerry &lt;b&gt;&quot;x&quot;&lt;/b&gt;"
        );
    }

    #[test]
    fn a_book_with_nothing_written_still_exports() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("Empty", None, "modern").unwrap();
        let doc = build(&db, book).unwrap();
        assert_eq!(doc.title, "Empty");
        assert_eq!(doc.sections.len(), 1);
        assert!(doc.sections[0].heading.contains("Nothing"));
    }

    #[test]
    fn terms_and_their_earlier_readings_are_carried_out() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("Ethics", Some("Spinoza"), "early_modern").unwrap();
        db.save_term(book, "substance", "a thing", crate::study::TermStatus::Unclear, None)
            .unwrap();
        db.save_term(
            book,
            "substance",
            "that which is in itself",
            crate::study::TermStatus::Working,
            None,
        )
        .unwrap();

        let doc = build(&db, book).unwrap();
        let terms = doc.sections.iter().find(|s| s.heading == "Terms").unwrap();
        assert_eq!(terms.entries[0].title.as_deref(), Some("substance"));
        assert_eq!(terms.entries[0].body[0], "that which is in itself");
        assert!(terms.entries[0].bullets[0].contains("a thing"));
    }

    #[test]
    fn every_format_produces_a_non_empty_file() {
        let doc = sample();
        for format in [Format::Markdown, Format::Docx, Format::Odt] {
            let bytes = render(&doc, format).unwrap();
            assert!(bytes.len() > 100, "{format:?} produced {} bytes", bytes.len());
        }
    }
}
