import { useEffect, useState } from "react";
import { api, type Book, type OllamaStatus } from "./lib/api";
import { Library } from "./components/Library";
import { Reader } from "./components/Reader";
import { SettingsWindow } from "./components/SettingsWindow";
import { ThemePicker } from "./components/ThemePicker";
import "./index.css";

export default function App() {
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [book, setBook] = useState<Book | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const refreshStatus = () =>
    api.checkOllama().then(setStatus).catch(() => setStatus(null));

  useEffect(() => {
    refreshStatus();
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
          <button
            onClick={() => setSettingsOpen(true)}
            title="Settings"
            aria-label="Settings"
            className="rounded p-1.5 text-ink-soft hover:bg-paper-dim hover:text-accent"
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.14.36.4.66.74.86H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </button>
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

      {settingsOpen && (
        <SettingsWindow
          onClose={() => setSettingsOpen(false)}
          // Re-check after saving: pointing at another machine changes what
          // the badge should say, and it should say it immediately.
          onSaved={refreshStatus}
        />
      )}
    </div>
  );
}

/**
 * Says something only when something is wrong.
 *
 * A badge reporting that everything is fine is noise on every page of every
 * session, and it earns its space perhaps once. The failure states stay,
 * because those are the ones you can act on.
 */
function OllamaBadge({ status }: { status: OllamaStatus | null }) {
  if (!status) return null;

  const ok = status.reachable && status.ocr_model_ready && status.text_model_ready;
  if (ok) return null;

  return (
    <span
      className="flex items-center gap-2 text-xs text-ink-soft"
      title={status.message}
    >
      <span
        className={`inline-block h-2 w-2 rounded-full ${
          status.reachable ? "bg-amber-500" : "bg-red-500"
        }`}
      />
      {status.reachable ? "models missing" : "offline"}
    </span>
  );
}
