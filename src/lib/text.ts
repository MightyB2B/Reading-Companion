/**
 * Corresponding words.
 *
 * Borrowed from Logos, and one of the few features there that changes how you
 * read rather than what you can look up. A philosopher building an argument
 * and a biblical author building a pericope both repeat their key terms, and
 * seeing the repetitions tinted makes the shape of the passage visible before
 * you have worked out what it says.
 *
 * Deliberately dumb about morphology. A crude suffix fold catches
 * `substance`/`substances` and `knowing`/`knows`, which is where nearly all
 * the value is; a real stemmer would catch a few more and occasionally group
 * two words that are not related at all, which is worse than missing one.
 */

/**
 * Words too common to be worth noticing when repeated.
 *
 * Kept deliberately short: the point is to drop the grammatical skeleton, not
 * to second-guess which content words matter. Anything domain-specific stays
 * in, because in a theology book `god` and `man` repeating *is* the signal.
 */
const STOPWORDS = new Set([
  "the", "a", "an", "and", "or", "but", "if", "then", "than", "that", "this",
  "these", "those", "of", "to", "in", "on", "at", "by", "for", "with", "from",
  "as", "is", "are", "was", "were", "be", "been", "being", "it", "its", "he",
  "she", "they", "them", "his", "her", "their", "we", "us", "our", "you",
  "your", "i", "me", "my", "not", "no", "nor", "so", "such", "which", "who",
  "whom", "what", "when", "where", "how", "all", "any", "some", "there",
  "here", "have", "has", "had", "do", "does", "did", "can", "could", "would",
  "should", "shall", "will", "may", "might", "must", "one", "also", "only",
  "into", "upon", "was", "were", "himself", "itself", "themselves",
]);

/** Occurrences must reach this many to count as a repetition worth tinting. */
const MIN_OCCURRENCES = 2;

/** Below this length a word is structural, not thematic. */
const MIN_LENGTH = 4;

/** How many distinct repeated words get their own colour. */
const MAX_GROUPS = 6;

export interface Echo {
  start: number;
  end: number;
  /** Which repeated word this belongs to. Drives the colour. */
  group: number;
}

/**
 * Fold a surface form towards its stem, well enough to group a plural with
 * its singular.
 *
 * Order matters: `-ies` before `-s`, `-ing`/`-ed` before the shorter rules.
 */
function fold(word: string): string {
  const w = word.toLowerCase();
  if (w.length <= MIN_LENGTH) return w;
  for (const [suffix, replacement] of [
    ["ies", "y"],
    ["ing", ""],
    ["edly", ""],
    ["ed", ""],
    ["es", ""],
    ["s", ""],
    ["ly", ""],
  ] as const) {
    if (w.endsWith(suffix) && w.length - suffix.length >= MIN_LENGTH) {
      return w.slice(0, w.length - suffix.length) + replacement;
    }
  }
  return w;
}

/**
 * Find the words a passage repeats, with every position of each.
 *
 * Offsets are character positions in `text`, matching every other offset in
 * the application, so they can be handed straight to the decorator.
 */
export function correspondingWords(text: string): Echo[] {
  const found: { start: number; end: number; stem: string }[] = [];

  // Unicode-aware so an em-dash or a curly apostrophe does not split a word
  // in a nineteenth-century text.
  const pattern = /[\p{L}][\p{L}'’-]*/gu;
  for (const m of text.matchAll(pattern)) {
    const surface = m[0];
    if (surface.length < MIN_LENGTH) continue;
    const lower = surface.toLowerCase();
    if (STOPWORDS.has(lower)) continue;
    found.push({ start: m.index, end: m.index + surface.length, stem: fold(surface) });
  }

  const byStem = new Map<string, typeof found>();
  for (const hit of found) {
    byStem.set(hit.stem, [...(byStem.get(hit.stem) ?? []), hit]);
  }

  // Most-repeated first, so when there are more repeated words than colours
  // the ones that carry the passage are the ones that get tinted.
  const repeated = [...byStem.entries()]
    .filter(([, hits]) => hits.length >= MIN_OCCURRENCES)
    .sort((a, b) => b[1].length - a[1].length)
    .slice(0, MAX_GROUPS);

  const echoes: Echo[] = [];
  repeated.forEach(([, hits], group) => {
    for (const hit of hits) {
      echoes.push({ start: hit.start, end: hit.end, group });
    }
  });
  return echoes.sort((a, b) => a.start - b.start);
}
