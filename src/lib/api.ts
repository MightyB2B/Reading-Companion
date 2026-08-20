import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Mirrors of the Rust types. Kept hand-written and small rather than
// generated: the IPC surface is narrow and the duplication is cheaper than a
// codegen step.

export type Era = "modern" | "early_20c" | "victorian" | "early_modern";

export const ERA_LABELS: Record<Era, string> = {
  modern: "Modern (1950-)",
  early_20c: "Early 20th century (1900-1950)",
  victorian: "Victorian (1800-1900)",
  early_modern: "Early modern (1500-1800)",
};

export interface Book {
  id: number;
  title: string;
  author: string | null;
  era: string;
  dict_corpus: string;
  /** The page you were last on. Null before the book has been opened. */
  last_page_id: number | null;
  created_at: string;
  updated_at: string;
}

export interface SentenceView {
  ordinal: number;
  text: string;
  /** The reader's own note on this sentence, if written. */
  note: string | null;
}

export interface BookStats {
  pages: number;
  paragraphs: number;
  /** Paragraphs you have summarised. */
  summaries: number;
  words_looked_up: number;
  /** Notes and marks together. */
  notes: number;
  terms: number;
  arguments: number;
}

/** A crop rectangle in fractions of the image, as drawn on the preview. */
export interface CropRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Page {
  id: number;
  book_id: number;
  /** What the model read off the page. Null until transcribed. */
  page_no: number | null;
  page_no_source: "detected" | "manual" | "unknown";
  image_orig: string;
  image_proc: string | null;
  ocr_status: "pending" | "running" | "done" | "failed";
  ocr_model: string | null;
  ocr_error: string | null;
}

export interface Block {
  id: number;
  page_id: number;
  ordinal: number;
  kind: string;
  text_raw: string;
  text_norm: string;
  user_edited: boolean;
}

export type BlockKind =
  | "paragraph"
  | "heading"
  | "quote"
  | "footnote"
  | "caption";

export type Verdict = "on_target" | "partial" | "off_target";

/** Composed in Rust from the model's rubric, never written by the model. */
export interface Feedback {
  verdict: Verdict;
  headline: string;
  notes: string[];
  problem: string | null;
  steering_question: string;
}

export interface CritiqueResult {
  summary_id: number;
  feedback: Feedback;
}

export interface Exemplar {
  sentence: string;
  reasoning: string;
}

export interface SpineEntry {
  page_no: number;
  ordinal: number;
  block_id: number;
  sentence: string;
}

export interface ImportResult {
  /** One id per page created — a photo of an open book yields two. */
  page_ids: number[];
  /** The page to open first. */
  page_id: number;
  /** Set when the file was converted on import, e.g. "HEIC" from an iPhone. */
  converted_from: string | null;
  /** True when the photo was recognised as an open book and split in two. */
  was_spread: boolean;
}

export interface DocumentImport {
  pages_added: number;
  blocks_added: number;
  first_page_id: number | null;
  /** Title the document declared, if any. */
  title: string | null;
  author: string | null;
  kind: "epub" | "pdf" | "web";
}

export type HeadingLevel =
  | "part"
  | "chapter"
  /** A numbered division — a span running until the next one begins. */
  | "section"
  /** A descriptive heading inside a section, not a replacement for it. */
  | "topic"
  | "minor";

export interface OutlineHeading {
  block_id: number;
  text: string;
  level: HeadingLevel;
}

export interface OutlinePage {
  page_id: number;
  page_no: number | null;
  ocr_status: "pending" | "running" | "done" | "failed";
  source_kind: "photo" | "epub" | "pdf" | "web";
  /** First line of prose, for recognising a page at a glance. */
  preview: string;
  headings: OutlineHeading[];
  paragraphs: number;
  summarised: number;
}

export interface ParagraphContext {
  /** The paragraph, stitched with its continuation on the adjacent page. */
  text: string;
  spans_pages: boolean;
  continued_from_page: number | null;
  continues_on_page: number | null;
  /** Runs on, but the page finishing it has not been imported. */
  incomplete: boolean;
}

export interface OllamaStatus {
  reachable: boolean;
  models: string[];
  ocr_model_ready: boolean;
  text_model_ready: boolean;
  message: string;
}

