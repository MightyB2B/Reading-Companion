//! The IPC surface for the study layer.
//!
//! Thin by design: these mostly hand arguments to `Db` and back. The
//! interesting decisions live in `db/study.rs`, `analyst`, and `scripture`,
//! where they can be tested without a Tauri runtime.

use tauri::State;

use crate::analyst;
use crate::commands::AppState;
use crate::db::reading::FlowBlock;
use crate::db::scripture::{Citation, StoredRef};
use crate::db::search::SearchHit;
use crate::db::study::{NewAnchor, NewPremise, StoredAnchor, TermMention};
use crate::error::{AppError, Result};
use crate::scripture;
use crate::study::{Argument, Move, MoveInfo, Note, Notebook, Term, TermStatus};

fn parse_move(s: Option<&str>) -> Result<Option<Move>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) => Move::from_str_opt(v)
            .map(Some)
            .ok_or_else(|| AppError::Invalid(format!("{v:?} is not a move"))),
    }
}

// ---- the vocabulary ------------------------------------------------------

/// Every move, with what it is and how to spot it.
///
/// Shipped to the frontend so the vocabulary is defined where the reader
/// meets it. A palette of nine unexplained words is not a tool.
#[tauri::command]
pub fn move_catalogue() -> Vec<MoveInfo> {
    Move::catalogue()
}

// ---- continuous reading --------------------------------------------------

/// Every block of a book in reading order, for the scrolling column.
#[tauri::command]
pub fn book_blocks(state: State<'_, AppState>, book_id: i64) -> Result<Vec<FlowBlock>> {
    state.db.book_blocks(book_id)
}

/// Pages that exist but hold no text yet, so the column can say where the
/// book stops rather than appearing to end mid-sentence.
#[tauri::command]
pub fn page_gaps(state: State<'_, AppState>, book_id: i64) -> Result<Vec<PageGap>> {
    Ok(state
        .db
        .empty_pages(book_id)?
        .into_iter()
        .map(|(page_id, page_no, status)| PageGap {
            page_id,
            page_no,
            status,
        })
        .collect())
}

#[derive(serde::Serialize)]
pub struct PageGap {
    pub page_id: i64,
    pub page_no: Option<i64>,
    pub status: String,
}

/// Everything the study layer knows about a window of blocks, in one call.
///
/// The reading column renders a window and needs notes, marks, term
/// occurrences and citations for all of it. Four round trips per scroll would
/// be four chances to tear.
#[tauri::command]
pub fn block_annotations(
    state: State<'_, AppState>,
    block_ids: Vec<i64>,
) -> Result<Annotations> {
    Ok(Annotations {
        notes: state.db.notes_for_blocks(&block_ids)?,
        terms: state.db.term_mentions_for_blocks(&block_ids)?,
        refs: state.db.refs_for_blocks(&block_ids)?,
    })
}

#[derive(serde::Serialize)]
pub struct Annotations {
    pub notes: Vec<Note>,
    pub terms: Vec<TermMention>,
    pub refs: Vec<StoredRef>,
}

