//! The canon, and the many ways people abbreviate it.
//!
//! Ids are OSIS abbreviations (`Gen`, `1Cor`, `Ps`) because that is the
//! interchange standard every Bible text format already speaks, so a text
//! imported from anywhere lines up with references found anywhere else.
//!
//! The alias lists are deliberately conservative. A two-letter abbreviation
//! that is also an English word costs far more in false positives than it
//! saves in recall: `Is` for Isaiah would fire on every other sentence of
//! ordinary prose, so it is simply absent. A citation the parser misses is a
//! reference the reader can still read; a citation it invents is noise in the
//! middle of the page.

/// Whether a book is numbered, and whether it must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ordinal {
    /// Genesis. An ordinal in front means this is not the book.
    Never,
    /// Corinthians. Without one the citation is incomplete.
    Always,
    /// John — the Gospel unnumbered, the epistles numbered.
    Optional,
}

pub struct BookDef {
    /// OSIS id, or its suffix for a numbered book: `Cor` becomes `1Cor`.
    pub osis: &'static str,
    pub name: &'static str,
    pub ordinal: Ordinal,
    /// Lower-case, without any ordinal prefix.
    pub aliases: &'static [&'static str],
    /// How much to trust the name alone. Below 1.0 for anything that is also
    /// an ordinary word.
    pub confidence: f32,
}

macro_rules! book {
    ($osis:literal, $name:literal, $ord:ident, [$($alias:literal),* $(,)?]) => {
        BookDef { osis: $osis, name: $name, ordinal: Ordinal::$ord,
                  aliases: &[$($alias),*], confidence: 1.0 }
    };
    ($osis:literal, $name:literal, $ord:ident, [$($alias:literal),* $(,)?], $conf:literal) => {
        BookDef { osis: $osis, name: $name, ordinal: Ordinal::$ord,
                  aliases: &[$($alias),*], confidence: $conf }
    };
}

