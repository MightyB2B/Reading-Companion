import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  api,
  sentenceAround,
  type Annotations,
  type BlockKind,
  type FlowBlock,
  type Move,
  type Note,
  type PageGap,
  type StoredRef,
  type TermMention,
} from "../lib/api";
import { anchorsFromRange, type FlowSelection } from "../lib/selection";

/**
 * The book as one continuous column.
 *
 * Pages are a fact about the paper, not about the argument. They are recorded
 * and citable — the marker hangs in the margin beside the text, the way a
 * scholarly edition prints the pagination of the edition it is keyed to — but
 * they never interrupt a sentence, and a paragraph that runs across a boundary
 * is drawn as one paragraph.
 *
 * Rendering is windowed. A 282-page EPUB is several thousand blocks, and
 * mounting them all makes scrolling stutter on the first pass and then stay
 * stuttering. Only the blocks near the viewport are real; the rest are
 * replaced by two spacers whose heights are measured from what has already
 * been seen.
 */

/** Every classification a paragraph can be given by hand. */
const BLOCK_KINDS: BlockKind[] = [
  "paragraph",
  "heading",
  "quote",
  "footnote",
  "caption",
];

/** How many blocks outside the viewport stay mounted. */
const OVERSCAN = 12;

/** Assumed height of a block never yet measured, in pixels. */
const ESTIMATED_HEIGHT = 120;

export interface FlowHandle {
  scrollToBlock: (blockId: number) => void;
}

/** A word the reader double-clicked, with the sentence it sits in. */
export interface WordHit {
  word: string;
  sentence: string;
  /** Null when recalled from the Words panel rather than clicked in the text. */
  blockId: number | null;
  x: number;
  y: number;
}

export type { FlowSelection };

/**
 * Something in the text the reader asked about.
 *
 * Answered in the Context panel rather than in a card floating over the page.
 * A popup covers the words you are reading at the moment you most want to
 * compare them with what it says, and it cannot be kept open while you scroll.
 */
export type ContextTarget =
  | { kind: "term"; term: TermMention }
  | { kind: "notes"; notes: Note[] }
  | { kind: "reference"; reference: StoredRef };