// ---- notes and marks -----------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_note(
    state: State<'_, AppState>,
    book_id: i64,
    block_id: Option<i64>,
    char_start: Option<i64>,
    char_end: Option<i64>,
    anchor_text: String,
    r#move: Option<String>,
    body: String,
    notebook_id: Option<i64>,
    // The rest of a selection that spanned more than one paragraph.
    extra_anchors: Option<Vec<NewAnchor>>,
) -> Result<i64> {
    let span = match (char_start, char_end) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let id = state.db.add_note(
        book_id,
        block_id,
        span,
        &anchor_text,
        parse_move(r#move.as_deref())?,
        &body,
        notebook_id,
    )?;
    if let Some(extra) = extra_anchors {
        state.db.add_note_anchors(id, &extra)?;
    }
    Ok(id)
}

/// Attach the current selection to a note that already exists.
#[tauri::command]
pub fn attach_anchors(
    state: State<'_, AppState>,
    note_id: i64,
    anchors: Vec<NewAnchor>,
) -> Result<usize> {
    state.db.add_note_anchors(note_id, &anchors)
}

/// Every place a note is attached.
#[tauri::command]
pub fn note_anchors(state: State<'_, AppState>, note_id: i64) -> Result<Vec<StoredAnchor>> {
    state.db.note_anchors(note_id)
}

#[tauri::command]
pub fn delete_note_anchor(state: State<'_, AppState>, anchor_id: i64) -> Result<()> {
    state.db.delete_note_anchor(anchor_id)
}

#[tauri::command]
pub fn update_note(
    state: State<'_, AppState>,
    note_id: i64,
    body: String,
    r#move: Option<String>,
) -> Result<()> {
    state
        .db
        .update_note(note_id, &body, parse_move(r#move.as_deref())?)
}

#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, note_id: i64) -> Result<()> {
    state.db.delete_note(note_id)
}

#[tauri::command]
pub fn list_notes(
    state: State<'_, AppState>,
    book_id: i64,
    notebook_id: Option<i64>,
) -> Result<Vec<Note>> {
    state.db.list_notes(book_id, notebook_id)
}

#[tauri::command]
pub fn list_notebooks(state: State<'_, AppState>, book_id: i64) -> Result<Vec<Notebook>> {
    state.db.list_notebooks(book_id)
}

#[tauri::command]
pub fn create_notebook(
    state: State<'_, AppState>,
    book_id: Option<i64>,
    name: String,
) -> Result<i64> {
    state.db.create_notebook(book_id, &name, "manual")
}

// ---- terms ---------------------------------------------------------------

#[tauri::command]
pub fn list_terms(state: State<'_, AppState>, book_id: i64) -> Result<Vec<Term>> {
    state.db.list_terms(book_id)
}

/// Save a term and index every place the book uses it.
///
/// Indexing on save rather than lazily: the underline has to appear
/// everywhere the moment the term exists, and a book is a few thousand
/// paragraphs, which is milliseconds.
#[tauri::command]
pub fn save_term(
    state: State<'_, AppState>,
    book_id: i64,
    term: String,
    gloss: String,
    status: String,
    first_block_id: Option<i64>,
) -> Result<i64> {
    let id = state.db.save_term(
        book_id,
        &term,
        &gloss,
        TermStatus::from_str_lossy(&status),
        first_block_id,
    )?;
    state.db.index_term_mentions(id, book_id, &term)?;
    Ok(id)
}

#[tauri::command]
pub fn delete_term(state: State<'_, AppState>, term_id: i64) -> Result<()> {
    state.db.delete_term(term_id)
}

/// Words this author may be using in a special sense.
///
/// Returns surface forms copied from the passage and verified to be in it.
/// Never a definition — the reader writes that.
#[tauri::command]
pub async fn suggest_terms(
    state: State<'_, AppState>,
    passage: String,
) -> Result<Vec<analyst::CandidateTerm>> {
    let settings = state.settings();
    let client = state.ollama();
    analyst::candidate_terms(&client, &settings.text_model, &passage).await
}

// ---- the suggest pass ----------------------------------------------------

/// Label the moves in a passage.
///
/// The model sees numbered sentences and returns numbers, so nothing it says
/// can become text on the page. The sentence text in the result is filled in
/// from the passage itself.
#[tauri::command]
pub async fn suggest_moves(
    state: State<'_, AppState>,
    passage: String,
) -> Result<Vec<analyst::SentenceRole>> {
    let settings = state.settings();
    let client = state.ollama();
    analyst::sentence_roles(&client, &settings.text_model, &passage).await
}

/// Who the passage is answering, in its own words.
#[tauri::command]
pub async fn find_interlocutor(
    state: State<'_, AppState>,
    passage: String,
) -> Result<analyst::Interlocutor> {
    let settings = state.settings();
    let client = state.ollama();
    analyst::interlocutor(&client, &settings.text_model, &passage).await
}

// ---- arguments -----------------------------------------------------------

#[tauri::command]
pub fn list_arguments(state: State<'_, AppState>, book_id: i64) -> Result<Vec<Argument>> {
    state.db.list_arguments(book_id)
}

#[tauri::command]
pub fn create_argument(
    state: State<'_, AppState>,
    book_id: i64,
    label: String,
    anchor_block_id: Option<i64>,
) -> Result<i64> {
    state.db.create_argument(book_id, &label, anchor_block_id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn save_argument(
    state: State<'_, AppState>,
    argument_id: i64,
    label: String,
    conclusion: String,
    verdict: String,
    gap: String,
    notes: String,
    premises: Vec<NewPremise>,
) -> Result<()> {
    state
        .db
        .save_argument(argument_id, &label, &conclusion, &verdict, &gap, &notes)?;
    state.db.set_premises(argument_id, &premises)
}

#[tauri::command]
pub fn delete_argument(state: State<'_, AppState>, argument_id: i64) -> Result<()> {
    state.db.delete_argument(argument_id)
}

#[tauri::command]
pub fn link_arguments(
    state: State<'_, AppState>,
    parent_id: i64,
    child_id: i64,
    role: String,
) -> Result<()> {
    state.db.link_arguments(parent_id, child_id, &role)
}

#[tauri::command]
pub fn unlink_arguments(state: State<'_, AppState>, link_id: i64) -> Result<()> {
    state.db.unlink_arguments(link_id)
}

/// Do the premises reach the conclusion? Names the gap; never fills it.
#[tauri::command]
pub async fn check_support(
    state: State<'_, AppState>,
    conclusion: String,
    premises: Vec<String>,
) -> Result<analyst::SupportCheck> {
    if premises.is_empty() {
        return Err(AppError::Invalid(
            "list at least one premise before checking it".into(),
        ));
    }
    let settings = state.settings();
    let client = state.ollama();
    analyst::support_check(&client, &settings.text_model, &conclusion, &premises).await
}

/// Which premise is weakest, and whether a better one exists.
#[tauri::command]
pub async fn check_charity(
    state: State<'_, AppState>,
    conclusion: String,
    premises: Vec<String>,
) -> Result<analyst::CharityCheck> {
    if premises.is_empty() {
        return Err(AppError::Invalid(
            "list at least one premise before checking it".into(),
        ));
    }
    let settings = state.settings();
    let client = state.ollama();
    analyst::charity_check(&client, &settings.text_model, &conclusion, &premises).await
}

// ---- thesis --------------------------------------------------------------

#[tauri::command]
pub fn set_thesis(state: State<'_, AppState>, book_id: i64, statement: String) -> Result<()> {
    state.db.set_thesis(book_id, &statement)
}

#[tauri::command]
pub fn get_thesis(state: State<'_, AppState>, book_id: i64) -> Result<Option<String>> {
    state.db.get_thesis(book_id)
}

// ---- export --------------------------------------------------------------

/// Write everything you have written about a book to a file.
///
/// A study tool that can only be read inside itself is a place work goes to be
/// forgotten, so this covers Markdown, Word, and LibreOffice.
#[tauri::command]
pub fn export_book(
    state: State<'_, AppState>,
    book_id: i64,
    format: String,
    dest: String,
) -> Result<String> {
    let format = crate::export::Format::from_str_opt(&format)
        .ok_or_else(|| AppError::Invalid(format!("{format:?} is not a format")))?;

    let doc = crate::export::build(&state.db, book_id)?;
    let mut path = std::path::PathBuf::from(&dest);
    // A picker that returns no extension, or the wrong one, would produce a
    // file the operating system opens with the wrong program.
    if path.extension().and_then(|e| e.to_str()) != Some(format.extension()) {
        path.set_extension(format.extension());
    }
    crate::export::write(&doc, format, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

// ---- search --------------------------------------------------------------

/// Search the text, the notes, and the terms together.
///
/// One box rather than three: asking which of them to look in is the
/// application's structure leaking into the reading.
#[tauri::command]
pub fn search_library(
    state: State<'_, AppState>,
    query: String,
    book_id: Option<i64>,
) -> Result<Vec<SearchHit>> {
    state.db.search_library(&query, book_id)
}

/// Every place in the library a word occurs — a concordance over the reader's
/// own books, which is the thing a bought Bible program cannot do.
#[tauri::command]
pub fn concordance(
    state: State<'_, AppState>,
    word: String,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    state.db.concordance(&word, limit.clamp(1, 200))
}

// ---- scripture -----------------------------------------------------------

/// Find every scripture citation in a book and record it.
///
/// Worth running on a book of theology the moment it is imported: it is what
/// turns `Rom. iii. 23` from ink into a link, and it needs no Bible.
#[tauri::command]
pub fn scan_references(state: State<'_, AppState>, book_id: i64) -> Result<usize> {
    state.db.scan_book_references(book_id)
}

/// Everything in the library that cites a passage — including the reader's
/// own books, which is the part no Bible software can do.
#[tauri::command]
pub fn citations_of(
    state: State<'_, AppState>,
    osis_book: String,
    chapter: i64,
    verse: Option<i64>,
) -> Result<Vec<Citation>> {
    state.db.citations_of(&osis_book, chapter, verse)
}

/// Where a passage lives inside one particular book.
///
/// The query behind linked panes: given a reference the reader just touched,
/// find the place in the *other* book to move to.
#[tauri::command]
pub fn locate_reference(
    state: State<'_, AppState>,
    book_id: i64,
    osis_book: String,
    chapter: i64,
    verse: Option<i64>,
) -> Result<Option<i64>> {
    state.db.locate_reference(book_id, &osis_book, chapter, verse)
}

/// The text of a verse, when a Bible has been imported. None otherwise, which
/// is the normal state for a library of theology.
#[tauri::command]
pub fn verse_text(
    state: State<'_, AppState>,
    osis_book: String,
    chapter: i64,
    verse: i64,
) -> Result<Option<String>> {
    state.db.verse_text(&osis_book, chapter, verse)
}

#[tauri::command]
pub fn correct_reference(
    state: State<'_, AppState>,
    ref_id: i64,
    osis_book: String,
    chapter: i64,
    verse_start: Option<i64>,
    verse_end: Option<i64>,
) -> Result<()> {
    if scripture::canonical_name(&osis_book) == osis_book
        && !scripture::BOOKS.iter().any(|b| b.osis == osis_book)
    {
        // canonical_name echoes its input when the id is unknown, so an
        // unrecognised book would be stored and then displayed as gibberish.
        return Err(AppError::Invalid(format!("{osis_book} is not a book")));
    }
    state
        .db
        .correct_ref(ref_id, &osis_book, chapter, verse_start, verse_end)
}

#[tauri::command]
pub fn delete_reference(state: State<'_, AppState>, ref_id: i64) -> Result<()> {
    state.db.delete_ref(ref_id)
}

/// The books of the canon, for the correction dropdown.
#[tauri::command]
pub fn canon() -> Vec<CanonBook> {
    scripture::BOOKS
        .iter()
        .flat_map(|d| match d.ordinal {
            scripture::books::Ordinal::Always => (1..=2)
                .map(|n| CanonBook {
                    osis: format!("{n}{}", d.osis),
                    name: format!("{n} {}", d.name),
                })
                .collect::<Vec<_>>(),
            scripture::books::Ordinal::Optional => {
                let mut v = vec![CanonBook {
                    osis: d.osis.to_string(),
                    name: d.name.to_string(),
                }];
                v.extend((1..=3).map(|n| CanonBook {
                    osis: format!("{n}{}", d.osis),
                    name: format!("{n} {}", d.name),
                }));
                v
            }
            scripture::books::Ordinal::Never => vec![CanonBook {
                osis: d.osis.to_string(),
                name: d.name.to_string(),
            }],
        })
        .collect()
}

#[derive(serde::Serialize)]
pub struct CanonBook {
    pub osis: String,
    pub name: String,
}
