import { useEffect, useState } from "react";
import {
  api,
  type Block,
  type Book,
  type Exemplar,
  type Feedback,
  type ParagraphContext,
  type SpineEntry,
} from "../lib/api";
import { SentencePass } from "./SentencePass";
import { Working } from "./Spinner";

/**
 * The five steps of the method, enforced by the interface rather than
 * described in it.
 *
 * The gating is the point. You cannot ask for feedback before you have written
 * a sentence and answered the who/what question yourself, because the value of
 * the method is in doing that work — not in seeing it done.
 */
type Step = "read" | "sentences" | "core" | "write" | "check" | "built";

/** Steps that come after the sentence pass, used for the "done" styling. */
const AFTER_SENTENCES: Step[] = ["core", "write", "check", "built"];

export function MethodPanel({
  book,
  block,
  position,
  onAdvance,
  onActiveSentence,
  onSummarySaved,
}: {
  book: Book;
  block: Block | null;
  position: { index: number; total: number } | null;
  onAdvance: () => void;
  onActiveSentence?: (text: string | null) => void;
  onSummarySaved?: () => void;
}) {
  const [step, setStep] = useState<Step>("read");
  const [sentence, setSentence] = useState("");
  const [selfChecked, setSelfChecked] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [exemplar, setExemplar] = useState<Exemplar | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tooManySentences, setTooMany] = useState(false);
  const [spine, setSpine] = useState<SpineEntry[]>([]);
  const [context, setContext] = useState<ParagraphContext | null>(null);

  // A page ends where the paper runs out, not where the argument does. Fetch
  // the paragraph together with whatever finishes it on the next leaf.
  useEffect(() => {
    if (!block) {
      setContext(null);
      return;
    }
    let cancelled = false;
    api
      .paragraphContext(block.id)
      .then((c) => !cancelled && setContext(c))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [block?.id]);

  // Moving to a new paragraph resets the whole cycle.
  useEffect(() => {
    setStep("read");
    setSentence("");
    setSelfChecked(false);
    setFeedback(null);
    setExemplar(null);
    setError(null);
    setTooMany(false);
  }, [block?.id]);

  useEffect(() => {
    api.summarySpine(book.id).then(setSpine).catch(() => {});
  }, [book.id, feedback]);

  // The one-sentence rule, checked by the same Rust the backend uses rather
  // than a second implementation that could disagree with it.
  useEffect(() => {
    if (!sentence.trim()) {
      setTooMany(false);
      return;
    }
    let cancelled = false;
    const t = setTimeout(() => {
      api.isMultipleSentences(sentence).then((v) => !cancelled && setTooMany(v));
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [sentence]);

  if (!block) {
    return (
      <div className="px-6 py-10 text-sm text-ink-soft">
        <h2 className="text-base font-semibold text-ink">The method</h2>
        <ol className="mt-4 space-y-2">
          <li>1. Read one paragraph — just one.</li>
          <li>2. Work through it a sentence at a time.</li>
          <li>3. Ask what the main point or action is.</li>
          <li>4. Write a single sentence that captures it.</li>
          <li>5. Check it answers who or what.</li>
          <li>6. Move on, and let the sentences build up.</li>
        </ol>
        <p className="mt-6">
          Click a paragraph in the book to begin — either pane will do.
        </p>
      </div>
    );
  }

  const askForFeedback = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.critiqueSummary(block.id, sentence, selfChecked);
      setFeedback(res.feedback);
      setStep("built");
      onSummarySaved?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const showExemplar = async () => {
    setBusy(true);
    try {
      setExemplar(await api.getExemplar(block.id));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const words = sentence.trim().split(/\s+/).filter(Boolean).length;

  return (
    <div className="px-6 py-5">
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="text-base font-semibold">Understand this paragraph</h2>
        {position && (
          <span className="shrink-0 text-xs text-ink-soft tabular-nums">
            {position.index + 1} of {position.total}
          </span>
        )}
      </div>

      {/* Which paragraph, in its own words. The panel may sit in the far pane
          with the book collapsed, and a step wizard with no sight of its
          subject is a form, not a reading tool. */}
      <p className="prose-page mt-2 line-clamp-3 border-l-2 border-rule pl-3 text-[0.9rem] text-ink-soft">
        {block.text_norm}
      </p>

      {/* A paragraph running across a page boundary used to be repeated here
          in full, with a warning to summarise all of it — necessary when the
          column showed one page at a time. The continuous column now draws it
          whole, so the copy was a second rendering of text already on screen.
          Only the fact is worth keeping. */}
      {context?.spans_pages && (
        <p className="mt-3 text-xs text-ink-soft">
          This paragraph runs across a page boundary
          {context.continued_from_page
            ? ` from page ${context.continued_from_page}`
            : context.continues_on_page
              ? ` onto page ${context.continues_on_page}`
              : ""}
          . The column shows it whole.
        </p>
      )}

      {context?.incomplete && (
        <div className="mt-3 rounded border border-warn/40 bg-paper-dim p-3">
          <p className="text-sm">This paragraph runs onto the next page.</p>
          <p className="mt-1 text-xs text-ink-soft">
            Photograph the following page and it will be joined up
            automatically. You can summarise what you have, but you are working
            from part of a thought.
          </p>
        </div>
      )}

      <StepRow n={1} label="Read this paragraph, and only this one." done={step !== "read"}>
        {step === "read" && (
          <button
            onClick={() => setStep("sentences")}
            className="mt-2 rounded bg-accent px-3 py-1.5 text-sm text-paper"
          >
            I've read it
          </button>
        )}
      </StepRow>

      {/* The sentence pass. Compressing a long paragraph in one leap asks the
          reader to hold every clause at once; taking it a sentence at a time
          is how that becomes possible. */}
      {step !== "read" && (
        <StepRow
          n={2}
          label="Take it one sentence at a time."
          done={AFTER_SENTENCES.includes(step)}
        >
          {step === "sentences" && (
            <SentencePass
              blockId={block.id}
              onDone={() => setStep("core")}
              onActiveSentence={onActiveSentence}
            />
          )}
        </StepRow>
      )}

      {AFTER_SENTENCES.includes(step) && (
        <StepRow n={3} label="Now — what is the main point of the whole paragraph?" done={step !== "core"}>
          {step === "core" && (
            <>
              <p className="mt-1 text-sm text-ink-soft">
                Don't write yet. Just find it.
              </p>
              <button
                onClick={() => setStep("write")}
                className="mt-2 rounded bg-accent px-3 py-1.5 text-sm text-paper"
              >
                I have it
              </button>
            </>
          )}
        </StepRow>
      )}

      {(step === "write" || step === "check" || step === "built") && (
        <StepRow n={4} label="Write one sentence that captures it." done={step !== "write"}>
          <textarea
            value={sentence}
            onChange={(e) => setSentence(e.target.value)}
            rows={3}
            placeholder="One sentence."
            className="mt-2 w-full resize-none rounded border border-rule bg-transparent px-3 py-2 text-sm outline-none focus:border-accent"
          />
          <div className="mt-1 flex items-center justify-between text-xs">
            <span className={tooManySentences ? "text-warn" : "text-ink-soft"}>
              {tooManySentences
                ? "That looks like more than one sentence — compress it further."
                : `${words} word${words === 1 ? "" : "s"}`}
            </span>
            {step === "write" && (
              <button
                onClick={() => setStep("check")}
                disabled={!sentence.trim()}
                className="rounded bg-accent px-3 py-1 text-paper disabled:opacity-40"
              >
                Next
              </button>
            )}
          </div>
        </StepRow>
      )}

      {(step === "check" || step === "built") && (
        <StepRow n={5} label="Does your sentence say who or what it is about?" done={step === "built"}>
          {/* The reader's own check, deliberately placed before the model is
              ever consulted. */}
          <label className="mt-2 flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={selfChecked}
              onChange={(e) => setSelfChecked(e.target.checked)}
              className="mt-1"
            />
            <span>
              Yes — I can point to the who or what in my sentence.
            </span>
          </label>
          {step === "check" && busy && (
            <div className="mt-3 rounded border border-rule bg-paper-dim p-3">
              <Working
                label="Reading your sentence against the paragraph"
                detail="On your machine — nothing is sent anywhere"
              />
            </div>
          )}

          {step === "check" && !busy && (
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <button
                onClick={askForFeedback}
                disabled={!sentence.trim()}
                className="rounded bg-accent px-3 py-1.5 text-sm text-paper disabled:opacity-40"
              >
                Check my understanding
              </button>
              <button
                onClick={() => {
                  api
                    .critiqueSummary(block.id, sentence, selfChecked)
                    .then(() => onSummarySaved?.())
                    .catch(() => {});
                  onAdvance();
                }}
                className="rounded border border-rule px-3 py-1.5 text-sm hover:border-accent"
              >
                Save and move on
              </button>
            </div>
          )}
        </StepRow>
      )}

      {error && <p className="mt-4 text-sm text-danger">{error}</p>}

      {feedback && (
        <div className="mt-5 rounded border border-rule bg-paper-dim p-4">
          <div className="flex items-center gap-2">
            <VerdictDot verdict={feedback.verdict} />
            <p className="text-sm font-medium">{feedback.headline}</p>
          </div>

          {feedback.notes.length > 0 && (
            <ul className="mt-2 space-y-1 text-sm text-ink-soft">
              {feedback.notes.map((n, i) => (
                <li key={i}>· {n}</li>
              ))}
            </ul>
          )}
          {feedback.problem && (
            <p className="mt-2 text-sm text-ink-soft">{feedback.problem}</p>
          )}

          <p className="mt-3 border-l-2 border-accent pl-3 text-sm italic">
            {feedback.steering_question}
          </p>

          <div className="mt-4 flex flex-wrap gap-2 text-sm">
            <button
              onClick={() => {
                setStep("write");
                setFeedback(null);
              }}
              className="rounded border border-rule px-3 py-1.5 hover:border-accent"
            >
              Try again
            </button>
            <button
              onClick={onAdvance}
              className="rounded bg-accent px-3 py-1.5 text-paper"
            >
              Next paragraph
            </button>
            {!exemplar && !busy && (
              <button
                onClick={showExemplar}
                className="rounded border border-rule px-3 py-1.5 text-ink-soft hover:border-accent"
                title="Only shown when you ask for it"
              >
                Show me one
              </button>
            )}
          </div>

          {busy && (
            <div className="mt-3">
              <Working label="Working out an example" size={22} />
            </div>
          )}

          {/* An exemplar is a separate, deliberate request — never a side
              effect of asking for feedback. */}
          {exemplar && (
            <div className="mt-4 rounded border border-dashed border-rule p-3">
              <p className="text-xs uppercase tracking-wide text-ink-soft">
                One possible sentence
              </p>
              <p className="mt-1 text-sm">{exemplar.sentence}</p>
              <p className="mt-2 text-sm text-ink-soft">{exemplar.reasoning}</p>
            </div>
          )}
        </div>
      )}

      {spine.length > 0 && (
        <div className="mt-8 border-t border-rule pt-4">
          <h3 className="text-sm font-semibold">
            6. What you have built so far
          </h3>
          <p className="mt-1 text-xs text-ink-soft">
            Your sentences in reading order. This is the book, in your own words.
          </p>
          <ol className="mt-3 space-y-1.5 text-sm">
            {spine.map((s) => (
              <li
                key={s.block_id}
                className={`flex gap-2 ${s.block_id === block.id ? "text-accent" : "text-ink-soft"}`}
              >
                <span className="shrink-0 tabular-nums opacity-60">
                  p{s.page_no}
                </span>
                <span>{s.sentence}</span>
              </li>
            ))}
          </ol>
        </div>
      )}
    </div>
  );
}

function StepRow({
  n,
  label,
  done,
  children,
}: {
  n: number;
  label: string;
  done: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div className={`mt-5 ${done ? "opacity-60" : ""}`}>
      <div className="flex gap-2">
        <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-rule text-xs">
          {n}
        </span>
        <div className="flex-1">
          <p className="text-sm font-medium">{label}</p>
          {children}
        </div>
      </div>
    </div>
  );
}

function VerdictDot({ verdict }: { verdict: Feedback["verdict"] }) {
  const color =
    verdict === "on_target"
      ? "bg-ok"
      : verdict === "partial"
        ? "bg-warn"
        : "bg-danger";
  return <span className={`inline-block h-2 w-2 rounded-full ${color}`} />;
}