export function Flow({
  bookId,
  blocks,
  gaps,
  loading,
  activeBlock,
  selection,
  onActivate,
  onSelect,
  onContext,
  onLookUpWord,
  onOpenPage,
  onBlockChanged,
  onTopBlockChange,
  annotationRevision,
  suggestions,
  corresponding,
  registerHandle,
}: {
  bookId: number;
  blocks: FlowBlock[];
  gaps: PageGap[];
  /** The book's text has not arrived yet. */
  loading: boolean;
  activeBlock: number | null;
  /** The standing selection, drawn by us rather than by the browser. */
  selection: FlowSelection | null;
  onActivate: (blockId: number) => void;
  onSelect: (sel: FlowSelection | null) => void;
  /** Marked text was clicked; the Context panel takes it from here. */
  onContext: (target: ContextTarget) => void;
  /** Double-clicking a word asks the dictionary about it. */
  onLookUpWord: (hit: WordHit) => void;
  /** Open the photograph behind a page — for checking a transcription. */
  onOpenPage: (pageId: number) => void;
  /** A paragraph was corrected, reclassified or removed; reload the book. */
  onBlockChanged: () => void;
  onTopBlockChange: (block: FlowBlock | null) => void;
  /** Bumped by the caller when notes, terms or references change. */
  annotationRevision: number;
  /** Un-saved Suggest results, drawn dotted against your own solid marks. */
  suggestions: Map<number, Move>;
  /** Repeated words in the active passage, keyed by block, then span. */
  corresponding: Map<number, { start: number; end: number; group: number }[]>;
  registerHandle?: (h: FlowHandle) => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [range, setRange] = useState({ start: 0, end: 40 });
  const [annotations, setAnnotations] = useState<Annotations>({
    notes: [],
    terms: [],
    refs: [],
  });
  const [annotationsFailed, setAnnotationsFailed] = useState<string | null>(null);

  // Measured block heights, so the spacers above and below the window are the
  // right size and scrolling back does not jump.
  const heights = useRef(new Map<number, number>());
  const nodes = useRef(new Map<number, HTMLElement>());

  const gapByPage = useMemo(() => {
    const m = new Map<number, PageGap>();
    for (const g of gaps) m.set(g.page_id, g);
    return m;
  }, [gaps]);

  // ---- windowing --------------------------------------------------------

  const recompute = useCallback(() => {
    const el = scroller.current;
    if (!el || blocks.length === 0) return;

    const top = el.scrollTop;
    const bottom = top + el.clientHeight;

    let y = 0;
    let start = 0;
    let end = blocks.length;
    for (let i = 0; i < blocks.length; i++) {
      const h = heights.current.get(blocks[i].id) ?? ESTIMATED_HEIGHT;
      if (y + h < top) start = i + 1;
      if (y > bottom) {
        end = i;
        break;
      }
      y += h;
    }


    const next = {
      start: Math.max(0, start - OVERSCAN),
      end: Math.min(blocks.length, end + OVERSCAN),
    };
    setRange((current) =>
      current.start === next.start && current.end === next.end ? current : next,
    );

    // The topmost visible block is what "where am I" means in a scroll, and
    // what the remembered page is derived from.
    let acc = 0;
    for (const b of blocks) {
      const h = heights.current.get(b.id) ?? ESTIMATED_HEIGHT;
      if (acc + h > top) {
        onTopBlockChange(b);
        break;
      }
      acc += h;
    }
  }, [blocks, onTopBlockChange]);

  useEffect(() => {
    offsets.current = null;
    recompute();
  }, [recompute]);

  /**
   * Measure what is on screen, once per animation frame at most.
   *
   * This effect had no dependency array, so it ran after *every* render and
   * read `offsetHeight` on every mounted node — each one a forced synchronous
   * layout, on the path that renders on every scroll tick. Deferring to a
   * frame collapses a burst of renders into one measuring pass.
   */
  const measurePending = useRef(0);
  useEffect(() => {
    if (measurePending.current) return;
    measurePending.current = requestAnimationFrame(() => {
      measurePending.current = 0;
      let dirty = false;
      for (const [id, node] of nodes.current) {
        const h = node.offsetHeight;
        if (h > 0 && heights.current.get(id) !== h) {
          heights.current.set(id, h);
          dirty = true;
        }
      }
      if (dirty) {
        offsets.current = null;
        recompute();
      }
    });
    return () => {
      if (measurePending.current) {
        cancelAnimationFrame(measurePending.current);
        measurePending.current = 0;
      }
    };
  });

  /**
   * Running totals of block height, so a spacer is a subtraction.
   *
   * The spacers were each a `reduce` over every block before or after the
   * window — mid-way through a five-thousand block book that is ten thousand
   * map lookups per scroll tick. Rebuilt only when a measurement actually
   * changes, which is rare after the first pass over a page.
   */
  const offsets = useRef<number[] | null>(null);
  const prefix = () => {
    if (!offsets.current || offsets.current.length !== blocks.length + 1) {
      const sums = new Array<number>(blocks.length + 1);
      sums[0] = 0;
      for (let i = 0; i < blocks.length; i++) {
        sums[i + 1] =
          sums[i] + (heights.current.get(blocks[i].id) ?? ESTIMATED_HEIGHT);
      }
      offsets.current = sums;
    }
    return offsets.current;
  };

  const spacerAbove = useMemo(
    () => prefix()[range.start],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [blocks, range.start, annotations],
  );
  const spacerBelow = useMemo(
    () => prefix()[blocks.length] - prefix()[range.end],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [blocks, range.end, annotations],
  );

  // ---- annotations ------------------------------------------------------

  const visibleIds = useMemo(
    () => blocks.slice(range.start, range.end).map((b) => b.id),
    [blocks, range.start, range.end],
  );

  useEffect(() => {
    if (visibleIds.length === 0) return;
    let cancelled = false;
    // Debounced: the visible set changes on every scroll tick, and firing a
    // database round trip per block that scrolls past put several queries a
    // second behind an unbroken drag of the scrollbar.
    const timer = window.setTimeout(() => {
    api
      .blockAnnotations(visibleIds)
      .then((a) => {
        if (!cancelled) {
          setAnnotations(a);
          setAnnotationsFailed(null);
        }
      })
      .catch((e) => {
        // Silently swallowing this was the worst failure in the application:
        // every note, term and citation vanishes from the page and the reader
        // reasonably concludes their work is gone.
        if (!cancelled) setAnnotationsFailed(String(e));
      });
    }, 90);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visibleIds.join(","), annotationRevision, bookId]);

  const byBlock = useMemo(() => {
    const m = new Map<
      number,
      { notes: Note[]; terms: TermMention[]; refs: StoredRef[] }
    >();
    const bucket = (id: number) => {
      let b = m.get(id);
      if (!b) {
        b = { notes: [], terms: [], refs: [] };
        m.set(id, b);
      }
      return b;
    };
    for (const n of annotations.notes) if (n.block_id) bucket(n.block_id).notes.push(n);
    for (const t of annotations.terms) bucket(t.block_id).terms.push(t);
    for (const r of annotations.refs) bucket(r.block_id).refs.push(r);
    return m;
  }, [annotations]);

  /** The standing selection, indexed for the block that has to draw it. */
  const pendingByBlock = useMemo(() => {
    const m = new Map<number, { start: number; end: number }[]>();
    for (const a of selection?.anchors ?? []) {
      const list = m.get(a.blockId) ?? [];
      list.push({ start: a.charStart, end: a.charEnd });
      m.set(a.blockId, list);
    }
    return m;
  }, [selection]);

  const selectionKind = selection?.kind ?? "drag";

  // ---- scrolling to a block ---------------------------------------------

  const scrollToBlock = useCallback(
    (blockId: number) => {
      const index = blocks.findIndex((b) => b.id === blockId);
      if (index < 0 || !scroller.current) return;
      // Bring it into the window first, then let the DOM node take over.
      setRange({
        start: Math.max(0, index - OVERSCAN),
        end: Math.min(blocks.length, index + OVERSCAN),
      });
      requestAnimationFrame(() => {
        nodes.current.get(blockId)?.scrollIntoView({ block: "center" });
      });
    },
    [blocks],
  );

  useEffect(() => {
    registerHandle?.({ scrollToBlock });
  }, [registerHandle, scrollToBlock]);

  // ---- selection --------------------------------------------------------

  /**
   * Read the selection into application state.
   *
   * Every block the drag touched is recorded, not just the one it started in.
   * A selection running across a paragraph or a page boundary is the ordinary
   * case for a reader following an argument, and treating it as a failure —
   * which resolving a single "host" block did — meant the note could not be
   * written at exactly the moment it was most worth writing.
   */
  const readSelection = useCallback((kind: "drag" | "lookup" = "drag") => {
    const sel = window.getSelection();
    const root = scroller.current;
    if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !root) return;

    const anchors = anchorsFromRange(sel.getRangeAt(0), root);
    if (anchors.length === 0) return;

    const pageOf = new Map(blocks.map((b) => [b.id, b.page_no]));
    onSelect({
      kind,
      anchors,
      text: anchors.map((a) => a.text).join(" "),
      pageNos: anchors.map((a) => pageOf.get(a.blockId) ?? null),
    });

    // The native highlight is now redundant — we draw our own, which is what
    // lets it survive the browser clearing this when focus moves to a panel.
    sel.removeAllRanges();
  }, [blocks, onSelect]);

  /**
   * Clearing happens here and nowhere else.
   *
   * Clicking into Notes, Terms or any other panel must leave the selection
   * standing: those are the panels you clicked *in order to* act on it, and
   * losing it on the way there made highlighting feel broken. Only a click
   * back in the text puts it away.
   */
  const clearSelection = useCallback(() => {
    if (selection) onSelect(null);
  }, [selection, onSelect]);

  /**
   * A double-click asks the dictionary about the word under it.
   *
   * The browser has already selected the word by the time this runs, so the
   * selection is the word and there is no caret arithmetic to get wrong.
   */
  const lookUp = useCallback(() => {
    const sel = window.getSelection();
    const word = sel?.toString().trim() ?? "";
    if (!sel || !word || /\s/.test(word) || sel.rangeCount === 0) return;

    const host = (
      sel.getRangeAt(0).startContainer.parentElement as HTMLElement | null
    )?.closest("[data-block-id]");
    if (!host) return;
    const blockId = Number(host.getAttribute("data-block-id"));
    const block = blocks.find((b) => b.id === blockId);
    if (!block) return;

    const rect = sel.getRangeAt(0).getBoundingClientRect();
    onLookUpWord({
      word: word.replace(/^[^\p{L}]+|[^\p{L}]+$/gu, ""),
      sentence: sentenceAround(block.text_norm, word),
      blockId,
      x: rect.left,
      y: rect.bottom,
    });
  }, [blocks, onLookUpWord]);

  /**
   * One handler, routed by how many clicks it was.
   *
   * These cannot be separate `onMouseUp` and `onDoubleClick` handlers, which
   * is what they were: the second mouseup of a double-click fires *before*
   * `dblclick`, and reading the selection clears the native range so that the
   * dictionary lookup then found nothing at all. Double-clicking a word
   * silently did nothing.
   *
   * Two clicks looks the word up *and* leaves it selected, so it can go
   * straight into Terms as well.
   */
  const onMouseUp = useCallback(
    (e: React.MouseEvent) => {
      const isLookup = e.detail === 2;
      if (isLookup) lookUp();
      // Order matters: `readSelection` drops the native range, so the lookup
      // above has to have read it already.
      readSelection(isLookup ? "lookup" : "drag");
    },
    [lookUp, readSelection],
  );

  return (
    <div
      ref={scroller}
      onScroll={recompute}
      onMouseDown={clearSelection}
      onMouseUp={onMouseUp}
      className="h-full overflow-y-auto"
    >
      {annotationsFailed && (
        <p className="sticky top-0 z-20 border-b border-warn bg-warn/15 px-4 py-2 text-xs">
          Your notes and highlights could not be loaded, so the page is showing
          without them. They are not lost. ({annotationsFailed})
        </p>
      )}
      {/* The left padding is the margin the page markers and note dots live
          in. Reserved rather than hung outside the container: `overflow-y`
          on the scroller makes `overflow-x` compute to auto, so anything
          sitting outside gets clipped or drags in a horizontal scrollbar. */}
      <div className="reading-measure mx-auto py-8 pl-16 pr-6">
        <div style={{ height: spacerAbove }} aria-hidden="true" />

        {blocks.slice(range.start, range.end).map((block) => {
          const marks = byBlock.get(block.id);
          const gap = block.starts_page ? gapByPage.get(block.page_id) : undefined;
          return (
            <div
              key={block.id}
              ref={(el) => {
                if (el) nodes.current.set(block.id, el);
                else nodes.current.delete(block.id);
              }}
            >
              {block.starts_page && (
                <PageMarker
                  pageNo={block.page_no}
                  pending={block.pending}
                  missing={!!gap}
                  onOpen={() => onOpenPage(block.page_id)}
                  onRemove={async () => {
                    const ok = await confirm(
                      `Remove page ${block.page_no ?? "?"} and everything transcribed from it?`,
                      { title: "Remove page", kind: "warning" },
                    );
                    if (!ok) return;
                    await api.deletePage(block.page_id);
                    onBlockChanged();
                  }}
                />
              )}
              <BlockText
                block={block}
                active={activeBlock === block.id}
                notes={marks?.notes ?? []}
                terms={marks?.terms ?? []}
                refs={marks?.refs ?? []}
                pending={pendingByBlock.get(block.id) ?? []}
                pendingKind={selectionKind}
                corresponding={corresponding.get(block.id) ?? []}
                suggestion={suggestions.get(block.id)}
                onActivate={() => onActivate(block.id)}
                onContext={onContext}
                onChanged={onBlockChanged}
              />
            </div>
          );
        })}

        <div style={{ height: spacerBelow }} aria-hidden="true" />

        {blocks.length === 0 &&
          (loading ? (
            <p className="py-16 text-center text-sm text-ink-soft">
              Opening the book…
            </p>
          ) : (
            <p className="py-16 text-center text-sm text-ink-soft">
              Nothing here yet. Add pages to begin.
            </p>
          ))}
      </div>
    </div>
  );
}

