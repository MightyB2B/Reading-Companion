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
  | "inflection";

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
}

export interface Lookup {
  word: string;
  lemma: string;
  resolution: Resolution;
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