export type Resolution =
  | "direct"
  | "cross_reference"
  | "archaic_spelling"
  | "irregular"
  | "inflection"
  /** Reached by stripping a prefix — unending -> ending. */
  | "prefixed"
  /** Reached by swapping a derivational ending — omnipotence -> omnipotent. */
  | "derived";

export const RESOLUTION_NOTE: Partial<Record<Resolution, string>> = {
  cross_reference: "an older spelling of",
  archaic_spelling: "an older spelling of",
  irregular: "a form of",
  inflection: "a form of",
};

export interface Sense {
  gloss: string;
  /** e.g. "obsolete", "archaic" — already out of use by 1913. */
  labels: string[];
  pos: string | null;
  /** Which dictionary answered: "webster1913" or "wordnet". */
  source: string;
}

export const CORPUS_LABEL: Record<string, string> = {
  webster1913: "Webster's 1913",
  wordnet: "WordNet",
};

export interface Lookup {
  word: string;
  lemma: string;
  resolution: Resolution;
  /** Set when a prefix was stripped: showing `ending` for `unending` without
   *  saying so would tell the reader the opposite of what the word means. */
  prefix: string | null;
  senses: Sense[];
}

export interface ContextualSense {
  /** Index into `senses`, or -1 when none of them fit. */
  sense_index: number;
  plain_meaning: string;
  why_this_sense: string;
}

export interface ContextResult {
  lookup: Lookup;
  in_context: ContextualSense;
}

export interface VocabEntry {
  word: string;
  lemma: string | null;
  count: number;
  sentence: string | null;
  gloss: string | null;
}

export interface ModelInfo {
  name: string;
  size_bytes: number;
  /** e.g. "4.0B". Empty when the server does not report it. */
  parameter_size: string;
  /** e.g. "Q4_K_M". */
  quantization: string;
  /** e.g. ["vision", "completion", "tools", "thinking"]. */
  capabilities: string[];
}

/** Can this model be shown an image? */
export const seesImages = (m: ModelInfo) => m.capabilities.includes("vision");

export interface Settings {
  /** Where Ollama is — not necessarily this machine. */
  ollama_host: string;
  /** Bearer token for a hosted server. Empty for a local Ollama. */
  ollama_api_key: string;
  ocr_model: string;
  text_model: string;
  vision_model: string;
  /** How long the server holds a model — "30m", "2h", or "-1" for never. */
  keep_alive: string;
}

/**
 * Ollama accepts any duration string, but a typo silently becomes its own
 * 5-minute default, so these are offered as a list rather than typed.
 */
export const KEEP_ALIVE_CHOICES: { value: string; label: string; hint: string }[] = [
  { value: "5m", label: "5 minutes", hint: "Frees memory quickly. Expect reloads." },
  { value: "30m", label: "30 minutes", hint: "Covers a reading session on a shared machine." },
  { value: "2h", label: "2 hours", hint: "A long sitting without paying for a reload." },
  { value: "-1", label: "Never unload", hint: "For a dedicated server. Models stay in VRAM." },
];

// ---- the study layer -----------------------------------------------------

/** What a passage is *doing*. */
export type Move =
  | "thesis"
  | "premise"
  | "conclusion"
  | "definition"
  | "objection"
  | "reply"
  | "example"
  | "concession"
  | "aporia";

export interface MoveInfo {
  id: Move;
  label: string;
  /** What the move is. */
  what: string;
  /** How to recognise it in the text. */
  tell: string;
}

/** A block in book order, carrying the page it came from. */
export interface FlowBlock {
  id: number;
  page_id: number;
  page_no: number | null;
  ordinal: number;
  kind: string;
  text_norm: string;
  user_edited: boolean;
  /** First block on its page — the marker goes above it. */
  starts_page: boolean;
  /** The page has not been transcribed, so the text stops here. */
  pending: boolean;
}

export interface PageGap {
  page_id: number;
  page_no: number | null;
  status: string;
}

/**
 * A note, a mark, or both. Body and no move is a note; move and no body is a
 * mark. The body never renders in the reading column — only a gutter dot.
 */
export interface Note {
  id: number;
  book_id: number;
  notebook_id: number | null;
  block_id: number | null;
  char_start: number | null;
  char_end: number | null;
  anchor_text: string;
  move: Move | null;
  body: string;
  /** Its anchor could not be found after a re-import. Kept, not deleted. */
  orphaned: boolean;
  created_at: string;
  updated_at: string;
}

