import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type VocabEntry } from "../../lib/api";
import { EmptyState, PanelHeader } from "../PanelChrome";

/**
 * The words you had to look up.
 *
 * Every in-context lookup already recorded the word, the sentence you met it
 * in, and the meaning that applied — and nothing ever showed them. Data
 * written and never read is either a missing screen or a bad idea; this is a
 * missing screen, because a list of the words a particular book made you stop
 * at is a genuinely good record of reading it.
 *
 * Sorted by how often you looked each one up, because a word you have checked
 * four times is one you have not learned yet, and that is the useful signal.
 */
export function WordsPanel({
  bookId,
  bookTitle,
  revision,
  onLookAgain,
}: {
  bookId: number;
  bookTitle: string;
  revision: number;
  /** Put the word back in the Context panel. */
  onLookAgain: (word: string, sentence: string) => void;
}) {
  const [words, setWords] = useState<VocabEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .vocabulary(bookId)
      .then(setWords)
      .catch((e) => setError(String(e)));
  }, [bookId]);

  useEffect(refresh, [refresh, revision]);

  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const list = words ?? [];
    if (!needle) return list;
    return list.filter(
      (w) =>
        w.word.toLowerCase().includes(needle) ||
        (w.gloss ?? "").toLowerCase().includes(needle),
    );
  }, [words, query]);

  const repeated = (words ?? []).filter((w) => w.count > 1).length;

  return (
    <div>
      <PanelHeader
        title={`Words · ${bookTitle}`}
        purpose="Every word you looked up in this book, with the sentence you met it in."
      />
      {error && <p className="px-4 py-2 text-sm text-danger">{error}</p>}

      {words !== null && words.length > 0 && (
        <div className="border-b border-rule px-4 py-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter"
            className="w-full rounded border border-rule bg-transparent px-2 py-1 text-sm outline-none focus:border-accent"
          />
          {repeated > 0 && (
            <p className="mt-1.5 text-xs text-ink-soft">
              {repeated} you have looked up more than once — those are the ones
              still to learn.
            </p>
          )}
        </div>
      )}

      {words === null ? (
        <p className="px-4 py-6 text-sm text-ink-soft">Gathering…</p>
      ) : words.length === 0 ? (
        <EmptyState
          lead="Nothing yet. Double-click a word in the text and ask what it means here, and it is kept — with the sentence you met it in."
          example={
            <div className="text-sm">
              <p className="font-semibold">
                perspicuity <span className="font-normal text-ink-soft">· 3 times</span>
              </p>
              <p className="mt-0.5 text-ink-soft">
                Clearness of statement; the quality of being easily understood.
              </p>
            </div>
          }
        />
      ) : (
        <ul className="divide-y divide-rule">
          {shown.map((w) => (
            <li key={w.word} className="px-4 py-2.5">
              <div className="flex items-baseline gap-2">
                <button
                  onClick={() => onLookAgain(w.word, w.sentence ?? "")}
                  title="Look it up again"
                  className="font-semibold hover:text-accent"
                >
                  {w.word}
                </button>
                {w.lemma && w.lemma !== w.word && (
                  <span className="text-xs text-ink-soft">→ {w.lemma}</span>
                )}
                <span className="ml-auto text-xs text-ink-soft tabular-nums">
                  {w.count}×
                </span>
              </div>

              {w.gloss && <p className="mt-0.5 text-sm">{w.gloss}</p>}

              {w.sentence && (
                <p className="prose-page mt-1 border-l-2 border-rule pl-2 text-[0.85rem] text-ink-soft">
                  {w.sentence}
                </p>
              )}
            </li>
          ))}
          {shown.length === 0 && (
            <li className="px-4 py-4 text-sm text-ink-soft">
              Nothing matches that.
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
