import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type JobProgress, type Page } from "../lib/api";
import { Spinner } from "./Spinner";

/**
 * One way in, instead of three buttons.
 *
 * The sources differ enormously underneath — a photograph needs a vision
 * model, an EPUB needs none — but that is the application's problem, not the
 * reader's. What they have is a book, a file, or a link, and the wizard asks
 * which before worrying about how.
 */
export type SourceChoice = "photo" | "file" | "web";

/** The filename, without the directory. */
function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * Sort filenames the way a person would: `IMG_9.jpg` before `IMG_10.jpg`.
 *
 * Plain string order puts 10 before 9, which for a stack of camera files means
 * the book imports scrambled and every page number has to be fixed by hand.
 */
function compareNatural(a: string, b: string): number {
  return basename(a).localeCompare(basename(b), undefined, {
    numeric: true,
    sensitivity: "base",
  });
}

type Step = "choose" | "url" | "working" | "numbering" | "done";

export interface WizardResult {
  summary: string;
  error?: string;
  /**
   * Pages whose number could not be read. Asked about inside the wizard, as
   * another step in the same flow — a second dialog with its own look
   * appearing after this one closed made the import feel like two unrelated
   * events rather than one.
   */
  needsNumbers?: Page[];
}

export function AddContentWizard({
  onClose,
  onImport,
  progress,
  transcript,
}: {
  onClose: () => void;
  /** Runs the import and resolves with what to tell the reader. */
  onImport: (choice: SourceChoice, source: string) => Promise<WizardResult>;
  progress: JobProgress | null;
  transcript: string;
}) {
  const [step, setStep] = useState<Step>("choose");
  const [direction, setDirection] = useState<"forward" | "back">("forward");
  const [url, setUrl] = useState("");
  const [result, setResult] = useState<WizardResult | null>(null);
  const [toNumber, setToNumber] = useState<Page[]>([]);
  const [numbered, setNumbered] = useState(0);
  /** Which file of how many, while a stack of photographs is importing. */
  const [batch, setBatch] = useState<{
    at: number;
    of: number;
    name: string;
  } | null>(null);

  const go = (next: Step, dir: "forward" | "back" = "forward") => {
    setDirection(dir);
    setStep(next);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Not while working: closing mid-import would leave the reader unsure
      // whether it finished.
      if (e.key === "Escape" && step !== "working") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, onClose]);

  const run = async (choice: SourceChoice, source: string) => {
    go("working");
    const outcome = await onImport(choice, source);
    setResult(outcome);

    // Numbering comes before the summary, so the reader finishes the job in
    // one pass rather than being asked afterwards.
    const unnumbered = outcome.needsNumbers ?? [];
    if (!outcome.error && unnumbered.length > 0) {
      setToNumber(unnumbered);
      go("numbering");
    } else {
      go("done");
    }
  };

  /**
   * Import a stack of photographs, one at a time.
   *
   * Sequential rather than parallel: each page runs a vision model, and firing
   * twenty at a GPU at once makes all twenty slow instead of the first one
   * fast. More importantly, a failure on the seventh photograph must not
   * abandon the eight through twenty — so each is caught on its own and the
   * run reports at the end what did not make it.
   */
  const runMany = async (sources: string[]) => {
    go("working");
    const unnumbered: Page[] = [];
    const failures: string[] = [];
    let done = 0;

    for (const [i, source] of sources.entries()) {
      setBatch({ at: i + 1, of: sources.length, name: basename(source) });
      const outcome = await onImport("photo", source);
      if (outcome.error) failures.push(`${basename(source)}: ${outcome.error}`);
      else done += 1;
      unnumbered.push(...(outcome.needsNumbers ?? []));
    }
    setBatch(null);

    setResult({
      summary:
        sources.length === 1
          ? (failures.length ? "" : "Page added.")
          : `${done} of ${sources.length} pages added.`,
      error: failures.length ? failures.join("\n") : undefined,
    });

    if (unnumbered.length > 0) {
      setToNumber(unnumbered);
      go("numbering");
    } else {
      go("done");
    }
  };

  const pickPhoto = async () => {
    const chosen = await open({
      // Photographing a book produces a stack, not a page. Making the picker
      // take one file at a time turned an evening's reading into an evening
      // of importing.
      multiple: true,
      filters: [
        {
          name: "Page photos",
          extensions: [
            "jpg", "jpeg", "png", "webp", "heic", "heif",
            "tif", "tiff", "bmp", "avif",
          ],
        },
      ],
    });
    const files = Array.isArray(chosen) ? chosen : chosen ? [chosen] : [];
    // Filenames from a camera sort into shooting order, which is page order.
    if (files.length) await runMany([...files].sort(compareNatural));
  };

  const pickFile = async () => {
    const chosen = await open({
      multiple: false,
      filters: [{ name: "Ebook or PDF", extensions: ["epub", "pdf"] }],
    });
    if (typeof chosen === "string") await run("file", chosen);
  };

  const normalisedUrl = url.trim().match(/^https?:\/\//i)
    ? url.trim()
    : url.trim()
      ? `https://${url.trim()}`
      : "";

  return (
    <div
      className="veil-in fixed inset-0 z-50 flex items-center justify-center bg-veil p-6 backdrop-blur-[2px]"
      onClick={() => step !== "working" && onClose()}
    >
      <div
        className="panel-in w-full max-w-lg overflow-hidden rounded-xl border border-rule bg-paper shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <header className="flex items-center justify-between border-b border-rule px-5 py-3.5">
          <div>
            <h2 className="text-base font-semibold">Add to this book</h2>
            <p className="text-xs text-ink-soft">{subtitle(step)}</p>
          </div>
          {step !== "working" && (
            <button
              onClick={onClose}
              aria-label="Close"
              className="rounded p-1 text-ink-soft hover:bg-paper-dim hover:text-accent"
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                <path d="M18 6 6 18M6 6l12 12" />
              </svg>
            </button>
          )}
        </header>

        <div key={step} className={`step-in-${direction} px-5 py-5`}>
          {step === "choose" && (
            <div className="grid gap-2.5">
              <SourceCard
                index={0}
                icon={<CameraIcon />}
                title="A photograph of a page"
                detail="Transcribed on your machine. An open book is split into two pages automatically."
                onClick={pickPhoto}
              />
              <SourceCard
                index={1}
                icon={<BookIcon />}
                title="An ebook or PDF"
                detail="EPUB, or a PDF with real text. Imported directly — no transcription needed."
                onClick={pickFile}
              />
              <SourceCard
                index={2}
                icon={<GlobeIcon />}
                title="A web page"
                detail="Fetched once, then stored in your library and read offline."
                onClick={() => go("url")}
              />
            </div>
          )}

          {step === "url" && (
            <div>
              <label className="text-sm font-medium" htmlFor="wizard-url">
                Address of the page
              </label>
              <input
                id="wizard-url"
                autoFocus
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && normalisedUrl) run("web", normalisedUrl);
                }}
                placeholder="example.com/an-article"
                className="mt-2 w-full rounded border border-rule bg-transparent px-3 py-2 outline-none focus:border-accent"
              />
              <p className="mt-2 text-xs text-ink-soft">
                {normalisedUrl && normalisedUrl !== url.trim()
                  ? `Will fetch ${normalisedUrl}`
                  : "This is the only time the app reaches the network for reading material."}
              </p>
              <div className="mt-4 flex gap-2">
                <button
                  onClick={() => normalisedUrl && run("web", normalisedUrl)}
                  disabled={!normalisedUrl}
                  className="rounded bg-accent px-3.5 py-1.5 text-sm text-paper disabled:opacity-40"
                >
                  Fetch it
                </button>
                <button
                  onClick={() => go("choose", "back")}
                  className="rounded border border-rule px-3.5 py-1.5 text-sm hover:border-accent"
                >
                  Back
                </button>
              </div>
            </div>
          )}

          {step === "working" && (
            <div>
              <div className="flex items-center gap-4">
                <Spinner />
                <div className="min-w-0">
                  <p className="font-medium">
                    {progress?.detail ?? "Getting started"}
                  </p>
                  <p className="mt-0.5 truncate text-xs text-ink-soft">
                    {batch
                      ? `Page ${batch.at} of ${batch.of} · ${batch.name}`
                      : progress
                        ? `Step ${progress.step} of ${progress.total} · everything runs on your machine`
                        : "Everything runs on your machine"}
                  </p>
                </div>
              </div>

              <div className="mt-4 h-1 overflow-hidden rounded-full bg-rule">
                <div
                  className="h-full rounded-full bg-accent transition-[width] duration-500 ease-out"
                  style={{
                    width: progress ? `${(progress.step / progress.total) * 100}%` : "8%",
                  }}
                />
              </div>

              {/* Watching the words arrive is the most convincing evidence
                  that the machine is working. */}
              {transcript && (
                <div className="mt-4 max-h-36 overflow-y-auto rounded border border-rule bg-paper-dim p-3">
                  <p className="prose-page text-[0.8rem] leading-relaxed text-ink-soft">
                    {transcript.slice(-700)}
                  </p>
                </div>
              )}
            </div>
          )}

          {step === "numbering" && (
            <NumberingStep
              pages={toNumber}
              onFinished={(count) => {
                setNumbered(count);
                go("done");
              }}
            />
          )}

          {step === "done" && (
            <div>
              <div className="flex items-start gap-3">
                {result?.error ? <CrossMark /> : <CheckMark />}
                <div className="min-w-0 flex-1">
                  <p className="font-medium">
                    {result?.error ? "That did not work" : "Added"}
                  </p>
                  <p className="mt-1 text-sm text-ink-soft">
                    {result?.error ?? result?.summary}
                    {numbered > 0 &&
                      ` You numbered ${numbered} page${numbered === 1 ? "" : "s"}.`}
                  </p>
                </div>
              </div>
              <div className="mt-5 flex gap-2">
                <button
                  onClick={onClose}
                  className="rounded bg-accent px-3.5 py-1.5 text-sm text-paper"
                >
                  {result?.error ? "Close" : "Start reading"}
                </button>
                <button
                  onClick={() => {
                    setResult(null);
                    setUrl("");
                    go("choose", "back");
                  }}
                  className="rounded border border-rule px-3.5 py-1.5 text-sm hover:border-accent"
                >
                  Add something else
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function subtitle(step: Step): string {
  switch (step) {
    case "choose":
      return "Where is it coming from?";
    case "url":
      return "Paste the address";
    case "working":
      return "Working — this stays on your machine";
    case "numbering":
      return "One thing I could not read";
    case "done":
      return "Finished";
  }
}

/**
 * Asks for page numbers the model could not read.
 *
 * Page numbers are load-bearing: they set reading order and decide which
 * paragraph continues which across a page break. A page left unnumbered sorts
 * to the end and never stitches to its neighbour, so this is worth a step of
 * its own rather than a silent omission.
 */
function NumberingStep({
  pages,
  onFinished,
}: {
  pages: Page[];
  onFinished: (numbered: number) => void;
}) {
  const [index, setIndex] = useState(0);
  const [value, setValue] = useState("");
  const [saved, setSaved] = useState(0);
  const [busy, setBusy] = useState(false);

  const page = pages[index];
  if (!page) return null;

  const advance = (didSave: boolean) => {
    const total = saved + (didSave ? 1 : 0);
    setSaved(total);
    setValue("");
    if (index < pages.length - 1) setIndex(index + 1);
    else onFinished(total);
  };

  const save = async () => {
    const n = parseInt(value, 10);
    if (!Number.isFinite(n)) return;
    setBusy(true);
    try {
      await api.setPageNumber(page.id, n);
      advance(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <p className="text-sm">
        The page number could not be read from this photograph — it may be
        cropped, obscured, or simply not printed.
      </p>

      {/* Show the page, so the reader is reading rather than guessing. */}
      <img
        src={convertFileSrc(page.image_proc ?? page.image_orig)}
        alt="The page being numbered"
        className="mt-3 max-h-48 w-full rounded border border-rule object-contain"
      />

      {pages.length > 1 && (
        <p className="mt-2 text-xs text-ink-soft">
          Page {index + 1} of {pages.length} to number
        </p>
      )}

      <input
        autoFocus
        type="number"
        inputMode="numeric"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
        }}
        placeholder="What page is this? e.g. 47"
        className="mt-3 w-full rounded border border-rule bg-transparent px-3 py-2 outline-none focus:border-accent"
      />

      <div className="mt-4 flex items-center gap-2">
        <button
          onClick={save}
          disabled={busy || !value.trim()}
          className="rounded bg-accent px-3.5 py-1.5 text-sm text-paper disabled:opacity-40"
        >
          Save
        </button>
        <button
          onClick={() => advance(false)}
          className="rounded border border-rule px-3.5 py-1.5 text-sm hover:border-accent"
        >
          Skip
        </button>
        <span className="ml-auto text-xs text-ink-soft">
          You can change this later.
        </span>
      </div>
    </div>
  );
}

function SourceCard({
  index,
  icon,
  title,
  detail,
  onClick,
}: {
  index: number;
  icon: React.ReactNode;
  title: string;
  detail: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      // Staggered so the three options resolve one after another rather than
      // landing as a block.
      style={{ animationDelay: `${index * 55}ms` }}
      className="card-in group flex items-start gap-3.5 rounded-lg border border-rule p-3.5 text-left transition-all hover:-translate-y-0.5 hover:border-accent hover:shadow-md"
    >
      <span className="mt-0.5 shrink-0 text-ink-soft transition-colors group-hover:text-accent">
        {icon}
      </span>
      <span className="min-w-0">
        <span className="block text-sm font-medium">{title}</span>
        <span className="mt-0.5 block text-xs leading-relaxed text-ink-soft">
          {detail}
        </span>
      </span>
      <span className="ml-auto self-center text-ink-soft opacity-0 transition-opacity group-hover:opacity-100">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M9 18l6-6-6-6" />
        </svg>
      </span>
    </button>
  );
}

const iconProps = {
  width: 22,
  height: 22,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.6,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
};

function CameraIcon() {
  return (
    <svg {...iconProps}>
      <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
      <circle cx="12" cy="13" r="4" />
    </svg>
  );
}

function BookIcon() {
  return (
    <svg {...iconProps}>
      <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
      <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" />
    </svg>
  );
}

function GlobeIcon() {
  return (
    <svg {...iconProps}>
      <circle cx="12" cy="12" r="10" />
      <path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
    </svg>
  );
}

function CheckMark() {
  return (
    <svg className="h-7 w-7 shrink-0 text-ok" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="1.5" className="opacity-30" />
      <path className="check-draw" d="M7.5 12.5l3 3 6-6.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function CrossMark() {
  return (
    <svg className="h-7 w-7 shrink-0 text-danger" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="1.5" className="opacity-30" />
      <path d="M15 9l-6 6M9 9l6 6" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
