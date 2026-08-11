import { useEffect, useState } from "react";
// Tauri's dialog, not window.confirm — WebView2 does not reliably implement it.
import { confirm } from "@tauri-apps/plugin-dialog";
import { api, ERA_LABELS, type Book, type Era } from "../lib/api";

export function Library({ onOpen }: { onOpen: (b: Book) => void }) {
  const [books, setBooks] = useState<Book[]>([]);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => api.listBooks().then(setBooks).catch((e) => setError(String(e)));
  useEffect(() => {
    refresh();
  }, []);

  /**
   * Delete a book, having said plainly what that costs.
   *
   * The reader's summaries are the part that cannot be recovered — the pages
   * can be photographed again, the thinking cannot — so the prompt counts
   * them rather than asking "are you sure?".
   */
  const remove = async (book: Book) => {
    let detail = "";
    try {
      const s = await api.bookStats(book.id);
      const parts = [
        `${s.pages} page${s.pages === 1 ? "" : "s"}`,
        `${s.paragraphs} paragraph${s.paragraphs === 1 ? "" : "s"}`,
      ];
      if (s.summaries > 0) {
        parts.push(
          `${s.summaries} summar${s.summaries === 1 ? "y" : "ies"} you wrote`,
        );
      }
      if (s.words_looked_up > 0) {
        parts.push(`${s.words_looked_up} saved word${s.words_looked_up === 1 ? "" : "s"}`);
      }
      detail = `\n\nThis removes ${parts.join(", ")}. It cannot be undone.`;
    } catch {
      // Worth confirming even if the tally cannot be fetched.
    }

    const ok = await confirm(`Delete “${book.title}”?${detail}`, {
      title: "Delete book",
      kind: "warning",
    });
    if (!ok) return;

    try {
      await api.deleteBook(book.id);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="mx-auto max-w-3xl px-6 py-10">
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold tracking-tight">Your books</h1>
        <button
          onClick={() => setAdding((v) => !v)}
          className="rounded border border-rule px-3 py-1.5 text-sm hover:border-accent hover:text-accent"
        >
          {adding ? "Cancel" : "Add a book"}
        </button>
      </div>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

      {adding && (
        <NewBookForm
          onCreated={() => {
            setAdding(false);
            refresh();
          }}
          onError={setError}
        />
      )}

      {books.length === 0 && !adding && (
        <p className="mt-8 text-sm text-ink-soft">
          Nothing here yet. Add a book, then photograph its pages one at a time.
        </p>
      )}

      <ul className="mt-6 divide-y divide-rule">
        {books.map((b) => (
          <li key={b.id} className="group flex items-center gap-2">
            <button
              onClick={() => onOpen(b)}
              className="min-w-0 flex-1 py-3 text-left hover:text-accent"
            >
              <div className="truncate font-medium">{b.title}</div>
              <div className="truncate text-sm text-ink-soft">
                {b.author ? `${b.author} · ` : ""}
                {ERA_LABELS[b.era as Era] ?? b.era}
              </div>
            </button>
            <button
              onClick={() => remove(b)}
              title={`Delete ${b.title}`}
              aria-label={`Delete ${b.title}`}
              className="shrink-0 rounded p-2 text-ink-soft opacity-0 transition-opacity hover:bg-paper-dim hover:text-red-600 focus:opacity-100 group-hover:opacity-100"
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6" />
              </svg>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function NewBookForm({
  onCreated,
  onError,
}: {
  onCreated: () => void;
  onError: (e: string) => void;
}) {
  const [title, setTitle] = useState("");
  const [author, setAuthor] = useState("");
  const [era, setEra] = useState<Era>("modern");

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await api.createBook(title, author.trim() || null, era);
      onCreated();
    } catch (err) {
      onError(String(err));
    }
  };

  return (
    <form onSubmit={submit} className="mt-6 space-y-3 rounded border border-rule p-4">
      <input
        autoFocus
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        placeholder="Title"
        className="w-full border-b border-rule bg-transparent py-1.5 outline-none focus:border-accent"
      />
      <input
        value={author}
        onChange={(e) => setAuthor(e.target.value)}
        placeholder="Author (optional)"
        className="w-full border-b border-rule bg-transparent py-1.5 outline-none focus:border-accent"
      />
      <div>
        <label className="text-sm text-ink-soft">When was it written?</label>
        <select
          value={era}
          onChange={(e) => setEra(e.target.value as Era)}
          className="mt-1 w-full rounded border border-rule bg-paper px-2 py-1.5 text-sm"
        >
          {Object.entries(ERA_LABELS).map(([k, label]) => (
            <option key={k} value={k}>
              {label}
            </option>
          ))}
        </select>
        {/* The era is not cosmetic: it decides which dictionary is
            authoritative, how hard the text is modernised, and how the coach
            calibrates its feedback. */}
        <p className="mt-1.5 text-xs text-ink-soft">
          This sets which dictionary counts as authoritative and how the coach
          reads the prose. An older setting expects longer sentences and looks
          words up as they were then understood.
        </p>
      </div>
      <button
        type="submit"
        disabled={!title.trim()}
        className="rounded bg-accent px-3 py-1.5 text-sm text-paper disabled:opacity-40"
      >
        Create
      </button>
    </form>
  );
}
