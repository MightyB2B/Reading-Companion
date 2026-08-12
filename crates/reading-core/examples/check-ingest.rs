//! Check EPUB / PDF / web extraction against real documents.
//!
//! Unit tests cover the HTML extractor with markup I wrote myself, which is
//! exactly the markup it is guaranteed to handle. Real EPUBs and real PDFs are
//! stranger, so this runs the actual files.
//!
//! Usage: cargo run --example check-ingest -- <file-or-url> [...]

use reading_core::ingest::{epub_source, html, pdf_source, Document, SourceKind};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: cargo run --example check-ingest -- <file-or-url> [...]");
        std::process::exit(2);
    }

    for source in &args {
        let kind = SourceKind::from_source(source);
        println!("\n=== {source}\n    kind: {:?}", kind);

        let result: Result<Document, _> = match kind {
            SourceKind::Epub => epub_source::read(std::path::Path::new(source)),
            SourceKind::Pdf => pdf_source::read(std::path::Path::new(source)),
            SourceKind::Web => html::fetch(source).await,
            SourceKind::Photo => {
                println!("    (an image — not a text source)");
                continue;
            }
        };

        match result {
            Err(e) => println!("    FAILED: {e}"),
            Ok(doc) => {
                let paragraphs: usize = doc.pages.iter().map(|p| p.paragraphs.len()).sum();
                let chars: usize = doc
                    .pages
                    .iter()
                    .flat_map(|p| &p.paragraphs)
                    .map(|s| s.len())
                    .sum();
                println!("    title : {:?}", doc.title);
                println!("    author: {:?}", doc.author);
                println!(
                    "    {} pages, {paragraphs} paragraphs, {chars} chars",
                    doc.pages.len()
                );

                if let Some(first) = doc.pages.iter().find(|p| !p.paragraphs.is_empty()) {
                    println!("    label : {:?}", first.label);
                    for para in first.paragraphs.iter().take(2) {
                        let shown: String = para.chars().take(150).collect();
                        println!("      | {shown}");
                    }
                }

                // The failure that would matter most: navigation or boilerplate
                // arriving as prose the reader is asked to summarise.
                let suspicious: Vec<&String> = doc
                    .pages
                    .iter()
                    .flat_map(|p| &p.paragraphs)
                    .filter(|p| {
                        let l = p.to_lowercase();
                        l.contains("cookie") || l.contains("skip to content") || l.contains("javascript is disabled")
                    })
                    .collect();
                if !suspicious.is_empty() {
                    println!("    WARNING: {} boilerplate-looking paragraphs", suspicious.len());
                    for s in suspicious.iter().take(2) {
                        println!("      ! {}", s.chars().take(100).collect::<String>());
                    }
                }
            }
        }
    }
}
