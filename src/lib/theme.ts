/**
 * Theme selection.
 *
 * A theme is six CSS custom properties, set by a `data-theme` attribute on the
 * root element (see index.css). "System" removes the attribute entirely so the
 * `prefers-color-scheme` rule takes over — which is why the CSS keys that
 * media query on `:root:not([data-theme])`.
 *
 * Stored in localStorage rather than the app database: it is a property of
 * this machine's display, not of the reader's library, and it must be applied
 * before the first paint, which a database round-trip cannot manage.
 */

export const THEMES = {
  system: "Follow the system",
  light: "Light",
  sepia: "Sepia",
  dark: "Dark",
  night: "Night",
  "high-contrast": "High contrast",
} as const;

export type Theme = keyof typeof THEMES;

const STORAGE_KEY = "reading-companion-theme";

export function isTheme(value: unknown): value is Theme {
  return typeof value === "string" && value in THEMES;
}

export function storedTheme(): Theme {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return isTheme(raw) ? raw : "system";
  } catch {
    // localStorage can be unavailable; the default is not worth an error.
    return "system";
  }
}

export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", theme);
  }
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Not being able to remember the choice should not break setting it.
  }
}

/**
 * Apply the saved theme immediately, before React mounts.
 *
 * Without this the first frame paints in the default palette and then snaps to
 * the chosen one — a flash of the wrong colour on every launch.
 */
export function initTheme(): Theme {
  const theme = storedTheme();
  applyTheme(theme);
  return theme;
}
