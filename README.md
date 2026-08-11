# Reading Companion

A desktop application for reading difficult books slowly and actually understanding them.

Photograph a page, or import an ebook, and work through it one paragraph at a time: read it, break it into sentences, compress it into a single sentence of your own, and check that sentence says who or what it is about. A local model then tells you what you missed — without telling you the answer. Words you don't know are looked up in a dictionary from **1913**, because in a book from 1830 the modern meaning is often the wrong one.

Everything runs on your machine. Nothing about what you read leaves it.

---

## Why it works this way

Three design decisions do most of the work, and each was forced by measurement rather than chosen up front.

### The coach never writes your summary

The point of the method is that *you* do the compression. A model that hands you a good sentence destroys the exercise.

The first version asked the model for prose feedback under a schema with no "answer" field, on the theory that the missing field made leaking impossible. Testing disproved it twice: the model stated the paragraph's point inside the prose fields anyway, and hardening the instructions made it *worse* — a 4B model began pattern-matching the examples and inverted the fields, reporting the paragraph's point as what the reader had "captured".

So the coach no longer writes prose. It returns **categorical judgements** — three booleans, an enum, a verdict — and the application composes the wording in Rust. Content the model never writes is content it cannot leak. One free-text field survives (a steering question), guarded by a check that rejects it if it quotes the paragraph back.

An exemplar exists, but it is a separate, deliberate request. It cannot arrive as a side effect of asking for feedback.

### The dictionary is from 1913

Webster's Revised Unabridged (1913) gives *nice* as "Foolish; silly; simple" and *awful* as "Oppressing with fear or horror". Those are the senses Austen and Gibbon were using. A modern dictionary answers those words confidently and wrongly, which is the worst possible failure for a tool whose purpose is comprehension.

Lookup is two layers. The first is instant, offline, and cannot invent: a word resolves against 98,828 headwords and 160,102 senses in SQLite, through a variant table and inflection rules, every candidate validated against the dictionary before use. The second — "what does it mean *here*?" — hands the sentence and the **already-retrieved senses** to the model, which selects and paraphrases rather than defining from memory.

Verified against shifted words: *nice* in "a woman of nice discrimination" resolves to "carefully discriminating"; *awful* in "the awful majesty of the mountains" to "inspiring awe"; *want* in "they that want honesty" to "lack".

### Structure is read, not guessed

Both import paths recover the book's own divisions rather than inventing them.

For photographs, headings are found by shape, then a chapter marker absorbs the title printed on the line below it — `CHAPTER I.` + `ON METHOD.` is one chapter, and reading them as two throws away the part you navigate by. Sections are treated as **spans**: `§ 1` runs until `§ 2` begins, which may be two pages later.

For EPUBs, the file's own table of contents is used, walked recursively — a real Bible listed 66 books as children of two testaments, and reading only the top level found three entries out of sixty-seven.

---

## What it does

**Import**

- **Photographs of pages.** Transcribed locally by a vision model. HEIC — what every iPhone produces — is converted to JPEG on import, detected by magic bytes rather than file extension.
- **A photograph of an open book** is detected and split at the spine into two pages.
- **A page held sideways** is turned upright automatically, by measuring which axis the lines of type repeat along.
- **EPUB** files, using the book's own contents for structure.
- **PDF** files with a text layer. A scanned PDF is detected and refused with an explanation rather than importing as blank pages.
- **Web articles**, fetched once and stored for offline reading.

**Read**

- Page text on the left, the method on the right, sentence highlighting in step between them.
- Page numbers read off the page, not assigned by import order — with a fallback that asks you when they can't be read.
- Paragraphs that run across a page break are **stitched**, so you are never asked to summarise half a thought.
- Navigation by chapter, with a filter, progress rings per page, and your place remembered between sessions.
- Any block can be corrected, reclassified, or deleted — transcription leaves artefacts no rule catches.
- Six themes, including sepia and a night mode that avoids pure black behind pale text.

---

## Getting started

### Prerequisites

