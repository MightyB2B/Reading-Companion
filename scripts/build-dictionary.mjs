#!/usr/bin/env node
/**
 * Builds `src-tauri/resources/dict.sqlite` from Webster's Revised Unabridged
 * Dictionary (1913).
 *
 * Why the 1913 edition rather than a modern wordlist: it records the senses
 * nineteenth-century and earlier authors were actually using. Its first sense
 * for "nice" is "Foolish; silly; simple" — which is what the word means in the
 * books this app exists to help you read, and what no modern dictionary will
 * tell you.
 *
 * Run:  node scripts/build-dictionary.mjs
 *
 * Source: https://github.com/ssvivian/WebstersDictionary (MIT tooling over
 * Project Gutenberg public-domain text).
 */

import { DatabaseSync } from "node:sqlite";
import { mkdirSync, existsSync, statSync, createWriteStream, rmSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { ensureWordnet, insertWordnet, readWordnet } from "./wordnet.mjs";
import { pipeline } from "node:stream/promises";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const CACHE = join(ROOT, ".cache");
const SOURCE_JSON = join(CACHE, "webster1913.json");
const OUT_DIR = join(ROOT, "src-tauri", "resources");
const OUT_DB = join(OUT_DIR, "dict.sqlite");

const SOURCE_URL =
  "https://raw.githubusercontent.com/ssvivian/WebstersDictionary/master/dictionary.json";

/**
 * Usage labels Webster's marks inline. Surfacing these matters: a reader needs
 * to know a sense was already obsolete in 1913, not just that it exists.
 */
const LABEL_PATTERNS = [
  [/\[Obs\.?[^\]]*\]/i, "obsolete"],
  [/\[Archaic[^\]]*\]/i, "archaic"],
  [/\[Colloq\.?[^\]]*\]/i, "colloquial"],
  [/\[R\.\]/, "rare"],
  [/\[Rare[^\]]*\]/i, "rare"],
  [/\[Poet(ic|\.)[^\]]*\]/i, "poetic"],
  [/\[Prov\.[^\]]*\]/i, "provincial"],
  [/\[Scot\.[^\]]*\]/i, "scottish"],
  [/\[Slang[^\]]*\]/i, "slang"],
  [/\[Law[^\]]*\]/i, "law"],
];

