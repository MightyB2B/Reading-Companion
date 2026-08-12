import { useCallback, useEffect, useRef, useState } from "react";
// File pickers now live in the wizard; only the delete confirmation is here.
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  api,
  onJobProgress,
  onOcrProgress,
  sentenceAround,
  type Block,
  type BlockKind,
  type Book,
  type JobProgress,
  type Page,
} from "../lib/api";
import {
  AddContentWizard,
  type SourceChoice,
  type WizardResult,
} from "./AddContentWizard";
import { MethodPanel } from "./MethodPanel";
import { PageNavigator } from "./PageNavigator";
import { WordPopover } from "./WordPopover";

interface Selected {
  word: string;
  sentence: string;
  blockId: number;
  x: number;
  y: number;
}

export function Reader({ book }: { book: Book }) {
  const [pages, setPages] = useState<Page[]>([]);
  const [pageId, setPageId] = useState<number | null>(null);
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [activeBlock, setActiveBlock] = useState<number | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selected, setSelected] = useState<Selected | null>(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  // Page numbers decide reading order and which paragraph continues which, so
  // a misread one has to be correctable without re-importing.
  const [renumbering, setRenumbering] = useState(false);
  const [job, setJob] = useState<JobProgress | null>(null);
  // The sentence currently being worked on, lit up in the page text.
  const [activeSentence, setActiveSentence] = useState<string | null>(null);
  /**
   * A page turn in progress.
   *
   * `leaving` is a frozen copy of the page being turned away from. The live
   * pane switches to the next page immediately and the copy is animated over
   * the top of it, so the page you can interact with is never the one being
   * rotated — which is what makes this work at all. StPageFlip renders the
   * real thing, but it clones the page elements into its own wrapper, and a
   * clone has no click-to-focus, no word selection, and no inline correction.
   */
  const [turn, setTurn] = useState<{
    direction: "forward" | "back";
    leaving: Block[];
  } | null>(null);

  // Bumped whenever pages change, so the navigator refetches its outline.
  const [outlineRevision, setOutlineRevision] = useState(0);

  const refreshPages = useCallback(
    () =>
      api.listPages(book.id).then((p) => {
        setPages(p);
        setOutlineRevision((r) => r + 1);
      }),
    [book.id],
  );

  useEffect(() => {
    refreshPages().catch((e) => setError(String(e)));
  }, [refreshPages]);

  // Open where the reader stopped. The backend checks the remembered page
  // still exists, so one deleted or re-imported falls back to the beginning
  // rather than leaving the pane empty.
  useEffect(() => {
    let cancelled = false;
    api
      .resumePage(book.id)
      .then((id) => {
        if (!cancelled && id != null) setPageId((current) => current ?? id);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [book.id]);

  // Record the page on every turn rather than at shutdown: a crash or a force
  // quit would lose a place saved only on the way out.
  useEffect(() => {
    if (pageId == null) return;
    api.setLastPage(book.id, pageId).catch(() => {});
  }, [book.id, pageId]);

  useEffect(() => {
    if (pageId == null) return;
    api.listBlocks(pageId).then((b) => {
      setBlocks(b);
      setActiveBlock(b.find((x) => x.kind === "paragraph")?.id ?? null);
    });
  }, [pageId]);

  // Live transcription, so importing a page shows the text appearing rather
  // than a spinner sitting still for several seconds.
  useEffect(() => {
    const un = onOcrProgress((p) => setProgress((s) => s + p.chunk));
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const un = onJobProgress(setJob);
    return () => {
      un.then((f) => f());
    };
  }, []);

  /**
   * Import a photograph: transcribe it, then number it.
   *
   * Returns a sentence for the wizard rather than setting a notice, so the
   * outcome is reported where the reader is already looking.
   */
  const importAndOcr = async (selected: string): Promise<WizardResult> => {
    setError(null);
    try {
      setBusy("Importing…");
      const { page_ids, page_id, converted_from, was_spread } =
        await api.importPage(book.id, selected);

      // Say so rather than silently changing the reader's file or splitting it.
      const notices = [
        was_spread
          ? "Two pages were detected and split at the spine."
          : "One page added.",
        converted_from && `Converted from ${converted_from} to JPEG.`,
      ].filter(Boolean) as string[];

      await refreshPages();
      setPageId(page_id);

      // A spread is two pages, each transcribed on its own.
      for (const [i, id] of page_ids.entries()) {
        setBusy(
          page_ids.length > 1
            ? `Reading page ${i + 1} of ${page_ids.length}…`
            : "Reading the page…",
        );
        setProgress("");
        const newBlocks = await api.runOcr(id, book.id);
        if (id === page_id) {
          setBlocks(newBlocks);
          setActiveBlock(
            newBlocks.find((b) => b.kind === "paragraph")?.id ?? null,
          );
        }
      }

      const after = await api.listPages(book.id);
      setPages(after);

      // Anything the model could not number gets asked about, so reading order
      // and cross-page stitching stay correct.
      // Handed to the wizard, which asks about them as its next step rather
      // than opening a second dialog after this one closes.
      const needsNumbers = after.filter(
        (p) => page_ids.includes(p.id) && p.page_no == null,
      );

      return { summary: notices.join(" "), needsNumbers };
    } catch (e) {
      return { summary: "", error: String(e) };
    } finally {
      setBusy(null);
      setProgress("");
      setJob(null);
    }
  };

  /**
   * Import a document whose words are already words.
   *
   * No OCR: an EPUB, a text-layer PDF, and a web article all carry real
   * characters, and putting them through a vision model would be slower and
   * strictly worse than reading them directly.
   */
  const importDocument = async (source: string): Promise<WizardResult> => {
    setError(null);
    try {
      setBusy("Importing…");
      const result = await api.importDocument(book.id, source);
      const after = await api.listPages(book.id);
      setPages(after);
      if (result.first_page_id) setPageId(result.first_page_id);

      const named = result.title ? ` from “${result.title}”` : "";
      return {
        summary:
          `${result.pages_added} page${result.pages_added === 1 ? "" : "s"} ` +
          `and ${result.blocks_added} paragraphs${named}.`,
      };
    } catch (e) {
      return { summary: "", error: String(e) };
    } finally {
      setBusy(null);
      setJob(null);
    }
  };

  /** The wizard hands back a choice; this routes it to the right importer. */
  const runImport = (choice: SourceChoice, source: string) =>
    choice === "photo" ? importAndOcr(source) : importDocument(source);

  // Pages arrive in reading order from the backend (by the number printed on
  // the page, not by import order), so index arithmetic is enough here.
  const pageIndex = pages.findIndex((p) => p.id === pageId);
  const canGoBack = pageIndex > 0;
  const canGoForward = pageIndex >= 0 && pageIndex < pages.length - 1;

  const turnTo = (direction: "forward" | "back") => {
    const next = pages[pageIndex + (direction === "forward" ? 1 : -1)];
    if (!next || turn) return;
    setSelected(null);

    // Freeze what is on screen, then move underneath it at once. The leaf
    // covers the swap, so the next page is uncovered by the turn rather than
    // appearing through it.
    setTurn({ direction, leaving: blocks });
    setPageId(next.id);
    window.setTimeout(() => setTurn(null), LEAF_DURATION);
  };

  // Arrow keys turn pages, but not while the reader is typing a summary.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName)) return;
      if (el?.isContentEditable) return;
      if (e.key === "ArrowRight" && canGoForward) turnTo("forward");
      if (e.key === "ArrowLeft" && canGoBack) turnTo("back");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  /** Drop an artefact the reader has identified as not being text. */
  const removeBlock = async (blockId: number) => {
    try {
      await api.deleteBlock(blockId);
      const remaining = blocks.filter((b) => b.id !== blockId);
      setBlocks(remaining);
      if (activeBlock === blockId) {
        setActiveBlock(remaining.find((b) => b.kind === "paragraph")?.id ?? null);
      }
      setOutlineRevision((r) => r + 1);
    } catch (e) {
      setError(String(e));
    }
  };

  /** Heading detection works from shape alone; this is the correction. */
  const reclassifyBlock = async (blockId: number, kind: BlockKind) => {
    try {
      await api.setBlockKind(blockId, kind);
      const updated = blocks.map((b) =>
        b.id === blockId ? { ...b, kind, user_edited: true } : b,
      );
      setBlocks(updated);
      // A paragraph turned into a heading is no longer something to summarise.
      if (activeBlock === blockId && kind !== "paragraph") {
        setActiveBlock(updated.find((b) => b.kind === "paragraph")?.id ?? null);
      }
      setOutlineRevision((r) => r + 1);
    } catch (e) {
      setError(String(e));
    }
  };

  const paragraphs = blocks.filter((b) => b.kind === "paragraph");
  const active = blocks.find((b) => b.id === activeBlock) ?? null;
  const activeIndex = paragraphs.findIndex((b) => b.id === activeBlock);

  const advance = () => {
    const next = paragraphs[activeIndex + 1];
    if (next) setActiveBlock(next.id);
  };

  return (
    <div className="flex h-full min-h-0">
      {/* LEFT: the page */}
      <section className="flex min-h-0 w-1/2 flex-col border-r border-rule">
        <div className="flex items-center gap-2 border-b border-rule px-4 py-2">
          <button
            onClick={() => setWizardOpen(true)}
            disabled={!!busy}
            className="flex items-center gap-1.5 rounded border border-rule px-2.5 py-1 text-xs hover:border-accent hover:text-accent disabled:opacity-40"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
              <path d="M12 5v14M5 12h14" />
            </svg>
            Add content
          </button>
          <PageNavigator
            bookId={book.id}
            currentPageId={pageId}
            onSelect={setPageId}
            revision={outlineRevision}
          />

          <div className="ml-auto flex items-center gap-1">
            <PageArrow
              direction="back"
              disabled={!canGoBack || !!turn}
              onClick={() => turnTo("back")}
            />
            {pageIndex >= 0 &&
              (renumbering ? (
                <input
                  autoFocus
                  type="number"
                  defaultValue={pages[pageIndex]?.page_no ?? ""}
                  onBlur={() => setRenumbering(false)}
                  onKeyDown={async (e) => {
                    if (e.key === "Escape") setRenumbering(false);
                    if (e.key !== "Enter") return;
                    const n = parseInt(e.currentTarget.value, 10);
                    setRenumbering(false);
                    if (!Number.isFinite(n) || pageId == null) return;
                    try {
                      await api.setPageNumber(pageId, n);
                      await refreshPages();
                    } catch (err) {
                      setError(String(err));
                    }
                  }}
                  className="w-16 rounded border border-accent bg-transparent px-1 py-0.5 text-center text-xs tabular-nums outline-none"
                />
              ) : (
                <button
                  onClick={() => setRenumbering(true)}
                  title="Set this page's number"
                  className="min-w-[3.5rem] rounded px-1 text-center text-xs tabular-nums text-ink-soft hover:bg-paper-dim hover:text-accent"
                >
                  {pages[pageIndex]?.page_no != null
                    ? pages[pageIndex].page_no
                    : `${pageIndex + 1}/${pages.length}`}
                </button>
              ))}
            <PageArrow
              direction="forward"
              disabled={!canGoForward || !!turn}
              onClick={() => turnTo("forward")}
            />
          </div>
          {pageId != null && !busy && (
            <button
              onClick={async () => {
                // Tauri's dialog rather than window.confirm, which WebView2
                // does not reliably implement.
                const ok = await confirm(
                  "Delete this page, its text, and your notes on it?",
                  { title: "Delete page", kind: "warning" },
                );
                if (!ok) return;
                const id = pageId;
                const fallback = pages.find((p) => p.id !== id)?.id ?? null;
                try {
                  await api.deletePage(id);
                  setPageId(fallback);
                  setBlocks([]);
                  await refreshPages();
                } catch (e) {
                  setError(String(e));
                }
              }}
              className="text-xs text-ink-soft hover:text-red-600"
              title="Delete this page"
            >
              delete page
            </button>
          )}
          {busy && <span className="text-xs text-ink-soft">{busy}</span>}
        </div>

        {error && <p className="px-4 py-3 text-sm text-red-600">{error}</p>}
        {notice && (
          <p className="flex items-center gap-2 px-4 py-2 text-xs text-ink-soft">
            {notice}
            <button onClick={() => setNotice(null)} className="hover:text-accent">
              dismiss
            </button>
          </p>
        )}

        <div
          className="page-stage relative min-h-0 flex-1 overflow-hidden"
          // Fed to CSS from the one constant, so the animation and the timer
          // that clears the leaf cannot drift apart.
          style={{ "--leaf-duration": `${LEAF_DURATION}ms` } as React.CSSProperties}
        >
          {/* The sheet being turned: the outgoing page on its front, blank
              paper on its back, rotating a full half-turn about the spine. */}
          {turn && (
            <>
              <div
                className={`leaf-cast-shadow leaf-cast-${turn.direction}`}
                aria-hidden="true"
              />
              <div className={`leaf leaf-${turn.direction}`} aria-hidden="true">
                <div className="leaf-face">
                  <div className="h-full overflow-hidden px-8 py-6">
                    <div className="space-y-4">
                      {turn.leaving.map((b) => (
                        <FrozenBlock key={b.id} block={b} />
                      ))}
                    </div>
                  </div>
                </div>
                <div className="leaf-face leaf-face-back" />
              </div>
            </>
          )}

          <div
            className={`h-full overflow-y-auto px-8 py-6 ${
              turn ? "page-settling" : ""
            }`}
          >
          {busy && progress && (
            <pre className="prose-page whitespace-pre-wrap text-ink-soft opacity-70">
              {progress}
            </pre>
          )}

          {!busy && blocks.length === 0 && (
            <p className="text-sm text-ink-soft">
              No text yet. Add a photograph of a page and it will be transcribed
              here, one paragraph at a time.
            </p>
          )}

          <div className="space-y-4">
            {blocks.map((b) => (
              <BlockView
                key={b.id}
                block={b}
                active={b.id === activeBlock}
                dimmed={activeBlock != null && b.id !== activeBlock}
                onFocus={() => b.kind === "paragraph" && setActiveBlock(b.id)}
                onEdited={(text) => {
                  setBlocks((bs) =>
                    bs.map((x) => (x.id === b.id ? { ...x, text_norm: text, user_edited: true } : x)),
                  );
                }}
                highlight={b.id === activeBlock ? activeSentence : null}
                onWordSelected={(word, rect) =>
                  setSelected({
                    word,
                    sentence: sentenceAround(b.text_norm, word),
                    blockId: b.id,
                    x: rect.left,
                    y: rect.bottom,
                  })
                }
                onRemove={() => removeBlock(b.id)}
                onReclassify={(kind) => reclassifyBlock(b.id, kind)}
              />
            ))}
            </div>
          </div>
        </div>
      </section>

      {/* RIGHT: the method */}
      <section className="min-h-0 w-1/2 overflow-y-auto">
        <MethodPanel
          book={book}
          block={active}
          position={activeIndex >= 0 ? { index: activeIndex, total: paragraphs.length } : null}
          onAdvance={advance}
          onActiveSentence={setActiveSentence}
          // Keeps the navigator's progress rings in step with the work.
          onSummarySaved={() => setOutlineRevision((r) => r + 1)}
        />
      </section>

      {wizardOpen && (
        <AddContentWizard
          progress={job}
          transcript={progress}
          onImport={runImport}
          onClose={() => {
            setWizardOpen(false);
            // Numbering happens inside the wizard, so the page list may have
            // changed order by the time it closes.
            refreshPages();
          }}
        />
      )}

      {selected && (
        <WordPopover
          word={selected.word}
          sentence={selected.sentence}
          blockId={selected.blockId}
          x={selected.x}
          y={selected.y}
          onClose={() => setSelected(null)}
        />
      )}
    </div>
  );
}

/**
 * How long a leaf takes to turn. Kept in step with `--leaf-duration` in the
 * stylesheet: the animation is CSS, but React has to know when to take the
 * spent leaf off the stage.
 */
const LEAF_DURATION = 620;

/**
 * A block as it appears on the sheet being turned away.
 *
 * Deliberately inert — no click target, no word selection, no editing. It
 * exists for about half a second and is only ever seen in motion, so it
 * carries the text and nothing else.
 */
function FrozenBlock({ block }: { block: Block }) {
  if (block.kind === "heading") {
    return <h2 className="prose-page font-semibold">{block.text_norm}</h2>;
  }
  return (
    <p className="prose-page px-3 py-2">{block.text_norm}</p>
  );
}

/**
 * Renders the paragraph with the sentence under discussion lit up.
 *
 * The sentence is matched by text rather than by offset, because the string
 * the workspace is working on may be the *stitched* paragraph — it can carry a
 * continuation from the neighbouring page that is not present in this block.
 * When the sentence is not found here, nothing is highlighted, which is the
 * correct outcome: that sentence lives on the other page.
 */
function HighlightedText({
  text,
  highlight,
}: {
  text: string;
  highlight: string | null;
}) {
  if (!highlight) return <>{text}</>;

  const needle = highlight.trim();
  const at = needle ? text.indexOf(needle) : -1;
  if (at < 0) return <>{text}</>;

  return (
    <>
      {text.slice(0, at)}
      <mark className="rounded bg-accent/20 px-0.5 text-ink transition-colors">
        {text.slice(at, at + needle.length)}
      </mark>
      {text.slice(at + needle.length)}
    </>
  );
}

function PageArrow({
  direction,
  disabled,
  onClick,
}: {
  direction: "forward" | "back";
  disabled: boolean;
  onClick: () => void;
}) {
  const back = direction === "back";
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={back ? "Previous page (←)" : "Next page (→)"}
      aria-label={back ? "Previous page" : "Next page"}
      className="rounded p-1.5 text-ink-soft transition-colors hover:bg-paper-dim hover:text-accent disabled:pointer-events-none disabled:opacity-25"
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d={back ? "M15 18l-6-6 6-6" : "M9 18l6-6-6-6"} />
      </svg>
    </button>
  );
}

function BlockView({
  block,
  active,
  dimmed,
  highlight,
  onFocus,
  onEdited,
  onWordSelected,
  onRemove,
  onReclassify,
}: {
  block: Block;
  active: boolean;
  dimmed: boolean;
  highlight: string | null;
  onFocus: () => void;
  onEdited: (text: string) => void;
  onWordSelected: (word: string, rect: DOMRect) => void;
  onRemove: () => void;
  onReclassify: (kind: BlockKind) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(block.text_norm);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [active]);

  const save = async () => {
    await api.editBlock(block.id, draft);
    onEdited(draft);
    setEditing(false);
  };

  if (block.kind === "heading") {
    return (
      <h2 className={`prose-page font-semibold ${dimmed ? "opacity-35" : ""}`}>
        {block.text_norm}
      </h2>
    );
  }

  if (editing) {
    return (
      <div className="rounded border border-accent p-3">
        <textarea
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          rows={Math.max(3, Math.ceil(draft.length / 60))}
          className="prose-page w-full resize-y bg-transparent outline-none"
        />
        <div className="mt-2 flex gap-2 text-xs">
          <button onClick={save} className="rounded bg-accent px-2 py-1 text-paper">
            Save
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
        </div>
      </div>
    );
  }

  // A selection of one word opens the dictionary; a longer selection is left
  // alone so the reader can still copy a phrase.
  const handleSelection = () => {
    const sel = window.getSelection();
    const text = sel?.toString().trim() ?? "";
    if (!text || /\s/.test(text) || text.length > 40) return;
    const rect = sel?.getRangeAt(0).getBoundingClientRect();
    if (rect) onWordSelected(text, rect);
  };

  return (
    <div
      ref={ref}
      onClick={onFocus}
      onMouseUp={handleSelection}
      onDoubleClick={handleSelection}
      className={`group cursor-pointer rounded px-3 py-2 transition-opacity ${
        active ? "bg-paper-dim ring-1 ring-accent/40" : ""
      } ${dimmed ? "opacity-35 hover:opacity-70" : ""}`}
    >
      <p className="prose-page selection:bg-accent/25">
        <HighlightedText text={block.text_norm} highlight={highlight} />
      </p>
      {/* Transcription leaves artefacts no rule catches — a signature mark
          fused with a folio, a caption, a line off the facing page. The reader
          can see what these are at a glance, so they get to say so. */}
      <div className="mt-1 flex items-center gap-3 text-xs text-ink-soft opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
        <button
          onClick={(e) => {
            e.stopPropagation();
            setEditing(true);
          }}
          className="hover:text-accent"
        >
          Fix the text
        </button>

        {block.kind === "paragraph" ? (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onReclassify("heading");
            }}
            title="Treat this as a heading, not something to summarise"
            className="hover:text-accent"
          >
            It's a heading
          </button>
        ) : (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onReclassify("paragraph");
            }}
            title="Treat this as prose to work through"
            className="hover:text-accent"
          >
            It's a paragraph
          </button>
        )}

        <button
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          title="Remove this — it is not part of the text"
          className="hover:text-red-600"
        >
          Not text
        </button>

        {block.user_edited && <span className="ml-auto">edited by you</span>}
      </div>
    </div>
  );
}
