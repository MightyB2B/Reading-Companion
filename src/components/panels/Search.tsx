import { useEffect, useState } from "react";
import { api, type Book, type SearchHit } from "../../lib/api";
import { EmptyState, PanelHeader } from "../PanelChrome";
import { SendTo } from "./Context";

/**
 * Searching the library.
 *
 * One box over the text, the notes, and the terms together. Asking the reader
 * which of three places to look in is the application's structure leaking into
 * the reading — what they have is a phrase they half-remember.
 */
export function SearchPanel({
  books,
  scopeBookId,
  initialQuery,
  onGoTo,
}: {
  books: Book[];
  /** The book in the focused pane, offered as a scope. */
  scopeBookId: number | null;
  /** Set when the search was started from elsewhere, e.g. a word lookup. */
  initialQuery: string;
  onGoTo: (bookId: number, blockId: number, into?: 0 | 1) => void;
}) {
  const [query, setQuery] = useState(initialQuery);
  const [scope, setScope] = useState<number | null>(null);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (initialQuery) setQuery(initialQuery);
  }, [initialQuery]);

  // Debounced, because searching on every keystroke of a long phrase runs a
  // dozen queries to show the answer to the last one.
  useEffect(() => {
    const text = query.trim();
    if (text.length < 2) {
      setHits(null);
      return;
    }
    let cancelled = false;
    setBusy(true);
    const timer = window.setTimeout(() => {
      api
        .searchLibrary(text, scope)
        .then((r) => {
          if (!cancelled) {
            setHits(r);
            setError(null);
          }
        })
        .catch((e) => !cancelled && setError(String(e)))
        .finally(() => !cancelled && setBusy(false));
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, scope]);

  const grouped = {
    text: (hits ?? []).filter((h) => h.kind === "text"),
    note: (hits ?? []).filter((h) => h.kind === "note"),
    term: (hits ?? []).filter((h) => h.kind === "term"),
  };

  return (
    <div>
      <PanelHeader
        title="Search"
        purpose="Every book you have imported, plus your own notes and terms. Stemmed, so a search for one form of a word finds the others."
      />

      <div className="space-y-2 border-b border-rule px-4 py-3">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="A phrase you half-remember"
          autoFocus
          className="w-full rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
        />
        <div className="flex items-center gap-2 text-xs">
          <select
            value={scope ?? ""}
            onChange={(e) =>
              setScope(e.target.value === "" ? null : Number(e.target.value))
            }
            className="rounded border border-rule bg-paper px-2 py-1 outline-none focus:border-accent"
          >
            <option value="">Everywhere</option>
            {books.map((b) => (
              <option key={b.id} value={b.id}>
                {b.title}
              </option>
            ))}
          </select>
          {scopeBookId != null && scope !== scopeBookId && (
            <button
              onClick={() => setScope(scopeBookId)}
              className="text-ink-soft hover:text-accent"
            >
              just this book
            </button>
          )}
          {busy && <span className="ml-auto text-ink-soft">searching…</span>}
        </div>
        {error && <p className="text-xs text-danger">{error}</p>}
      </div>

      {hits === null ? (
        <EmptyState
          lead="Type a phrase. This searches the words of every book you have imported, not just their titles."
          example={
            <div className="text-sm">
              <p className="text-xs uppercase tracking-wider text-ink-soft">
                Systematic Theology · p. 1
              </p>
              <p className="mt-0.5">
                In every science there are two <mark className="mark-pending">factors</mark>: facts and ideas.
              </p>
            </div>
          }
        />
      ) : hits.length === 0 ? (
        <p className="px-4 py-6 text-sm text-ink-soft">
          Nothing matches. Try fewer words — the search looks for the whole
          phrase.
        </p>
      ) : (
        <div className="divide-y divide-rule">
          <Group label="In the text" hits={grouped.text} onGoTo={onGoTo} />
          <Group label="In your notes" hits={grouped.note} onGoTo={onGoTo} />
          <Group label="In your terms" hits={grouped.term} onGoTo={onGoTo} />
        </div>
      )}
    </div>
  );
}

function Group({
  label,
  hits,
  onGoTo,
}: {
  label: string;
  hits: SearchHit[];
  onGoTo: (bookId: number, blockId: number, into?: 0 | 1) => void;
}) {
  if (hits.length === 0) return null;
  return (
    <section>
      <h3 className="sticky top-0 bg-paper-dim px-4 py-1 text-[0.65rem] uppercase tracking-wider text-ink-soft">
        {label} · {hits.length}
      </h3>
      <ul className="divide-y divide-rule">
        {hits.map((h, i) => (
          <li key={i} className="group px-4 py-2 hover:bg-paper-dim">
            <button
              onClick={() => h.block_id && onGoTo(h.book_id, h.block_id)}
              disabled={!h.block_id}
              className="block w-full text-left"
            >
              <span className="text-xs font-semibold">
                {h.book_title}
                {h.page_no != null && (
                  <span className="font-normal text-ink-soft"> · p. {h.page_no}</span>
                )}
              </span>
              <span className="mt-0.5 block text-sm">
                <Snippet text={h.snippet} />
              </span>
            </button>
            {h.block_id != null && (
              <SendTo onGoTo={(into) => onGoTo(h.book_id, h.block_id!, into)} />
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * FTS5 marks the hit with guillemets rather than HTML, so the snippet can be
 * carried through the IPC boundary as plain text and marked up here — nothing
 * from the database is ever interpreted as markup.
 */
function Snippet({ text }: { text: string }) {
  const parts = text.split(/[«»]/);
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <mark key={i} className="mark-pending">
            {part}
          </mark>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  );
}