/**
 * The page boundary: a hairline and a number hanging in the margin.
 *
 * Deliberately not a divider across the column. The text is continuous; this
 * only says where the paper changed.
 */
function PageMarker({
  pageNo,
  pending,
  missing,
  onOpen,
  onRemove,
}: {
  pageNo: number | null;
  pending: boolean;
  missing: boolean;
  onOpen: () => void;
  onRemove: () => void;
}) {
  const label = pageNo != null ? `p. ${pageNo}` : "page —";
  return (
    <div className="group/page relative my-6 first:mt-0">
      <div className="h-px w-full bg-rule" />
      <button
        onClick={onOpen}
        title="Show the photograph of this page"
        className="absolute -left-16 -top-[0.6rem] w-14 truncate rounded text-right text-[0.65rem] uppercase tracking-wider text-ink-soft/70 tabular-nums hover:bg-paper-dim hover:text-accent"
      >
        {label}
      </button>
      {(pending || missing) && (
        <span className="absolute -top-[0.6rem] left-2 bg-paper px-1 text-[0.6rem] uppercase tracking-wider text-ink-soft/70">
          {missing ? "not imported" : "still reading"}
        </span>
      )}
      {/* A whole page can be the wrong page — a duplicate shot, or the facing
          one caught by accident. */}
      <button
        onClick={onRemove}
        title="Remove this page and everything transcribed from it"
        className="absolute -top-[0.7rem] right-0 hidden rounded border border-rule bg-paper px-1.5 py-0.5 text-[0.6rem] text-ink-soft hover:text-danger group-hover/page:block"
      >
        remove page
      </button>
    </div>
  );
}

