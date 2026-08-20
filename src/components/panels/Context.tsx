import { useEffect, useState } from "react";
import {
  api,
  CORPUS_LABEL,
  TERM_STATUS_LABEL,
  type Citation,
  type ContextResult,
  type Lookup,
  type SearchHit,
  type TermStatus,
} from "../../lib/api";
import type { ContextTarget, WordHit } from "../Flow";
import { EmptyState, PanelHeader } from "../PanelChrome";

/**
 * What the thing you just clicked means.
 *
 * This replaces the card that used to float over the text. A popup covers the
 * words you are reading at exactly the moment you want to compare them with
 * what it says, cannot be kept open while you scroll, and has nowhere to put
 * anything longer than a sentence. Here the context sits still, stays as long
 * as it is useful, and has room for the concordance underneath.
 */
export function ContextPanel({
  target,
  word,
  pinned,
  onPin,
  onGoTo,
  onSearch,
}: {
  /** A term, note, or reference clicked in the text. */
  target: ContextTarget | null;
  /** A word double-clicked in the text, looked up in the dictionary. */
  word: WordHit | null;
  /** Held in view while reading on, until unpinned. */
  pinned: { target: ContextTarget | null; word: WordHit | null } | null;
  onPin: () => void;
  onGoTo: (bookId: number, blockId: number, into?: 0 | 1) => void;
  onSearch: (query: string) => void;
}) {
  const showing = word || target;
  return (
    <div>
      <PanelHeader
        title="Context"
        purpose="Whatever you last clicked in the text — a definition, a term you glossed, a note, or a scripture reference — with everywhere else it turns up."
        action={
          (showing || pinned) && (
            <button
              onClick={onPin}
              title={
                pinned
                  ? "Stop holding this in view"
                  : "Keep this in view while you read on"
              }
              className={`rounded border px-2 py-1 text-xs ${
                pinned
                  ? "border-accent text-accent"
                  : "border-rule hover:border-accent hover:text-accent"
              }`}
            >
              {pinned ? "Pinned" : "Pin"}
            </button>
          )
        }
      />

      {/* The pinned item sits above whatever you click next, so a verse can
          stay beside you for as long as it is useful. */}
      {pinned && (
        <div className="border-b-2 border-accent/40 bg-accent/5">
          {pinned.word ? (
            <WordContext word={pinned.word} onSearch={onSearch} />
          ) : pinned.target?.kind === "term" ? (
            <TermContext target={pinned.target} onSearch={onSearch} />
          ) : pinned.target?.kind === "notes" ? (
            <NotesContext target={pinned.target} />
          ) : pinned.target?.kind === "reference" ? (
            <ReferenceContext target={pinned.target} onGoTo={onGoTo} />
          ) : null}
        </div>
      )}
      {word ? (
        <WordContext word={word} onSearch={onSearch} />
      ) : target?.kind === "term" ? (
        <TermContext target={target} onSearch={onSearch} />
      ) : target?.kind === "notes" ? (
        <NotesContext target={target} />
      ) : target?.kind === "reference" ? (
        <ReferenceContext target={target} onGoTo={onGoTo} />
      ) : (
        <EmptyState
          lead="Nothing selected. Double-click a word for its definition, or click anything underlined in the text — a term you have glossed, a note, or a scripture reference."
          example={
            <div className="text-sm">
              <p className="font-semibold">substance</p>
              <p className="mt-0.5 text-ink-soft">
                Whatever exists in itself and is understood through itself.
              </p>
              <p className="mt-1 text-xs text-ink-soft">
                Working on it · 34 mentions in this book
              </p>
            </div>
          }
        />
      )}
    </div>
  );
}