pub static BOOKS: &[BookDef] = &[
    // ---- Old Testament ---------------------------------------------------
    book!("Gen", "Genesis", Never, ["genesis", "gen", "gn"]),
    book!("Exod", "Exodus", Never, ["exodus", "exod", "exo", "ex"]),
    book!("Lev", "Leviticus", Never, ["leviticus", "lev", "lv"]),
    book!("Num", "Numbers", Never, ["numbers", "num", "nu", "nb"]),
    book!("Deut", "Deuteronomy", Never, ["deuteronomy", "deut", "deu", "dt"]),
    book!("Josh", "Joshua", Never, ["joshua", "josh", "jos"]),
    book!("Judg", "Judges", Never, ["judges", "judg", "jdg"]),
    book!("Ruth", "Ruth", Never, ["ruth", "rth"]),
    book!("Sam", "Samuel", Always, ["samuel", "sam", "sa"]),
    book!("Kgs", "Kings", Always, ["kings", "kgs", "kin", "ki"]),
    book!("Chr", "Chronicles", Always, ["chronicles", "chron", "chr"]),
    book!("Ezra", "Ezra", Never, ["ezra", "ezr"]),
    book!("Neh", "Nehemiah", Never, ["nehemiah", "neh"]),
    book!("Esth", "Esther", Never, ["esther", "esth", "est"]),
    // "Job" is an English word, but a chapter number must follow it, which
    // "Job security" does not supply.
    book!("Job", "Job", Never, ["job"], 0.9),
    book!("Ps", "Psalms", Never, ["psalms", "psalm", "psa", "pss", "ps"]),
    book!("Prov", "Proverbs", Never, ["proverbs", "prov", "prv", "pr"]),
    book!("Eccl", "Ecclesiastes", Never, ["ecclesiastes", "eccles", "eccl", "ecc", "qoh"]),
    book!("Song", "Song of Solomon", Never,
          ["song of solomon", "song of songs", "canticles", "cant", "sos"]),
    book!("Isa", "Isaiah", Never, ["isaiah", "isa", "isai"]),
    book!("Jer", "Jeremiah", Never, ["jeremiah", "jer"]),
    book!("Lam", "Lamentations", Never, ["lamentations", "lam"]),
    book!("Ezek", "Ezekiel", Never, ["ezekiel", "ezek", "eze", "ezk"]),
    book!("Dan", "Daniel", Never, ["daniel", "dan", "dn"]),
    book!("Hos", "Hosea", Never, ["hosea", "hos"]),
    book!("Joel", "Joel", Never, ["joel", "joe"]),
    book!("Amos", "Amos", Never, ["amos"]),
    book!("Obad", "Obadiah", Never, ["obadiah", "obad", "oba"]),
    book!("Jonah", "Jonah", Never, ["jonah", "jon"]),
    book!("Mic", "Micah", Never, ["micah", "mic"]),
    book!("Nah", "Nahum", Never, ["nahum", "nah"]),
    book!("Hab", "Habakkuk", Never, ["habakkuk", "hab"]),
    book!("Zeph", "Zephaniah", Never, ["zephaniah", "zeph", "zep"]),
    book!("Hag", "Haggai", Never, ["haggai", "hag"]),
    book!("Zech", "Zechariah", Never, ["zechariah", "zech", "zec"]),
    book!("Mal", "Malachi", Never, ["malachi", "mal"]),
    // ---- New Testament ---------------------------------------------------
    book!("Matt", "Matthew", Never, ["matthew", "matt", "mat", "mt"]),
    book!("Mark", "Mark", Never, ["mark", "mrk", "mk"], 0.95),
    book!("Luke", "Luke", Never, ["luke", "luk", "lk"]),
    book!("John", "John", Optional, ["john", "joh", "jhn", "jn"]),
    book!("Acts", "Acts", Never, ["acts", "act"]),
    book!("Rom", "Romans", Never, ["romans", "rom", "ro", "rm"]),
    book!("Cor", "Corinthians", Always, ["corinthians", "corin", "cor", "co"]),
    book!("Gal", "Galatians", Never, ["galatians", "gal"]),
    book!("Eph", "Ephesians", Never, ["ephesians", "eph"]),
    book!("Phil", "Philippians", Never, ["philippians", "philip", "phil", "php"]),
    book!("Col", "Colossians", Never, ["colossians", "col"]),
    book!("Thess", "Thessalonians", Always, ["thessalonians", "thess", "thes", "th"]),
    book!("Tim", "Timothy", Always, ["timothy", "tim", "ti"]),
    book!("Titus", "Titus", Never, ["titus", "tit"]),
    book!("Phlm", "Philemon", Never, ["philemon", "philem", "phlm", "phm"]),
    book!("Heb", "Hebrews", Never, ["hebrews", "heb"]),
    book!("Jas", "James", Never, ["james", "jas", "jam"]),
    book!("Pet", "Peter", Always, ["peter", "pet", "pt"]),
    book!("Jude", "Jude", Never, ["jude", "jud"]),
    book!("Rev", "Revelation", Never,
          ["revelation", "revelations", "rev", "apocalypse", "apoc"]),
];

/// Resolve a written book name, with any ordinal that preceded it.
///
/// Returns the OSIS id and how much the name alone is worth.
pub fn resolve_book(word: &str, ordinal: Option<u8>) -> Option<(String, f32)> {
    let needle = word.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    for def in BOOKS {
        if !def.aliases.iter().any(|a| *a == needle) {
            continue;
        }
        return match (def.ordinal, ordinal) {
            // "1 Genesis" is not a citation, it is a misparse.
            (Ordinal::Never, Some(_)) => None,
            (Ordinal::Never, None) => Some((def.osis.to_string(), def.confidence)),

            // "Corinthians 3:1" with no number names no book.
            (Ordinal::Always, None) => None,
            (Ordinal::Always, Some(n)) if n <= 3 => {
                Some((format!("{n}{}", def.osis), def.confidence))
            }
            (Ordinal::Always, Some(_)) => None,

            (Ordinal::Optional, None) => Some((def.osis.to_string(), def.confidence)),
            (Ordinal::Optional, Some(n)) if n <= 3 => {
                Some((format!("{n}{}", def.osis), def.confidence))
            }
            (Ordinal::Optional, Some(_)) => None,
        };
    }
    None
}