/** "See Show." / "Same as Ancient." — the dictionary's own variant mappings. */
const CROSS_REF = /^(?:See|Same as)\s+([A-Za-z][A-Za-z' -]*?)\s*[.,]/;

/**
 * Early-modern spellings absent from Webster's as headwords. Each is only
 * accepted if the target actually exists in the dictionary, so a bad guess
 * here degrades to "no result" rather than to a wrong definition.
 */
const ARCHAIC_SPELLINGS = {
  publick: "public", musick: "music", logick: "logic", physick: "physic",
  magick: "magic", tragick: "tragic", republick: "republic", criticks: "critics",
  critick: "critic", classick: "classic", domestick: "domestic",
  antient: "ancient", antients: "ancients", chuse: "choose", chused: "chosen",
  compleat: "complete", cloathes: "clothes", cloath: "cloth",
  divers: "diverse", enquire: "inquire", entituled: "entitled",
  farther: "further", gaol: "jail", honour: "honor", labour: "labor",
  sopha: "sofa", stedfast: "steadfast", subtil: "subtle", surprize: "surprise",
  tho: "though", thro: "through", vizard: "visor", wo: "woe",
  yeares: "years", yeare: "year", olde: "old", bee: "be", hee: "he",
  shee: "she", mee: "me", wee: "we", doe: "do", goe: "go", soe: "so",
  onely: "only", untill: "until", uppon: "upon", sonne: "son",
  wandring: "wandering", entring: "entering", offring: "offering",
};

/** Irregular inflections a suffix rule cannot reach. */
const IRREGULAR_FORMS = {
  ran: "run", run: "run", went: "go", gone: "go", was: "be", were: "be",
  been: "be", am: "be", is: "be", are: "be", had: "have", has: "have",
  did: "do", done: "do", said: "say", made: "make", took: "take",
  taken: "take", saw: "see", seen: "see", came: "come", knew: "know",
  known: "know", gave: "give", given: "give", found: "find", thought: "think",
  told: "tell", became: "become", left: "leave", felt: "feel", brought: "bring",
  began: "begin", begun: "begin", kept: "keep", held: "hold", wrote: "write",
  written: "write", stood: "stand", heard: "hear", let: "let", meant: "mean",
  set: "set", met: "meet", paid: "pay", sat: "sit", spoke: "speak",
  spoken: "speak", lay: "lie", led: "lead", grew: "grow", grown: "grow",
  lost: "lose", fell: "fall", fallen: "fall", sent: "send", built: "build",
  understood: "understand", drew: "draw", drawn: "draw", broke: "break",
  broken: "break", spent: "spend", cut: "cut", rose: "rise", risen: "rise",
  driven: "drive", drove: "drive", bought: "buy", wore: "wear", worn: "wear",
  chose: "choose", chosen: "choose", ate: "eat", eaten: "eat",
  men: "man", women: "woman", children: "child", feet: "foot", teeth: "tooth",
  mice: "mouse", geese: "goose", oxen: "ox", lives: "life", wives: "wife",
  knives: "knife", leaves: "leaf", halves: "half", selves: "self",
  doth: "do", dost: "do", hath: "have", hast: "have", saith: "say",
  art: "be", wert: "be", shalt: "shall", wilt: "will", canst: "can",
};

async function ensureSource() {
  mkdirSync(CACHE, { recursive: true });
  if (existsSync(SOURCE_JSON) && statSync(SOURCE_JSON).size > 1_000_000) {
    console.log(`· using cached source (${mb(statSync(SOURCE_JSON).size)})`);
    return;
  }
  console.log(`· downloading ${SOURCE_URL}`);
  const res = await fetch(SOURCE_URL);
  if (!res.ok) throw new Error(`download failed: ${res.status} ${res.statusText}`);
  await pipeline(Readable.fromWeb(res.body), createWriteStream(SOURCE_JSON));
  console.log(`· downloaded (${mb(statSync(SOURCE_JSON).size)})`);
}

const mb = (n) => `${(n / 1024 / 1024).toFixed(1)} MB`;

function extractLabels(text) {
  const labels = [];
  for (const [re, name] of LABEL_PATTERNS) {
    if (re.test(text) && !labels.includes(name)) labels.push(name);
  }
  return labels;
}

/** Strip Webster's bracketed usage notes from the reading text. */
function cleanGloss(text) {
  return text.replace(/\s+/g, " ").trim();
}

function createSchema(db) {
  db.exec(`
    PRAGMA journal_mode = OFF;
    PRAGMA synchronous = OFF;

    CREATE TABLE entries (
      id       INTEGER PRIMARY KEY,
      headword TEXT NOT NULL,   -- lowercased, for lookup
      display  TEXT NOT NULL,   -- as printed in the dictionary
      pos      TEXT,
      source   TEXT NOT NULL
    );

    CREATE TABLE senses (
      id       INTEGER PRIMARY KEY,
      entry_id INTEGER NOT NULL REFERENCES entries(id),
      ordinal  INTEGER NOT NULL,
      gloss    TEXT NOT NULL,
      labels   TEXT NOT NULL DEFAULT '[]'  -- JSON array
    );

    -- Surface form -> headword. Populated from the dictionary's own
    -- cross-references plus a curated early-modern list.
    CREATE TABLE variants (
      variant TEXT PRIMARY KEY,
      lemma   TEXT NOT NULL,
      kind    TEXT NOT NULL   -- cross_reference | archaic_spelling | irregular
    );

    CREATE INDEX idx_entries_headword ON entries(headword);
    CREATE INDEX idx_senses_entry     ON senses(entry_id, ordinal);
    CREATE INDEX idx_variants_lemma   ON variants(lemma);
  `);
}

async function main() {
  await ensureSource();

  console.log("· parsing");
  const raw = JSON.parse(await readFile(SOURCE_JSON, "utf8"));
  console.log(`· ${raw.length.toLocaleString()} raw entries`);

  mkdirSync(OUT_DIR, { recursive: true });
  if (existsSync(OUT_DB)) rmSync(OUT_DB);

  const db = new DatabaseSync(OUT_DB);
  createSchema(db);

  const insertEntry = db.prepare(
    "INSERT INTO entries (headword, display, pos, source) VALUES (?, ?, ?, 'webster1913')",
  );
  const insertSense = db.prepare(
    "INSERT INTO senses (entry_id, ordinal, gloss, labels) VALUES (?, ?, ?, ?)",
  );
  const insertVariant = db.prepare(
    "INSERT OR IGNORE INTO variants (variant, lemma, kind) VALUES (?, ?, ?)",
  );

  const headwords = new Set();
  const crossRefs = [];
  let senseCount = 0;

  db.exec("BEGIN");
  for (const entry of raw) {
    const display = String(entry.word ?? "").trim();
    if (!display) continue;
    const headword = display.toLowerCase();
    headwords.add(headword);

    const { lastInsertRowid } = insertEntry.run(
      headword,
      display,
      entry.pos ? String(entry.pos).trim() : null,
    );

    const defs = Array.isArray(entry.definitions) ? entry.definitions : [];
    defs.forEach((def, i) => {
      const text = cleanGloss(String(def ?? ""));
      if (!text) return;
      insertSense.run(
        lastInsertRowid,
        i,
        text,
        JSON.stringify(extractLabels(text)),
      );
      senseCount++;

      // "See Show." tells us `shew` is a variant of `show`.
      const m = text.match(CROSS_REF);
      if (m && i === 0) {
        const target = m[1].trim().toLowerCase();
        if (target && target !== headword) crossRefs.push([headword, target]);
      }
    });
  }
  db.exec("COMMIT");

  console.log(`· ${headwords.size.toLocaleString()} headwords, ${senseCount.toLocaleString()} senses`);

  // --- variants -----------------------------------------------------------
  // Every mapping is validated against the headword set, so a bad guess
  // becomes "no result" rather than a confidently wrong definition.
  db.exec("BEGIN");
  let kept = { cross_reference: 0, archaic_spelling: 0, irregular: 0 };

  for (const [variant, lemma] of crossRefs) {
    if (headwords.has(lemma)) {
      insertVariant.run(variant, lemma, "cross_reference");
      kept.cross_reference++;
    }
  }
  for (const [variant, lemma] of Object.entries(ARCHAIC_SPELLINGS)) {
    if (headwords.has(lemma)) {
      insertVariant.run(variant, lemma, "archaic_spelling");
      kept.archaic_spelling++;
    }
  }
  for (const [variant, lemma] of Object.entries(IRREGULAR_FORMS)) {
    if (headwords.has(lemma)) {
      insertVariant.run(variant, lemma, "irregular");
      kept.irregular++;
    }
  }
  db.exec("COMMIT");

  console.log(
    `· variants: ${kept.cross_reference} cross-references, ` +
      `${kept.archaic_spelling} archaic spellings, ${kept.irregular} irregular forms`,
  );

  // --- WordNet, as the corpus underneath ---------------------------------
  //
  // Webster's stays authoritative and stays first in every result: its sense
  // of "nice" is the one Austen meant. But it is a 1913 book, and a reader who
  // meets a word it never recorded gets nothing at all. This is what answers
  // for those.
  console.log("");
  console.log("· WordNet");
  const wordnetDir = await ensureWordnet(CACHE);
  const wordnet = await readWordnet(wordnetDir);
  const added = insertWordnet(db, wordnet);
  console.log(
    `  ${added.entryCount.toLocaleString()} entries, ` +
      `${added.senseCount.toLocaleString()} senses`,
  );

  // --- full-text index ----------------------------------------------------
  console.log("· building full-text index");
  db.exec(`
    CREATE VIRTUAL TABLE senses_fts USING fts5(
      gloss, content='senses', content_rowid='id', tokenize='porter unicode61'
    );
    INSERT INTO senses_fts(rowid, gloss) SELECT id, gloss FROM senses;
    INSERT INTO senses_fts(senses_fts) VALUES('optimize');
  `);

  db.exec("PRAGMA journal_mode = DELETE");
  db.exec("VACUUM");
  db.close();

  console.log(`\n✓ ${OUT_DB} (${mb(statSync(OUT_DB).size)})`);

  // --- sanity checks ------------------------------------------------------
  // These are the lookups the app depends on; failing loudly here is far
  // better than shipping a dictionary that silently misses archaic words.
  const check = new DatabaseSync(OUT_DB, { readOnly: true });
  const probes = [
    ["nice", "obsolete first sense — the period-correct payoff"],
    ["omnipotence", "absent from Webster's; WordNet answers"],
    ["unending", "absent from Webster's; WordNet answers"],
    ["shew", "cross-reference variant"],
    ["hath", "archaic verb form"],
    ["publick", "curated archaic spelling"],
    ["ran", "irregular inflection"],
  ];
  console.log("\nsanity checks:");
  let failures = 0;
  for (const [word, why] of probes) {
    const direct = check
      .prepare("SELECT COUNT(*) n FROM entries WHERE headword = ?")
      .get(word).n;
    const via = check
      .prepare("SELECT lemma FROM variants WHERE variant = ?")
      .get(word);
    const ok = direct > 0 || !!via;
    if (!ok) failures++;
    console.log(
      `  ${ok ? "✓" : "✗"} ${word.padEnd(9)} ${
        direct ? `${direct} entr${direct === 1 ? "y" : "ies"}` : `-> ${via?.lemma ?? "MISSING"}`
      }  (${why})`,
    );
  }
  check.close();
  if (failures) {
    console.error(`\n${failures} sanity check(s) failed`);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
