/**
 * A rotating arc, used wherever the app is waiting on a local model.
 *
 * It reads as "working" without implying a known duration, which we never
 * have: a transcription is a few seconds, a critique two or three, and a cold
 * model load rather more. A progress bar would have to lie about all of them.
 *
 * Shared rather than duplicated so every wait in the app looks the same — an
 * import, a critique, and a word lookup are all the same machine doing the
 * same kind of thinking, and should not each have their own idiom.
 */
export function Spinner({ size = 36 }: { size?: number }) {
  return (
    <svg
      className="shrink-0 animate-spin text-accent"
      style={{ width: size, height: size }}
      viewBox="0 0 40 40"
      fill="none"
      aria-hidden="true"
    >
      <circle
        cx="20"
        cy="20"
        r="16"
        stroke="currentColor"
        strokeWidth="3"
        className="opacity-20"
      />
      <path
        d="M20 4a16 16 0 0 1 16 16"
        stroke="currentColor"
        strokeWidth="3"
        strokeLinecap="round"
      />
    </svg>
  );
}

/**
 * A spinner with a line saying what is being waited on.
 *
 * The words matter more than the animation. "Reading the sentence" tells the
 * reader the machine is somewhere sensible; a bare spinner tells them only
 * that time is passing.
 */
export function Working({
  label,
  detail,
  size = 28,
}: {
  label: string;
  detail?: string;
  size?: number;
}) {
  return (
    <div className="flex items-center gap-3">
      <Spinner size={size} />
      <div className="min-w-0">
        <p className="text-sm font-medium">{label}</p>
        {detail && <p className="mt-0.5 text-xs text-ink-soft">{detail}</p>}
      </div>
    </div>
  );
}