/** A place to attach a note. */
export interface NewAnchor {
  block_id: number;
  char_start: number;
  char_end: number;
  anchor_text: string;
}

export interface StoredAnchor {
  id: number;
  block_id: number | null;
  char_start: number | null;
  char_end: number | null;
  anchor_text: string;
  orphaned: boolean;
  page_no: number | null;
}

export interface Notebook {
  id: number;
  book_id: number | null;
  name: string;
  kind: string;
  notes: number;
}

export type TermStatus = "unclear" | "working" | "settled";

export const TERM_STATUS_LABEL: Record<TermStatus, string> = {
  unclear: "Still unclear",
  working: "Working on it",
  settled: "Settled",
};

export interface TermRevision {
  my_gloss: string;
  status: TermStatus;
  created_at: string;
}

export interface Term {
  id: number;
  book_id: number;
  term: string;
  my_gloss: string;
  status: TermStatus;
  first_block_id: number | null;
  mentions: number;
  /** Superseded glosses, newest first. Empty until the sense moves. */
  revisions: TermRevision[];
  created_at: string;
  updated_at: string;
}

export interface TermMention {
  block_id: number;
  char_start: number;
  char_end: number;
  term_id: number;
  term: string;
  my_gloss: string;
  status: string;
}

export interface StoredRef {
  id: number;
  book_id: number;
  block_id: number;
  char_start: number;
  char_end: number;
  surface: string;
  osis_book: string;
  chapter: number;
  verse_start: number | null;
  verse_end: number | null;
  confidence: number;
  corrected: boolean;
  /** "Romans 3:23". */
  label: string;
}

/** Somewhere in the library that cites a passage. */
export interface Citation {
  book_id: number;
  book_title: string;
  block_id: number;
  page_no: number | null;
  surface: string;
  label: string;
  context: string;
}

export interface Annotations {
  notes: Note[];
  terms: TermMention[];
  refs: StoredRef[];
}

export interface Premise {
  id: number;
  ordinal: number;
  text: string;
  /** You supplied it, not the author. */
  implicit: boolean;
  source_block_id: number | null;
  char_start: number | null;
  char_end: number | null;
}

export interface NewPremise {
  text: string;
  implicit: boolean;
  source_block_id: number | null;
  char_start: number | null;
  char_end: number | null;
}

export interface ArgumentLink {
  id: number;
  child_id: number;
  child_label: string;
  child_conclusion: string;
  role: string;
}

export const VERDICT_LABEL: Record<string, string> = {
  undecided: "Not yet judged",
  sound: "Sound",
  valid_unsound: "Valid, but a premise is false",
  invalid: "Does not follow",
};

export const GAP_LABEL: Record<string, string> = {
  none: "No gap",
  missing_premise: "Something unstated is needed",
  equivocation: "A word shifts meaning",
  irrelevance: "A premise does not bear on it",
  circularity: "It assumes what it proves",
};

export interface Argument {
  id: number;
  book_id: number;
  label: string;
  conclusion: string;
  verdict: string;
  gap: string;
  anchor_block_id: number | null;
  notes: string;
  premises: Premise[];
  links: ArgumentLink[];
  created_at: string;
  updated_at: string;
}

/** A labelled sentence from the Suggest pass. Text comes from the passage. */
export interface SentenceRole {
  ordinal: number;
  role: Move;
  text: string;
}

export interface CandidateTerm {
  surface_form: string;
  stipulated: boolean;
}

export interface SupportCheck {
  reaches_conclusion: boolean;
  gap: string;
  steering_question: string;
}

export interface CharityCheck {
  /** One-based; 0 when none stands out. */
  weakest_premise: number;
  stronger_available: boolean;
  steering_question: string;
}

export interface Interlocutor {
  addressee_quoted: string;
  position_quoted: string;
}

export type ExportFormat = "markdown" | "docx" | "odt";

export const EXPORT_FORMATS: { value: ExportFormat; label: string; ext: string }[] = [
  { value: "docx", label: "Word document", ext: "docx" },
  { value: "odt", label: "LibreOffice document", ext: "odt" },
  { value: "markdown", label: "Markdown", ext: "md" },
];

export type HitKind = "text" | "note" | "term";

