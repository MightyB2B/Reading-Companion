# Reading Companion

A desktop application for reading difficult books slowly and actually understanding them.

Photograph a page, or import an ebook, and work through it one paragraph at a time: read it, break it into sentences, compress it into a single sentence of your own, and check that sentence says who or what it is about. A model then tells you what you missed — without telling you the answer. Words you don't know are looked up in a dictionary from **1913**, because in a book from 1830 the modern meaning is often the wrong one.

**Your library lives on a server you run.** A desktop application talks to it; the server holds the books, the page images, and the model work, so a laptop can read a library that a machine with a real GPU is doing the thinking for. Accounts are separate: one person cannot see another's books, and the checks that guarantee it are in SQL rather than in code anyone could forget to call.

Everything runs on machines you own. Nothing about what you read reaches a third party.

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
| [Ollama](https://ollama.com) | running somewhere | `ollama --version` |
| [PostgreSQL](https://www.postgresql.org) | 15 or later | `psql --version` |
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

Create the library database. This makes a Postgres role and database and writes the connection string to `.env`:

```bash
$env:PGPASSWORD = 'your-postgres-password'; powershell -ExecutionPolicy Bypass -File .\scripts\setup-postgres.ps1
```

The schema itself is applied by the server on first run, not by that script — migrations have one owner, and a deployed server has no repository beside it to read `.sql` files from.

### Running it

Two processes. **The server**, which holds the library:

```bash
cargo run -p reading-server
```

It listens on `127.0.0.1:7878` and prints what it found — the database, the dictionary, and the inference server it is configured for. Set `BIND_ADDRESS=0.0.0.0:7878` to accept connections from other machines; loopback is the default because binding every interface is a decision with consequences.

**The application**, in a second terminal:

```bash
npm run tauri dev
```

The first launch compiles Rust and takes a few minutes; later ones take seconds. Keep both terminals open.

**There is no default account.** A shipped username and password is a backdoor, and one nobody remembers to change. Instead, the first time you run it the sign-in screen offers to *create* an account — and that first account on an empty library becomes the administrator, since someone has to be able to set the inference server's address.

Registration then closes behind it. To let somebody else join, turn on **Settings → Who may join**, have them register, and turn it off again; accounts created that way are ordinary, not administrators. It is off by default because a server reachable from a network would otherwise stay an open signup form forever — but an empty library always accepts its first account, so leaving it off can never lock you out of a new one.

Ollama must be running for transcription and coaching. If it isn't, the app says so rather than failing obscurely.

**Environment the server reads**, all optional except the first:

| | |
|---|---|
| `DATABASE_URL` | written to `.env` by the setup script |
| `BIND_ADDRESS` | default `127.0.0.1:7878` |
| `LIBRARY_DIR` | where page images go, default `./library` |
| `DICT_PATH` | default `./dict.sqlite` |

### Putting the server on another machine

The point of the split: the server runs on the box with the GPU, and you read on whatever is to hand.

**One script, and it installs its own dependencies.** No Rust toolchain on the server. Build the two artefacts on your development machine:

```bash
cargo build --release -p reading-server
```
```bash
npm run dict:build
```

Copy **three files** to the server, into one directory:

```
scripts/install-server.ps1
target/release/reading-server.exe
src-tauri/resources/dict.sqlite
```

Then, from an **elevated** PowerShell there:

```bash
powershell -ExecutionPolicy Bypass -File .\install-server.ps1
```

That is the whole thing. It installs PostgreSQL if it is missing — generating the superuser password itself, so there is nothing to invent or remember — creates the role and database with a second generated password, installs into `C:\ReadingCompanion`, writes the configuration, opens the port to your subnet only, registers a service that starts automatically and restarts on failure, and checks the result actually answers before claiming success. The address it prints is what goes into the application's sign-in screen.

Both generated passwords are saved to files readable only by Administrators. You never type either one.

Every check runs **before** anything is changed — elevation, the files, the port — and a failure prints the whole list rather than stopping at the first. Safe to re-run: an existing database is left alone unless you pass `-Fresh`, and the service is replaced rather than duplicated.

If PostgreSQL is already installed, it needs its superuser password: `$env:PGPASSWORD = '…'` before running, or `reset-postgres-password.ps1` if it has been lost.

`setup-postgres.ps1` and `deploy-server.ps1` still exist and do the two halves separately, for when you want one without the other. `install-server.ps1` does not call them — it is self-contained, so it is the only file you need to copy.

The server's configuration lives in `.env` beside the binary, readable only by Administrators. That is deliberate: a Windows service starts in `system32` rather than where it was installed, so it can only find configuration next to itself — and the alternative, machine-wide environment variables, would put the database password where every account on the box can read it.

**The connection is encrypted.** The installer generates a certificate and the server terminates TLS itself — no reverse proxy. It is self-signed, because no public authority will sign a certificate for `192.168.x.x`, so the reading machine has to be told to trust that one certificate. Copy `server-cert.pem` (which the installer leaves beside itself) to the reading machine and use **Trust a certificate…** on the sign-in screen. Once, not every launch.

That is a stronger guarantee than a public certificate authority, not a weaker one: only your server can present that certificate. The alternative — accepting any certificate — would let anything on the network impersonate your library and read everything you read, which is why the application will not do it.

The certificate is public by design; the private key never leaves the server and is readable only by Administrators.

To check a server before trusting it from the app:

```bash
cargo run -p reading-core --example check-tls -- https://192.168.4.252:7878 server-cert.pem
```

That uses the same client the application does, and reports both halves: that the connection is refused *without* the certificate, and accepted with it. Windows `curl` cannot answer this — it uses schannel, which ignores `--cacert`.

### Removing the server

```bash
.\scripts\uninstall-server.ps1 -DryRun
```

That takes an inventory and changes nothing. It reports what it found and, crucially, what the database holds — *"3 books, 412 pages, 96 summaries, written by 2 accounts"* — because that is the honest way to say what deleting it costs.

To go through with it:

```bash
.\scripts\uninstall-server.ps1 -Backup C:\library-backup.sql
```

It removes the service, the install directory, the page photographs, the firewall rule, the database, and the role. It asks you to type `REMOVE` first — a y/n keystroke is too easy to make by accident.

**The backup runs before the confirmation, and on a dry run too.** A backup you have not seen work is not a backup, and the moment to discover `pg_dump` is missing is not after the database has gone. Run with `-DryRun -Backup <path>` to produce a real dump and inspect it before committing to anything.

Left alone unless you ask: PostgreSQL (`-RemovePostgres`), since other things may use it, and Ollama with its models (`-RemoveOllama`), since that is several GB to download again. `-KeepLibrary` rescues the page photographs; `-KeepDatabase` removes the service and files only.

The desktop application is a separate program — uninstall it from Add or Remove Programs on the machine you read on.

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

Install it and the app launches from the Start menu with no terminal. It is only the window: your books, pages, and summaries live in the library server's Postgres database and its `LIBRARY_DIR`, so rebuilding or reinstalling the application never touches them. The app remembers only which server to talk to.

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

src-tauri/src/            The window. Holds no logic and no library.
  commands.rs             Each command is one HTTP call to the server
  client.rs               The session token lives here, never in the webview
  lib.rs                  Plugin and command registration

crates/reading-server/src/  The library service.
  routes/
    accounts.rs             Register, sign in, sign out
    library.rs              Books, pages, blocks, summaries, outline
    import.rs               Photographs, EPUB, PDF, web
    study.rs                The coach and the dictionary
    images.rs               Page photographs, with a path-traversal guard
    models.rs               Settings and the inference server
  auth_layer.rs           Caller and Admin extractors
  state.rs                Shared state and server settings

crates/reading-core/src/  The engine. No UI, no transport.
  auth.rs                 argon2id passwords, session tokens
  db/pg.rs                Postgres, and every ownership check
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
  examples/               Diagnostic tools — see "Diagnostic tools" below
```

This is a cargo workspace. `reading-core` holds everything the app does that is not drawing, and depends on nothing from Tauri — the check is `grep -r "tauri::" crates/reading-core/`, which should stay empty. That constraint is what lets the same engine run in-process on a laptop and behind an HTTP server, rather than the logic existing twice.

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

### Checking the two boundaries that matter

The database layer and the HTTP layer each have a script that tries to break their isolation. Both create throwaway accounts, attempt every crossing, and clean up after themselves.

```bash
cargo run -p reading-core --example check-db
```

```bash
bash scripts/check-api.sh
```

They are scripts rather than `cargo test` cases on purpose: both need a live Postgres, and `cargo test` has to pass on a fresh clone that has none. Run them after touching anything in `db/pg.rs`, `auth_layer.rs`, or the routes.

### Diagnostic tools

Unit tests use fixtures; these run the real thing. Each exists because something failed in a way the tests did not predict.

```bash
cd src-tauri

# Orientation and spread detection, with the measurements behind the decision
cargo run -p reading-core --example check-gutter -- path/to/photo.jpg

# What an EPUB extracts to: spine, contents, chapters per division
cargo run -p reading-core --example check-epub -- path/to/book.epub

# The database layer, against a real Postgres, including isolation
cargo run -p reading-core --example check-db

# EPUB / PDF / web extraction
cargo run -p reading-core --example check-ingest -- <file-or-url>
```

`check-gutter` prints the numbers, not just the verdict, which is what makes a threshold arguable:

```
text    : runs down the page  -> turn 90° anticlockwise
          swing rows 0.148  cols 0.452  ratio 3.05
gutter  : darkest 113, page 151, ratio 0.751 (need <0.80)
```

### Inspecting your library

It is plain Postgres. Nothing is hidden:

```bash
psql "$env:DATABASE_URL" -c "SELECT page_no, kind, substr(text_norm,1,60) FROM blocks JOIN pages ON pages.id = blocks.page_id LIMIT 20;"
```

Page images are files under `LIBRARY_DIR`, one directory per book.

### Changing the models

Day to day, use **Settings** (the gear in the header). It asks the server what it has installed and offers those as dropdowns, so a model you never pulled cannot be chosen by mistake. The two roles that look at an image only list models reporting the `vision` capability.

Only an administrator may change them, and the same is true of the inference server's address — that is a URL the *server* then fetches, so an ordinary account able to set it would have server-side request forgery.

Defaults, used until you change them, are in `crates/reading-core/src/ollama/mod.rs`:

```rust
pub const DEFAULT_OCR_MODEL: &str = "glm-ocr";
pub const DEFAULT_TEXT_MODEL: &str = "qwen3:4b";
pub const DEFAULT_VISION_MODEL: &str = "qwen3-vl:4b";
```

The OCR prompt and its guardrails are in the same file. If you swap the OCR model, re-run `check-gutter` and a real page first — the constants there were calibrated against measurements, not chosen from documentation.

### Running the models somewhere else

Settings also takes the **Ollama address**. Inference does not have to happen on the machine you are reading on: point it at a desktop with a real GPU and an 8GB laptop can use a 30B model. Test the address before saving — the button says outright whether the server answered.

A bare `192.168.1.50:11434` is expanded to `http://192.168.1.50:11434`. The remote Ollama needs `OLLAMA_HOST=0.0.0.0` set, or it listens only to itself.

On a Windows server with Ollama already installed, `scripts/setup-ollama-server.ps1` does the whole thing — binds every interface, adds a firewall rule scoped to your subnet, restarts Ollama, pulls the models, and prints the address to paste into Settings. Run it **on the server**, not on the reading machine:

```bash
powershell -ExecutionPolicy Bypass -File .\scripts\setup-ollama-server.ps1
```

Run it from an elevated prompt so it can add the firewall rule; without elevation it does everything else and prints the one command you need to run as admin. Defaults assume a dedicated box: three models resident, never unloaded. `-MaxLoaded 2` and `-KeepAlive 30m` suit a machine you also use for other things.

**Ollama should not be reachable at all.** Since the split, the only thing that talks to it is `reading-server`, usually on the same machine — so bind it to loopback and give it no firewall rule. It has no authentication of its own and does not need any: the request that reaches it has already been authenticated by the library server, against a real account.

That replaces the reverse-proxy arrangement entirely. `secure-ollama-wan.ps1` is kept for exposing Ollama to *something else*, but Reading Companion no longer needs it, and neither does the API key field unless you point the server at a hosted endpoint.

The path is: **app → reading-server → Ollama.** Encrypted and authenticated on the first hop, loopback on the second. AI usage is tied to whoever is signed in rather than to one shared key.

### Keeping models loaded

Loading a model costs 5–10 seconds, so Settings has **Keeping models loaded**: how long the server holds one after it has been used. Default 30 minutes, which covers a reading session on a machine you also use for other things. On a dedicated server pick **Never unload** — there is nothing to give the VRAM back to.

This is sent with every request, which means it *overrides* `OLLAMA_KEEP_ALIVE` on the server. Setting the environment variable alone does nothing while the app is talking; the setting is the one that decides.

For a hosted or public Ollama-compatible endpoint, fill in the **API key**; it is sent as `Authorization: Bearer …` on every request. Leave it blank for a local server, which wants no authentication at all. The key is stored in the `settings` table in plain text and is never returned to any client — reading settings tells you only whether one is set.

Changing either takes effect immediately. The HTTP client is rebuilt, not the app restarted, so you can move inference mid-chapter.

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

**"rejected the API key"** when testing a server — the server is reachable, so the network is fine. Check the key, or clear it if the server does not want one.

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

## Privacy and access

No telemetry, and no third-party inference. Everything happens on machines you run: the application, the library server, Postgres, and Ollama.

That said, be precise about what "local" means now. If you point the app at a server on another machine — which is the point of the split — then **page images and paragraph text travel to that machine**, and to whatever Ollama the server is configured for. On a LAN that is your own network. Over the internet it is not, which is why `scripts/secure-ollama-wan.ps1` exists and why the server logs a warning when you bind it to a public interface. There is no TLS in the server itself; put a reverse proxy in front before exposing it.

**No account ships with the application.** The first person to reach an empty library creates one and becomes its administrator; after that, registration is closed unless an administrator opens it. Both paths are covered by `scripts/check-api.sh`, including that a later account never gets administrator rights and that a client which does not know about the setting cannot enable it by omitting the field.

**Accounts are isolated by construction.** `books.user_id` is the only place ownership is recorded, and every query reaches it by joining rather than by a check a new endpoint could forget. Asking for someone else's book reports "not found" rather than "forbidden", because telling those apart confirms the id exists. `scripts/check-api.sh` tries to cross the boundary in every direction and fails the build if any attempt succeeds.

Passwords are argon2id. Session tokens are 256 bits of CSPRNG output, stored as SHA-256 digests — a database read yields no working credentials. The token never enters the webview, because the webview renders imported content and a script hidden in a saved web page could otherwise read it.

The single outbound exception remains importing a web page, which fetches the URL you type and is labelled as such in the interface.

Your photographs are kept alongside the text, because the OCR model silently modernises archaic spelling and ligatures: the image, not the transcription, is the record of what was actually printed.

---

## Licence and credits

- **Webster's Revised Unabridged Dictionary (1913)** — public domain, via [Project Gutenberg](https://www.gutenberg.org) and the [`ssvivian/WebstersDictionary`](https://github.com/ssvivian/WebstersDictionary) parse.
- Models are pulled from [Ollama](https://ollama.com) under their own licences.
- HEIC decoding by [`imazen/heic`](https://github.com/imazen/heic), pure Rust.

Book text you import is your own. This tool stores it locally and does not transmit it.
