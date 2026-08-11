import { useEffect, useState } from "react";
import { api, type SentenceView } from "../lib/api";

/**
 * The sentence pass: working through a paragraph one sentence at a time
 * before compressing the whole thing.
 *
 * This is where a long periodic sentence in an older book actually becomes
 * legible. Compressing a Hodge or a Gibbon paragraph in one leap asks the
 * reader to hold four subordinate clauses in their head at once, which is
 * precisely the thing they cannot yet do — that being why they are using this
 * app.
 *
 * No coaching here, deliberately. These are the reader's own working notes;
 * the model is brought in once, at the paragraph level, where the judgement
 * actually matters.
 */
export function SentencePass({
  blockId,
  onDone,
  onActiveSentence,
}: {
  blockId: number;
  onDone: () => void;
  /** Reports the sentence being worked on, so it can be lit up on the page. */
  onActiveSentence?: (text: string | null) => void;
}) {
  const [sentences, setSentences] = useState<SentenceView[]>([]);
  const [active, setActive] = useState(0);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.blockSentences(blockId).then((s) => {
      if (cancelled) return;
      setSentences(s);
      setActive(0);
      setDraft(s[0]?.note ?? "");
    });
    return () => {
      cancelled = true;
    };
  }, [blockId]);

  const go = (index: number) => {
    setActive(index);
    setDraft(sentences[index]?.note ?? "");
  };

  // Keep the page in step with the workspace: the reader should never have to
  // hunt for the sentence they are being asked about.
  useEffect(() => {
    onActiveSentence?.(sentences[active]?.text ?? null);
  }, [sentences, active, onActiveSentence]);

  // Clear the highlight when the pass is left.
  useEffect(() => () => onActiveSentence?.(null), [onActiveSentence]);

  const saveAndAdvance = async () => {
    const text = draft.trim();
    if (text) {
      setSaving(true);
      try {
        await api.saveSentenceNote(blockId, active, text);
        setSentences((s) =>
          s.map((x) => (x.ordinal === active ? { ...x, note: text } : x)),
        );
      } finally {
        setSaving(false);
      }
    }
    if (active < sentences.length - 1) {
      go(active + 1);
    } else {
      onDone();
    }
  };

  if (sentences.length === 0) {
    return <p className="mt-2 text-sm text-ink-soft">Loading sentences…</p>;
  }

  // A single-sentence paragraph needs no pass of its own.
  if (sentences.length === 1) {
    return (
      <div className="mt-2">
        <p className="text-sm text-ink-soft">
          This paragraph is a single sentence, so there is nothing to break down.
        </p>
        <button
          onClick={onDone}
          className="mt-2 rounded bg-accent px-3 py-1.5 text-sm text-paper"
        >
          Go on
        </button>
      </div>
    );
  }

  const done = sentences.filter((s) => s.note).length;

  return (
    <div className="mt-2">
      <div className="flex items-center justify-between text-xs text-ink-soft">
        <span>
          Sentence {active + 1} of {sentences.length}
        </span>
        <span>{done} noted</span>
      </div>

      {/* One dash per sentence: progress without a progress bar. */}
      <div className="mt-1.5 flex gap-1">
        {sentences.map((s, i) => (
          <button
            key={s.ordinal}
            onClick={() => go(i)}
            title={s.text}
            className={`h-1 flex-1 rounded-full transition-colors ${
              i === active
                ? "bg-accent"
                : s.note
                  ? "bg-accent/40"
                  : "bg-rule"
            }`}
          />
        ))}
      </div>

      <blockquote className="prose-page mt-3 border-l-2 border-accent/40 pl-3 text-[0.95rem]">
        {sentences[active].text}
      </blockquote>

      <textarea
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        rows={2}
        placeholder="What is this sentence doing?"
        className="mt-2 w-full resize-none rounded border border-rule bg-transparent px-3 py-2 text-sm outline-none focus:border-accent"
      />

      <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
        <button
          onClick={saveAndAdvance}
          disabled={saving}
          className="rounded bg-accent px-3 py-1.5 text-paper disabled:opacity-40"
        >
          {active < sentences.length - 1 ? "Next sentence" : "Done — sum it up"}
        </button>
        {active > 0 && (
          <button
            onClick={() => go(active - 1)}
            className="rounded border border-rule px-3 py-1.5 hover:border-accent"
          >
            Back
          </button>
        )}
        {/* Not every paragraph needs the slow path. */}
        <button
          onClick={onDone}
          className="text-xs text-ink-soft hover:text-accent"
        >
          skip to the paragraph
        </button>
      </div>
    </div>
  );
}