export interface SearchHit {
  kind: HitKind;
  book_id: number;
  book_title: string;
  block_id: number | null;
  page_no: number | null;
  /** The match, with the hit wrapped in guillemets for the UI to mark up. */
  snippet: string;
  rank: number;
}

export interface CanonBook {
  osis: string;
  name: string;
}

export const api = {
  checkOllama: () => invoke<OllamaStatus>("check_ollama"),

  getSettings: () => invoke<Settings>("get_settings"),
  defaultSettings: () => invoke<Settings>("default_settings"),
  /** Saves and returns the cleaned-up values actually stored. */
  saveSettings: (settings: Settings) =>
    invoke<Settings>("save_settings", { settings }),
  /** Try a server without committing to it. */
  testOllamaHost: (host: string, apiKey?: string) =>
    invoke<OllamaStatus>("test_ollama_host", { host, apiKey }),

  /** Models installed on a server — the typed one, or the configured one. */
  listModels: (host?: string, apiKey?: string) =>
    invoke<ModelInfo[]>("list_models", { host, apiKey }),

  listBooks: () => invoke<Book[]>("list_books"),
  createBook: (title: string, author: string | null, era: Era) =>
    invoke<number>("create_book", { title, author, era }),

  /** What a book contains, so deletion can say what it costs. */
  bookStats: (bookId: number) => invoke<BookStats>("book_stats", { bookId }),

  deleteBook: (bookId: number) => invoke<void>("delete_book", { bookId }),

  listPages: (bookId: number) => invoke<Page[]>("list_pages", { bookId }),

  /** Remember where you stopped, so closing the app keeps your place. */
  setLastPage: (bookId: number, pageId: number | null) =>
    invoke<void>("set_last_page", { bookId, pageId }),

  /** Which page to open a book at — where you stopped, or its first page. */
  resumePage: (bookId: number) => invoke<number | null>("resume_page", { bookId }),
  importPage: (bookId: number, sourcePath: string) =>
    invoke<ImportResult>("import_page", { bookId, sourcePath }),
  runOcr: (pageId: number, bookId: number) =>
    invoke<Block[]>("run_ocr", { pageId, bookId }),

  /** Crop a page's photograph to a region and re-read it. */
  recropPage: (pageId: number, rect: CropRect) =>
    invoke<Block[]>("recrop_page", { pageId, rect }),

  listBlocks: (pageId: number) => invoke<Block[]>("list_blocks", { pageId }),
  editBlock: (blockId: number, text: string) =>
    invoke<void>("edit_block", { blockId, text }),

  /** Remove a transcription artefact. */
  deleteBlock: (blockId: number) => invoke<void>("delete_block", { blockId }),

  /** Reclassify a block — prose, heading, quotation. */
  setBlockKind: (blockId: number, kind: BlockKind) =>
    invoke<void>("set_block_kind", { blockId, kind }),

  critiqueSummary: (blockId: number, sentence: string, selfChecked: boolean) =>
    invoke<CritiqueResult>("critique_summary", {
      blockId,
      sentence,
      selfChecked,
    }),
  getExemplar: (blockId: number) => invoke<Exemplar>("get_exemplar", { blockId }),

  summarySpine: (bookId: number) => invoke<SpineEntry[]>("summary_spine", { bookId }),

  /** The one-sentence rule, checked by the same code the backend uses. */
  isMultipleSentences: (text: string) =>
    invoke<boolean>("is_multiple_sentences", { text }),

  /** Layer one: instant, offline. Null when the word cannot be resolved. */
  lookUpWord: (word: string) => invoke<Lookup | null>("look_up_word", { word }),

  /** Layer two: which sense applies in this sentence. */
  wordInContext: (word: string, sentence: string, blockId: number | null) =>
    invoke<ContextResult>("word_in_context", { word, sentence, blockId }),

  vocabulary: (bookId: number) => invoke<VocabEntry[]>("vocabulary", { bookId }),

  /** The sentences of a paragraph, with any notes already written. */
  blockSentences: (blockId: number) =>
    invoke<SentenceView[]>("block_sentences", { blockId }),

  saveSentenceNote: (blockId: number, sentenceOrdinal: number, text: string) =>
    invoke<number>("save_sentence_note", { blockId, sentenceOrdinal, text }),

  setPageNumber: (pageId: number, pageNo: number | null) =>
    invoke<void>("set_page_number", { pageId, pageNo }),

  deletePage: (pageId: number) => invoke<void>("delete_page", { pageId }),

  /** A paragraph plus whatever continues it on the neighbouring page. */
  paragraphContext: (blockId: number) =>
    invoke<ParagraphContext>("paragraph_context", { blockId }),

  /** The book's structure, built from its headings. */
  bookOutline: (bookId: number) => invoke<OutlinePage[]>("book_outline", { bookId }),

  // ---- the study layer ---------------------------------------------------

  /** Every move, with what it is and the tell that gives it away. */
  moveCatalogue: () => invoke<MoveInfo[]>("move_catalogue"),

  /** Every block of a book in reading order, for the scrolling column. */
  bookBlocks: (bookId: number) => invoke<FlowBlock[]>("book_blocks", { bookId }),

  /** Pages that exist but hold no text yet. */
  pageGaps: (bookId: number) => invoke<PageGap[]>("page_gaps", { bookId }),

  /** Notes, term occurrences and citations for a window of blocks, in one call. */
  blockAnnotations: (blockIds: number[]) =>
    invoke<Annotations>("block_annotations", { blockIds }),

  addNote: (n: {
    bookId: number;
    blockId: number | null;
    charStart: number | null;
    charEnd: number | null;
    anchorText: string;
    move: Move | null;
    body: string;
    notebookId?: number | null;
    /** The rest of a selection that spanned more than one paragraph. */
    extraAnchors?: NewAnchor[];
  }) =>
    invoke<number>("add_note", {
      bookId: n.bookId,
      blockId: n.blockId,
      charStart: n.charStart,
      charEnd: n.charEnd,
      anchorText: n.anchorText,
      move: n.move,
      body: n.body,
      notebookId: n.notebookId ?? null,
      extraAnchors: n.extraAnchors ?? [],
    }),

  /** Attach the current selection to a note that already exists. */
  attachAnchors: (noteId: number, anchors: NewAnchor[]) =>
    invoke<number>("attach_anchors", { noteId, anchors }),
  noteAnchors: (noteId: number) =>
    invoke<StoredAnchor[]>("note_anchors", { noteId }),
  deleteNoteAnchor: (anchorId: number) =>
    invoke<void>("delete_note_anchor", { anchorId }),

  updateNote: (noteId: number, body: string, move: Move | null) =>
    invoke<void>("update_note", { noteId, body, move }),
  deleteNote: (noteId: number) => invoke<void>("delete_note", { noteId }),
  listNotes: (bookId: number, notebookId?: number | null) =>
    invoke<Note[]>("list_notes", { bookId, notebookId: notebookId ?? null }),
  listNotebooks: (bookId: number) => invoke<Notebook[]>("list_notebooks", { bookId }),
  createNotebook: (bookId: number | null, name: string) =>
    invoke<number>("create_notebook", { bookId, name }),

  listTerms: (bookId: number) => invoke<Term[]>("list_terms", { bookId }),
  saveTerm: (
    bookId: number,
    term: string,
    gloss: string,
    status: TermStatus,
    firstBlockId: number | null,
  ) => invoke<number>("save_term", { bookId, term, gloss, status, firstBlockId }),
  deleteTerm: (termId: number) => invoke<void>("delete_term", { termId }),

  /** Words the author may be using specially. Never a definition. */
  suggestTerms: (passage: string) =>
    invoke<CandidateTerm[]>("suggest_terms", { passage }),

  /** Label the moves in a passage. Returns ordinals; text comes from the passage. */
  suggestMoves: (passage: string) =>
    invoke<SentenceRole[]>("suggest_moves", { passage }),

  /** Who the passage is answering, in its own words. */
  findInterlocutor: (passage: string) =>
    invoke<Interlocutor>("find_interlocutor", { passage }),

  listArguments: (bookId: number) => invoke<Argument[]>("list_arguments", { bookId }),
  createArgument: (bookId: number, label: string, anchorBlockId: number | null) =>
    invoke<number>("create_argument", { bookId, label, anchorBlockId }),
  saveArgument: (a: {
    argumentId: number;
    label: string;
    conclusion: string;
    verdict: string;
    gap: string;
    notes: string;
    premises: NewPremise[];
  }) => invoke<void>("save_argument", a),
  deleteArgument: (argumentId: number) => invoke<void>("delete_argument", { argumentId }),
  linkArguments: (parentId: number, childId: number, role: string) =>
    invoke<void>("link_arguments", { parentId, childId, role }),
  unlinkArguments: (linkId: number) => invoke<void>("unlink_arguments", { linkId }),

  /** Do the premises reach the conclusion? Names the gap; never fills it. */
  checkSupport: (conclusion: string, premises: string[]) =>
    invoke<SupportCheck>("check_support", { conclusion, premises }),
  /** Which premise is weakest, and whether a better one exists. */
  checkCharity: (conclusion: string, premises: string[]) =>
    invoke<CharityCheck>("check_charity", { conclusion, premises }),

  setThesis: (bookId: number, statement: string) =>
    invoke<void>("set_thesis", { bookId, statement }),
  getThesis: (bookId: number) => invoke<string | null>("get_thesis", { bookId }),

  /** Find every scripture citation in a book. Needs no Bible. */
  scanReferences: (bookId: number) => invoke<number>("scan_references", { bookId }),

  /** Everything in the library that cites a passage — including your own books. */
  citationsOf: (osisBook: string, chapter: number, verse: number | null) =>
    invoke<Citation[]>("citations_of", { osisBook, chapter, verse }),

  /** Where a passage lives in one book — what linked panes follow. */
  locateReference: (
    bookId: number,
    osisBook: string,
    chapter: number,
    verse: number | null,
  ) => invoke<number | null>("locate_reference", { bookId, osisBook, chapter, verse }),

  /** The verse itself, when a Bible has been imported. */
  verseText: (osisBook: string, chapter: number, verse: number) =>
    invoke<string | null>("verse_text", { osisBook, chapter, verse }),

  correctReference: (
    refId: number,
    osisBook: string,
    chapter: number,
    verseStart: number | null,
    verseEnd: number | null,
  ) => invoke<void>("correct_reference", { refId, osisBook, chapter, verseStart, verseEnd }),
  deleteReference: (refId: number) => invoke<void>("delete_reference", { refId }),

  canon: () => invoke<CanonBook[]>("canon"),

  /** Write everything you have written about a book to a file. */
  exportBook: (bookId: number, format: ExportFormat, dest: string) =>
    invoke<string>("export_book", { bookId, format, dest }),

  /** Search the text, the notes, and the terms together. */
  searchLibrary: (query: string, bookId: number | null) =>
    invoke<SearchHit[]>("search_library", { query, bookId }),

  /** Every place in the library a word occurs. */
  concordance: (word: string, limit: number) =>
    invoke<SearchHit[]>("concordance", { word, limit }),

  /** Import an EPUB, PDF, or web page — text sources, so no OCR. */
  importDocument: (bookId: number, source: string) =>
    invoke<DocumentImport>("import_document", { bookId, source }),
};

