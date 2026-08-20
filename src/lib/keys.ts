/**
 * Keyboard shortcuts.
 *
 * The application had none at all, which for something meant to be used for
 * hours put every action behind a pointer journey to a panel.
 *
 * The one rule that matters: a shortcut must never fire while the reader is
 * typing. `n` is "write a note" over the text and the letter n inside the note
 * they are writing, and getting that wrong is worse than having no shortcuts.
 */

/** True when focus is somewhere text is being entered. */
export function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    el.isContentEditable === true
  );
}

export interface Binding {
  /** Lower-case `event.key`. */
  key: string;
  /** Require Ctrl (or Cmd on a Mac). Defaults to false. */
  mod?: boolean;
  run: () => void;
  /** Fire even while typing — only for Escape and modified keys. */
  whileTyping?: boolean;
}

/**
 * Match an event against a list of bindings, returning true if one ran.
 *
 * Kept as a pure function so the decision — which is all the interesting
 * behaviour — is testable without a DOM.
 */
export function dispatch(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey"> & {
    target?: EventTarget | null;
  },
  bindings: Binding[],
): boolean {
  if (event.altKey) return false;
  const key = event.key.toLowerCase();
  const mod = event.ctrlKey || event.metaKey;
  const typing = isTyping(event.target ?? null);

  for (const binding of bindings) {
    if (binding.key !== key) continue;
    if (!!binding.mod !== mod) continue;
    // A bare letter must never fire mid-sentence; a modified key or Escape may.
    if (typing && !binding.whileTyping && !binding.mod) continue;
    binding.run();
    return true;
  }
  return false;
}

/** Shown in tooltips, so the buttons teach their own shortcuts. */
export function hint(key: string, mod = false): string {
  return mod ? `Ctrl+${key.toUpperCase()}` : key.toUpperCase();
}
