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

| | | verify with |
|---|---|---|
| [Node.js](https://nodejs.org) | 20 or later | `node --version` |
| [Rust](https://rustup.rs) | stable toolchain | `rustc --version` |
| [Ollama](https://ollama.com) | running locally | `ollama --version` |
| C++ build tools | see below | `rustc --print target-list` succeeds |

**On Windows**, Rust needs the MSVC toolchain: install **Visual Studio Build Tools** with the *Desktop development with C++* workload and a Windows SDK. `rustup` picks it up automatically. WebView2 already ships with Windows 11.

**On macOS**, `xcode-select --install`. **On Linux**, the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) list the WebKitGTK packages.

### First-time setup

```bash
git clone https://github.com/MightyB2B/Reading-Companion.git
cd Reading-Companion
npm install
```

Pull the three models (about 8GB total, once):

```bash
ollama pull glm-ocr
ollama pull qwen3:4b
ollama pull qwen3-vl:4b
```

Build the dictionary. This downloads Webster's 1913 and compiles it into a ~40MB SQLite database:

```bash
npm run dict:build
```

It prints its own sanity checks and fails loudly if any of them break:

```
· 98,828 headwords, 160,102 senses
· variants: 4308 cross-references, 46 archaic spellings, 99 irregular forms
✓ src-tauri/resources/dict.sqlite (39.9 MB)
  ✓ nice      1 entry   (obsolete first sense — the period-correct payoff)
  ✓ publick   -> public (curated archaic spelling)
```

The database is **not** committed — it is regenerable, and 40MB of derived data does not belong in a repository. The app runs without it: hyphenation falls back to a case heuristic and word lookup is unavailable, but nothing breaks.

### Running it

```bash
npm run tauri dev
```

The first launch compiles the Rust side and takes a few minutes — `rusqlite` builds SQLite from source. Later launches take seconds. Keep the terminal open; closing it closes the app.

Ollama must be running. If it isn't, the app opens and says so rather than failing obscurely.

### Building an installer

```bash
npm run tauri build
```

A full release build, several minutes. The installer lands in:

```
src-tauri/target/release/bundle/
  msi/      Windows .msi
  nsis/     Windows .exe setup
  dmg/      macOS
  deb/  appimage/   Linux
```

Install it and the app launches from the Start menu with no terminal. Your library lives in the platform application-data directory — `%APPDATA%\com.rec0n.book-companion` on Windows — **outside** the install, so rebuilding or reinstalling never touches your books, pages, or summaries.

---

## Developing

### The edit–run loop

With `npm run tauri dev` running:

- **Anything under `src/`** (TypeScript, CSS) hot-reloads. The window updates without losing state.
- **Anything under `src-tauri/src/`** (Rust) triggers a rebuild and relaunches the window. Ten to thirty seconds.

Two checks worth running before you consider a change done:

```bash
npx tsc --noEmit                # frontend types
cd src-tauri && cargo test      # 196 tests
```

### Where things live

```
src/
  App.tsx                 Shell: Ollama status, theme picker, library ↔ reader
  components/
    Library.tsx             Book list, creation, deletion
    Reader.tsx              Split pane, page turning, block controls
    MethodPanel.tsx         The five steps
    SentencePass.tsx        Sentence-by-sentence tier
    PageNavigator.tsx       Outline: parts, chapters, sections
    AddContentWizard.tsx    Import flow, including page numbering
    WordPopover.tsx         Dictionary
    ThemePicker.tsx  Spinner.tsx
  lib/
    api.ts                  Every IPC call, typed. Start here.
    theme.ts

src-tauri/src/
  commands.rs             The whole IPC surface. Thin — logic lives below.
  lib.rs                  Plugin and command registration
  models.rs  error.rs
  db/
    mod.rs                  Queries and migrations
    schema.sql              Schema for a fresh install
  ocr/
    preprocess.rs           Orientation, HEIC, spread splitting
    orient.rs               Which way up a page is
    segment.rs              Paragraphs, hyphenation, long-s, commentary
    outline.rs              Parts, chapters, sections, topics
    page_number.rs          Reading the folio; reconciling facing pages
    legible.rs              Did that transcription come out as English?
  ingest/                 EPUB, PDF, web — no OCR
  dict/                   Two-layer lookup
  coach/                  The rubric, and why it is a rubric
  ollama/                 HTTP client, prompts, guardrails
```

`commands.rs` and `src/lib/api.ts` are the two ends of the same wire. Reading them side by side is the fastest way to understand the app.

### Adding a feature that crosses the boundary

Four edits, always in this order:

**1.** Put the logic in the module it belongs to, with tests — `src-tauri/src/ocr/`, `dict/`, `coach/`, `ingest/`.

**2.** Expose a thin command in `commands.rs`:

```rust
#[tauri::command]
pub fn my_thing(state: State<'_, AppState>, book_id: i64) -> Result<Vec<Thing>> {
    state.db.my_thing(book_id)
}
```

Make it `async` if it does anything slow. A synchronous command runs on the main thread and freezes the window — that is a bug this project has already had, and it is why `import_page` hands its work to `spawn_blocking`.

**3.** Register it in `lib.rs`, in `invoke_handler![...]`. Forgetting this compiles fine and fails at runtime.

**4.** Add the typed wrapper in `src/lib/api.ts`:

```ts
myThing: (bookId: number) => invoke<Thing[]>("my_thing", { bookId }),
```

Tauri converts `snake_case` parameters to `camelCase` across the boundary. Pass `bookId`, receive `book_id`.

### Changing the database schema

Never edit `schema.sql` alone — that file is only used for a fresh install, and an existing library would be left behind. Three edits:

1. Add the change to `schema.sql`, for new installs.
2. Add a `MIGRATE_N_TO_M` constant in `db/mod.rs` with the `ALTER TABLE`.
3. Bump `SCHEMA_VERSION` and add the step to `init()`.

Migrations run in order on open, so a library from any earlier version upgrades in place. **Test destructive migrations against a copy of a real library first.** The v1→v2 migration rebuilds `pages`, and `blocks` cascade-delete from it — with foreign keys enforced, that would have destroyed every block in the library. There are regression tests pinning exactly that.

### Diagnostic tools

Unit tests use fixtures; these run the real thing. Each exists because something failed in a way the tests did not predict.

```bash
cd src-tauri

# Orientation and spread detection, with the measurements behind the decision
cargo run --example check-gutter -- path/to/photo.jpg

# What an EPUB extracts to: spine, contents, chapters per division
cargo run --example check-epub -- path/to/book.epub

# The outline the navigator will show, from a real library
cargo run --example check-outline -- "%APPDATA%/com.rec0n.book-companion/library.sqlite"

# EPUB / PDF / web extraction
cargo run --example check-ingest -- <file-or-url>
```

`check-gutter` prints the numbers, not just the verdict, which is what makes a threshold arguable:

```
text    : runs down the page  -> turn 90° anticlockwise
          swing rows 0.148  cols 0.452  ratio 3.05
gutter  : darkest 113, page 151, ratio 0.751 (need <0.80)
```

### Inspecting your library

It is plain SQLite. Nothing is hidden:

```bash
sqlite3 "$APPDATA/com.rec0n.book-companion/library.sqlite" \
  "SELECT page_no, kind, substr(text_norm,1,60) FROM blocks
   JOIN pages ON pages.id = blocks.page_id LIMIT 20;"
```

### Changing the models

Defaults are in `src-tauri/src/ollama/mod.rs`:

```rust
pub const DEFAULT_OCR_MODEL: &str = "glm-ocr";
pub const DEFAULT_TEXT_MODEL: &str = "qwen3:4b";
pub const DEFAULT_VISION_MODEL: &str = "qwen3-vl:4b";
```

The OCR prompt and its guardrails are in the same file. If you swap the OCR model, re-run `check-gutter` and a real page first — the constants there were calibrated against measurements, not chosen from documentation.

### Troubleshooting

**`Port 1420 is already in use`** — a dev server from a previous run survived.

```bash
npx kill-port 1420
```

**`failed to remove file book-companion.exe: Access is denied`** — the app is still running and holding its own binary. Close the window, or:

```bash
taskkill /IM book-companion.exe /F
```

**"Ollama is not running"** in the app header — start it with `ollama serve`, then reload. The badge turns green when both models are present.

**"models are missing"** — the header names them and gives the `ollama pull` command.

**Dictionary lookups do nothing** — `dict.sqlite` was never built. Run `npm run dict:build`. The app logs `dictionary unavailable` on startup when it is absent.

**npm warns about `allow-scripts` / esbuild** — npm 11 blocks postinstall scripts. Harmless here; esbuild ships prebuilt binaries as optional dependencies.

**First `cargo` build is very slow** — `rusqlite` compiles SQLite from source and Tauri is a large dependency tree. Once. Later builds are incremental.

**A page imported sideways, split in half, or numbered wrongly** — run `check-gutter` on the photograph. It prints every measurement behind the decision, and the thresholds are constants in `preprocess.rs` and `orient.rs` with the reasoning beside them.

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

## Stack and testing

**React 19 + TypeScript + Vite + Tailwind 4** in the window, **Rust** underneath, **SQLite** for storage, **Ollama** over HTTP for the models. See [Where things live](#where-things-live) for the layout.

Tauri v2 rather than Electron, for a reason specific to this app: on an 8GB GPU, memory the shell doesn't take is memory the models get. About 40MB idle against roughly 350MB.

**196 tests.** Many are built from *verbatim output of real models on real photographs*, because the failures that mattered were not the ones that seemed likely in advance. Several encode a specific mistake so it cannot recur — a gutter-darkness threshold measured at 0.751 on a real spread, and an orientation heuristic that got three of six real pages wrong and was deleted rather than tuned.

```bash
cd src-tauri && cargo test
npx tsc --noEmit
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
