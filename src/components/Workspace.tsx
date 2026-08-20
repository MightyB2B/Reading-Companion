import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Block, type Book } from "../lib/api";
import { BookView, type BookApi } from "./BookView";
import { BookPicker } from "./BookPicker";
import { dispatch } from "../lib/keys";
import { PageNavigator } from "./PageNavigator";
import type { ContextTarget, FlowSelection, WordHit } from "./Flow";
import { MethodPanel } from "./MethodPanel";
import { MoveGlossary, useMoveCatalogue } from "./PanelChrome";
import { ArgumentsPanel } from "./panels/Arguments";
import { ContextPanel } from "./panels/Context";
import { SpinePanel } from "./panels/Spine";
import { WordsPanel } from "./panels/Words";
import { ExportPanel } from "./panels/Export";
import { NotesPanel } from "./panels/Notes";
import { SearchPanel } from "./panels/Search";
import { TermsPanel } from "./panels/Terms";

/**
 * The workbench: two panes, each holding whatever you put in it.
 *
 * Both panes are the same kind of thing — a strip of tabs over a pane of
 * content — and a tab is either a book or a tool. That symmetry is the whole
 * point: it is what lets you read a commentary in the left pane against the
 * text in the right, or keep Notes open beside a book, without the application
 * having an opinion about which side is "the reading side".
 *
 * Shared state lives here rather than in either pane, so a passage highlighted
 * in one can be turned into a note by a panel in the other.
 */

type ToolId =
  | "context"
  | "search"
  | "method"
  | "notes"
  | "terms"
  | "arguments"
  | "spine"
  | "words"
  | "outline"
  | "export"
  | "moves";

/**
 * The rail, grouped and captioned.
 *
 * Ten unlabelled glyphs — `⋔` for Arguments, a bare `A` for Terms — is not a
 * navigation bar, it is a memory test. The groups say what each tool is *for*,
 * which is the part that makes a workbench legible.
 */
const TOOL_GROUPS: { group: string; tools: ToolId[] }[] = [
  { group: "Read", tools: ["outline", "search"] },
  { group: "Study", tools: ["method", "notes", "terms", "arguments"] },
  { group: "Your work", tools: ["spine", "words"] },
  { group: "Look up", tools: ["context", "moves"] },
  { group: "Book", tools: ["export"] },
];

const TOOLS: { id: ToolId; label: string; icon: string }[] = [
  { id: "outline", label: "Outline", icon: "☰" },
  { id: "search", label: "Search", icon: "⌕" },
  { id: "method", label: "Method", icon: "◆" },
  { id: "notes", label: "Notes", icon: "✎" },
  { id: "terms", label: "Terms", icon: "◉" },
  { id: "arguments", label: "Arguments", icon: "⋔" },
  { id: "spine", label: "Spine", icon: "≡" },
  { id: "words", label: "Words", icon: "Aa" },
  { id: "context", label: "Context", icon: "◈" },
  { id: "moves", label: "Moves", icon: "?" },
  { id: "export", label: "Export", icon: "⤓" },
];

interface Tab {
  /** Stable across renders; `book:3` or `tool:notes`. */
  id: string;
  kind: "book" | "tool";
  bookId?: number;
  tool?: ToolId;
  label: string;
}

interface Pane {
  tabs: Tab[];
  activeId: string | null;
}

const bookTab = (b: Book): Tab => ({
  id: `book:${b.id}`,
  kind: "book",
  bookId: b.id,
  label: b.title,
});

const toolTab = (t: ToolId): Tab => ({
  id: `tool:${t}`,
  kind: "tool",
  tool: t,
  label: TOOLS.find((x) => x.id === t)!.label,
});

