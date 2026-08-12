//! Print the outline the navigator would show, from a real library.
//!
//! Unit tests use headings I typed out; this uses the ones actually sitting in
//! the database after a real import, which is where the chapter-title problem
//! showed up in the first place.
//!
//! Usage: cargo run --example check-outline -- <path-to-library.sqlite>

use reading_core::db::Db;
use reading_core::ocr::outline::{fold_titles, heading_level, Heading, HeadingLevel};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run --example check-outline -- <library.sqlite>");
        std::process::exit(2);
    };

    let db = match Db::open(&path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("could not open {path}: {e}");
            std::process::exit(1);
        }
    };

    for book in db.list_books().unwrap() {
        println!("\n=== {} ===", book.title);

        // Each level carries forward until something replaces it, which is
        // what the navigator shows.
        let mut part: Option<String> = None;
        let mut chapter: Option<String> = None;
        let mut section: Option<String> = None;

        for page in db.list_pages(book.id).unwrap() {
            let blocks = db.list_blocks(page.id).unwrap();
            let headings = fold_titles(
                blocks
                    .iter()
                    .filter(|b| b.kind == "heading")
                    .map(|b| Heading {
                        block_id: b.id,
                        text: b.text_norm.clone(),
                        level: heading_level(&b.text_norm),
                    })
                    .collect(),
            );

            let mut topics: Vec<&str> = Vec::new();
            for h in &headings {
                match h.level {
                    HeadingLevel::Part => part = Some(h.text.clone()),
                    HeadingLevel::Chapter => {
                        chapter = Some(h.text.clone());
                        section = None;
                    }
                    HeadingLevel::Section => section = Some(h.text.clone()),
                    HeadingLevel::Topic => topics.push(&h.text),
                    HeadingLevel::Minor => {}
                }
            }

            let paragraphs = blocks.iter().filter(|b| b.kind == "paragraph").count();
            println!(
                "\n  page {}  ({paragraphs} paragraphs)",
                page.page_no.map_or("—".to_string(), |n| n.to_string()),
            );
            println!("      part    : {}", part.as_deref().unwrap_or("—"));
            println!("      chapter : {}", chapter.as_deref().unwrap_or("—"));
            println!("      section : {}", section.as_deref().unwrap_or("—"));
            if !topics.is_empty() {
                println!("      topics  : {}", topics.join(" / "));
            }
        }
    }
}
