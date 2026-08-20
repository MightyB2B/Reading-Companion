/**
 * How the page is set.
 *
 * Settings covered three model pickers, a host and an API key, and said
 * nothing at all about reading — for an application whose premise is spending
 * hours with difficult books, with the type size hard-coded in a stylesheet.
 *
 * Stored in localStorage rather than the library database, for the same reason
 * the theme is: this is a property of the screen you are sitting at, not of
 * the books you own, and it has to be applied before the first paint.
 */

export interface ReadingPrefs {
  /** Body size in rem. */
  size: number;
  /** Unitless line height. */
  leading: number;
  /** Characters per line — the measure. */
  measure: number;
  /** Serif is the default; some readers strongly prefer a sans face. */
  face: "serif" | "sans";
}

export const DEFAULT_PREFS: ReadingPrefs = {
  size: 1.05,
  leading: 1.75,
  measure: 68,
  face: "serif",
};

export const SIZE_RANGE = { min: 0.85, max: 1.6, step: 0.05 };
export const LEADING_RANGE = { min: 1.3, max: 2.2, step: 0.05 };
export const MEASURE_RANGE = { min: 45, max: 100, step: 1 };

const STORAGE_KEY = "reading-companion-reading";

export function storedPrefs(): ReadingPrefs {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_PREFS;
    const parsed = JSON.parse(raw) as Partial<ReadingPrefs>;
    // Merged over the defaults rather than trusted: a preference file from an
    // older build is missing keys, and a NaN here would blank the page.
    return {
      size: clamp(parsed.size, SIZE_RANGE, DEFAULT_PREFS.size),
      leading: clamp(parsed.leading, LEADING_RANGE, DEFAULT_PREFS.leading),
      measure: clamp(parsed.measure, MEASURE_RANGE, DEFAULT_PREFS.measure),
      face: parsed.face === "sans" ? "sans" : "serif",
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

function clamp(
  value: unknown,
  range: { min: number; max: number },
  fallback: number,
): number {
  const n = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.min(range.max, Math.max(range.min, n));
}

/**
 * Apply the preferences as CSS variables on the root.
 *
 * Variables rather than a class per size, so `.prose-page` stays one rule and
 * the reading column can be adjusted while it is on screen.
 */
export function applyPrefs(prefs: ReadingPrefs): void {
  const root = document.documentElement;
  root.style.setProperty("--reading-size", `${prefs.size}rem`);
  root.style.setProperty("--reading-leading", String(prefs.leading));
  root.style.setProperty("--reading-measure", `${prefs.measure}ch`);
  root.style.setProperty(
    "--reading-face",
    prefs.face === "sans" ? "var(--font-sans-reading)" : "var(--font-serif)",
  );
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Not being able to remember the choice should not break setting it.
  }
}

/** Apply before React mounts, so the first frame is already correct. */
export function initPrefs(): ReadingPrefs {
  const prefs = storedPrefs();
  applyPrefs(prefs);
  return prefs;
}
