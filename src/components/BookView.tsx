import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  onJobProgress,
  onOcrProgress,
  type Block,
  type Book,
  type FlowBlock,
  type JobProgress,
  type Move,
  type Page,
  type PageGap,
  type SentenceRole,
} from "../lib/api";
import { correspondingWords } from "../lib/text";
import { dispatch } from "../lib/keys";
import { pageLabel } from "../lib/selection";
import {
  AddContentWizard,
  type SourceChoice,
  type WizardResult,
} from "./AddContentWizard";
import {
  Flow,
  type ContextTarget,
  type FlowHandle,
  type FlowSelection,
  type WordHit,
} from "./Flow";
import { PageImage } from "./PageImage";
import { MoveChip, useMoveCatalogue } from "./PanelChrome";

/**
 * One book, open in one pane.
 *
 * Self-contained so two of these can sit side by side: each loads its own
 * blocks and keeps its own scroll position, which is what makes reading a
 * commentary against the text it comments on actually work.
 *
 * The selection is *not* local — it lives in the workspace, so a passage
 * highlighted in the left pane can be turned into a note by a panel in the
 * right one.
 */
/** What the workspace can ask a book pane to do. */
export interface BookApi {
  goTo: (blockId: number) => void;
  advance: () => void;
}

export function BookView({
  book,
  selection,
  onSelect,
  onContext,
  onLookUpWord,
  annotationRevision,
  onAnnotationsChanged,
  registerBookApi,
  onActiveBlock,
  onSwitchBook,
  onScanReferences,
  onPosition,
  pendingBlock,
  onPendingConsumed,
  onQuickNote,
  onQuickTerm,
  isFocused,
  onFocus,
}: {
  book: Book;
  selection: FlowSelection | null;
  onSelect: (s: FlowSelection | null) => void;
  onContext: (t: ContextTarget) => void;
  onLookUpWord: (w: WordHit) => void;
  annotationRevision: number;
  onAnnotationsChanged: () => void;
  /** Hands the workspace a way to scroll this book to a block. */
  /** Passing null withdraws the handle, which unmount must do. */
  registerBookApi: (bookId: number, api: BookApi | null) => void;
  /** The paragraph being worked on, lifted so the tools can see it. */
  onActiveBlock: (bookId: number, block: Block | null) => void;
  /** Show a different book in this tab, rather than opening another one. */
  onSwitchBook: () => void;
  onScanReferences: () => void;
  /** Where the active paragraph sits, for the Method panel's counter. */
  onPosition: (
    bookId: number,
    position: { index: number; total: number } | null,
  ) => void;
  /**
   * A block another panel asked us to jump to, honoured once this book's
   * text has actually loaded.
   *
   * A caller cannot simply call `goTo` on a pane that is still mounting: the
   * blocks arrive over IPC, and the old code guessed at 200ms and lost the
   * race on anything large. Waiting for the data is the only reliable way.
   */
  pendingBlock: number | null;
  onPendingConsumed: () => void;
  /** Open Notes with this selection already carried in, cursor in the box. */
  onQuickNote: () => void;
  /** Same, for Terms. */
  onQuickTerm: () => void;
  isFocused: boolean;
  onFocus: () => void;
}) {
  const [flow, setFlow] = useState<FlowBlock[]>([]);
  const [gaps, setGaps] = useState<PageGap[]>([]);
  const [pages, setPages] = useState<Page[]>([]);
  const [activeBlock, setActiveBlock] = useState<number | null>(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [viewingPage, setViewingPage] = useState<Page | null>(null);
  const [showEchoes, setShowEchoes] = useState(false);
  const [suggestions, setSuggestions] = useState<Map<number, Move>>(new Map());
  const [suggestList, setSuggestList] = useState<SentenceRole[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [job, setJob] = useState<JobProgress | null>(null);
  const [transcript, setTranscript] = useState("");
  const [menu, setMenu] = useState(false);
  const [marking, setMarking] = useState(false);
  /**
   * True when the pane is too narrow for the full toolbar.
   *
   * The splitter allows a pane down to a fifth of the window, at which point
   * title, menu, selection chip, Repeats and Suggest wrap into an unreadable
   * stack. Measured rather than guessed from the viewport, because what
   * matters is this pane's width, not the window's.
   */
  const [narrow, setNarrow] = useState(false);
  /**
   * Whether the book's text has arrived yet.
   *
   * Without it the column showed "Nothing here yet. Add pages to begin."
   * for the whole of a large book's load — the one message guaranteed to be
   * wrong at that moment, and one that invites the reader to import pages
   * they already have.
   */
  const [loading, setLoading] = useState(true);
  const root = useRef<HTMLDivElement>(null);
  const flowHandle = useRef<FlowHandle | null>(null);
  const moves = useMoveCatalogue();

  useEffect(() => {
    const el = root.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) =>
      setNarrow(entry.contentRect.width < 460),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [blocks, gapList, pageList] = await Promise.all([
        api.bookBlocks(book.id),
        api.pageGaps(book.id),
        api.listPages(book.id),
      ]);
      setFlow(blocks);
      setGaps(gapList);
      setPages(pageList);
    } finally {
      setLoading(false);
    }
  }, [book.id]);

  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, [refresh]);

  useEffect(() => {
    const un = onJobProgress(setJob);
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const un = onOcrProgress((p) => setTranscript((t) => t + p.chunk));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const goTo = useCallback((blockId: number) => {
    setActiveBlock(blockId);
    flowHandle.current?.scrollToBlock(blockId);
  }, []);

  /**
   * Move to the next paragraph.
   *
   * Lives here rather than in the workspace because only this pane knows the
   * book's block order; the Method panel drives it from the other pane.
   */
  /**
   * Where this paragraph sits among the book's prose.
   *
   * Orientation is exactly what a continuous scroll takes away, and the
   * Method panel has had a slot for it all along that was being passed null.
   */
  const position = useMemo(() => {
    const prose = flow.filter((b) => b.kind === "paragraph");
    const index = prose.findIndex((b) => b.id === activeBlock);
    return index < 0 ? null : { index, total: prose.length };
  }, [flow, activeBlock]);

  useEffect(() => {
    onPosition(book.id, position);
  }, [onPosition, book.id, position]);

  const advance = useCallback(() => {
    setActiveBlock((current) => {
      const i = flow.findIndex((b) => b.id === current);
      const next = flow.slice(i + 1).find((b) => b.kind === "paragraph");
      if (next) flowHandle.current?.scrollToBlock(next.id);
      return next?.id ?? current;
    });
  }, [flow]);

  useEffect(() => {
    registerBookApi(book.id, { goTo, advance });
    // Withdrawn on unmount. Left behind, it is a handle into a dead component:
    // the workspace finds it, calls it, and nothing happens — which is exactly
    // how jumping to a book on a background tab came to fail in silence.
    return () => registerBookApi(book.id, null);
  }, [registerBookApi, book.id, goTo, advance]);

  // Honour a jump once the text is here to jump within.
  useEffect(() => {
    if (pendingBlock == null || flow.length === 0) return;
    goTo(pendingBlock);
    onPendingConsumed();
  }, [pendingBlock, flow.length, goTo, onPendingConsumed]);

  // The tools work on a full `Block`, which carries the raw text the flow
  // view has no use for.
  useEffect(() => {
    if (activeBlock == null) {
      onActiveBlock(book.id, null);
      return;
    }
    const found = flow.find((b) => b.id === activeBlock);
    if (!found) return;
    let cancelled = false;
    api
      .listBlocks(found.page_id)
      .then((bs) => {
        if (!cancelled) {
          onActiveBlock(book.id, bs.find((b) => b.id === activeBlock) ?? null);
        }
      })
      .catch(() => onActiveBlock(book.id, null));
    return () => {
      cancelled = true;
    };
  }, [activeBlock, flow, book.id, onActiveBlock]);

  // Where the reader stopped, derived from what is at the top of the column.
  const lastPage = useRef<number | null>(null);
  const onTopBlockChange = useCallback(
    (b: FlowBlock | null) => {
      if (!b || lastPage.current === b.page_id) return;
      lastPage.current = b.page_id;
      api.setLastPage(book.id, b.page_id).catch(() => {});
    },
    [book.id],
  );

  const loaded = flow.length > 0;
  useEffect(() => {
    if (!loaded) return;
    let cancelled = false;
    api
      .resumePage(book.id)
      .then((pageId) => {
        if (cancelled || pageId == null) return;
        const first = flow.find((b) => b.page_id === pageId);
        if (first) flowHandle.current?.scrollToBlock(first.id);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // Once per book, not on every refetch of its blocks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [book.id, loaded]);

  const passage = useMemo(
    () => flow.find((b) => b.id === activeBlock)?.text_norm ?? "",
    [activeBlock, flow],
  );

  /**
   * Repeated words in the passage being worked on.
   *
   * Scoped to the active paragraph rather than the whole book: repetition is
   * only informative inside a stretch of argument, and tinting every "God" in
   * a systematic theology would tint the entire book.
   */
  const corresponding = useMemo(() => {
    const m = new Map<number, { start: number; end: number; group: number }[]>();
    if (!showEchoes || activeBlock == null || !passage) return m;
    m.set(activeBlock, correspondingWords(passage));
    return m;
  }, [showEchoes, activeBlock, passage]);

  const suggest = async () => {
    if (!passage || activeBlock == null) return;
    setBusy("Reading the passage");
    setError(null);
    try {
      const roles = await api.suggestMoves(passage);
      setSuggestList(roles);
      setSuggestions(
        roles.length ? new Map([[activeBlock, roles[0].role]]) : new Map(),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const acceptSuggestion = async (role: SentenceRole) => {
    if (activeBlock == null) return;
    const at = passage.indexOf(role.text);
    await api.addNote({
      bookId: book.id,
      blockId: activeBlock,
      charStart: at < 0 ? null : at,
      charEnd: at < 0 ? null : at + role.text.length,
      anchorText: role.text,
      move: role.role,
      body: "",
    });
    setSuggestList((l) => l?.filter((r) => r.ordinal !== role.ordinal) ?? null);
    onAnnotationsChanged();
  };

  /** Tag the selection without leaving the page. */
  const applyMark = async (move: Move) => {
    if (!selection) return;
    setMarking(false);
    const [first, ...rest] = selection.anchors;
    if (!first) return;
    await api.addNote({
      bookId: book.id,
      blockId: first.blockId,
      charStart: first.charStart,
      charEnd: first.charEnd,
      anchorText: first.text,
      extraAnchors: rest.map((a) => ({
        block_id: a.blockId,
        char_start: a.charStart,
        char_end: a.charEnd,
        anchor_text: a.text,
      })),
      move,
      body: "",
    });
    onSelect(null);
    onAnnotationsChanged();
  };

  /**
   * The bindings for reading.
   *
   * Only while this pane has focus, so the two panes cannot both act on one
   * keystroke, and never while typing — see `lib/keys.ts`.
   */
  useEffect(() => {
    if (!isFocused) return;
    const onKey = (e: KeyboardEvent) => {
      const ran = dispatch(e, [
        { key: "escape", whileTyping: true, run: () => onSelect(null) },
        { key: "n", run: () => selection && onQuickNote() },
        { key: "t", run: () => selection && onQuickTerm() },
        { key: "m", run: () => selection && setMarking((v) => !v) },
        { key: "r", run: () => passage && setShowEchoes((v) => !v) },
        { key: "s", run: () => passage && suggest() },
        { key: "j", run: () => advance() },
        {
          key: "k",
          run: () =>
            setActiveBlock((current) => {
              const i = flow.findIndex((b) => b.id === current);
              const previous = [...flow.slice(0, Math.max(0, i))]
                .reverse()
                .find((b) => b.kind === "paragraph");
              if (previous) flowHandle.current?.scrollToBlock(previous.id);
              return previous?.id ?? current;
            }),
        },
      ]);
      if (ran) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isFocused, selection, passage, flow, advance]);

  const unnumbered = pages.filter(
    (p) => p.page_no == null && p.ocr_status === "done",
  );

  const runImport = async (
    choice: SourceChoice,
    source: string,
  ): Promise<WizardResult> => {
    setTranscript("");
    try {
      if (choice === "photo") {
        const imported = await api.importPage(book.id, source);
        for (const id of imported.page_ids) await api.runOcr(id, book.id);
        await refresh();
        onAnnotationsChanged();
        const fresh = await api.listPages(book.id);
        return {
          summary: imported.was_spread
            ? "Two pages added from one photograph."
            : "Page added.",
          needsNumbers: fresh.filter(
            (p) =>
              imported.page_ids.includes(p.id) &&
              p.page_no == null &&
              p.ocr_status === "done",
          ),
        };
      }
      const doc = await api.importDocument(book.id, source);
      await refresh();
      await api.scanReferences(book.id).catch(() => 0);
      onAnnotationsChanged();
      return {
        summary: `${doc.pages_added} pages and ${doc.blocks_added} paragraphs added.`,
      };
    } catch (e) {
      return { summary: "", error: String(e) };
    }
  };

  return (
    <div
      ref={root}
      onMouseDown={onFocus}
      className={`flex h-full min-h-0 flex-col ${
        isFocused ? "" : "opacity-95"
      }`}
    >
      <div className="relative flex flex-wrap items-center gap-2 border-b border-rule px-3 py-1.5 text-xs">
        {/* The title is the book switcher. Changing what a tab shows is far
            more common than wanting another tab, and burying it in a menu of
            every book in the library does not survive a hundred of them. */}
        <button
          onClick={onSwitchBook}
          title="Show a different book in this tab"
          className="flex min-w-0 items-center gap-1 rounded px-1 py-0.5 font-medium hover:bg-paper-dim hover:text-accent"
        >
          <span className="truncate">{book.title}</span>
          <span aria-hidden="true" className="text-ink-soft">▾</span>
        </button>

        <button
          onClick={() => setMenu((v) => !v)}
          title="More for this book"
          aria-label="More for this book"
          aria-expanded={menu}
          className="rounded px-1.5 py-0.5 text-ink-soft hover:bg-paper-dim hover:text-accent"
        >
          ⋯
        </button>

        {menu && (
          <>
            <div className="fixed inset-0 z-20" onClick={() => setMenu(false)} />
            <div className="absolute left-2 top-full z-30 mt-0.5 w-56 rounded border border-rule bg-paper py-1 shadow-xl">
              <p className="px-3 py-1 text-[0.65rem] uppercase tracking-wider text-ink-soft">
                {book.author ?? "Unknown author"}
              </p>
              <button
                onClick={() => {
                  setMenu(false);
                  onSwitchBook();
                }}
                className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent"
              >
                Show a different book…
              </button>
              <button
                onClick={() => {
                  setMenu(false);
                  setWizardOpen(true);
                }}
                className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent"
              >
                Add pages…
              </button>
              {narrow && (
                <>
                  <button
                    onClick={() => {
                      setMenu(false);
                      setShowEchoes((v) => !v);
                    }}
                    disabled={!passage}
                    className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent disabled:opacity-40"
                  >
                    {showEchoes ? "Hide repeated words" : "Tint repeated words"}
                  </button>
                  <button
                    onClick={() => {
                      setMenu(false);
                      suggest();
                    }}
                    disabled={!passage || busy !== null}
                    className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent disabled:opacity-40"
                  >
                    Suggest what this passage is doing
                  </button>
                </>
              )}
              <button
                onClick={() => {
                  setMenu(false);
                  onScanReferences();
                }}
                title="Find every scripture citation in this book"
                className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent"
              >
                Scan for scripture references
              </button>
              {unnumbered.length > 0 && (
                <button
                  onClick={() => {
                    setMenu(false);
                    setViewingPage(unnumbered[0]);
                  }}
                  className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent"
                >
                  Number {unnumbered.length} page
                  {unnumbered.length === 1 ? "" : "s"}…
                </button>
              )}
            </div>
          </>
        )}

        {busy && <span className="text-ink-soft">{busy}</span>}

        {selection && selection.kind === "drag" && (
          <span className="flex min-w-0 items-center gap-1 rounded border border-accent/40 bg-accent/10 px-1.5 py-0.5">
            <span className="max-w-[10rem] truncate text-ink-soft">
              “{selection.text}”
            </span>
            {pageLabel(selection.pageNos) && (
              <span className="tabular-nums text-ink-soft">
                {pageLabel(selection.pageNos)}
              </span>
            )}

            {/* One click each, instead of hunting for the panel first. This is
                the commonest thing anyone does here and it used to take six
                steps. Chrome rather than an overlay, so nothing covers the
                page. */}
            <button
              onClick={onQuickNote}
              title="Write a note on this (N)"
              className="rounded px-1.5 py-0.5 font-medium hover:bg-paper hover:text-accent"
            >
              Note
            </button>
            <button
              onClick={onQuickTerm}
              title="Gloss this as a term (T)"
              className="rounded px-1.5 py-0.5 font-medium hover:bg-paper hover:text-accent"
            >
              Term
            </button>
            <button
              onClick={() => setMarking((v) => !v)}
              title="Tag what this passage is doing (M)"
              aria-expanded={marking}
              className="rounded px-1.5 py-0.5 font-medium hover:bg-paper hover:text-accent"
            >
              Mark ▾
            </button>
            <button
              onClick={() => onSelect(null)}
              title="Clear the selection (Esc)"
              className="rounded px-1 text-ink-soft hover:bg-paper hover:text-accent"
            >
              ✕
            </button>
          </span>
        )}

        {marking && selection && (
          <>
            <div className="fixed inset-0 z-20" onClick={() => setMarking(false)} />
            <div className="absolute left-2 top-full z-30 mt-0.5 w-64 rounded border border-rule bg-paper p-2 shadow-xl">
              <p className="mb-1.5 text-[0.65rem] uppercase tracking-wider text-ink-soft">
                What is this passage doing?
              </p>
              <div className="flex flex-wrap gap-1">
                {moves.map((m) => (
                  <MoveChip
                    key={m.label}
                    move={m.label}
                    info={m}
                    onClick={() => applyMark(m.id)}
                  />
                ))}
              </div>
            </div>
          </>
        )}

        <div className="ml-auto flex items-center gap-1.5">
          {unnumbered.length > 0 && (
            <button
              onClick={() => setViewingPage(unnumbered[0])}
              title={`${unnumbered.length} pages still need a number`}
              className="rounded border border-warn/60 px-1.5 py-0.5 font-semibold text-warn"
            >
              {unnumbered.length}
            </button>
          )}
          <button
            onClick={() => setWizardOpen(true)}
            title="Add pages to this book"
            className="rounded border border-rule px-2 py-1 hover:border-accent hover:text-accent"
          >
            +
          </button>
          {!narrow && (
            <>
              <button
                onClick={() => setShowEchoes((v) => !v)}
                disabled={!passage}
                title="Tint the words this passage repeats (R)"
                className={`rounded border px-2 py-1 disabled:opacity-40 ${
                  showEchoes
                    ? "border-accent text-accent"
                    : "border-rule hover:border-accent hover:text-accent"
                }`}
              >
                Repeats
              </button>
              <button
                onClick={suggest}
                disabled={!passage || busy !== null}
                title="Ask what this paragraph is doing (S). Nothing is saved until you accept it."
                className="rounded border border-rule px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
              >
                Suggest
              </button>
            </>
          )}
        </div>
      </div>

      {error && (
        <p className="border-b border-rule px-3 py-1.5 text-xs text-danger">
          {error}
        </p>
      )}

      {suggestList && (
        <div className="border-b border-rule bg-paper-dim px-3 py-2">
          {suggestList.length === 0 ? (
            <p className="text-xs text-ink-soft">
              Nothing here reads clearly as one of the nine moves.
            </p>
          ) : (
            <ul className="space-y-1">
              {suggestList.map((r) => (
                <li key={r.ordinal} className="flex items-start gap-2 text-xs">
                  <span
                    className={`mark mark-${r.role} flex-none rounded px-1.5 font-semibold`}
                  >
                    {r.role}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-ink-soft">
                    {r.text}
                  </span>
                  <button
                    onClick={() => acceptSuggestion(r)}
                    className="flex-none text-accent hover:underline"
                  >
                    accept
                  </button>
                </li>
              ))}
            </ul>
          )}
          <button
            onClick={() => {
              setSuggestList(null);
              setSuggestions(new Map());
            }}
            className="mt-1.5 text-xs text-ink-soft hover:text-accent"
          >
            dismiss
          </button>
        </div>
      )}

      <div className="min-h-0 flex-1">
        <Flow
          bookId={book.id}
          blocks={flow}
          gaps={gaps}
          loading={loading}
          activeBlock={activeBlock}
          selection={selection}
          corresponding={corresponding}
          onActivate={setActiveBlock}
          onSelect={onSelect}
          onContext={onContext}
          onLookUpWord={onLookUpWord}
          onBlockChanged={() => {
            refresh().catch((e) => setError(String(e)));
            onAnnotationsChanged();
          }}
          onOpenPage={(pageId) => {
            const page = pages.find((p) => p.id === pageId);
            if (page) setViewingPage(page);
          }}
          onTopBlockChange={onTopBlockChange}
          annotationRevision={annotationRevision}
          suggestions={suggestions}
          registerHandle={(h) => {
            flowHandle.current = h;
          }}
        />
      </div>

      {viewingPage && (
        <PageImage
          page={viewingPage}
          queue={pages}
          onClose={() => setViewingPage(null)}
          onSaved={() => {
            refresh().catch((e) => setError(String(e)));
          }}
        />
      )}

      {wizardOpen && (
        <AddContentWizard
          onClose={() => {
            setWizardOpen(false);
            refresh().catch((e) => setError(String(e)));
          }}
          onImport={runImport}
          progress={job}
          transcript={transcript}
        />
      )}
    </div>
  );
}
