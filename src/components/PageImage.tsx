import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api, type CropRect, type Page } from "../lib/api";

/**
 * The photograph behind a page, at full resolution.
 *
 * Page numbers hide in the gutter and in the corners, shot at an angle, in
 * the shadow of the reader's own hand — which is exactly where automatic
 * detection fails. The recourse has to be looking at the actual photograph,
 * so this zooms to the cursor and pans by dragging, and the number box takes
 * Enter to save and move to the next unnumbered page.
 *
 * Numbering fourteen photographs should be fourteen keystrokes and a scroll,
 * not fourteen dialogs.
 */
export function PageImage({
  page,
  queue,
  onClose,
  onSaved,
}: {
  page: Page;
  /** Other pages still missing a number, for the run-through. */
  queue: Page[];
  onClose: () => void;
  onSaved: (pageId: number, pageNo: number | null) => void;
}) {
  const [current, setCurrent] = useState(page);
  const [value, setValue] = useState(
    current.page_no != null ? String(current.page_no) : "",
  );
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [error, setError] = useState<string | null>(null);
  const [cropping, setCropping] = useState(false);
  /** The box being drawn, in client coordinates. */
  const [box, setBox] = useState<{
    x0: number;
    y0: number;
    x1: number;
    y1: number;
  } | null>(null);
  const [working, setWorking] = useState(false);
  const dragging = useRef<{ x: number; y: number } | null>(null);
  const image = useRef<HTMLImageElement>(null);
  const numberBox = useRef<HTMLInputElement>(null);
  const frame = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setValue(current.page_no != null ? String(current.page_no) : "");
    setZoom(1);
    setPan({ x: 0, y: 0 });
    setCropping(false);
    setBox(null);
    numberBox.current?.focus();
    numberBox.current?.select();
  }, [current]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const remaining = queue.filter(
    (p) => p.id !== current.id && p.page_no == null,
  );

  const save = async (advance: boolean) => {
    setError(null);
    const trimmed = value.trim();
    const parsed = trimmed === "" ? null : Number(trimmed);
    if (parsed !== null && (!Number.isInteger(parsed) || parsed < 0)) {
      setError("A page number is a whole number.");
      return;
    }
    try {
      await api.setPageNumber(current.id, parsed);
      onSaved(current.id, parsed);
      if (advance && remaining.length > 0) {
        setCurrent(remaining[0]);
      } else if (advance) {
        onClose();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * Turn the drawn box into fractions of the photograph.
   *
   * Measured against the image element's own rectangle, so the zoom and pan
   * are already accounted for — the transform moves the element, and its
   * bounding box moves with it.
   */
  const toRect = (): CropRect | null => {
    const el = image.current;
    if (!el || !box) return null;
    const r = el.getBoundingClientRect();
    const left = Math.min(box.x0, box.x1);
    const top = Math.min(box.y0, box.y1);
    const right = Math.max(box.x0, box.x1);
    const bottom = Math.max(box.y0, box.y1);
    return {
      x: (left - r.left) / r.width,
      y: (top - r.top) / r.height,
      width: (right - left) / r.width,
      height: (bottom - top) / r.height,
    };
  };

  const applyCrop = async () => {
    const rect = toRect();
    if (!rect) return;
    setWorking(true);
    setError(null);
    try {
      await api.recropPage(current.id, rect);
      onSaved(current.id, current.page_no);
      setCropping(false);
      setBox(null);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setWorking(false);
    }
  };

  // Zoom at the cursor rather than at the centre: the number is in a corner,
  // and centre-zoom pushes it off screen exactly when you close in on it.
  const onWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const rect = frame.current?.getBoundingClientRect();
    if (!rect) return;
    const cx = e.clientX - rect.left - rect.width / 2;
    const cy = e.clientY - rect.top - rect.height / 2;
    const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
    const next = Math.min(8, Math.max(1, zoom * factor));
    const scale = next / zoom;
    setPan((p) => ({
      x: cx - (cx - p.x) * scale,
      y: cy - (cy - p.y) * scale,
    }));
    setZoom(next);
  };

  const src = convertFileSrc(current.image_orig);

  return (
    <div className="veil-in fixed inset-0 z-50 flex flex-col bg-veil p-6">
      <div className="mx-auto flex min-h-0 w-full max-w-6xl flex-1 flex-col overflow-hidden rounded-lg border border-rule bg-paper shadow-2xl">
        <div className="flex items-center gap-3 border-b border-rule px-4 py-2">
          <h2 className="text-sm font-semibold">
            {current.page_no != null
              ? `Page ${current.page_no}`
              : "This page has no number"}
          </h2>
          <span className="text-xs text-ink-soft">
            {cropping
              ? "drag a box around the page you meant"
              : "scroll to zoom · drag to pan · double-click to fit"}
          </span>
          <div className="ml-auto flex items-center gap-2">
            {remaining.length > 0 && (
              <span className="text-xs text-ink-soft tabular-nums">
                {remaining.length} more without a number
              </span>
            )}
            <button
              onClick={onClose}
              className="rounded p-1 text-ink-soft hover:bg-paper-dim hover:text-accent"
              aria-label="Close"
            >
              ✕
            </button>
          </div>
        </div>

        <div
          ref={frame}
          onWheel={onWheel}
          onDoubleClick={() => {
            setZoom(1);
            setPan({ x: 0, y: 0 });
          }}
          onMouseDown={(e) => {
            if (cropping) {
              setBox({ x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY });
              return;
            }
            dragging.current = { x: e.clientX - pan.x, y: e.clientY - pan.y };
          }}
          onMouseMove={(e) => {
            if (cropping) {
              setBox((b) => (b ? { ...b, x1: e.clientX, y1: e.clientY } : b));
              return;
            }
            if (!dragging.current) return;
            setPan({
              x: e.clientX - dragging.current.x,
              y: e.clientY - dragging.current.y,
            });
          }}
          onMouseUp={() => (dragging.current = null)}
          onMouseLeave={() => (dragging.current = null)}
          className="relative min-h-0 flex-1 overflow-hidden bg-paper-dim"
          style={{
            cursor: cropping ? "crosshair" : zoom > 1 ? "grab" : "zoom-in",
          }}
        >
          <img
            ref={image}
            src={src}
            alt="The photograph of this page"
            draggable={false}
            className="absolute left-1/2 top-1/2 max-h-none max-w-none select-none"
            style={{
              transform: `translate(-50%, -50%) translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
              transformOrigin: "center",
              height: "100%",
              width: "auto",
            }}
          />

          {box && (
            <div
              className="pointer-events-none fixed border-2 border-accent bg-accent/15"
              style={{
                left: Math.min(box.x0, box.x1),
                top: Math.min(box.y0, box.y1),
                width: Math.abs(box.x1 - box.x0),
                height: Math.abs(box.y1 - box.y0),
              }}
            />
          )}
        </div>

        <div className="flex flex-wrap items-center gap-3 border-t border-rule px-4 py-3">
          <label className="flex items-center gap-2 text-sm">
            <span className="font-semibold">Page number</span>
            <input
              ref={numberBox}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  save(true);
                }
              }}
              inputMode="numeric"
              placeholder="—"
              className="w-24 rounded border border-accent bg-transparent px-2 py-1 text-center tabular-nums outline-none"
            />
          </label>
          <span className="text-xs text-ink-soft">
            Enter saves and moves to the next page without one.
          </span>
          {error && <span className="text-xs text-danger">{error}</span>}
          <div className="ml-auto flex gap-2">
            {cropping ? (
              <>
                <button
                  onClick={() => {
                    setCropping(false);
                    setBox(null);
                  }}
                  className="rounded border border-rule px-3 py-1 text-xs hover:border-accent hover:text-accent"
                >
                  Cancel crop
                </button>
                <button
                  onClick={applyCrop}
                  disabled={!box || working}
                  title="Keep only this region and read it again"
                  className="rounded bg-accent px-3 py-1 text-xs font-semibold text-paper disabled:opacity-40"
                >
                  {working ? "Reading…" : "Crop and re-read"}
                </button>
              </>
            ) : (
              <button
                onClick={() => setCropping(true)}
                title="The photograph caught something that is not this page"
                className="rounded border border-rule px-3 py-1 text-xs hover:border-accent hover:text-accent"
              >
                Crop
              </button>
            )}
            <button
              onClick={() => save(false)}
              className="rounded border border-rule px-3 py-1 text-xs hover:border-accent hover:text-accent"
            >
              Save
            </button>
            <button
              onClick={() => save(true)}
              className="rounded bg-accent px-3 py-1 text-xs font-semibold text-paper"
            >
              {remaining.length > 0 ? "Save and next" : "Save and close"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
