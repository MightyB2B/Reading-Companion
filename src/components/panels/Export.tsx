import { useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, EXPORT_FORMATS, type Book, type ExportFormat } from "../../lib/api";
import { PanelHeader } from "../PanelChrome";

/**
 * Getting your work out.
 *
 * Everything written here — the spine, the glosses, the reconstructions — is
 * the reader's, and a study tool that can only be read inside itself is a
 * place work goes to be forgotten.
 */
export function ExportPanel({ book }: { book: Book }) {
  const [format, setFormat] = useState<ExportFormat>("docx");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setError(null);
    setDone(null);
    const chosen = EXPORT_FORMATS.find((f) => f.value === format)!;

    // Slashes and colons are legal in a title and illegal in a filename.
    const suggested = `${book.title.replace(/[\\/:*?"<>|]/g, "-")}.${chosen.ext}`;
    const dest = await save({
      defaultPath: suggested,
      filters: [{ name: chosen.label, extensions: [chosen.ext] }],
    });
    if (!dest) return;

    setBusy(true);
    try {
      setDone(await api.exportBook(book.id, format, dest));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <PanelHeader
        title="Export"
        purpose="Your summaries, terms, arguments and notes, written out as a document you can open anywhere."
      />
      <div className="space-y-3 px-4 py-3">
        <p className="text-sm text-ink-soft">
          Exporting <strong className="text-ink">{book.title}</strong>.
        </p>

        <fieldset className="space-y-1.5">
          <legend className="text-xs font-semibold">Format</legend>
          {EXPORT_FORMATS.map((f) => (
            <label key={f.value} className="flex items-center gap-2 text-sm">
              <input
                type="radio"
                name="export-format"
                checked={format === f.value}
                onChange={() => setFormat(f.value)}
                className="accent-[var(--color-accent)]"
              />
              {f.label}
              <span className="text-xs text-ink-soft">.{f.ext}</span>
            </label>
          ))}
        </fieldset>

        <button
          onClick={run}
          disabled={busy}
          className="rounded bg-accent px-3 py-1.5 text-xs font-semibold text-paper disabled:opacity-40"
        >
          {busy ? "Writing…" : "Choose a place and export"}
        </button>

        {done && (
          <p className="break-all rounded bg-paper-dim px-2 py-1.5 text-xs text-ink-soft">
            Written to {done}
          </p>
        )}
        {error && <p className="text-xs text-danger">{error}</p>}
      </div>
    </div>
  );
}
