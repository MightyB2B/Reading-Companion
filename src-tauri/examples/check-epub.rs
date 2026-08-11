//! Inspect an EPUB's own structure: spine, table of contents, and what each
//! chapter extracts to.
//!
//! Written because a real EPUB3 imported badly — 3,110 pages, a table of
//! contents imported as though it were text, and no chapter names at all —
//! and none of that was visible from the outside.
//!
//! Usage: cargo run --example check-epub -- <file.epub>

use book_companion_lib::ingest::html;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run --example check-epub -- <file.epub>");
        std::process::exit(2);
    };

    let mut doc = match epub::doc::EpubDoc::new(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not open: {e}");
            std::process::exit(1);
        }
    };

    println!("title : {:?}", doc.mdata("title").map(|m| m.value.clone()));
    println!("author: {:?}", doc.mdata("creator").map(|m| m.value.clone()));
    println!("chapters (spine): {}", doc.get_num_chapters());
    println!("toc entries     : {}", doc.toc.len());

    println!("\n--- table of contents, top level ---");
    for nav in doc.toc.iter().take(4) {
        println!(
            "  {:?} ({} children) -> {}",
            nav.label,
            nav.children.len(),
            nav.content.display()
        );
    }

    // What the importer actually produces now.
    println!("\n--- full import ---");
    match book_companion_lib::ingest::epub_source::read(std::path::Path::new(&path)) {
        Err(e) => println!("  FAILED: {e}"),
        Ok(document) => {
            let paragraphs: usize = document.pages.iter().map(|p| p.paragraphs.len()).sum();
            let labelled = document.pages.iter().filter(|p| p.label.is_some()).count();
            println!(
                "  {} pages, {paragraphs} paragraphs, {labelled} labelled",
                document.pages.len()
            );
            // Pages between one label and the next are that division's
            // chapters. Checkable against what the book actually has.
            let mut divisions: Vec<(String, usize)> = Vec::new();
            for page in &document.pages {
                match &page.label {
                    Some(label) => divisions.push((label.clone(), 1)),
                    None => {
                        if let Some(last) = divisions.last_mut() {
                            last.1 += 1;
                        }
                    }
                }
            }
            println!("  divisions: {}", divisions.len());
            for (label, chapters) in divisions.iter().take(8) {
                println!("    {chapters:>4} chapters  {label}");
            }

            let longest = document
                .pages
                .iter()
                .flat_map(|p| &p.paragraphs)
                .map(|t| t.chars().count())
                .max()
                .unwrap_or(0);
            println!("  longest paragraph: {longest} chars");
            // Navigation must not survive as reading matter.
            let navish = document
                .pages
                .iter()
                .flat_map(|p| &p.paragraphs)
                .filter(|t| t.starts_with("The First Book of Moses"))
                .count();
            println!("  contents entries leaking into the text: {navish}");
        }
    }

    println!("\n--- spine, with what each chapter extracts to ---");
    let count = doc.get_num_chapters();
    for index in 0..count.min(14) {
        if !doc.set_current_chapter(index) {
            continue;
        }
        let path = doc.get_current_path().map(|p| p.display().to_string());
        let Some((content, _)) = doc.get_current_str() else {
            println!("  [{index:3}] {path:?}  (no content)");
            continue;
        };

        let paragraphs = html::paragraphs(&content);
        let links = content.matches("<a ").count();
        let chars: usize = paragraphs.iter().map(|p| p.len()).sum();

        println!(
            "  [{index:3}] {}\n         {} paragraphs, {chars} chars, {links} links{}",
            path.as_deref().unwrap_or("?"),
            paragraphs.len(),
            // A document that is mostly links is navigation, not reading.
            if links > 20 && links as f32 > paragraphs.len() as f32 * 0.5 {
                "   <-- looks like navigation"
            } else {
                ""
            }
        );
        if let Some(first) = paragraphs.first() {
            println!("         | {}", first.chars().take(110).collect::<String>());
        }
    }
}