/**
 * Priority when two annotations cover the same words.
 *
 * The standing selection wins outright: it is the thing the reader is looking
 * at right now, and seeing exactly what a note will attach to matters more
 * than any mark already on the page.
 */
const PRIORITY = { pending: 5, ref: 4, note: 3, term: 2, echo: 1 } as const;

type Decoration = {
  start: number;
  end: number;
  kind: keyof typeof PRIORITY;
  ref?: StoredRef;
  term?: TermMention;
  notes?: Note[];
  group?: number;
};

/**
 * Split text into runs, one per overlapping-free decoration.
 *
 * Overlaps are resolved by dropping the lower-priority decoration entirely
 * rather than by splitting it: half an underline reads as a rendering bug, and
 * a scripture reference matters more than a term underline running through it.
 */
function decorate(
  text: string,
  decorations: Decoration[],
): { text: string; deco?: Decoration }[] {
  const sorted = [...decorations]
    .filter((d) => d.start >= 0 && d.end <= text.length && d.end > d.start)
    .sort((a, b) => a.start - b.start || PRIORITY[b.kind] - PRIORITY[a.kind]);

  const kept: Decoration[] = [];
  for (const d of sorted) {
    const last = kept[kept.length - 1];
    if (last && d.start < last.end) continue;
    kept.push(d);
  }

  const runs: { text: string; deco?: Decoration }[] = [];
  let at = 0;
  for (const d of kept) {
    if (d.start > at) runs.push({ text: text.slice(at, d.start) });
    runs.push({ text: text.slice(d.start, d.end), deco: d });
    at = d.end;
  }
  if (at < text.length) runs.push({ text: text.slice(at) });
  return runs;
}