export function Workspace({
  book,
  onCloseBook,
}: {
  book: Book;
  onCloseBook: () => void;
}) {
  const [books, setBooks] = useState<Book[]>([]);
  const [panes, setPanes] = useState<[Pane, Pane]>(() => {
    const first = bookTab(book);
    return [
      { tabs: [first], activeId: first.id },
      { tabs: [toolTab("context")], activeId: "tool:context" },
    ];
  });
  const [focused, setFocused] = useState<0 | 1>(0);
  const [splitPct, setSplitPct] = useState(50);
  /**
   * Linked panes follow each other.
   *
   * Touch a scripture reference in one and the other moves to the matching
   * place — the verse itself if that book is a Bible, otherwise where that
   * book cites it. This is the reason to have two panes at all: reading a
   * commentary against the text it comments on.
   */
  const [linked, setLinked] = useState(false);

  // Shared across both panes: the selection made in one is what the panels in
  // the other act on.
  const [selection, setSelection] = useState<FlowSelection | null>(null);
  const [context, setContext] = useState<ContextTarget | null>(null);
  const [word, setWord] = useState<WordHit | null>(null);
  /**
   * Something held in view while reading on.
   *
   * Without it the Context panel is replaced by whatever you click next, so a
   * verse you wanted beside you for the next three paragraphs survives exactly
   * one click.
   */
  const [pinned, setPinned] = useState<{
    target: ContextTarget | null;
    word: WordHit | null;
  } | null>(null);
  const [searchSeed, setSearchSeed] = useState("");
  /**
   * The book picker, and what choosing will do.
   *
   * `open` adds a tab; `switch` replaces what a tab is showing, which is the
   * commoner action and the reason the title in the toolbar is a button.
   */
  const [picker, setPicker] = useState<
    { mode: "open" | "switch"; pane: 0 | 1; tabId?: string } | null
  >(null);
  const [revision, setRevision] = useState(0);
  const bump = useCallback(() => setRevision((r) => r + 1), []);
  const moves = useMoveCatalogue();

  const bookApis = useRef(new Map<number, BookApi>());
  const registerBookApi = useCallback((bookId: number, a: BookApi | null) => {
    if (a) bookApis.current.set(bookId, a);
    else bookApis.current.delete(bookId);
  }, []);

  /**
   * The book the tools act on.
   *
   * Deliberately *not* derived from which pane has focus. Clicking into the
   * Notes panel focuses the pane holding Notes, whose active tab is a tool and
   * therefore names no book — which used to send every tool back to whichever
   * book the workspace happened to open with. This tracks the last book pane
   * the reader actually touched, which is what they mean by "this book".
   */
  const [workingBookId, setWorkingBookId] = useState(book.id);

  /**
   * The active paragraph, per book.
   *
   * Every mounted book reports its own, so they cannot be kept in one slot:
   * with background tabs staying alive, the last one to mount would otherwise
   * win and the Method panel would follow a book nobody is reading.
   */
  const [blockByBook, setBlockByBook] = useState<Record<number, Block | null>>(
    {},
  );
  const [positionByBook, setPositionByBook] = useState<
    Record<number, { index: number; total: number } | null>
  >({});
  const onPosition = useCallback(
    (bookId: number, position: { index: number; total: number } | null) => {
      setPositionByBook((current) =>
        current[bookId]?.index === position?.index &&
        current[bookId]?.total === position?.total
          ? current
          : { ...current, [bookId]: position },
      );
    },
    [],
  );

  const onActiveBlock = useCallback((bookId: number, block: Block | null) => {
    setBlockByBook((current) =>
      current[bookId] === block ? current : { ...current, [bookId]: block },
    );
  }, []);

  /**
   * Bumped when a panel should take the cursor.
   *
   * The point of the toolbar's Note and Term buttons is to skip the hunting,
   * so the panel has to open *and* be ready to type in — otherwise it has
   * saved one click out of six.
   */
  const [focusSignal, setFocusSignal] = useState(0);

  /** Late-bound so `showContext` need not depend on `goTo`. */
  const goToRef = useRef<
    ((bookId: number, blockId: number, into?: 0 | 1) => void) | null
  >(null);

  /** A jump waiting for its book to finish loading. */
  const [pendingJump, setPendingJump] = useState<{
    bookId: number;
    blockId: number;
    /** Bumped per request, so asking for the same passage twice works. */
    seq: number;
  } | null>(null);

  useEffect(() => {
    api.listBooks().then(setBooks).catch(() => setBooks([]));
  }, []);

  /**
   * Bring a tool to the front, opening it if it is not already somewhere.
   *
   * Prefers a pane that already has it; otherwise it opens in whichever pane
   * is *not* focused, so answering a question does not replace the book you
   * asked it about.
   */
  const revealTool = useCallback((tool: ToolId) => {
    setPanes((current) => {
      const id = `tool:${tool}`;
      const existing = current.findIndex((p) => p.tabs.some((t) => t.id === id));
      if (existing >= 0) {
        const next = [...current] as [Pane, Pane];
        next[existing] = { ...next[existing], activeId: id };
        return next;
      }
      // Prefer a pane that is empty, then one already showing a tool, and
      // only then the pane without focus. Targeting "not the focused pane"
      // alone meant that once you clicked into the tool side, the next tool
      // you opened landed on top of the book you were reading.
      const paneShowing = (p: Pane) =>
        p.tabs.find((t) => t.id === p.activeId)?.kind ?? "empty";
      const empty = current.findIndex((p) => paneShowing(p) === "empty");
      const showingTool = current.findIndex((p) => paneShowing(p) === "tool");
      const target: 0 | 1 = (
        empty >= 0 ? empty : showingTool >= 0 ? showingTool : focused === 0 ? 1 : 0
      ) as 0 | 1;
      const next = [...current] as [Pane, Pane];
      next[target] = {
        tabs: [...next[target].tabs, toolTab(tool)],
        activeId: id,
      };
      return next;
    });
  }, [focused]);

  // Clicking something in the text answers in the Context panel, so that
  // panel has to be in front of whatever else was showing.
  const showContext = useCallback(
    (t: ContextTarget) => {
      setWord(null);
      setContext(t);
      revealTool("context");

      // With the panes linked, a reference moves the other side too.
      if (!linked || t.kind !== "reference") return;
      const r = t.reference;
      const other: 0 | 1 = bookInPane(0) === r.book_id ? 1 : 0;
      const otherBook = bookInPane(other);
      if (otherBook == null || otherBook === r.book_id) return;

      api
        .locateReference(otherBook, r.osis_book, r.chapter, r.verse_start)
        .then((blockId) => {
          if (blockId != null) goToRef.current?.(otherBook, blockId, other);
        })
        .catch(() => {});
    },
    // `goTo` is reached through a ref to keep this from being redefined on
    // every pane change, which would remount the reading columns.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [revealTool, linked, panes],
  );

  const showWord = useCallback(
    (w: WordHit) => {
      setContext(null);
      setWord(w);
      revealTool("context");
    },
    [revealTool],
  );

  const openInPane = (paneIndex: 0 | 1, tab: Tab) => {
    setPanes((current) => {
      const next = [...current] as [Pane, Pane];
      const already = next[paneIndex].tabs.some((t) => t.id === tab.id);
      next[paneIndex] = {
        tabs: already ? next[paneIndex].tabs : [...next[paneIndex].tabs, tab],
        activeId: tab.id,
      };
      return next;
    });
  };

  /** Point an existing tab at a different book, keeping its place in the strip. */
  const switchBook = (paneIndex: 0 | 1, tabId: string, next: Book) => {
    setPanes((current) => {
      const pane = current[paneIndex];
      const replacement = bookTab(next);
      // Already open in this pane: just go to it rather than making a second
      // tab for the same book.
      if (pane.tabs.some((t) => t.id === replacement.id)) {
        const copy = [...current] as [Pane, Pane];
        copy[paneIndex] = { ...pane, activeId: replacement.id };
        return copy;
      }
      const copy = [...current] as [Pane, Pane];
      copy[paneIndex] = {
        tabs: pane.tabs.map((t) => (t.id === tabId ? replacement : t)),
        activeId: replacement.id,
      };
      return copy;
    });
  };

  const closeTab = (paneIndex: 0 | 1, tabId: string) => {
    setPanes((current) => {
      const next = [...current] as [Pane, Pane];
      const pane = next[paneIndex];
      const tabs = pane.tabs.filter((t) => t.id !== tabId);
      next[paneIndex] = {
        tabs,
        // Fall back to the last remaining tab rather than to nothing, so
        // closing one does not leave an empty pane with a live neighbour.
        activeId:
          pane.activeId === tabId ? (tabs[tabs.length - 1]?.id ?? null) : pane.activeId,
      };
      return next;
    });
  };

  /**
   * Move a tab from one pane to the other.
   *
   * A tab dropped on the pane it already lives in is just brought to the
   * front — treating that as a move would delete and re-add it, losing its
   * place in the strip for no reason.
   */
  const moveTab = useCallback((from: 0 | 1, tabId: string, to: 0 | 1) => {
    setPanes((current) => {
      const source = current[from];
      const tab = source.tabs.find((t) => t.id === tabId);
      if (!tab) return current;

      if (from === to) {
        const next = [...current] as [Pane, Pane];
        next[to] = { ...next[to], activeId: tabId };
        return next;
      }

      const next = [...current] as [Pane, Pane];
      const remaining = source.tabs.filter((t) => t.id !== tabId);
      next[from] = {
        tabs: remaining,
        activeId:
          source.activeId === tabId
            ? (remaining[remaining.length - 1]?.id ?? null)
            : source.activeId,
      };
      const already = next[to].tabs.some((t) => t.id === tabId);
      next[to] = {
        tabs: already ? next[to].tabs : [...next[to].tabs, tab],
        activeId: tabId,
      };
      return next;
    });
    setFocused(to);
  }, []);

  /** Workspace-wide bindings: reaching a tool, and choosing a pane. */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ran = dispatch(e, [
        { key: "f", mod: true, run: () => revealTool("search") },
        { key: "1", mod: true, run: () => setFocused(0) },
        { key: "2", mod: true, run: () => setFocused(1) },
        { key: "\\", mod: true, run: () => setSplitPct(50) },
      ]);
      if (ran) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [revealTool]);

  // ---- the splitter -----------------------------------------------------

  const dragging = useRef(false);
  const frame = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      if (!dragging.current || !frame.current) return;
      const box = frame.current.getBoundingClientRect();
      const pct = ((e.clientX - box.left) / box.width) * 100;
      // A pane may be closed entirely — reading full width is a real thing to
      // want — but it snaps rather than sliding through unusable widths, and
      // the splitter itself stays grabbable so it can always be brought back.
      const clamped = Math.min(100, Math.max(0, pct));
      setSplitPct(clamped < 8 ? 0 : clamped > 92 ? 100 : Math.max(15, Math.min(85, clamped)));
    };
    const up = () => {
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, []);

  const bookInPane = (index: 0 | 1) =>
    panes[index].tabs.find((t) => t.id === panes[index].activeId)?.bookId ?? null;

  const workingBook = books.find((b) => b.id === workingBookId) ?? book;
  const workingBlock = blockByBook[workingBookId] ?? null;

  /**
   * Go to a passage, wherever the book happens to be.
   *
   * Three cases, and the last two used to fail without saying anything: the
   * book is in front, the book is open on a background tab, or the book is not
   * open at all. The pane is brought to the front first, and the scroll is
   * left as a pending jump for the book to honour once its text has loaded.
   */
  const goTo = useCallback(
    (bookId: number, blockId: number, into?: 0 | 1) => {
      setWorkingBookId(bookId);

      const tabId = `book:${bookId}`;
      let found = false;
      setPanes((current) => {
        // An explicit side wins: "open this on the right" has to put it on the
        // right even when the book is already open on the left.
        const index =
          into != null
            ? current[into].tabs.some((t) => t.id === tabId)
              ? into
              : -1
            : current.findIndex((p) => p.tabs.some((t) => t.id === tabId));
        if (index < 0) return current;
        found = true;
        if (current[index].activeId === tabId) return current;
        const next = [...current] as [Pane, Pane];
        next[index] = { ...next[index], activeId: tabId };
        return next;
      });

      if (!found) {
        const target = books.find((b) => b.id === bookId);
        if (!target) return;
        // Into the pane not showing a book, so following a citation never
        // replaces the passage that prompted it.
        setPanes((current) => {
          const holdsBook = (p: Pane) =>
            p.tabs.find((t) => t.id === p.activeId)?.kind === "book";
          const side: 0 | 1 =
            into ?? (!holdsBook(current[1]) ? 1 : !holdsBook(current[0]) ? 0 : 1);
          const next = [...current] as [Pane, Pane];
          next[side] = {
            tabs: next[side].tabs.some((t) => t.id === tabId)
              ? next[side].tabs
              : [...next[side].tabs, bookTab(target)],
            activeId: tabId,
          };
          return next;
        });
      }

      // Set regardless: a book already in front still has to scroll, and a
      // freshly opened one needs its text before it can.
      setPendingJump((prev) => ({ bookId, blockId, seq: (prev?.seq ?? 0) + 1 }));
    },
    [books],
  );

  goToRef.current = goTo;

  const renderTab = (paneIndex: 0 | 1, tab: Tab) => {
    if (tab.kind === "book") {
      const b = books.find((x) => x.id === tab.bookId);
      if (!b) return <p className="p-4 text-sm text-ink-soft">Loading…</p>;
      return (
        <BookView
          book={b}
          selection={selection}
          onSelect={setSelection}
          onContext={showContext}
          onLookUpWord={showWord}
          annotationRevision={revision}
          onAnnotationsChanged={bump}
          registerBookApi={registerBookApi}
          onActiveBlock={onActiveBlock}
          pendingBlock={
            pendingJump?.bookId === b.id ? pendingJump.blockId : null
          }
          onPendingConsumed={() => setPendingJump(null)}
          onQuickNote={() => {
            revealTool("notes");
            setFocusSignal((n) => n + 1);
          }}
          onQuickTerm={() => {
            revealTool("terms");
            setFocusSignal((n) => n + 1);
          }}
          onSwitchBook={() =>
            setPicker({ mode: "switch", pane: paneIndex, tabId: tab.id })
          }
          onPosition={onPosition}
          onScanReferences={async () => {
            await api.scanReferences(b.id);
            bump();
          }}
          isFocused={focused === paneIndex}
          onFocus={() => {
            setFocused(paneIndex);
            setWorkingBookId(b.id);
          }}
        />
      );
    }

    switch (tab.tool) {
      case "context":
        return (
          <ContextPanel
            target={context}
            word={word}
            pinned={pinned}
            onPin={() => setPinned(pinned ? null : { target: context, word })}
            onGoTo={goTo}
            onSearch={(q) => {
              setSearchSeed(q);
              revealTool("search");
            }}
          />
        );
      case "search":
        return (
          <SearchPanel
            books={books}
            scopeBookId={workingBookId}
            initialQuery={searchSeed}
            onGoTo={goTo}
          />
        );
      case "method":
        // Driven by whichever book pane last had a paragraph clicked, so the
        // method can sit in one pane and work on the book in the other.
        return (
          <MethodPanel
            book={workingBook}
            block={workingBlock}
            position={positionByBook[workingBookId] ?? null}
            onAdvance={() => bookApis.current.get(workingBookId)?.advance()}
            onSummarySaved={bump}
          />
        );
      case "outline":
        return (
          <PageNavigator
            bookId={workingBook.id}
            currentPageId={null}
            onSelect={(pageId) => {
              // The outline speaks in pages; the column speaks in blocks.
              api
                .bookBlocks(workingBook.id)
                .then((bs) => {
                  const first = bs.find((b) => b.page_id === pageId);
                  if (first) goTo(workingBook.id, first.id);
                })
                .catch(() => {});
            }}
            revision={revision}
          />
        );
      case "notes":
        return (
          <NotesPanel
            bookId={workingBook.id}
            bookTitle={workingBook.title}
            focusSignal={focusSignal}
            selection={selection}
            revision={revision}
            onChanged={bump}
            onGoTo={(blockId) => goTo(workingBook.id, blockId)}
          />
        );
      case "terms":
        return (
          <TermsPanel
            bookId={workingBook.id}
            bookTitle={workingBook.title}
            focusSignal={focusSignal}
            selection={selection}
            passage={workingBlock?.text_norm ?? selection?.text ?? ""}
            revision={revision}
            onChanged={bump}
            onGoTo={(blockId) => goTo(workingBook.id, blockId)}
          />
        );
      case "arguments":
        return (
          <ArgumentsPanel
            bookId={workingBook.id}
            bookTitle={workingBook.title}
            selection={selection}
            revision={revision}
            onChanged={bump}
            onGoTo={(blockId) => goTo(workingBook.id, blockId)}
          />
        );
      case "export":
        return <ExportPanel book={workingBook} />;
      case "spine":
        return (
          <SpinePanel
            bookId={workingBook.id}
            bookTitle={workingBook.title}
            revision={revision}
            onGoTo={(blockId) => goTo(workingBook.id, blockId)}
          />
        );
      case "words":
        return (
          <WordsPanel
            bookId={workingBook.id}
            bookTitle={workingBook.title}
            revision={revision}
            onLookAgain={(w, sentence) => {
              // Straight back into Context, as though it had been
              // double-clicked in the text again.
              setContext(null);
              setWord({ word: w, sentence, blockId: null, x: 0, y: 0 });
              revealTool("context");
            }}
          />
        );
      case "moves":
        return <MoveGlossary moves={moves} />;
      default:
        return null;
    }
  };

  return (
    <div className="flex h-full min-h-0">
      <nav className="flex w-[4.75rem] flex-none flex-col items-center gap-0.5 overflow-y-auto border-r border-rule bg-paper-dim px-1.5 py-2">
        <button
          onClick={onCloseBook}
          title="Back to the library"
          className="rail-item"
        >
          <span aria-hidden="true" className="text-base leading-none">⌂</span>
          Library
        </button>

        {TOOL_GROUPS.map(({ group, tools }) => (
          <div key={group} className="w-full">
            <p className="rail-group">{group}</p>
            {tools.map((id) => {
              const tool = TOOLS.find((t) => t.id === id)!;
              const showing = panes.some((p) => p.activeId === `tool:${id}`);
              return (
                <button
                  key={id}
                  onClick={() => revealTool(id)}
                  title={tool.label}
                  aria-current={showing}
                  className="rail-item"
                >
                  <span aria-hidden="true" className="text-base leading-none">
                    {tool.icon}
                  </span>
                  {tool.label}
                </button>
              );
            })}
          </div>
        ))}
      </nav>

      <div ref={frame} className="flex min-h-0 flex-1">
        <PaneView
          pane={panes[0]}
          paneIndex={0}
          width={`${splitPct}%`}
          focused={focused === 0}
          onFocus={() => setFocused(0)}
          onSelectTab={(id) => {
            setPanes((c) => [{ ...c[0], activeId: id }, c[1]]);
            const b = panes[0].tabs.find((t) => t.id === id)?.bookId;
            if (b != null) setWorkingBookId(b);
          }}
          onCloseTab={(id) => closeTab(0, id)}
          onOpen={(tab) => openInPane(0, tab)}
          onOpenBook={() => setPicker({ mode: "open", pane: 0 })}
          onDropTab={moveTab}
          render={(tab) => renderTab(0, tab)}
        />

        <div className="relative flex-none">
          <button
            onClick={() => setLinked((v) => !v)}
            title={
              linked
                ? "Panes are following each other — click to unlink"
                : "Link the panes: a reference in one moves the other to match"
            }
            aria-pressed={linked}
            className={`absolute left-1/2 top-2 z-10 -translate-x-1/2 rounded border px-1 py-0.5 text-[0.6rem] ${
              linked
                ? "border-accent bg-accent text-paper"
                : "border-rule bg-paper text-ink-soft hover:border-accent hover:text-accent"
            }`}
          >
            {linked ? "⇄" : "⇹"}
          </button>
        </div>

        <div
          onMouseDown={() => {
            dragging.current = true;
            document.body.style.cursor = "col-resize";
            document.body.style.userSelect = "none";
          }}
          onDoubleClick={() => setSplitPct(50)}
          title={
            splitPct === 0 || splitPct === 100
              ? "Drag out the hidden pane · double-click to even them up"
              : "Drag to resize · double-click to even them up"
          }
          className={`flex-none cursor-col-resize transition-colors hover:bg-accent ${
            splitPct === 0 || splitPct === 100
              ? "w-2 bg-accent/40"
              : "w-1 bg-rule"
          }`}
        />

        <PaneView
          pane={panes[1]}
          paneIndex={1}
          width={`${100 - splitPct}%`}
          focused={focused === 1}
          onFocus={() => setFocused(1)}
          onSelectTab={(id) => {
            setPanes((c) => [c[0], { ...c[1], activeId: id }]);
            const b = panes[1].tabs.find((t) => t.id === id)?.bookId;
            if (b != null) setWorkingBookId(b);
          }}
          onCloseTab={(id) => closeTab(1, id)}
          onOpen={(tab) => openInPane(1, tab)}
          onOpenBook={() => setPicker({ mode: "open", pane: 1 })}
          onDropTab={moveTab}
          render={(tab) => renderTab(1, tab)}
        />
      </div>

      {picker && (
        <BookPicker
          books={books}
          title={
            picker.mode === "open"
              ? "Open a book"
              : "Show a different book in this tab"
          }
          onChoose={(chosen) => {
            if (picker.mode === "open") {
              openInPane(picker.pane, bookTab(chosen));
            } else if (picker.tabId) {
              switchBook(picker.pane, picker.tabId, chosen);
            }
            setWorkingBookId(chosen.id);
            setPicker(null);
          }}
          onClose={() => setPicker(null)}
        />
      )}
    </div>
  );
}