/** A word looked up in the dictionary, plus where else the library uses it. */
function WordContext({
  word,
  onSearch,
}: {
  word: WordHit;
  onSearch: (q: string) => void;
}) {
  const [lookup, setLookup] = useState<Lookup | null | "loading">("loading");
  /**
   * The model's reading of the word in this sentence.
   *
   * Kept whole rather than reduced to its text: `sense_index` says which
   * dictionary sense it chose, or -1 for "none of these fit", and that
   * distinction is the entire point of the two-layer design. Showing only the
   * paraphrase made a meaning invented from memory look exactly like one
   * grounded in Webster's.
   */
  const [inContext, setInContext] = useState<ContextResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
  const [concordance, setConcordance] = useState<SearchHit[] | null>(null);

  useEffect(() => {
    setLookup("loading");
    setInContext(null);
    setConcordance(null);
    api
      .lookUpWord(word.word)
      .then(setLookup)
      .catch(() => setLookup(null));
  }, [word.word]);

  const askInContext = async () => {
    setBusy(true);
    try {
      setInContext(await api.wordInContext(word.word, word.sentence, word.blockId));
      setAskError(null);
    } catch (e) {
      setAskError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="px-4 py-3">
      <h3 className="text-base font-semibold">{word.word}</h3>

      {lookup === "loading" ? (
        <p className="mt-2 text-sm text-ink-soft">Looking it up…</p>
      ) : lookup === null ? (
        <p className="mt-2 text-sm text-ink-soft">
          No entry, even after stripping prefixes and endings. It may be a proper
          name, or a coinage of this author — worth adding to Terms.
        </p>
      ) : (
        <>
          {/* How the word was reached matters. Showing the stem's definition
              for `unending` without saying a negation was removed tells the
              reader the opposite of what the word means. */}
          {lookup.resolution !== "direct" && (
            <p className="mt-1 text-xs text-ink-soft">
              {lookup.prefix
                ? `${lookup.prefix}- + `
                : lookup.resolution === "derived"
                  ? "a form of "
                  : lookup.resolution === "inflection"
                    ? "a form of "
                    : "an older spelling of "}
              <strong>{lookup.lemma}</strong>
            </p>
          )}
          <ol className="mt-2 space-y-2">
            {lookup.senses.slice(0, 8).map((s, i) => (
              <li key={i} className="text-sm">
                {s.labels.length > 0 && (
                  <span className="mr-1 text-xs uppercase tracking-wide text-ink-soft">
                    {s.labels.join(", ")}
                  </span>
                )}
                {s.gloss}
                {/* Which book this came out of. The two corpora disagree on
                    purpose — Webster's first sense of "nice" is "foolish" —
                    so a sense is only usable if you know its source. */}
                <span className="ml-1.5 text-[0.65rem] uppercase tracking-wider text-ink-soft/70">
                  {CORPUS_LABEL[s.source] ?? s.source}
                </span>
              </li>
            ))}
          </ol>
        </>
      )}

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          onClick={askInContext}
          disabled={busy}
          className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
        >
          {busy ? "Thinking…" : "What does it mean here?"}
        </button>
        <button
          onClick={() => onSearch(word.word)}
          className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
        >
          Find it in my library
        </button>
        <button
          onClick={() =>
            api
              .concordance(word.word, 12)
              .then(setConcordance)
              .catch(() => setConcordance([]))
          }
          className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
        >
          Show every use
        </button>
      </div>

      {askError && (
        <p className="mt-3 text-sm text-danger">{askError}</p>
      )}

      {inContext && <InContext result={inContext} />}

      {concordance && (
        <div className="mt-3">
          <p className="text-xs uppercase tracking-wider text-ink-soft">
            {concordance.length} uses in your library
          </p>
          <ul className="mt-1 space-y-1.5">
            {concordance.map((h, i) => (
              <li key={i} className="text-xs">
                <span className="font-semibold">{h.book_title}</span>
                {h.page_no != null && (
                  <span className="text-ink-soft"> · p. {h.page_no}</span>
                )}
                <p className="text-ink-soft">{stripMarkers(h.snippet)}</p>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/**
 * What the model made of the word in this sentence, and how far to trust it.
 *
 * The application's guarantee is that this layer *chooses among senses the
 * dictionary supplied* rather than defining from memory. When it reports that
 * none of them fit, that is exactly the case the design exists to catch, and
 * the reader has to be told — presenting the two identically hands over an
 * ungrounded definition wearing the dictionary's authority.
 */
function InContext({ result }: { result: ContextResult }) {
  const { sense_index, plain_meaning, why_this_sense } = result.in_context;
  const grounded = sense_index >= 0 && sense_index < result.lookup.senses.length;

  return (
    <div
      className={`mt-3 rounded border p-2 text-sm ${
        grounded ? "border-rule bg-paper-dim" : "border-warn bg-warn/10"
      }`}
    >
      <p className="text-xs uppercase tracking-wider text-ink-soft">
        {grounded ? `In this sentence · sense ${sense_index + 1}` : "In this sentence"}
      </p>
      <p className="mt-1">{plain_meaning}</p>

      {why_this_sense && (
        <p className="mt-1.5 text-xs text-ink-soft">{why_this_sense}</p>
      )}

      <p className="mt-2 text-xs text-ink-soft">
        {grounded ? (
          <>
            Chosen from the dictionary entry above, not written from memory.
          </>
        ) : (
          <>
            <strong className="text-warn">Not in the dictionary.</strong> None of
            the entries above fitted this sentence, so this is the model&rsquo;s
            own reading — treat it as a suggestion, not a definition.
          </>
        )}
      </p>
    </div>
  );
}

function TermContext({
  target,
  onSearch,
}: {
  target: Extract<ContextTarget, { kind: "term" }>;
  onSearch: (q: string) => void;
}) {
  const t = target.term;
  return (
    <div className="px-4 py-3">
      <div className="flex items-baseline gap-2">
        <h3 className="text-base font-semibold">{t.term}</h3>
        <span className="text-[0.65rem] uppercase tracking-wider text-accent">
          {TERM_STATUS_LABEL[t.status as TermStatus] ?? t.status}
        </span>
      </div>
      <p className="mt-2 text-sm">
        {t.my_gloss || (
          <em className="text-ink-soft">
            No gloss written yet — open Terms to write one.
          </em>
        )}
      </p>
      <p className="mt-3 text-xs text-ink-soft">
        This is what <em>this author</em> means by it, not what the dictionary
        says.
      </p>
      <button
        onClick={() => onSearch(t.term)}
        className="mt-2 rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
      >
        Find it everywhere
      </button>
    </div>
  );
}

function NotesContext({
  target,
}: {
  target: Extract<ContextTarget, { kind: "notes" }>;
}) {
  return (
    <div className="px-4 py-3">
      <p className="text-xs uppercase tracking-wider text-ink-soft">
        {target.notes.length} note{target.notes.length === 1 ? "" : "s"} here
      </p>
      <ul className="mt-2 space-y-3">
        {target.notes.map((n) => (
          <li key={n.id}>
            {n.move && (
              <span
                className={`mark mark-${n.move} rounded px-1 text-xs font-semibold`}
              >
                {n.move}
              </span>
            )}
            {n.anchor_text && (
              <p className="mt-1 text-xs italic text-ink-soft">
                “{n.anchor_text}”
              </p>
            )}
            {n.body.trim() ? (
              <p className="mt-1 whitespace-pre-wrap text-sm">{n.body}</p>
            ) : (
              <p className="mt-1 text-sm text-ink-soft">
                <em>Tagged, with nothing written.</em>
              </p>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** A scripture reference, and everything in the library that cites it. */
function ReferenceContext({
  target,
  onGoTo,
}: {
  target: Extract<ContextTarget, { kind: "reference" }>;
  onGoTo: (bookId: number, blockId: number, into?: 0 | 1) => void;
}) {
  const r = target.reference;
  const [citations, setCitations] = useState<Citation[] | null>(null);
  const [verse, setVerse] = useState<string | null>(null);

  useEffect(() => {
    setCitations(null);
    setVerse(null);
    api
      .citationsOf(r.osis_book, r.chapter, r.verse_start)
      .then(setCitations)
      .catch(() => setCitations([]));
    if (r.verse_start != null) {
      api
        .verseText(r.osis_book, r.chapter, r.verse_start)
        .then(setVerse)
        .catch(() => {});
    }
  }, [r]);

  return (
    <div className="px-4 py-3">
      <h3 className="text-base font-semibold">{r.label}</h3>
      <p className="mt-0.5 text-xs text-ink-soft">
        printed as “{r.surface}”
        {r.confidence < 1 && " · uncertain reading"}
      </p>

      {verse ? (
        <blockquote className="prose-page mt-3 border-l-2 border-accent pl-3 text-sm">
          {verse}
        </blockquote>
      ) : (
        <p className="mt-3 rounded bg-paper-dim px-2 py-1.5 text-xs text-ink-soft">
          No Bible in your library yet, so the words of the verse are not here.
          The citations below work regardless.
        </p>
      )}

      <p className="mt-4 text-xs uppercase tracking-wider text-ink-soft">
        Cited in your library
      </p>
      {citations === null ? (
        <p className="mt-1 text-sm text-ink-soft">Looking…</p>
      ) : citations.length === 0 ? (
        <p className="mt-1 text-sm text-ink-soft">
          Nothing else in your books cites this.
        </p>
      ) : (
        <ul className="mt-1 divide-y divide-rule">
          {citations.map((c, i) => (
            <li key={i} className="group py-2">
              <button
                onClick={() => onGoTo(c.book_id, c.block_id)}
                className="block w-full text-left hover:text-accent"
              >
                <span className="text-xs font-semibold">
                  {c.book_title}
                  {c.page_no != null && ` · p. ${c.page_no}`}
                </span>
                <span className="mt-0.5 block text-sm text-ink-soft">
                  {c.context}
                </span>
              </button>
              <SendTo onGoTo={(into) => onGoTo(c.book_id, c.block_id, into)} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Which side to open it on.
 *
 * Jumps used to land in the right pane by convention, which meant following a
 * citation could replace the very passage that prompted it.
 */
export function SendTo({ onGoTo }: { onGoTo: (into: 0 | 1) => void }) {
  return (
    <span className="mt-1 flex gap-1 text-[0.65rem] opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
      <button
        onClick={() => onGoTo(0)}
        title="Open on the left"
        className="rounded border border-rule px-1.5 py-0.5 hover:border-accent hover:text-accent"
      >
        ◧ left
      </button>
      <button
        onClick={() => onGoTo(1)}
        title="Open on the right"
        className="rounded border border-rule px-1.5 py-0.5 hover:border-accent hover:text-accent"
      >
        ◨ right
      </button>
    </span>
  );
}

/** FTS5 wraps hits in guillemets; the panel shows them plainly. */
function stripMarkers(snippet: string): string {
  return snippet.replace(/[«»]/g, "");
}