/** The sentence containing `word` within `text`, for the in-context lookup. */
export function sentenceAround(text: string, word: string): string {
  const idx = text.toLowerCase().indexOf(word.toLowerCase());
  if (idx < 0) return text;

  // Walk out to sentence boundaries. Abbreviations are not worth handling
  // here: an over-long sentence costs the model a few tokens, nothing more.
  let start = 0;
  for (let i = idx; i > 0; i--) {
    if (/[.!?]/.test(text[i - 1]) && /\s/.test(text[i] ?? "")) {
      start = i;
      break;
    }
  }
  let end = text.length;
  for (let i = idx + word.length; i < text.length; i++) {
    if (/[.!?]/.test(text[i])) {
      end = i + 1;
      break;
    }
  }
  return text.slice(start, end).trim();
}

export interface OcrProgress {
  page_id: number;
  chunk: string;
}

/** Streams transcription text as it arrives, so import shows progress. */
export function onOcrProgress(handler: (p: OcrProgress) => void) {
  return listen<OcrProgress>("ocr-progress", (e) => handler(e.payload));
}

export interface JobProgress {
  /** Machine-readable stage, for choosing an icon. */
  stage: string;
  /** What to show the reader. */
  detail: string;
  step: number;
  total: number;
}

/** Stage-by-stage progress for long jobs like importing a page. */
export function onJobProgress(handler: (p: JobProgress) => void) {
  return listen<JobProgress>("job-progress", (e) => handler(e.payload));
}