| | |
|---|---|
| [Node.js](https://nodejs.org) | 20 or later |
| [Rust](https://rustup.rs) | stable, `x86_64-pc-windows-msvc` on Windows |
| [Ollama](https://ollama.com) | running locally |
| Build tools | Visual Studio Build Tools with the C++ workload and Windows SDK |

Pull the models:

```bash
ollama pull glm-ocr && ollama pull qwen3:4b && ollama pull qwen3-vl:4b
```

### Build the dictionary

This downloads Webster's 1913 and compiles it into a ~40MB SQLite database. It is not committed to the repository.

```bash
npm run dict:build
```

The app runs without it — hyphenation falls back to a heuristic and word lookup is unavailable — but you want it.

### Run

```bash
npm install
npm run tauri dev
```

### Install

```bash
npm run tauri build
```

Leaves an installer in `src-tauri/target/release/bundle/`. Your library lives in the platform application-data directory, outside the install, so upgrading never touches your work.

---

## The models

Three, chosen by testing rather than by benchmark ranking.

| role | model | why |
|---|---|---|
| Transcription | `glm-ocr` | 0.9B, 2.2GB. Reads a page in 2–6s once warm. |
| Coaching, dictionary | `qwen3:4b` | 2.6GB. Rubric and sense selection in 1.7–2.5s. |
| Page numbers | `qwen3-vl:4b` | Only when the cheaper passes fail — `glm-ocr` ignores questions and just transcribes. |

The first two stay resident together in about 4.8GB, so switching between transcribing and coaching never pays a model reload. The vision model has a short keep-alive because all three exceed 8GB of VRAM.

Small OCR models are **not well behaved by default**, and most of the guardrails exist because something failed:

- Uncapped, `glm-ocr` produced **59,282 characters over 124 seconds** for one page. `num_predict` bounds that to seconds.
- It transcribed a page and then began again from the top; "output the transcription once, then stop" fixed it.
- It apologised in prose when it ran out of tokens, and that apology became a paragraph the reader was asked to summarise. Model commentary is now stripped, by phrase and by position.

All three are configurable.

---

## Architecture

```
src/                    React 19 + TypeScript + Vite + Tailwind 4
  components/           Reader, method panel, navigator, wizard, dictionary popover
  lib/                  Typed IPC wrappers, theme

src-tauri/src/
  ocr/                  Photograph -> text -> paragraph blocks
    preprocess.rs         Orientation, HEIC decode, spread splitting
    orient.rs             Which way up a page is
    segment.rs            Paragraphs, hyphenation, long-s repair, commentary
    outline.rs            Parts, chapters, sections, topics
    page_number.rs        Reading the folio; reconciling facing pages
    legible.rs            Did that transcription come out as English?
  ingest/               EPUB, PDF, and web import (no OCR)
  dict/                 Two-layer dictionary lookup
  coach/                The rubric, and why it is a rubric
  db/                   SQLite schema and migrations
  ollama/               HTTP client and guardrails

scripts/                Dictionary build
```

Tauri v2 was chosen over Electron for a reason specific to this app: on an 8GB GPU, memory the shell doesn't take is memory the models get. ~40MB idle against ~350MB.

### Tests

```bash
cd src-tauri && cargo test
```

196 tests. Many are built from **verbatim output of real models on real photographs**, because the failures that mattered were not the ones that seemed likely in advance. Several encode a specific mistake so it cannot be repeated — a gutter-darkness threshold measured at 0.751, an orientation heuristic that got 3 of 6 real pages wrong and was deleted rather than tuned.

Diagnostic tools for calibrating against your own files:

```bash
cargo run --example check-gutter  -- <image>     # orientation and spread detection
cargo run --example check-epub    -- <file.epub> # what an EPUB extracts to
cargo run --example check-outline -- <library.sqlite>
```

---

## Privacy

No account, no telemetry, no cloud inference. Transcription, coaching, and the dictionary are local. The single exception is importing a web page, which fetches the URL you type — and is labelled as such in the interface.

Your photographs are kept alongside the text, because the OCR model silently modernises archaic spelling and ligatures: the image, not the transcription, is the record of what was actually printed.

---

## Licence and credits

- **Webster's Revised Unabridged Dictionary (1913)** — public domain, via [Project Gutenberg](https://www.gutenberg.org) and the [`ssvivian/WebstersDictionary`](https://github.com/ssvivian/WebstersDictionary) parse.
- Models are pulled from [Ollama](https://ollama.com) under their own licences.
- HEIC decoding by [`imazen/heic`](https://github.com/imazen/heic), pure Rust.

Book text you import is your own. This tool stores it locally and does not transmit it.
