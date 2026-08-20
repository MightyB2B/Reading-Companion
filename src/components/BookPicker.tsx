import { useEffect, useMemo, useRef, useState } from "react";
import { ERA_LABELS, type Book, type Era } from "../lib/api";

/**
 * Choosing a book, out of however many there are.
 *
 * A dropdown listing every book works for three and is unusable at a hundred,
 * so this is a filter box first and a list second: type a few letters of the
 * title or the author and press Enter. The keyboard path is the fast one
 * because choosing a book is something you do constantly.
 */
export function BookPicker({
  books,
  title,
  onChoose,
  onClose,
}: {
  books: Book[];
  /** What choosing will do — "Open a book" or "Switch this tab to…". */
  title: string;
  onChoose: (book: Book) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return books;
    return books.filter(
      (b) =>
        b.title.toLowerCase().includes(needle) ||
        (b.author ?? "").toLowerCase().includes(needle),
    );
  }, [books, query]);

  // A filter that leaves the cursor past the end of the list would make Enter
  // do nothing, which reads as the picker being broken.
  useEffect(() => setCursor(0), [query]);

  useEffect(() => {
    listRef.current
      ?.querySelectorAll("li")
      [cursor]?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(matches.length - 1, c + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(0, c - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const chosen = matches[cursor];
      if (chosen) onChoose(chosen);
    } else if (e.key === "Escape") {
      onClose();
    }
  };

  return (
    <div
      className="veil-in fixed inset-0 z-50 flex items-start justify-center bg-veil p-6 pt-[12vh]"
      onClick={onClose}
    >
      <div
        className="panel-in flex max-h-[70vh] w-full max-w-lg flex-col overflow-hidden rounded-xl border border-rule bg-paper shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="border-b border-rule px-4 py-3">
          <h2 className="text-sm font-semibold">{title}</h2>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKey}
            autoFocus
            placeholder="Filter by title or author"
            className="mt-2 w-full rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
          />
        </div>

        <ul ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
          {matches.length === 0 && (
            <li className="px-4 py-6 text-center text-sm text-ink-soft">
              {books.length === 0
                ? "No books yet. Add one from the library."
                : "Nothing matches that."}
            </li>
          )}
          {matches.map((b, i) => (
            <li key={b.id}>
              <button
                onMouseEnter={() => setCursor(i)}
                onClick={() => onChoose(b)}
                className={`block w-full px-4 py-2 text-left ${
                  i === cursor ? "bg-paper-dim text-accent" : "hover:bg-paper-dim"
                }`}
              >
                <span className="block truncate text-sm font-medium">
                  {b.title}
                </span>
                <span className="block truncate text-xs text-ink-soft">
                  {b.author ?? "Unknown author"} ·{" "}
                  {ERA_LABELS[b.era as Era] ?? b.era}
                </span>
              </button>
            </li>
          ))}
        </ul>

        <p className="border-t border-rule px-4 py-2 text-xs text-ink-soft">
          ↑↓ to move · Enter to choose · Esc to close
        </p>
      </div>
    </div>
  );
}
