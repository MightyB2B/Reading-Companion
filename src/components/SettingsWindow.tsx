import { useEffect, useState } from "react";
import {
  api,
  KEEP_ALIVE_CHOICES,
  seesImages,
  type ModelInfo,
  type OllamaStatus,
  type Settings,
} from "../lib/api";
import { applyTheme, storedTheme, THEMES, type Theme } from "../lib/theme";
import { ReadingSettings } from "./ReadingSettings";
import { Spinner } from "./Spinner";

/**
 * Settings.
 *
 * The address field is the one that changes what the app can do. Everything
 * else here runs on whatever machine you point it at, so a laptop with 8GB of
 * VRAM can hand the work to a desktop with 24GB and run models it could not
 * otherwise load — the difference between a 4B model and a 30B one.
 *
 * The address is testable before it is saved. Committing a setting and then
 * discovering the server is not there would leave the reader unsure whether
 * they typed it wrong or the machine is off.
 *
 * The model fields follow the address rather than the saved setting: type a
 * new server and the lists refill from what that machine actually has, before
 * anything is committed.
 */
export function SettingsWindow({
  onClose,
  onSaved,
}: {
  onClose: () => void;
  onSaved: () => void;
}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [theme, setTheme] = useState<Theme>(storedTheme);
  const [test, setTest] = useState<OllamaStatus | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loadingModels, setLoadingModels] = useState(true);
  const [showKey, setShowKey] = useState(false);

  useEffect(() => {
    api.getSettings().then(setSettings).catch((e) => setError(String(e)));
  }, []);

  const host = settings?.ollama_host;
  const apiKey = settings?.ollama_api_key;

  // Ask whichever server is currently typed what it has. Debounced because
  // this fires on every keystroke in the address field, and each run
  // invalidates itself on cleanup so a slow reply for an address the reader
  // has already moved on from cannot overwrite the current list.
  useEffect(() => {
    if (host === undefined) return;
    let live = true;
    setLoadingModels(true);

    const timer = setTimeout(() => {
      api
        .listModels(host, apiKey)
        .then((found) => live && setModels(found))
        // A server that is not there is not an error worth a red banner —
        // the fields say so themselves, and Test explains it properly.
        .catch(() => live && setModels([]))
        .finally(() => live && setLoadingModels(false));
    }, 400);

    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [host, apiKey]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const edit = (patch: Partial<Settings>) =>
    setSettings((s) => (s ? { ...s, ...patch } : s));

  const runTest = async () => {
    if (!settings) return;
    setTesting(true);
    setTest(null);
    try {
      setTest(
        await api.testOllamaHost(settings.ollama_host, settings.ollama_api_key),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!settings) return;
    setSaving(true);
    try {
      // The backend tidies the address and fills blanks with defaults, so
      // take back what it actually stored rather than assuming.
      setSettings(await api.saveSettings(settings));
      onSaved();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const restoreDefaults = async () => {
    try {
      setSettings(await api.defaultSettings());
      setTest(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div
      // See AddContentWizard: a full-screen backdrop-blur is recomposited on
      // every frame that anything above it animates.
      className="veil-in fixed inset-0 z-50 flex items-center justify-center bg-veil p-6"
      onClick={onClose}
    >
      <div
        className="panel-in flex max-h-[85vh] w-full max-w-xl flex-col overflow-hidden rounded-xl border border-rule bg-paper shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <header className="flex items-center justify-between border-b border-rule px-5 py-3.5">
          <div>
            <h2 className="text-base font-semibold">Settings</h2>
            <p className="text-xs text-ink-soft">
              Stored in your library, not in the install
            </p>
          </div>
          <button
            onClick={onClose}
            aria-label="Close"
            className="rounded p-1 text-ink-soft hover:bg-paper-dim hover:text-accent"
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-5">
          {!settings ? (
            <div className="flex justify-center py-8">
              <Spinner />
            </div>
          ) : (
            <>
              <Section
                title="Where the models run"
                detail="Everything stays on the machine you point at. Leave this alone to use your own."
              >
                <label className="block text-sm font-medium" htmlFor="host">
                  Ollama address
                </label>
                <div className="mt-1.5 flex gap-2">
                  <input
                    id="host"
                    value={settings.ollama_host}
                    onChange={(e) => {
                      edit({ ollama_host: e.target.value });
                      setTest(null);
                    }}
                    placeholder="http://127.0.0.1:11434"
                    spellCheck={false}
                    className="flex-1 rounded border border-rule bg-transparent px-3 py-2 font-mono text-sm outline-none focus:border-accent"
                  />
                  <button
                    onClick={runTest}
                    disabled={testing}
                    className="shrink-0 rounded border border-rule px-3 py-2 text-sm hover:border-accent disabled:opacity-40"
                  >
                    {testing ? "Trying…" : "Test"}
                  </button>
                </div>
                <p className="mt-1.5 text-xs text-ink-soft">
                  A bare address works — <code>192.168.1.50:11434</code> becomes{" "}
                  <code>http://192.168.1.50:11434</code>. The remote Ollama needs{" "}
                  <code>OLLAMA_HOST=0.0.0.0</code> set to accept connections
                  from other machines.
                </p>

                <label
                  className="mt-3.5 block text-sm font-medium"
                  htmlFor="api-key"
                >
                  API key <span className="font-normal text-ink-soft">— optional</span>
                </label>
                <div className="mt-1.5 flex gap-2">
                  <input
                    id="api-key"
                    type={showKey ? "text" : "password"}
                    value={settings.ollama_api_key}
                    onChange={(e) => {
                      edit({ ollama_api_key: e.target.value });
                      setTest(null);
                    }}
                    placeholder="Leave blank for a local server"
                    spellCheck={false}
                    autoComplete="off"
                    className="flex-1 rounded border border-rule bg-transparent px-3 py-2 font-mono text-sm outline-none focus:border-accent"
                  />
                  <button
                    type="button"
                    onClick={() => setShowKey((s) => !s)}
                    className="shrink-0 rounded border border-rule px-3 py-2 text-sm hover:border-accent"
                  >
                    {showKey ? "Hide" : "Show"}
                  </button>
                </div>
                <p className="mt-1.5 text-xs text-ink-soft">
                  Sent as <code>Authorization: Bearer …</code> — what a hosted
                  or public Ollama-compatible server expects. Stored in your
                  library database in plain text, so treat that file as you
                  would the key itself.
                </p>

                {test && (
                  <p
                    className={`mt-2.5 rounded border px-3 py-2 text-xs ${
                      test.reachable
                        ? "border-ok/30 bg-ok/5 text-ok"
                        : "border-danger/30 bg-danger/5 text-danger"
                    }`}
                  >
                    {test.message}
                    {test.reachable && test.models.length > 0 && (
                      <span className="mt-1 block font-mono text-ink-soft">
                        {test.models.slice(0, 6).join(", ")}
                        {test.models.length > 6 && ` +${test.models.length - 6} more`}
                      </span>
                    )}
                  </p>
                )}
              </Section>

              <Section
                title="Models"
                detail="Whatever is installed on the server above. The two that look at an image only offer models that can see."
              >
                <ModelField
                  label="Transcribing pages"
                  hint="Reads a photograph. Small and fast matters more than clever."
                  value={settings.ocr_model}
                  models={models}
                  needsVision
                  loading={loadingModels}
                  onChange={(v) => edit({ ocr_model: v })}
                />
                <ModelField
                  label="Coaching and the dictionary"
                  hint="Grades your summary and picks word senses. A larger model here is the biggest quality win if you have the memory."
                  value={settings.text_model}
                  models={models}
                  loading={loadingModels}
                  onChange={(v) => edit({ text_model: v })}
                />
                <ModelField
                  label="Reading page numbers"
                  hint="Only used when the number cannot be found in the text."
                  value={settings.vision_model}
                  models={models}
                  needsVision
                  loading={loadingModels}
                  onChange={(v) => edit({ vision_model: v })}
                />
              </Section>

              <Section
                title="Keeping models loaded"
                detail="Loading a model takes 5-10 seconds. This is how long the server holds one after it is used."
              >
                <select
                  value={settings.keep_alive}
                  onChange={(e) => edit({ keep_alive: e.target.value })}
                  className="w-full rounded border border-rule bg-paper px-3 py-2 text-sm"
                >
                  {/* Any duration Ollama accepts is valid, so a value set by
                      hand stays selectable rather than being silently
                      rewritten to whichever option happens to be first. */}
                  {!KEEP_ALIVE_CHOICES.some((c) => c.value === settings.keep_alive) && (
                    <option value={settings.keep_alive}>{settings.keep_alive}</option>
                  )}
                  {KEEP_ALIVE_CHOICES.map((c) => (
                    <option key={c.value} value={c.value}>
                      {c.label}
                    </option>
                  ))}
                </select>
                <p className="mt-1.5 text-xs text-ink-soft">
                  {KEEP_ALIVE_CHOICES.find((c) => c.value === settings.keep_alive)
                    ?.hint ?? `Sent to the server as “${settings.keep_alive}”.`}
                </p>
                <p className="mt-1.5 text-xs text-ink-soft">
                  Sent with every request, which is why it overrides{" "}
                  <code>OLLAMA_KEEP_ALIVE</code> on the server.
                </p>
              </Section>

              <Section title="Appearance">
                <label className="block text-sm font-medium" htmlFor="theme">
                  Theme
                </label>
                <select
                  id="theme"
                  value={theme}
                  onChange={(e) => {
                    const next = e.target.value as Theme;
                    // Applied at once: a theme you have to save to see is a
                    // theme you cannot judge.
                    applyTheme(next);
                    setTheme(next);
                  }}
                  className="mt-1.5 w-full rounded border border-rule bg-paper px-3 py-2 text-sm"
                >
                  {Object.entries(THEMES).map(([key, label]) => (
                    <option key={key} value={key}>
                      {label}
                    </option>
                  ))}
                </select>
                <p className="mt-1.5 text-xs text-ink-soft">
                  Kept on this machine rather than in your library — it is a
                  property of the screen you are reading on.
                </p>

                {/* Type belongs beside the palette: both are how the page
                    looks to the person in front of it, and this application
                    had settings for three model names and nothing at all for
                    the words. */}
                <div className="mt-5">
                  <ReadingSettings />
                </div>
              </Section>

              {error && <p className="mt-4 text-sm text-danger">{error}</p>}
            </>
          )}
        </div>

        <footer className="flex items-center gap-2 border-t border-rule px-5 py-3.5">
          <button
            onClick={save}
            disabled={saving || !settings}
            className="rounded bg-accent px-3.5 py-1.5 text-sm text-paper disabled:opacity-40"
          >
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            onClick={onClose}
            className="rounded border border-rule px-3.5 py-1.5 text-sm hover:border-accent"
          >
            Cancel
          </button>
          <button
            onClick={restoreDefaults}
            className="ml-auto text-xs text-ink-soft hover:text-accent"
          >
            Restore defaults
          </button>
        </footer>
      </div>
    </div>
  );
}

function Section({
  title,
  detail,
  children,
}: {
  title: string;
  detail?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="mb-6 last:mb-0">
      <h3 className="text-sm font-semibold">{title}</h3>
      {detail && <p className="mt-0.5 mb-2.5 text-xs text-ink-soft">{detail}</p>}
      <div className={detail ? "" : "mt-2"}>{children}</div>
    </section>
  );
}

/**
 * Ollama reports `glm-ocr:latest` but the stored default is `glm-ocr`, and
 * both work when sent to the API. Matching on the base name keeps the saved
 * value selected instead of looking absent.
 */
const sameModel = (a: string, b: string) =>
  a === b || a.split(":")[0] === b.split(":")[0];

/**
 * Choose a model from what is actually installed.
 *
 * Typed by hand, a name that was never pulled fails at the moment you try to
 * read a page — and reads as a bug in this application rather than a wrong
 * choice. The two roles that look at an image are filtered to models that
 * report the `vision` capability, so a text-only model cannot be picked for
 * transcription at all.
 */
function ModelField({
  label,
  hint,
  value,
  models,
  needsVision,
  loading,
  onChange,
}: {
  label: string;
  hint: string;
  value: string;
  models: ModelInfo[];
  needsVision?: boolean;
  loading: boolean;
  onChange: (v: string) => void;
}) {
  const eligible = needsVision ? models.filter(seesImages) : models;
  const installed = eligible.some((m) => sameModel(m.name, value));

  return (
    <div className="mb-4 last:mb-0">
      <label className="block text-sm">{label}</label>

      <select
        value={eligible.find((m) => sameModel(m.name, value))?.name ?? value}
        onChange={(e) => onChange(e.target.value)}
        disabled={loading || eligible.length === 0}
        className="mt-1 w-full rounded border border-rule bg-paper px-3 py-1.5 font-mono text-sm disabled:opacity-50"
      >
        {/* The saved value is always offered, even when the server has not
            got it — otherwise changing servers would silently reassign it.
            While the list is still arriving, say nothing about it: every
            value looks missing before the reply lands. */}
        {!installed && (
          <option value={value}>
            {loading ? value : `${value} — not installed here`}
          </option>
        )}
        {eligible.map((m) => (
          <option key={m.name} value={m.name}>
            {m.name}
          </option>
        ))}
      </select>

      <p className="mt-1 text-xs text-ink-soft">{hint}</p>

      {loading && <p className="mt-1 text-xs text-ink-soft">Asking the server…</p>}

      {!loading && eligible.length === 0 && (
        <p className="mt-1 text-xs text-warn">
          {needsVision
            ? "No model on that server reports being able to read images."
            : "No models found on that server."}
        </p>
      )}

      {!loading && !installed && eligible.length > 0 && (
        <p className="mt-1 text-xs text-warn">
          Not on that server — pull it with{" "}
          <code>ollama pull {value}</code>, or choose one above.
        </p>
      )}
    </div>
  );
}
