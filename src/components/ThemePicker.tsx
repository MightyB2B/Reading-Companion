import { useEffect, useRef, useState } from "react";
import { applyTheme, storedTheme, THEMES, type Theme } from "../lib/theme";

/** The paper and ink of each theme, for the swatches. */
const SWATCH: Record<Theme, [string, string, string]> = {
  system: ["#faf7f2", "#1f1d1a", "#7c5c3e"],
  light: ["#faf7f2", "#1f1d1a", "#7c5c3e"],
  sepia: ["#f4ecd8", "#3a2f21", "#8a5a2b"],
  dark: ["#171614", "#ece7df", "#c99b6b"],
  night: ["#0f141c", "#c6cedb", "#7aa2d6"],
  "high-contrast": ["#ffffff", "#000000", "#0043a8"],
};

export function ThemePicker() {
  const [theme, setTheme] = useState<Theme>(storedTheme);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const choose = (t: Theme) => {
    applyTheme(t);
    setTheme(t);
    setOpen(false);
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        title="Change the theme"
        aria-label="Change the theme"
        aria-expanded={open}
        className="flex items-center gap-1.5 rounded border border-rule px-2 py-1 text-xs text-ink-soft hover:border-accent hover:text-accent"
      >
        <Swatch theme={theme} />
        {THEMES[theme]}
      </button>

      {open && (
        <div className="absolute right-0 z-50 mt-1 w-52 overflow-hidden rounded border border-rule bg-paper shadow-lg">
          {(Object.keys(THEMES) as Theme[]).map((t) => (
            <button
              key={t}
              onClick={() => choose(t)}
              className={`flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm hover:bg-paper-dim ${
                t === theme ? "text-accent" : ""
              }`}
            >
              <Swatch theme={t} />
              <span className="flex-1">{THEMES[t]}</span>
              {t === theme && <span aria-hidden="true">✓</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** Paper, ink, and accent as three bands — the theme in miniature. */
function Swatch({ theme }: { theme: Theme }) {
  const [paper, ink, accent] = SWATCH[theme];
  return (
    <span
      className="inline-flex h-4 w-4 shrink-0 overflow-hidden rounded-full border border-rule"
      aria-hidden="true"
    >
      <span style={{ background: paper, width: "40%" }} />
      <span style={{ background: ink, width: "35%" }} />
      <span style={{ background: accent, width: "25%" }} />
    </span>
  );
}
