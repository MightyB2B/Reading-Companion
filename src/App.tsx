import { useEffect, useState } from "react";
import { api, type Book, type OllamaStatus } from "./lib/api";
import { Library } from "./components/Library";
import { Reader } from "./components/Reader";
import { ThemePicker } from "./components/ThemePicker";
import "./index.css";

export default function App() {
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [book, setBook] = useState<Book | null>(null);

  useEffect(() => {
    api.checkOllama().then(setStatus).catch(() => setStatus(null));
  }, []);

  return (
    <div className="flex h-full flex-col bg-paper text-ink">
      <header className="flex items-center gap-4 border-b border-rule px-5 py-3">
        <button
          onClick={() => setBook(null)}
          className="text-sm font-semibold tracking-tight hover:text-accent"
        >
          Reading Companion
        </button>
        {book && (
          <span className="truncate text-sm text-ink-soft">
            {book.title}
            {book.author ? ` — ${book.author}` : ""}
          </span>
        )}
        <div className="ml-auto flex items-center gap-3">
          <OllamaBadge status={status} />
          <ThemePicker />
        </div>
      </header>

      {status && !status.reachable && (
        <div className="border-b border-rule bg-paper-dim px-5 py-3 text-sm">
          <strong>Ollama is not running.</strong> {status.message}
          <p className="mt-1 text-ink-soft">
            Everything here runs on your machine; nothing about the books you read
            leaves it. Start Ollama and reload.
          </p>
        </div>
      )}

      {status?.reachable && (!status.ocr_model_ready || !status.text_model_ready) && (
        <div className="border-b border-rule bg-paper-dim px-5 py-3 text-sm">
          {status.message}
        </div>
      )}

      <main className="min-h-0 flex-1">
        {book ? (
          <Reader book={book} />
        ) : (
          <Library onOpen={setBook} />
        )}
      </main>
    </div>
  );
}

function OllamaBadge({ status }: { status: OllamaStatus | null }) {
  if (!status) {
    return <span className="text-xs text-ink-soft">checking…</span>;
  }
  const ok = status.reachable && status.ocr_model_ready && status.text_model_ready;
  return (
    <span
      className="flex items-center gap-2 text-xs text-ink-soft"
      title={status.message}
    >
      <span
        className={`inline-block h-2 w-2 rounded-full ${
          ok ? "bg-emerald-600" : status.reachable ? "bg-amber-500" : "bg-red-500"
        }`}
      />
      {ok ? "local models ready" : status.reachable ? "models missing" : "offline"}
    </span>
  );
}
