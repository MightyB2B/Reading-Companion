import { useState } from "react";
import {
  applyPrefs,
  DEFAULT_PREFS,
  LEADING_RANGE,
  MEASURE_RANGE,
  SIZE_RANGE,
  storedPrefs,
  type ReadingPrefs,
} from "../lib/reading";

/**
 * How the page is set.
 *
 * Applied live as the slider moves, with a specimen of real prose above it —
 * a number of rem tells you nothing, and the only way to judge type is to look
 * at it while you change it.
 */
export function ReadingSettings() {
  const [prefs, setPrefs] = useState<ReadingPrefs>(storedPrefs);

  const update = (patch: Partial<ReadingPrefs>) => {
    const next = { ...prefs, ...patch };
    setPrefs(next);
    applyPrefs(next);
  };

  return (
    <section className="space-y-3">
      <div className="flex items-baseline justify-between">
        <h3 className="text-sm font-semibold">Reading</h3>
        <button
          onClick={() => update(DEFAULT_PREFS)}
          className="text-xs text-ink-soft hover:text-accent"
        >
          reset
        </button>
      </div>

      <div className="rounded border border-rule bg-paper-dim p-3">
        <p className="prose-page">
          In every science there are two factors: facts and ideas; or, facts and
          the mind. Science is more than knowledge.
        </p>
      </div>

      <Slider
        label="Size"
        value={prefs.size}
        range={SIZE_RANGE}
        format={(v) => `${Math.round(v * 100)}%`}
        onChange={(size) => update({ size })}
      />
      <Slider
        label="Line spacing"
        value={prefs.leading}
        range={LEADING_RANGE}
        format={(v) => v.toFixed(2)}
        onChange={(leading) => update({ leading })}
      />
      <Slider
        label="Line width"
        value={prefs.measure}
        range={MEASURE_RANGE}
        format={(v) => `${Math.round(v)} characters`}
        onChange={(measure) => update({ measure })}
      />

      <div>
        <span className="text-xs font-semibold">Face</span>
        <div className="mt-1 flex gap-2">
          {(["serif", "sans"] as const).map((face) => (
            <button
              key={face}
              onClick={() => update({ face })}
              className={`rounded border px-3 py-1 text-sm ${
                prefs.face === face
                  ? "border-accent text-accent"
                  : "border-rule hover:border-accent hover:text-accent"
              }`}
              style={{
                fontFamily:
                  face === "serif"
                    ? "var(--font-serif)"
                    : "var(--font-sans-reading)",
              }}
            >
              {face === "serif" ? "Serif" : "Sans"}
            </button>
          ))}
        </div>
        <p className="mt-1 text-xs text-ink-soft">
          Serif suits long stretches of printed prose; some readers find sans
          clearer on screen.
        </p>
      </div>
    </section>
  );
}

function Slider({
  label,
  value,
  range,
  format,
  onChange,
}: {
  label: string;
  value: number;
  range: { min: number; max: number; step: number };
  format: (v: number) => string;
  onChange: (v: number) => void;
}) {
  return (
    <label className="block">
      <span className="flex items-baseline justify-between text-xs">
        <span className="font-semibold">{label}</span>
        <span className="text-ink-soft tabular-nums">{format(value)}</span>
      </span>
      <input
        type="range"
        min={range.min}
        max={range.max}
        step={range.step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="mt-1 w-full accent-[var(--color-accent)]"
      />
    </label>
  );
}
