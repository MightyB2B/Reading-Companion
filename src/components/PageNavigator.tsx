import { useEffect, useMemo, useRef, useState } from "react";
import { api, type OutlinePage } from "../lib/api";

/**
 * Navigating the book by its structure rather than by a list of numbers.
 *
 * A `<select>` of page numbers is workable for the dozen pages you photograph
 * in an evening and useless for the two hundred and eighty-two an EPUB
 * produces. Pages are grouped under the chapter headings that segmentation
 * already found, each showing its opening line and how far through it you are,
 * and a filter box cuts the whole thing down when the book is long.
 */
export function PageNavigator({
  bookId,
  currentPageId,
  onSelect,
  /** Bumped by the caller when pages change, to refetch. */
  revision,
}: {
  bookId: number;
  currentPageId: number | null;
  onSelect: (pageId: number) => void;
  revision: number;
}) {
  const [open, setOpen] = useState(false);
  const [pages, setPages] = useState<OutlinePage[]>([]);
  const [query, setQuery] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const currentRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    api.bookOutline(bookId).then(setPages).catch(() => {});
  }, [bookId, revision]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // Bring the page you are on into view when the list opens.
  useEffect(() => {
    if (open) currentRef.current?.scrollIntoView({ block: "center" });
  }, [open]);

  /**
   * Group pages under the division in force at that point.
   *
   * Every division is a *span*, not a property of the page it is printed on.
   * `§ 1` runs until `§ 2` begins, which may be two pages later; a chapter
   * runs until the next chapter. Carrying each level forward is the whole
   * point — without it, a page in the middle of a section looked as though it
   * belonged nowhere, which is exactly what a photographed spread produced.
   *
   * Parts, chapters, and sections nest, so a new group starts wherever any of
   * them changes.
   */
  const groups = useMemo(() => {
    const filter = query.trim().toLowerCase();
    const matches = (p: OutlinePage) =>
      !filter ||
      p.preview.toLowerCase().includes(filter) ||
      String(p.page_no ?? "").includes(filter) ||
      p.headings.some((h) => h.text.toLowerCase().includes(filter));

    type Group = {
      part: string | null;
      chapter: string | null;
      section: string | null;
      pages: OutlinePage[];
    };
    const out: Group[] = [];
    let current: Group | null = null;
    let part: string | null = null;
    let chapter: string | null = null;
    let section: string | null = null;

    for (const page of pages) {
      const newPart = page.headings.find((h) => h.level === "part");
      const newChapter = page.headings.find((h) => h.level === "chapter");
      const newSection = page.headings.find((h) => h.level === "section");

      if (newPart) part = newPart.text;
      if (newChapter) {
        chapter = newChapter.text;
        // A new chapter clears the section: numbering restarts within it.
        section = null;
      }
      if (newSection) section = newSection.text;

      if (newPart || newChapter || newSection || !current) {
        current = { part, chapter, section, pages: [] };
        out.push(current);
      }
      if (matches(page)) current.pages.push(page);
    }
    return out.filter((g) => g.pages.length > 0);
  }, [pages, query]);

  const current = pages.find((p) => p.page_id === currentPageId);
  const position = pages.findIndex((p) => p.page_id === currentPageId);

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex items-center gap-2 rounded border border-rule px-2.5 py-1 text-xs hover:border-accent hover:text-accent"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
          <path d="M4 6h16M4 12h16M4 18h10" />
        </svg>
        <span className="tabular-nums">{label(current, position, pages.length)}</span>
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>

      {open && (
        <div className="panel-in absolute left-0 z-50 mt-1.5 w-[26rem] overflow-hidden rounded-lg border border-rule bg-paper shadow-xl">
          {pages.length > 8 && (
            <div className="border-b border-rule p-2">
              <input
                autoFocus
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Find a chapter or a phrase…"
                className="w-full rounded border border-rule bg-transparent px-2.5 py-1.5 text-sm outline-none focus:border-accent"
              />
            </div>
          )}

          <div className="max-h-[26rem] overflow-y-auto py-1">
            {groups.length === 0 && (
              <p className="px-3 py-6 text-center text-sm text-ink-soft">
                {pages.length === 0 ? "Nothing added yet." : "No page matches that."}
              </p>
            )}

            {groups.map((group, gi) => {
              // Repeat a level only when it changes, so a chapter spanning
              // many sections is stated once rather than over every one.
              const previous = groups[gi - 1];
              const showPart = group.part && group.part !== previous?.part;
              const showChapter =
                group.chapter && group.chapter !== previous?.chapter;
              return (
              <div key={gi}>
                {showPart && (
                  <div className="px-3 pb-0.5 pt-3 text-[0.65rem] font-medium uppercase tracking-[0.14em] text-ink-soft">
                    {group.part}
                  </div>
                )}
                {showChapter && (
                  <div className="px-3 pb-0.5 pt-1.5 text-[0.72rem] font-semibold uppercase tracking-wider text-accent">
                    {group.chapter}
                  </div>
                )}
                {group.section && (
                  <div className="sticky top-0 z-10 bg-paper/95 px-3 pb-1 pt-1 font-serif text-[0.84rem] italic backdrop-blur">
                    {group.section}
                  </div>
                )}
                {group.pages.map((page) => {
                  const isCurrent = page.page_id === currentPageId;
                  return (
                    <button
                      key={page.page_id}
                      ref={isCurrent ? currentRef : undefined}
                      onClick={() => {
                        onSelect(page.page_id);
                        setOpen(false);
                      }}
                      className={`flex w-full items-start gap-2.5 px-3 py-2 text-left transition-colors hover:bg-paper-dim ${
                        isCurrent ? "bg-paper-dim" : ""
                      }`}
                    >
                      <span
                        className={`mt-0.5 w-9 shrink-0 text-right text-xs tabular-nums ${
                          isCurrent ? "font-semibold text-accent" : "text-ink-soft"
                        }`}
                      >
                        {page.page_no ?? "—"}
                      </span>

                      <span className="min-w-0 flex-1">
                        {/* Topics sit inside the section shown above, so they
                            belong on the page rather than in the header. */}
                        {page.headings
                          .filter((h) => h.level === "topic")
                          .map((h) => (
                            <span
                              key={h.block_id}
                              className="block truncate font-serif text-[0.82rem]"
                            >
                              {h.text}
                            </span>
                          ))}
                        <span className="block truncate text-xs text-ink-soft">
                          {page.preview || statusText(page)}
                        </span>
                      </span>

                      <Progress page={page} />
                    </button>
                  );
                })}
              </div>
              );
            })}
          </div>

          {pages.length > 0 && (
            <div className="border-t border-rule px-3 py-1.5 text-[0.7rem] text-ink-soft">
              {pages.length} page{pages.length === 1 ? "" : "s"} ·{" "}
              {pages.reduce((n, p) => n + p.summarised, 0)} of{" "}
              {pages.reduce((n, p) => n + p.paragraphs, 0)} paragraphs summarised
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function label(
  current: OutlinePage | undefined,
  position: number,
  total: number,
): string {
  if (!current) return total ? "Choose a page" : "No pages yet";
  if (current.page_no != null) return `Page ${current.page_no}`;
  return `${position + 1} of ${total}`;
}

function statusText(page: OutlinePage): string {
  switch (page.ocr_status) {
    case "pending":
      return "not yet transcribed";
    case "running":
      return "transcribing…";
    case "failed":
      return "could not be transcribed";
    default:
      return "no text on this page";
  }
}

/**
 * How much of the page has been worked through, as a ring rather than a bar —
 * it has to read at 16 pixels beside a line of text.
 */
function Progress({ page }: { page: OutlinePage }) {
  if (page.paragraphs === 0) return null;
  const done = page.summarised / page.paragraphs;
  const circumference = 2 * Math.PI * 7;

  return (
    <span
      className="mt-0.5 shrink-0"
      title={`${page.summarised} of ${page.paragraphs} paragraphs summarised`}
    >
      <svg width="16" height="16" viewBox="0 0 18 18" aria-hidden="true">
        <circle cx="9" cy="9" r="7" fill="none" stroke="currentColor" strokeWidth="2" className="text-rule" />
        {done > 0 && (
          <circle
            cx="9"
            cy="9"
            r="7"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            className="text-accent"
            strokeDasharray={circumference}
            strokeDashoffset={circumference * (1 - done)}
            transform="rotate(-90 9 9)"
          />
        )}
      </svg>
    </span>
  );
}
