import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  TERM_STATUS_LABEL,
  type CandidateTerm,
  type Term,
  type TermStatus,
} from "../../lib/api";
import type { FlowSelection } from "../Flow";
import { EmptyState, Field, PanelHeader } from "../PanelChrome";

/**
 * The author's own vocabulary — Adler's rule 5, and the gap no dictionary can
 * fill.
 *
 * Webster's 1913 reports what *substance* meant in 1913. It cannot report what
 * Spinoza meant by it, and that is the meaning the reader actually needs. So
 * this sits beside the dictionary rather than inside it.
 *
 * The model's only job here is to point: it proposes words the author may be
 * using specially, copied from the passage and verified to be in it. The gloss
 * is always the reader's.
 */
export function TermsPanel({
  bookId,
  bookTitle,
  focusSignal,
  selection,
  passage,
  revision,
  onChanged,
  onGoTo,
}: {
  bookId: number;
  /** Named in the header, so which book these belong to is never a guess. */
  bookTitle: string;
  focusSignal: number;
  selection: FlowSelection | null;
  /** The paragraph in view, for the Suggest pass. */
  passage: string;
  revision: number;
  onChanged: () => void;
  onGoTo: (blockId: number) => void;
}) {
  const [terms, setTerms] = useState<Term[]>([]);
  const [word, setWord] = useState("");
  const [gloss, setGloss] = useState("");
  const [status, setStatus] = useState<TermStatus>("unclear");
  const [editing, setEditing] = useState<number | null>(null);
  const [candidates, setCandidates] = useState<CandidateTerm[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const glossBox = useRef<HTMLTextAreaElement>(null);
  /** Closed by default, so the panel opens on the terms rather than a form. */
  const [composing, setComposing] = useState(false);

  // Arriving from the toolbar's Term button: the word is already filled in
  // from the selection, so the cursor belongs in the gloss.
  useEffect(() => {
    if (focusSignal > 0) {
      setComposing(true);
      requestAnimationFrame(() => glossBox.current?.focus());
    }
  }, [focusSignal]);

  const refresh = useCallback(() => {
    api.listTerms(bookId).then(setTerms).catch((e) => setError(String(e)));
  }, [bookId]);

  useEffect(refresh, [refresh, revision]);

  // A selection in the text is almost always what you want to gloss.
  useEffect(() => {
    if (selection && selection.text.length < 40 && !editing) {
      setWord(selection.text.trim());
      setComposing(true);
    }
  }, [selection, editing]);

  const save = async () => {
    if (!word.trim()) return;
    setError(null);
    try {
      await api.saveTerm(
        bookId,
        word.trim(),
        gloss,
        status,
        selection?.anchors[0]?.blockId ?? null,
      );
      setWord("");
      setGloss("");
      setStatus("unclear");
      setEditing(null);
      refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const suggest = async () => {
    setBusy(true);
    setError(null);
    try {
      setCandidates(await api.suggestTerms(passage));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const known = new Set(terms.map((t) => t.term.toLowerCase()));

  return (
    <div>
      <PanelHeader
        title={`Terms · ${bookTitle}`}
        purpose="Words this author uses in a special sense. Write what he means by it — the dictionary already told you what everyone else means."
        action={
          <>
          <button
            onClick={() => setComposing((v) => !v)}
            className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
          >
            {composing ? "Close" : "Add"}
          </button>
          <button
            onClick={suggest}
            disabled={busy || !passage}
            className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
            title="Ask which words this passage is using specially. It proposes words, never meanings."
          >
            {busy ? "Looking…" : "Suggest"}
          </button>
          </>
        }
      />

      {candidates && (
        <div className="border-b border-rule bg-paper-dim px-4 py-3">
          <p className="mb-2 text-xs text-ink-soft">
            {candidates.length === 0
              ? "Nothing here looks like a technical term."
              : "Words from this passage worth pinning down. You write the meaning."}
          </p>
          <div className="flex flex-wrap gap-1.5">
            {candidates.map((c) => (
              <button
                key={c.surface_form}
                onClick={() => {
                  setWord(c.surface_form);
                  setCandidates(null);
                  setComposing(true);
                }}
                disabled={known.has(c.surface_form.toLowerCase())}
                className="rounded-full border border-rule px-2 py-0.5 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
                title={
                  c.stipulated
                    ? "The author defines this here"
                    : "Used in a special sense"
                }
              >
                {c.surface_form}
                {c.stipulated && " ·"}
              </button>
            ))}
          </div>
          <button
            onClick={() => setCandidates(null)}
            className="mt-2 text-xs text-ink-soft hover:text-accent"
          >
            dismiss
          </button>
        </div>
      )}

      <div
        className={
          composing ? "space-y-3 border-b border-rule px-4 py-3" : "hidden"
        }
      >
        <Field label="Term" asks="The word as this author uses it.">
          <input
            value={word}
            onChange={(e) => setWord(e.target.value)}
            placeholder="substance"
            className="mt-1 w-full rounded border border-rule bg-transparent px-2 py-1 text-sm outline-none focus:border-accent"
          />
        </Field>

        <Field
          label="What he means by it"
          asks="In your own words, not the dictionary's."
          example={
            "substance — whatever exists in itself and is understood through itself. For him there turns out to be exactly one."
          }
        >
          <textarea
            ref={glossBox}
            value={gloss}
            onChange={(e) => setGloss(e.target.value)}
            rows={3}
            className="mt-1 w-full resize-y rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
          />
        </Field>

        <Field
          label="Where you are with it"
          asks="Start everything at unclear. Moving one to settled is the reward."
        >
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value as TermStatus)}
            className="mt-1 w-full rounded border border-rule bg-paper px-2 py-1 text-sm outline-none focus:border-accent"
          >
            {(Object.keys(TERM_STATUS_LABEL) as TermStatus[]).map((s) => (
              <option key={s} value={s}>
                {TERM_STATUS_LABEL[s]}
              </option>
            ))}
          </select>
        </Field>

        <button
          onClick={save}
          disabled={!word.trim()}
          className="rounded bg-accent px-3 py-1 text-xs font-semibold text-paper disabled:opacity-40"
        >
          {editing ? "Save changes" : "Add term"}
        </button>
        {error && <p className="text-xs text-danger">{error}</p>}
      </div>

      {terms.length === 0 ? (
        <EmptyState
          lead="No terms yet. These are the words that carry the weight — the ones you have to pin down before the argument can be followed at all."
          example={
            <div className="text-sm">
              <p className="font-semibold">substance</p>
              <p className="mt-0.5 text-ink-soft">
                Whatever exists in itself and is understood through itself. For
                him there turns out to be exactly one.
              </p>
              <p className="mt-1 text-xs text-ink-soft">Working on it · 34 mentions</p>
            </div>
          }
        />
      ) : (
        <ul className="divide-y divide-rule">
          {terms.map((t) => (
            <li key={t.id} className="group px-4 py-3">
              <div className="flex items-baseline gap-2">
                <button
                  onClick={() => t.first_block_id && onGoTo(t.first_block_id)}
                  disabled={!t.first_block_id}
                  className="font-semibold hover:text-accent disabled:hover:text-ink"
                  title="Go to where you first met it"
                >
                  {t.term}
                </button>
                <span
                  className={`text-[0.65rem] uppercase tracking-wider ${
                    t.status === "settled"
                      ? "text-ink-soft"
                      : t.status === "working"
                        ? "text-accent"
                        : "text-warn"
                  }`}
                >
                  {TERM_STATUS_LABEL[t.status]}
                </span>
                <span className="ml-auto text-xs text-ink-soft tabular-nums">
                  {t.mentions}×
                </span>
              </div>

              <p className="mt-1 text-sm text-ink-soft">
                {t.my_gloss || (
                  <em className="opacity-70">No gloss written yet.</em>
                )}
              </p>

              {t.revisions.length > 0 && (
                <details className="mt-1.5">
                  <summary className="cursor-pointer text-xs text-ink-soft hover:text-accent">
                    {t.revisions.length} earlier reading
                    {t.revisions.length > 1 ? "s" : ""} — the sense moved
                  </summary>
                  <ul className="mt-1 space-y-1 border-l border-rule pl-3">
                    {t.revisions.map((r, i) => (
                      <li key={i} className="text-xs text-ink-soft">
                        {r.my_gloss}
                      </li>
                    ))}
                  </ul>
                </details>
              )}

              <div className="mt-1.5 flex gap-2 text-xs opacity-0 transition-opacity group-hover:opacity-100">
                <button
                  onClick={() => {
                    setEditing(t.id);
                    setWord(t.term);
                    setGloss(t.my_gloss);
                    setStatus(t.status);
                  }}
                  className="text-ink-soft hover:text-accent"
                >
                  revise
                </button>
                <button
                  onClick={async () => {
                    await api.deleteTerm(t.id);
                    refresh();
                    onChanged();
                  }}
                  className="text-ink-soft hover:text-danger"
                >
                  delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