/// `1Cor` becomes "1 Corinthians", for anything a person will read.
pub fn canonical_name(osis: &str) -> String {
    let (prefix, base) = match osis.chars().next() {
        Some(c) if c.is_ascii_digit() => (Some(c), &osis[1..]),
        _ => (None, osis),
    };
    let name = BOOKS
        .iter()
        .find(|d| d.osis == base)
        .map(|d| d.name)
        .unwrap_or(base);
    match prefix {
        Some(n) => format!("{n} {name}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_books_need_their_number() {
        assert_eq!(resolve_book("corinthians", None), None);
        assert_eq!(
            resolve_book("corinthians", Some(1)).unwrap().0,
            "1Cor"
        );
        assert_eq!(resolve_book("cor", Some(2)).unwrap().0, "2Cor");
    }

    #[test]
    fn unnumbered_books_refuse_a_number() {
        // "1 Genesis" is a misparse, not a book.
        assert_eq!(resolve_book("genesis", Some(1)), None);
        assert_eq!(resolve_book("genesis", None).unwrap().0, "Gen");
    }

    #[test]
    fn john_is_both() {
        assert_eq!(resolve_book("john", None).unwrap().0, "John");
        assert_eq!(resolve_book("john", Some(1)).unwrap().0, "1John");
        assert_eq!(resolve_book("john", Some(3)).unwrap().0, "3John");
    }

    #[test]
    fn names_round_trip_for_display() {
        assert_eq!(canonical_name("Rom"), "Romans");
        assert_eq!(canonical_name("1Cor"), "1 Corinthians");
        assert_eq!(canonical_name("3John"), "3 John");
        assert_eq!(canonical_name("Ps"), "Psalms");
    }

    #[test]
    fn every_book_resolves_from_its_own_aliases() {
        for def in BOOKS {
            let ord = match def.ordinal {
                Ordinal::Always => Some(1),
                _ => None,
            };
            for alias in def.aliases {
                assert!(
                    resolve_book(alias, ord).is_some(),
                    "{alias} did not resolve to {}",
                    def.osis
                );
            }
        }
    }

    #[test]
    fn the_canon_is_complete() {
        // 66 books, with the five numbered families counted once each.
        let numbered = BOOKS.iter().filter(|d| d.ordinal == Ordinal::Always).count();
        let optional = BOOKS.iter().filter(|d| d.ordinal == Ordinal::Optional).count();
        let plain = BOOKS.iter().filter(|d| d.ordinal == Ordinal::Never).count();
        // Sam, Kgs, Chr, Cor, Thess, Tim, Pet are 2 each; John is 1 + 3.
        assert_eq!(numbered, 7);
        assert_eq!(optional, 1);
        assert_eq!(plain + numbered * 2 + 4, 66, "the canon is miscounted");
    }

    #[test]
    fn no_alias_is_claimed_by_two_books() {
        let mut seen = std::collections::HashMap::new();
        for def in BOOKS {
            for alias in def.aliases {
                if let Some(other) = seen.insert(*alias, def.osis) {
                    panic!("{alias} is claimed by both {other} and {}", def.osis);
                }
            }
        }
    }

    #[test]
    fn dangerous_short_words_are_not_aliases() {
        // Each of these would fire constantly in ordinary prose.
        for word in ["is", "so", "am", "he", "it", "we", "on", "no", "as", "at"] {
            assert_eq!(
                resolve_book(word, None),
                None,
                "{word:?} should not resolve to a book"
            );
            assert_eq!(resolve_book(word, Some(1)), None);
        }
    }
}