/**
 * One pane: a strip of tabs over whichever one is showing.
 *
 * The strip scrolls sideways when there are more tabs than room, which is why
 * the "open something" menu is a sibling of the strip rather than a child of
 * it: an absolutely-positioned menu inside an `overflow-x: auto` box is
 * clipped to the height of that box, so it opens and is instantly invisible.
 */
function PaneView({
  pane,
  paneIndex,
  width,
  focused,
  onFocus,
  onSelectTab,
  onCloseTab,
  onOpen,
  onOpenBook,
  onDropTab,
  render,
}: {
  pane: Pane;
  paneIndex: 0 | 1;
  width: string;
  focused: boolean;
  onFocus: () => void;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onOpen: (tab: Tab) => void;
  /** Opens the searchable book picker rather than listing every book inline. */
  onOpenBook: () => void;
  /** A tab dragged in from either pane, dropped here. */
  onDropTab: (from: 0 | 1, tabId: string, to: 0 | 1) => void;
  render: (tab: Tab) => React.ReactNode;
}) {
  const [menu, setMenu] = useState(false);
  const [dropTarget, setDropTarget] = useState(false);
  const active = pane.tabs.find((t) => t.id === pane.activeId);

  const accept = (e: React.DragEvent) => {
    const raw = e.dataTransfer.getData("application/x-tab");
    if (!raw) return;
    e.preventDefault();
    setDropTarget(false);
    try {
      const { from, tabId } = JSON.parse(raw) as { from: 0 | 1; tabId: string };
      onDropTab(from, tabId, paneIndex);
    } catch {
      // A drag from outside the application. Nothing to do.
    }
  };

  return (
    <section
      onMouseDown={onFocus}
      style={{ width }}
      className="flex min-h-0 min-w-0 flex-col"
    >
      <div className="relative flex-none">
        <div
          className={`tabstrip ${dropTarget ? "tabstrip-drop" : ""} ${
            focused ? "tabstrip-focused" : ""
          }`}
          role="tablist"
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes("application/x-tab")) {
              e.preventDefault();
              setDropTarget(true);
            }
          }}
          onDragLeave={() => setDropTarget(false)}
          onDrop={accept}
        >
          {pane.tabs.map((t) => (
            <div
              key={t.id}
              role="tab"
              tabIndex={0}
              aria-selected={t.id === pane.activeId}
              title={t.label}
              draggable
              onDragStart={(e) => {
                e.dataTransfer.setData(
                  "application/x-tab",
                  JSON.stringify({ from: paneIndex, tabId: t.id }),
                );
                e.dataTransfer.effectAllowed = "move";
              }}
              onClick={() => onSelectTab(t.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") onSelectTab(t.id);
              }}
              onAuxClick={(e) => {
                // Middle click closes, as it does everywhere else with tabs.
                if (e.button === 1) onCloseTab(t.id);
              }}
              className="tab group"
            >
              <span className="max-w-[11rem] truncate">{t.label}</span>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseTab(t.id);
                }}
                title={`Close ${t.label}`}
                aria-label={`Close ${t.label}`}
                className="tab-close"
              >
                ✕
              </button>
            </div>
          ))}

          <button
            onClick={() => setMenu((v) => !v)}
            title="Open a book or a tool here"
            aria-expanded={menu}
            className="tab tab-add"
          >
            +
          </button>
        </div>

        {menu && (
          <>
            <div className="fixed inset-0 z-20" onClick={() => setMenu(false)} />
            <div className="absolute left-2 top-full z-30 mt-0.5 max-h-80 w-60 overflow-y-auto rounded border border-rule bg-paper py-1 shadow-xl">
              {/* One entry rather than every book: a library of a hundred
                  turns an inline list into an unusable wall. */}
              <button
                onClick={() => {
                  onOpenBook();
                  setMenu(false);
                }}
                className="block w-full px-3 py-1.5 text-left text-sm font-medium hover:bg-paper-dim hover:text-accent"
              >
                Open a book…
              </button>
              <p className="mt-1 border-t border-rule px-3 py-1 pt-2 text-[0.65rem] uppercase tracking-wider text-ink-soft">
                Tools
              </p>
              {TOOLS.map((t) => (
                <button
                  key={t.id}
                  onClick={() => {
                    onOpen(toolTab(t.id));
                    setMenu(false);
                  }}
                  className="block w-full px-3 py-1.5 text-left text-sm hover:bg-paper-dim hover:text-accent"
                >
                  {t.label}
                </button>
              ))}
            </div>
          </>
        )}
      </div>

      <div
        className="min-h-0 flex-1"
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("application/x-tab")) e.preventDefault();
        }}
        onDrop={accept}
      >
        {pane.tabs.map((t) => {
          const showing = t.id === pane.activeId;
          // Tools are cheap and keep their state in the database, so only the
          // one on top is mounted. Books are not: unmounting one loses the
          // reader's place and takes its jump handle with it.
          if (t.kind === "tool" && !showing) return null;
          return (
            <div
              key={t.id}
              className={showing ? "h-full overflow-y-auto" : "hidden"}
            >
              {render(t)}
            </div>
          );
        })}
        {!active && (
          <p className="p-6 text-center text-sm text-ink-soft">
            Nothing open here. Use <strong>+</strong> to open a book or a tool,
            or drag a tab across from the other side.
          </p>
        )}
      </div>
    </section>
  );
}
