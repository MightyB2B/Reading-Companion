import { useEffect, useState, type ReactNode } from "react";
import { api, type MoveInfo } from "../lib/api";

/**
 * The furniture every study panel wears.
 *
 * A panel called "Terms" with an empty list and a plus button teaches
 * nothing. The whole point of this application is that it knows a method the
 * reader is still learning, so the method has to be visible in the furniture
 * rather than in a manual they would have to go and find.
 */

/** A column holding several tools at once, switched like browser tabs. */
export function TabGroup({
  tabs,
  active,
  onSelect,
  onClose,
  children,
}: {
  tabs: { id: string; label: string }[];
  active: string;
  onSelect: (id: string) => void;
  /** Omitted for a group whose tabs are fixed. */
  onClose?: (id: string) => void;
  children: ReactNode;
}) {
  return (
    <div className="flex min-h-0 flex-col">
      <div className="tabstrip" role="tablist">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={t.id === active}
            onClick={() => onSelect(t.id)}
            onAuxClick={(e) => {
              // Middle click closes, as it does everywhere else with tabs.
              if (e.button === 1) onClose?.(t.id);
            }}
            className="tab"
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
    </div>
  );
}

/**
 * The purpose line: one sentence under the tab title, always visible.
 *
 * Not a tooltip. A tooltip is a thing you have to already suspect exists.
 */
export function PanelHeader({
  title,
  purpose,
  action,
}: {
  title: string;
  purpose: string;
  action?: ReactNode;
}) {
  return (
    <div className="border-b border-rule px-4 py-3">
      <div className="flex items-baseline gap-3">
        <h2 className="text-sm font-bold">{title}</h2>
        <div className="ml-auto flex items-center gap-2">{action}</div>
      </div>
      <p className="mt-1 max-w-[52ch] text-xs leading-relaxed text-ink-soft">
        {purpose}
      </p>
    </div>
  );
}

/**
 * What an empty panel says.
 *
 * "No terms yet" is the moment a reader most needs help and the moment most
 * software gives them least. So an empty panel shows one worked entry
 * instead, and a single button that starts one.
 */
export function EmptyState({
  lead,
  example,
  children,
}: {
  lead: string;
  /** A worked entry, shown rather than described. */
  example: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="px-4 py-6">
      <p className="max-w-[46ch] text-sm text-ink-soft">{lead}</p>
      <div className="mt-4 rounded border border-dashed border-rule bg-paper-dim p-3">
        <p className="mb-2 text-[0.65rem] uppercase tracking-wider text-ink-soft/70">
          For example
        </p>
        {example}
      </div>
      {children && <div className="mt-4">{children}</div>}
    </div>
  );
}

/** A field label with the question the field is actually asking. */
export function Field({
  label,
  asks,
  example,
  children,
}: {
  label: string;
  asks: string;
  /** Revealed on request, so the prompt does not become a wall of text. */
  example?: string;
  children: ReactNode;
}) {
  const [shown, setShown] = useState(false);
  return (
    <label className="block">
      <span className="text-xs font-semibold">{label}</span>
      <span className="mt-0.5 block text-xs text-ink-soft">{asks}</span>
      {children}
      {example && (
        <span className="mt-1 block text-xs">
          <button
            type="button"
            onClick={() => setShown((v) => !v)}
            className="text-ink-soft underline decoration-dotted underline-offset-2 hover:text-accent"
          >
            {shown ? "hide the example" : "show me one"}
          </button>
          {shown && (
            <span className="mt-1 block rounded bg-paper-dim px-2 py-1.5 text-ink-soft">
              {example}
            </span>
          )}
        </span>
      )}
    </label>
  );
}

/**
 * The nine moves, with what each is and the tell that gives it away.
 *
 * Loaded once from the backend rather than duplicated here: the definitions
 * are also what the classifier is prompted against, and two copies of a
 * vocabulary drift.
 */
export function useMoveCatalogue(): MoveInfo[] {
  const [moves, setMoves] = useState<MoveInfo[]>([]);
  useEffect(() => {
    api.moveCatalogue().then(setMoves).catch(() => setMoves([]));
  }, []);
  return moves;
}

/** A move as a chip, carrying its own explanation on hover. */
export function MoveChip({
  move,
  info,
  selected,
  onClick,
}: {
  move: string;
  info?: MoveInfo;
  selected?: boolean;
  onClick?: () => void;
}) {
  const title = info ? `${info.what}\n\nHow to spot it: ${info.tell}` : move;
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={`mark mark-${move} rounded-full border px-2 py-0.5 text-xs font-semibold ${
        selected ? "ring-1 ring-current" : ""
      }`}
      style={{ borderColor: "currentColor" }}
    >
      {move}
    </button>
  );
}

/** The whole vocabulary, as a reference table. */
export function MoveGlossary({ moves }: { moves: MoveInfo[] }) {
  return (
    <div className="px-4 py-3">
      <p className="mb-3 max-w-[52ch] text-xs text-ink-soft">
        Nine things a passage can be doing. The third column is what gives each
        one away on the page.
      </p>
      <dl className="space-y-3">
        {moves.map((m) => (
          <div key={m.label} className="grid grid-cols-[6rem_1fr] gap-3">
            <dt>
              <MoveChip move={m.label} info={m} />
            </dt>
            <dd className="text-xs leading-relaxed">
              {m.what}
              <span className="mt-0.5 block text-ink-soft">{m.tell}</span>
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}
