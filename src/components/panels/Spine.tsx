import { useCallback, useEffect, useState } from "react";
import { api, type SpineEntry } from "../../lib/api";
import { EmptyState, PanelHeader } from "../PanelChrome";

/**
 * The book, reduced to your own sentences.
 *
 * This is what the whole method builds toward — read a paragraph, find its
 * core, write one sentence, and let the sentences accumulate — and it used to
 * live at the bottom of the Method panel's step wizard, four collapsed steps
 * down, reachable only if you happened to scroll. The payoff of a month's
 * reading deserves a place you can go to.
 *
 * Read in order it is a précis of the book in your own words; that is a
 * genuinely useful artefact, and it is the thing the export leads with.
 */
export function SpinePanel({
  bookId,
  bookTitle,
  revision,
  onGoTo,
}: {
  bookId: number;
  bookTitle: string;
  revision: number;
  onGoTo: (blockId: number) => void;
}) {
  const [spine, setSpine] = useState<SpineEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .summarySpine(bookId)
      .then(setSpine)
      .catch((e) => setError(String(e)));
  }, [bookId]);

  useEffect(refresh, [refresh, revision]);

  return (
    <div>
      <PanelHeader
        title={`Spine · ${bookTitle}`}
        purpose="Every paragraph you have summarised, in reading order. The book in your own words."
      />
      {error && <p className="px-4 py-2 text-sm text-danger">{error}</p>}

      {spine === null ? (
        <p className="px-4 py-6 text-sm text-ink-soft">Gathering…</p>
      ) : spine.length === 0 ? (
        <EmptyState
          lead="Nothing summarised yet. Work a paragraph through the Method panel and its sentence appears here, and the next, and the next."
          example={
            <div className="text-sm">
              <p className="text-xs text-ink-soft">p. 1</p>
              <p className="mt-0.5">
                Theology is a science because it has facts and the mind that
                orders them.
              </p>
            </div>
          }
        />
      ) : (
        <>
          <p className="border-b border-rule px-4 py-2 text-xs text-ink-soft">
            {spine.length} paragraph{spine.length === 1 ? "" : "s"} in your words
          </p>
          <ol className="divide-y divide-rule">
            {spine.map((entry) => (
              <li key={entry.block_id}>
                <button
                  onClick={() => onGoTo(entry.block_id)}
                  className="block w-full px-4 py-2.5 text-left hover:bg-paper-dim"
                >
                  <span className="text-[0.65rem] uppercase tracking-wider text-ink-soft tabular-nums">
                    p. {entry.page_no}
                  </span>
                  <span className="prose-page mt-0.5 block text-[0.95rem]">
                    {entry.sentence}
                  </span>
                </button>
              </li>
            ))}
          </ol>
        </>
      )}
    </div>
  );
}
