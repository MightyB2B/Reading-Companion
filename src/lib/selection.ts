/**
 * Turning a DOM selection into character offsets in the book's text.
 *
 * Two things make this harder than `sel.toString()`:
 *
 * 1. A paragraph is not one text node. Term underlines, scripture links and
 *    marks split it into many spans, so DOM node offsets bear no relation to
 *    character positions in the paragraph.
 * 2. A selection routinely covers several paragraphs — that is what dragging
 *    across a page boundary *is*. Resolving one "host" block and searching for
 *    the whole selected string inside it finds nothing, which is why selecting
 *    across a boundary used to silently produce no selection at all.
 *
 * So a selection is a *list* of per-block ranges, and each block's offsets are
 * computed by walking that block's own text nodes and counting characters.
 */

export interface Anchor {
  blockId: number;
  charStart: number;
  charEnd: number;
  /** The covered text within this block. */
  text: string;
}

/**
 * Every block the range touches, with the characters it covers in each.
 *
 * `root` is the scrolling container; only elements carrying `data-block-id`
 * are considered, and that attribute lives on the prose element itself rather
 * than a wrapper — a wrapper would include the page marker's own text in the
 * count and shift every offset in the paragraph.
 */
export function anchorsFromRange(range: Range, root: HTMLElement): Anchor[] {
  const out: Anchor[] = [];

  for (const el of root.querySelectorAll<HTMLElement>("[data-block-id]")) {
    if (!range.intersectsNode(el)) continue;
    const hit = offsetsWithin(el, range);
    const blockId = Number(el.dataset.blockId);
    if (hit && Number.isFinite(blockId)) out.push({ blockId, ...hit });
  }
  return out;
}

/**
 * Where the range starts and ends inside one element's text.
 *
 * Returns null when the range touches the element but covers none of its text
 * — which happens at the very edges of a drag, and would otherwise produce a
 * zero-width anchor that renders as an invisible highlight.
 */
function offsetsWithin(
  el: HTMLElement,
  range: Range,
): { charStart: number; charEnd: number; text: string } | null {
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);

  let offset = 0;
  let charStart: number | null = null;
  let charEnd = 0;
  let text = "";

  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.nodeValue?.length ?? 0;
    if (length === 0) continue;

    // `comparePoint` returns -1 before the range, 0 inside it, 1 after.
    // A node lying wholly outside still advances the counter: offsets are
    // against the paragraph's full text, not against what was selected.
    const endsBeforeRange = range.comparePoint(node, length) < 0;
    const startsAfterRange = range.comparePoint(node, 0) > 0;
    if (endsBeforeRange || startsAfterRange) {
      offset += length;
      continue;
    }

    // If the range begins or ends inside this node, the DOM guarantees the
    // node is the corresponding container, so no interpolation is needed.
    const from = node === range.startContainer ? range.startOffset : 0;
    const to = node === range.endContainer ? range.endOffset : length;
    if (to > from) {
      if (charStart === null) charStart = offset + from;
      charEnd = offset + to;
      text += node.nodeValue!.slice(from, to);
    }
    offset += length;
  }

  if (charStart === null || charEnd <= charStart) return null;
  return { charStart, charEnd, text };
}

/** A selection, as the panels receive it. */
export interface FlowSelection {
  /**
   * How it was made.
   *
   * A deliberate drag is something the reader is about to act on and gets the
   * full highlight. A word they double-clicked to check is incidental — every
   * lookup leaving a wash behind striped the paragraph after half a dozen —
   * so it is drawn faintly and replaced by the next one.
   */
  kind: "drag" | "lookup";
  anchors: Anchor[];
  /** Everything selected, joined across blocks. */
  text: string;
  /** The pages it spans, for showing "p. 47–48". */
  pageNos: (number | null)[];
}

/**
 * Describe which pages a selection covers.
 *
 * A selection that runs across a boundary is the normal case now, so the
 * label has to say so rather than naming only where the drag started.
 */
export function pageLabel(pageNos: (number | null)[]): string {
  const known = [...new Set(pageNos.filter((n): n is number => n != null))].sort(
    (a, b) => a - b,
  );
  if (known.length === 0) return "";
  if (known.length === 1) return `p. ${known[0]}`;
  return `pp. ${known[0]}–${known[known.length - 1]}`;
}