function BlockText({
  block,
  active,
  notes,
  terms,
  refs,
  pending,
  pendingKind,
  corresponding,
  suggestion,
  onActivate,
  onContext,
  onChanged,
}: {
  block: FlowBlock;
  active: boolean;
  notes: Note[];
  terms: TermMention[];
  refs: StoredRef[];
  pending: { start: number; end: number }[];
  pendingKind: "drag" | "lookup";
  corresponding: { start: number; end: number; group: number }[];
  suggestion?: Move;
  onActivate: () => void;
  onContext: (target: ContextTarget) => void;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(block.text_norm);
  const [reclassing, setReclassing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  /**
   * Every anchored note marks its words, whether or not it carries a tag.
   *
   * The note's *body* still never renders into the column — that is what the
   * gutter dot and the hover card are for. But an anchor you cannot see is an
   * anchor you cannot find again, which made written notes feel lost.
   *
   * Notes sharing an anchor are grouped so the hover card shows all of them
   * rather than whichever happened to win the overlap.
   */
  const anchored = useMemo(() => {
    const groups = new Map<string, Note[]>();
    for (const n of notes) {
      if (n.char_start == null || n.char_end == null) continue;
      const key = `${n.char_start}:${n.char_end}`;
      groups.set(key, [...(groups.get(key) ?? []), n]);
    }
    return [...groups.entries()].map(([key, list]) => {
      const [start, end] = key.split(":").map(Number);
      return { start, end, notes: list };
    });
  }, [notes]);

  const runs = useMemo(
    () =>
      decorate(block.text_norm, [
        ...pending.map((p) => ({
          start: p.start,
          end: p.end,
          kind: "pending" as const,
        })),
        ...refs.map((r) => ({
          start: r.char_start,
          end: r.char_end,
          kind: "ref" as const,
          ref: r,
        })),
        ...anchored.map((a) => ({
          start: a.start,
          end: a.end,
          kind: "note" as const,
          notes: a.notes,
        })),
        ...terms.map((t) => ({
          start: t.char_start,
          end: t.char_end,
          kind: "term" as const,
          term: t,
        })),
        ...corresponding.map((c) => ({
          start: c.start,
          end: c.end,
          kind: "echo" as const,
          group: c.group,
        })),
      ]),
    [block.text_norm, refs, terms, anchored, pending, corresponding],
  );

  const written = notes.filter((n) => n.body.trim().length > 0);

  const save = async () => {
    setSaving(true);
    setFailed(null);
    try {
      await api.editBlock(block.id, draft);
      setEditing(false);
      onChanged();
    } catch (e) {
      setFailed(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    // Tauri's dialog rather than window.confirm — WebView2 does not
    // reliably implement the latter.
    const ok = await confirm(
      `Remove this paragraph?\n\n"${block.text_norm.slice(0, 120)}${
        block.text_norm.length > 120 ? "…" : ""
      }"\n\nAnything anchored to it comes loose.`,
      { title: "Remove paragraph", kind: "warning" },
    );
    if (!ok) return;
    try {
      await api.deleteBlock(block.id);
      onChanged();
    } catch (e) {
      setFailed(String(e));
    }
  };

  const reclassify = async (kind: BlockKind) => {
    setReclassing(false);
    try {
      await api.setBlockKind(block.id, kind);
      onChanged();
    } catch (e) {
      setFailed(String(e));
    }
  };

  /**
   * Correcting the text, where the mistake is.
   *
   * Transcription is never perfect and the error is only ever noticed while
   * reading, so the fix belongs in the column rather than behind a mode. The
   * controls sit in the gap between paragraphs and are absolutely positioned,
   * so revealing them on hover does not change any block's height — which
   * would otherwise make the virtualised window jump under the pointer.
   */
  if (editing) {
    return (
      <div className="my-3 rounded border border-accent p-2">
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          rows={Math.max(3, Math.ceil(draft.length / 60))}
          autoFocus
          className="prose-page w-full resize-y bg-transparent outline-none"
        />
        <div className="mt-2 flex items-center gap-2 text-xs">
          <button
            onClick={save}
            disabled={saving}
            className="rounded bg-accent px-2 py-1 text-paper disabled:opacity-40"
          >
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            onClick={() => {
              setDraft(block.text_norm);
              setEditing(false);
            }}
            className="rounded border border-rule px-2 py-1"
          >
            Cancel
          </button>
          <span className="text-ink-soft">
            Notes anchored here follow the words they were attached to.
          </span>
        </div>
        {failed && <p className="mt-1 text-xs text-danger">{failed}</p>}
      </div>
    );
  }

  /**
   * Every kind a block can be, set in type rather than flattened to prose.
   *
   * `BlockKind` has five values and only two were styled, so a footnote sat in
   * the argument's own type — actively misleading in a scholarly text, where
   * telling the note from the claim is most of the work.
   */
  const Tag = block.kind === "heading" ? "h3" : "p";
  const tone =
    block.kind === "heading"
      ? "mt-7 mb-2 text-center font-semibold tracking-wide"
      : block.kind === "quote"
        ? "my-3 border-l-2 border-rule pl-4 text-ink-soft"
        : block.kind === "footnote"
          ? "my-2 text-[0.82em] leading-snug text-ink-soft"
          : block.kind === "caption"
            ? "my-2 text-center text-[0.85em] italic text-ink-soft"
            : "my-3";

  return (
    <div className="group relative">
      {written.length > 0 && (
        <span
          className="absolute -left-4 top-4 h-1.5 w-1.5 rounded-full bg-accent/70"
          aria-label={`${written.length} notes here`}
        />
      )}
      {/* Absolutely positioned into the gap below the paragraph, so showing
          it on hover cannot reflow the column. */}
      <div className="absolute -bottom-1.5 right-0 z-10 hidden items-center gap-1 rounded border border-rule bg-paper px-1 py-0.5 text-[0.65rem] text-ink-soft shadow-sm group-hover:flex">
        <button
          onClick={() => {
            setDraft(block.text_norm);
            setEditing(true);
          }}
          title="Fix the transcription"
          className="rounded px-1 hover:text-accent"
        >
          edit
        </button>
        <button
          onClick={() => setReclassing((v) => !v)}
          title="This is not an ordinary paragraph"
          className="rounded px-1 hover:text-accent"
        >
          {block.kind}
        </button>
        <button
          onClick={remove}
          title="Remove this paragraph — for text the camera caught by mistake"
          className="rounded px-1 hover:text-danger"
        >
          remove
        </button>
        {block.user_edited && <span className="pl-1 opacity-70">edited</span>}
      </div>

      {reclassing && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setReclassing(false)} />
          <div className="absolute -bottom-1 right-0 z-30 rounded border border-rule bg-paper py-1 shadow-xl">
            {BLOCK_KINDS.map((k) => (
              <button
                key={k}
                onClick={() => reclassify(k)}
                className={`block w-full px-3 py-1 text-left text-xs hover:bg-paper-dim hover:text-accent ${
                  k === block.kind ? "text-accent" : ""
                }`}
              >
                {k}
              </button>
            ))}
          </div>
        </>
      )}

      {failed && (
        <p className="absolute -bottom-4 right-0 text-[0.65rem] text-danger">
          {failed}
        </p>
      )}

      <Tag
        data-block-id={block.id}
        onClick={onActivate}
        className={`prose-page cursor-text selection:bg-accent/25 ${tone} ${
          suggestion ? `sugg sugg-${suggestion}` : ""
        } ${active ? "rounded bg-accent/[0.06]" : ""}`}
      >
        {runs.map((run, i) =>
          run.deco?.kind === "pending" ? (
            <mark
              key={i}
              className={pendingKind === "lookup" ? "mark-lookup" : "mark-pending"}
            >
              {run.text}
            </mark>
          ) : run.deco?.kind === "ref" ? (
            <button
              key={i}
              onClick={(e) => {
                e.stopPropagation();
                onContext({ kind: "reference", reference: run.deco!.ref! });
              }}
              className="vref"
            >
              {run.text}
            </button>
          ) : run.deco?.kind === "note" ? (
            <mark
              key={i}
              onClick={(e) => {
                e.stopPropagation();
                onContext({ kind: "notes", notes: run.deco!.notes! });
              }}
              className={`mark mark-${run.deco.notes![0].move ?? "note"} cursor-pointer`}
            >
              {run.text}
            </mark>
          ) : run.deco?.kind === "term" ? (
            <span
              key={i}
              onClick={(e) => {
                e.stopPropagation();
                onContext({ kind: "term", term: run.deco!.term! });
              }}
              className={`term term-${run.deco.term!.status} cursor-pointer`}
            >
              {run.text}
            </span>
          ) : run.deco?.kind === "echo" ? (
            <span key={i} className={`echo echo-${run.deco.group! % 6}`}>
              {run.text}
            </span>
          ) : (
            <span key={i}>{run.text}</span>
          ),
        )}
      </Tag>
    </div>
  );
}
