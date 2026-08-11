import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  api,
  RESOLUTION_NOTE,
  type ContextualSense,
  type Lookup,
} from "../lib/api";
import { Working } from "./Spinner";

/**
 * The word popover, in two layers.
 *
 * Layer one — the dictionary senses — appears instantly and offline. Layer two
 * — what the word means in *this* sentence — is a deliberate second click,
 * because it costs a couple of seconds and is not always needed.
 *
 * Obsolete and archaic senses are labelled rather than hidden. In a book from
 * 1830 the obsolete sense is very often the right one, and the reader needs to
 * be able to see that.
 */
/** Distance from the selected word. */
const GAP = 10;
/** Minimum breathing room from the viewport edge. */
const MARGIN = 12;
/** Below this, flipping above the word is worth doing. */
const MIN_HEIGHT = 180;

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

export function WordPopover({
  word,
  sentence,
  blockId,
  x,
  y,
  onClose,
}: {
  word: string;
  sentence: string;
  blockId: number | null;
  x: number;
  y: number;
  onClose: () => void;
}) {
  const [lookup, setLookup] = useState<Lookup | null>(null);
  const [state, setState] = useState<"loading" | "found" | "missing" | "error">(
    "loading",
  );
  const [error, setError] = useState<string | null>(null);
  const [inContext, setInContext] = useState<ContextualSense | null>(null);
  const [thinking, setThinking] = useState(false);

  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({
    left: x,
    top: y + GAP,
    maxHeight: 400,
    placed: false,
  });

  /**
   * Place the panel from its own measured size.
   *
   * The previous version subtracted fixed guesses (380px wide, 320px tall)
   * from the viewport, so a panel taller than the guess — a word with a dozen
   * Webster's senses, or one grown by the in-context answer — ran off the
   * bottom and got clipped.
   *
   * This measures what was actually rendered, flips above the word when there
   * is more room there, and caps the height to whatever space remains.
   * `useLayoutEffect` so it happens before paint, and it re-runs whenever the
   * content changes size.
   */
  useLayoutEffect(() => {
    const el = panelRef.current;
    if (!el) return;

    const place = () => {
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const width = el.offsetWidth;

      const spaceBelow = vh - y - GAP - MARGIN;
      const spaceAbove = y - GAP - MARGIN;
      const openUpward = spaceBelow < MIN_HEIGHT && spaceAbove > spaceBelow;

      const maxHeight = Math.max(
        MIN_HEIGHT,
        Math.min(vh * 0.6, openUpward ? spaceAbove : spaceBelow),
      );
      const height = Math.min(el.scrollHeight, maxHeight);

      setPos({
        left: clamp(x, MARGIN, Math.max(MARGIN, vw - width - MARGIN)),
        top: openUpward
          ? Math.max(MARGIN, y - GAP - height)
          : clamp(y + GAP, MARGIN, Math.max(MARGIN, vh - height - MARGIN)),
        maxHeight,
        placed: true,
      });
    };

    place();

    // The panel grows when senses load and again when the in-context answer
    // arrives, so re-place rather than measuring only once.
    const ro = new ResizeObserver(place);
    ro.observe(el);
    window.addEventListener("resize", place);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", place);
    };
  }, [x, y, state, inContext, thinking]);

  useEffect(() => {
    let cancelled = false;
    setState("loading");
    setInContext(null);
    api
      .lookUpWord(word)
      .then((r) => {
        if (cancelled) return;
        setLookup(r);
        setState(r ? "found" : "missing");
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
        setState("error");
      });
    return () => {
      cancelled = true;
    };
  }, [word]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const askContext = async () => {
    setThinking(true);
    try {
      const res = await api.wordInContext(word, sentence, blockId);
      setInContext(res.in_context);
    } catch (e) {
      setError(String(e));
    } finally {
      setThinking(false);
    }
  };

  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} />
      <div
        ref={panelRef}
        className="fixed z-50 w-[22rem] overflow-y-auto overscroll-contain rounded border border-rule bg-paper p-4 shadow-lg"
        style={{
          left: pos.left,
          top: pos.top,
          maxHeight: pos.maxHeight,
          // Hidden until measured, so the reader never sees it jump from an
          // unplaced position into its final one.
          visibility: pos.placed ? "visible" : "hidden",
        }}
      >
        <div className="flex items-baseline justify-between gap-2">
          <h3 className="font-serif text-lg">{lookup?.lemma ?? word}</h3>
          <button onClick={onClose} className="text-xs text-ink-soft hover:text-accent">
            close
          </button>
        </div>

        {/* Say when the printed word was not the headword, so the reader is
            not left wondering why they got a different word back. */}
        {lookup && lookup.resolution !== "direct" && (
          <p className="mt-0.5 text-xs text-ink-soft">
            “{word}” is {RESOLUTION_NOTE[lookup.resolution] ?? "a form of"}{" "}
            {lookup.lemma}
          </p>
        )}

        {state === "loading" && (
          <p className="mt-3 text-sm text-ink-soft">Looking up…</p>
        )}

        {state === "missing" && (
          <p className="mt-3 text-sm text-ink-soft">
            Not in Webster's 1913. Proper nouns and words coined after 1913 will
            not be there.
          </p>
        )}

        {state === "error" && (
          <p className="mt-3 text-sm text-red-600">{error}</p>
        )}

        {lookup && lookup.senses.length > 0 && (
          <>
            <ol className="mt-3 space-y-2">
              {lookup.senses.map((s, i) => (
                <li
                  key={i}
                  className={`text-sm ${
                    inContext && inContext.sense_index === i
                      ? "rounded bg-paper-dim p-2 ring-1 ring-accent/40"
                      : ""
                  }`}
                >
                  <span className="mr-1.5 text-xs text-ink-soft">{i + 1}.</span>
                  {s.pos && (
                    <span className="mr-1.5 font-serif text-xs italic text-ink-soft">
                      {s.pos}
                    </span>
                  )}
                  {s.labels.map((l) => (
                    <span
                      key={l}
                      className="mr-1.5 rounded bg-paper-dim px-1 py-0.5 text-[10px] uppercase tracking-wide text-ink-soft"
                    >
                      {l}
                    </span>
                  ))}
                  <span>{s.gloss}</span>
                </li>
              ))}
            </ol>

            <div className="mt-4 border-t border-rule pt-3">
              {thinking ? (
                <Working
                  label="Reading the sentence"
                  detail="Choosing among the senses above"
                  size={22}
                />
              ) : !inContext ? (
                <button
                  onClick={askContext}
                  className="rounded bg-accent px-3 py-1.5 text-sm text-paper"
                >
                  What does it mean here?
                </button>
              ) : (
                <div>
                  <p className="text-xs uppercase tracking-wide text-ink-soft">
                    In this sentence
                  </p>
                  {inContext.sense_index >= 0 ? (
                    <>
                      <p className="mt-1 text-sm">{inContext.plain_meaning}</p>
                      <p className="mt-1.5 text-sm text-ink-soft">
                        {inContext.why_this_sense}
                      </p>
                    </>
                  ) : (
                    /* A model allowed to say "none of these" is more useful
                       than one forced to pick. */
                    <p className="mt-1 text-sm text-ink-soft">
                      None of these senses seems to fit here.{" "}
                      {inContext.why_this_sense}
                    </p>
                  )}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </>
  );
}
