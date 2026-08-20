import { useCallback, useEffect, useState } from "react";
import {
  api,
  GAP_LABEL,
  VERDICT_LABEL,
  type Argument,
  type ArgumentLink,
  type CharityCheck,
  type NewPremise,
  type SupportCheck,
} from "../../lib/api";
import { pageLabel } from "../../lib/selection";
import type { FlowSelection } from "../Flow";
import { EmptyState, Field, PanelHeader } from "../PanelChrome";

/**
 * Taking an argument apart, in standard form.
 *
 * The conclusion is one field and the premises are rows, which is what lets
 * the map be *rendered* from the data rather than drawn by hand. A drawn map
 * is a second copy of the argument, and second copies drift away from the
 * text they came from.
 *
 * The model checks; it never writes. `check_support` names the kind of gap and
 * asks a question — it has no field in which a missing premise could arrive.
 */
export function ArgumentsPanel({
  bookId,
  bookTitle,
  selection,
  revision,
  onChanged,
  onGoTo,
}: {
  bookId: number;
  /** Named in the header, so which book these belong to is never a guess. */
  bookTitle: string;
  selection: FlowSelection | null;
  revision: number;
  onChanged: () => void;
  onGoTo: (blockId: number) => void;
}) {
  const [args, setArgs] = useState<Argument[]>([]);
  const [openId, setOpenId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.listArguments(bookId).then(setArgs).catch((e) => setError(String(e)));
  }, [bookId]);

  useEffect(refresh, [refresh, revision]);

  const start = async () => {
    const id = await api.createArgument(
      bookId,
      pageLabel(selection?.pageNos ?? []) || "Untitled",
      selection?.anchors[0]?.blockId ?? null,
    );
    refresh();
    setOpenId(id);
  };

  const open = args.find((a) => a.id === openId);

  return (
    <div>
      <PanelHeader
        title={`Arguments · ${bookTitle}`}
        purpose="Take an argument apart: what it concludes, and the reasons given for it. Works on one paragraph or on thirty pages."
        action={
          <button
            onClick={start}
            className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
          >
            New
          </button>
        }
      />
      {error && <p className="px-4 py-2 text-xs text-danger">{error}</p>}

      {open ? (
        <ArgumentEditor
          argument={open}
          siblings={args.filter((a) => a.id !== open.id)}
          onClose={() => setOpenId(null)}
          onSaved={() => {
            refresh();
            onChanged();
          }}
          onGoTo={onGoTo}
          selection={selection}
        />
      ) : args.length === 0 ? (
        <EmptyState
          lead="Nothing reconstructed yet. Put an argument in standard form and the gaps in it stop being invisible."
          example={
            <div className="text-sm">
              <p className="text-xs uppercase tracking-wider text-ink-soft">
                Premises
              </p>
              <p>1. Distinct things must be distinguished by something.</p>
              <p>2. Substances can only be distinguished by attributes.</p>
              <p className="mt-1 text-ink-soft">
                3. <em>(unstated)</em> Modes cannot distinguish substances.
              </p>
              <p className="mt-2 text-xs uppercase tracking-wider text-ink-soft">
                Therefore
              </p>
              <p>No two substances can share an attribute.</p>
            </div>
          }
        >
          <button
            onClick={start}
            className="rounded bg-accent px-3 py-1.5 text-xs font-semibold text-paper"
          >
            Start one
          </button>
        </EmptyState>
      ) : (
        <ul className="divide-y divide-rule">
          {args.map((a) => (
            <li key={a.id}>
              <button
                onClick={() => setOpenId(a.id)}
                className="block w-full px-4 py-3 text-left hover:bg-paper-dim"
              >
                <div className="flex items-baseline gap-2">
                  <span className="text-sm font-semibold">
                    {a.label || "Untitled"}
                  </span>
                  <span className="ml-auto text-[0.65rem] uppercase tracking-wider text-ink-soft">
                    {a.premises.length} premise
                    {a.premises.length === 1 ? "" : "s"}
                  </span>
                </div>
                <p className="mt-0.5 line-clamp-2 text-sm text-ink-soft">
                  {a.conclusion || "No conclusion written yet."}
                </p>
                {a.verdict !== "undecided" && (
                  <p className="mt-1 text-xs text-accent">
                    {VERDICT_LABEL[a.verdict] ?? a.verdict}
                  </p>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** The six stages of reconstruction, as an editable form. */
const LINK_ROLES: { value: string; label: string; hint: string }[] = [
  { value: "supports", label: "supports", hint: "It is a reason for this one." },
  { value: "objects_to", label: "objects to", hint: "It argues against this one." },
  { value: "replies_to", label: "replies to", hint: "It answers an objection." },
];

function ArgumentEditor({
  argument,
  siblings,
  onClose,
  onSaved,
  onGoTo,
  selection,
}: {
  argument: Argument;
  /** The book's other arguments, for linking this one to them. */
  siblings: Argument[];
  onClose: () => void;
  onSaved: () => void;
  onGoTo: (blockId: number) => void;
  selection: FlowSelection | null;
}) {
  const [label, setLabel] = useState(argument.label);
  const [conclusion, setConclusion] = useState(argument.conclusion);
  const [notes, setNotes] = useState(argument.notes);
  const [verdict, setVerdict] = useState(argument.verdict);
  const [gap, setGap] = useState(argument.gap);
  const [premises, setPremises] = useState<NewPremise[]>(
    argument.premises.map((p) => ({
      text: p.text,
      implicit: p.implicit,
      source_block_id: p.source_block_id,
      char_start: p.char_start,
      char_end: p.char_end,
    })),
  );
  const [support, setSupport] = useState<SupportCheck | null>(null);
  const [charity, setCharity] = useState<CharityCheck | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [linkTo, setLinkTo] = useState("");
  const [linkRole, setLinkRole] = useState("supports");

  const save = async () => {
    setError(null);
    try {
      await api.saveArgument({
        argumentId: argument.id,
        label,
        conclusion,
        verdict,
        gap,
        notes,
        premises: premises.filter((p) => p.text.trim()),
      });
      onSaved();
    } catch (e) {
      setError(String(e));
    }
  };

  const addPremise = (implicit: boolean) =>
    setPremises((ps) => [
      ...ps,
      {
        text: implicit ? "" : (selection?.text ?? ""),
        implicit,
        source_block_id: implicit ? null : (selection?.anchors[0]?.blockId ?? null),
        char_start: implicit ? null : (selection?.anchors[0]?.charStart ?? null),
        char_end: implicit ? null : (selection?.anchors[0]?.charEnd ?? null),
      },
    ]);

  const run = async (which: "support" | "charity") => {
    const texts = premises.map((p) => p.text).filter((t) => t.trim());
    if (!texts.length || !conclusion.trim()) {
      setError("Write a conclusion and at least one premise first.");
      return;
    }
    setBusy(which);
    setError(null);
    try {
      if (which === "support") {
        const r = await api.checkSupport(conclusion, texts);
        setSupport(r);
        setGap(r.gap);
      } else {
        setCharity(await api.checkCharity(conclusion, texts));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-4 px-4 py-3">
      <button onClick={onClose} className="text-xs text-ink-soft hover:text-accent">
        ← all arguments
      </button>

      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        placeholder="What to call this"
        className="w-full border-b border-rule bg-transparent pb-1 text-sm font-semibold outline-none focus:border-accent"
      />

      <Field
        label="Conclusion"
        asks="What is this passage trying to get you to accept?"
        example="No two substances can share a nature."
      >
        <textarea
          value={conclusion}
          onChange={(e) => setConclusion(e.target.value)}
          rows={2}
          className="mt-1 w-full resize-y rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
        />
      </Field>

      <div>
        <p className="text-xs font-semibold">Premises</p>
        <p className="mt-0.5 text-xs text-ink-soft">
          The reasons given for it. Look for <em>since</em>, <em>because</em>,{" "}
          <em>for</em>.
        </p>

        <ol className="mt-2 space-y-2">
          {premises.map((p, i) => (
            <li key={i} className="flex gap-2">
              <span className="pt-1.5 text-xs tabular-nums text-ink-soft">
                {i + 1}.
              </span>
              <div className="flex-1">
                <textarea
                  value={p.text}
                  onChange={(e) =>
                    setPremises((ps) =>
                      ps.map((q, j) =>
                        j === i ? { ...q, text: e.target.value } : q,
                      ),
                    )
                  }
                  rows={2}
                  placeholder={
                    p.implicit
                      ? "Something the argument needs but never says."
                      : "A reason given for the conclusion."
                  }
                  className={`w-full resize-y rounded border bg-transparent px-2 py-1 text-sm outline-none focus:border-accent ${
                    charity?.weakest_premise === i + 1
                      ? "border-warn"
                      : "border-rule"
                  }`}
                />
                <div className="mt-0.5 flex items-center gap-2 text-xs text-ink-soft">
                  {p.implicit && <span className="italic">unstated — yours</span>}
                  {p.source_block_id && (
                    <button
                      onClick={() => onGoTo(p.source_block_id!)}
                      className="hover:text-accent"
                    >
                      in the text
                    </button>
                  )}
                  <button
                    onClick={() =>
                      setPremises((ps) => ps.filter((_, j) => j !== i))
                    }
                    className="ml-auto hover:text-danger"
                  >
                    remove
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ol>

        <div className="mt-2 flex gap-2">
          <button
            onClick={() => addPremise(false)}
            className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
            title={
              selection
                ? "Add the selected text as a premise"
                : "Add a premise from the author"
            }
          >
            {selection ? "Add selection" : "Add premise"}
          </button>
          <button
            onClick={() => addPremise(true)}
            className="rounded border border-dashed border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
            title="A premise the author needs but never states"
          >
            Add unstated
          </button>
        </div>
      </div>

      <div className="space-y-2 rounded border border-rule bg-paper-dim p-3">
        <div className="flex flex-wrap gap-2">
          <button
            onClick={() => run("support")}
            disabled={busy !== null}
            className="rounded border border-rule bg-paper px-2 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {busy === "support" ? "Checking…" : "Does it follow?"}
          </button>
          <button
            onClick={() => run("charity")}
            disabled={busy !== null}
            className="rounded border border-rule bg-paper px-2 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {busy === "charity" ? "Checking…" : "Am I being fair?"}
          </button>
        </div>

        {support && (
          <div className="text-xs">
            <p className={support.reaches_conclusion ? "text-ink" : "text-warn"}>
              {support.reaches_conclusion
                ? "The premises reach the conclusion."
                : (GAP_LABEL[support.gap] ?? support.gap)}
            </p>
            <p className="mt-1 italic text-ink-soft">
              {support.steering_question}
            </p>
          </div>
        )}
        {charity && (
          <div className="text-xs">
            <p className="text-ink-soft">
              {charity.weakest_premise > 0
                ? `Premise ${charity.weakest_premise} is carrying the least weight.`
                : "No premise stands out as weak."}
              {charity.stronger_available &&
                " A more plausible one would do the same job."}
            </p>
            <p className="mt-1 italic text-ink-soft">
              {charity.steering_question}
            </p>
          </div>
        )}
        <p className="text-[0.65rem] leading-relaxed text-ink-soft">
          These name what is missing. They never supply it — that part is
          yours.
        </p>
      </div>

      <Field label="Your verdict" asks="What you make of it, once it is laid out.">
        <select
          value={verdict}
          onChange={(e) => setVerdict(e.target.value)}
          className="mt-1 w-full rounded border border-rule bg-paper px-2 py-1 text-sm outline-none focus:border-accent"
        >
          {Object.entries(VERDICT_LABEL).map(([k, v]) => (
            <option key={k} value={k}>
              {v}
            </option>
          ))}
        </select>
      </Field>

      <Field label="Why" asks="Anything worth saying about the reconstruction.">
        <textarea
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          rows={3}
          className="mt-1 w-full resize-y rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
        />
      </Field>

      {error && <p className="text-xs text-danger">{error}</p>}

      <div className="flex gap-2">
        <button
          onClick={save}
          className="rounded bg-accent px-3 py-1.5 text-xs font-semibold text-paper"
        >
          Save
        </button>
        <button
          onClick={async () => {
            await api.deleteArgument(argument.id);
            onSaved();
            onClose();
          }}
          className="ml-auto text-xs text-ink-soft hover:text-danger"
        >
          delete this argument
        </button>
      </div>

      {/* Arguments rarely stand alone: one supports another, a third objects,
          a fourth answers the objection. Recording that is what turns a list
          of reconstructions into the shape of a chapter. */}
      <div className="rounded border border-rule p-3">
        <p className="text-xs font-semibold">Related arguments</p>
        <p className="mt-0.5 text-xs text-ink-soft">
          How this one stands to the others you have reconstructed.
        </p>

        {argument.links.length > 0 && (
          <ul className="mt-2 space-y-1">
            {argument.links.map((l) => (
              <li key={l.id} className="flex items-start gap-2 text-xs">
                <span className="flex-none text-ink-soft">
                  {LINK_ROLES.find((r) => r.value === l.role)?.label ?? l.role}
                </span>
                <span className="min-w-0 flex-1 truncate">
                  {l.child_label || l.child_conclusion || "Untitled"}
                </span>
                <button
                  onClick={async () => {
                    await api.unlinkArguments(l.id);
                    onSaved();
                  }}
                  title="Remove this link"
                  className="flex-none text-ink-soft hover:text-danger"
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        )}

        {siblings.length === 0 ? (
          <p className="mt-2 text-xs text-ink-soft">
            Reconstruct another argument and you can link them.
          </p>
        ) : (
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <select
              value={linkRole}
              onChange={(e) => setLinkRole(e.target.value)}
              className="rounded border border-rule bg-paper px-1.5 py-1 text-xs outline-none focus:border-accent"
              title={LINK_ROLES.find((r) => r.value === linkRole)?.hint}
            >
              {LINK_ROLES.map((r) => (
                <option key={r.value} value={r.value}>
                  {r.label}
                </option>
              ))}
            </select>
            <select
              value={linkTo}
              onChange={(e) => setLinkTo(e.target.value)}
              className="min-w-0 flex-1 rounded border border-rule bg-paper px-1.5 py-1 text-xs outline-none focus:border-accent"
            >
              <option value="">choose an argument…</option>
              {siblings.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.label || a.conclusion.slice(0, 50) || "Untitled"}
                </option>
              ))}
            </select>
            <button
              onClick={async () => {
                if (!linkTo) return;
                setError(null);
                try {
                  await api.linkArguments(argument.id, Number(linkTo), linkRole);
                  setLinkTo("");
                  onSaved();
                } catch (e) {
                  setError(String(e));
                }
              }}
              disabled={!linkTo}
              className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
            >
              Link
            </button>
          </div>
        )}
      </div>

      <ArgumentMap
        conclusion={conclusion}
        premises={premises}
        links={argument.links}
        weakest={charity?.weakest_premise ?? 0}
      />
    </div>
  );
}

/**
 * The map, rendered from the rows rather than drawn.
 *
 * Deliberately not a canvas. A free-draw map is a second copy of the argument
 * and drifts from the text it came from; this one cannot, because there is
 * nothing to keep in sync.
 */
function ArgumentMap({
  conclusion,
  premises,
  links,
  weakest,
}: {
  conclusion: string;
  premises: NewPremise[];
  links: ArgumentLink[];
  weakest: number;
}) {
  const live = premises.filter((p) => p.text.trim());
  if (!conclusion.trim() || live.length === 0) return null;

  return (
    <div className="rounded border border-rule p-3">
      <p className="mb-3 text-[0.65rem] uppercase tracking-wider text-ink-soft">
        In standard form
      </p>
      <div className="flex flex-col items-center gap-1">
        <div className="flex flex-wrap justify-center gap-2">
          {live.map((p, i) => (
            <div
              key={i}
              className={`max-w-[13rem] rounded border px-2 py-1 text-xs ${
                p.implicit ? "border-dashed" : ""
              } ${
                weakest === i + 1
                  ? "border-warn text-warn"
                  : "border-rule"
              }`}
            >
              {p.text}
            </div>
          ))}
        </div>
        <div className="h-4 w-px bg-rule" />
        <div className="text-xs text-ink-soft">therefore</div>
        <div className="h-4 w-px bg-rule" />
        <div className="max-w-[20rem] rounded border-2 border-accent px-3 py-1.5 text-center text-xs font-semibold">
          {conclusion}
        </div>

        {links.length > 0 && (
          <>
            <div className="h-4 w-px bg-rule" />
            <div className="flex flex-wrap justify-center gap-2">
              {links.map((l) => (
                <div
                  key={l.id}
                  className={`max-w-[13rem] rounded border px-2 py-1 text-xs ${
                    l.role === "objects_to"
                      ? "border-[var(--color-move-objection)] text-[var(--color-move-objection)]"
                      : l.role === "replies_to"
                        ? "border-[var(--color-move-reply)] text-[var(--color-move-reply)]"
                        : "border-[var(--color-move-premise)] text-[var(--color-move-premise)]"
                  }`}
                >
                  <span className="block text-[0.6rem] uppercase tracking-wider opacity-80">
                    {l.role.replace("_", " ")}
                  </span>
                  {l.child_label || l.child_conclusion || "Untitled"}
                </div>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
