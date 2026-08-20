/**
 * WordNet 3.1 as a second dictionary corpus.
 *
 * Webster's 1913 is the period-correct authority and stays first in every
 * result — its sense of *nice* is the one Austen meant. But it is a 1913 book,
 * and a reader meeting a word it never recorded gets nothing at all.
 * `omnipotence` is absent from it; so is most vocabulary coined since.
 *
 * So WordNet sits underneath: consulted when Webster's has no entry, and
 * always labelled, so the reader knows which dictionary answered.
 *
 * Source: https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz (Princeton
 * WordNet licence — free to use and redistribute with attribution).
 */

import { execFileSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, statSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const ARCHIVE_URL = "https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz";

/** The four data files, and the part of speech each carries. */
const FILES = [
  ["data.noun", "noun"],
  ["data.verb", "verb"],
  ["data.adj", "adjective"],
  ["data.adv", "adverb"],
];

export async function ensureWordnet(cacheDir) {
  mkdirSync(cacheDir, { recursive: true });
  const archive = join(cacheDir, "wn31.tar.gz");

  if (!existsSync(archive) || statSync(archive).size < 1_000_000) {
    console.log("· downloading WordNet 3.1 (~16 MB)");
    const response = await fetch(ARCHIVE_URL);
    if (!response.ok) {
      throw new Error(`WordNet download failed: ${response.status}`);
    }
    await pipeline(Readable.fromWeb(response.body), createWriteStream(archive));
  }

  const dict = join(cacheDir, "dict");
  if (!existsSync(join(dict, "data.noun"))) {
    console.log("· extracting");
    // bsdtar ships with Windows 10+ and GNU tar with everything else; this is
    // a developer build step, so shelling out beats a tar dependency.
    execFileSync("tar", [
      "-xzf",
      archive,
      "-C",
      cacheDir,
      ...FILES.map(([f]) => `dict/${f}`),
    ]);
  }
  return dict;
}

/**
 * One line of a WordNet data file.
 *
 * Format: `offset lex_filenum ss_type w_cnt word lex_id [word lex_id...]
 * p_cnt [pointers...] | gloss`. `w_cnt` is two hex digits, which is the only
 * part that is easy to get wrong.
 */
function parseLine(line) {
  const bar = line.indexOf(" | ");
  if (bar < 0) return null;

  const fields = line.slice(0, bar).split(" ");
  if (fields.length < 5) return null;

  const wordCount = Number.parseInt(fields[3], 16);
  if (!Number.isFinite(wordCount) || wordCount < 1) return null;

  const words = [];
  for (let i = 0; i < wordCount; i++) {
    // Words and their lex_ids alternate from field 4.
    const raw = fields[4 + i * 2];
    if (!raw) break;
    words.push(
      raw
        // WordNet joins multi-word entries with underscores, and marks
        // adjective syntax with a trailing "(a)", "(p)", "(ip)".
        .replace(/\(.*?\)$/, "")
        .replace(/_/g, " "),
    );
  }

  const whole = line.slice(bar + 3).trim();
  // A gloss is `definition; "example"; "example"`. The examples are quoted
  // sentences and belong to the word, not to its meaning, so the definition
  // is everything before the first quoted run.
  const quote = whole.indexOf('; "');
  const gloss = (quote > 0 ? whole.slice(0, quote) : whole).trim();

  return { words, gloss };
}

/**
 * Read WordNet into `{ headword -> { display, pos, glosses[] } }`.
 *
 * Grouped by word and part of speech so a noun with nine senses is one entry
 * with nine senses, matching how Webster's is stored.
 */
export async function readWordnet(dictDir) {
  /**
   * Keyed by "headword::pos". A separator rather than a space, because
   * WordNet headwords are frequently several words — `abstract entity` — and
   * splitting one on a space truncates it to `abstract`.
   *
   * @type {Map<string, {headword: string, display: string, pos: string, glosses: string[]}>}
   */
  const entries = new Map();

  for (const [file, pos] of FILES) {
    const text = await readFile(join(dictDir, file), "latin1");
    for (const line of text.split("\n")) {
      // The licence header is indented; every data line starts with a digit.
      if (!line || !/^\d/.test(line)) continue;

      const parsed = parseLine(line);
      if (!parsed || !parsed.gloss) continue;

      for (const word of parsed.words) {
        const headword = word.toLowerCase();
        if (!headword || !/[a-z]/.test(headword)) continue;

        const key = `${headword}::${pos}`;
        const existing = entries.get(key);
        if (existing) {
          if (!existing.glosses.includes(parsed.gloss)) {
            existing.glosses.push(parsed.gloss);
          }
        } else {
          entries.set(key, { headword, display: word, pos, glosses: [parsed.gloss] });
        }
      }
    }
  }
  return entries;
}

/**
 * Add WordNet to an open dictionary database.
 *
 * Returns how many entries and senses were written.
 */
export function insertWordnet(db, entries) {
  const insertEntry = db.prepare(
    "INSERT INTO entries (headword, display, pos, source) VALUES (?, ?, ?, 'wordnet')",
  );
  const insertSense = db.prepare(
    "INSERT INTO senses (entry_id, ordinal, gloss, labels) VALUES (?, ?, ?, '[]')",
  );

  let entryCount = 0;
  let senseCount = 0;

  db.exec("BEGIN");
  for (const entry of entries.values()) {
    const { lastInsertRowid } = insertEntry.run(
      entry.headword,
      entry.display,
      entry.pos,
    );
    entryCount++;
    // WordNet orders senses by frequency, most common first, which is the
    // order a reader wants.
    entry.glosses.forEach((gloss, i) => {
      insertSense.run(lastInsertRowid, i, gloss);
      senseCount++;
    });
  }
  db.exec("COMMIT");

  return { entryCount, senseCount };
}
