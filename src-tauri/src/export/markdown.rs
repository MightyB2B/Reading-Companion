//! Markdown, for anything text-shaped.

use super::Document;

pub fn render(doc: &Document) -> String {
    let mut out = String::new();

    out.push_str(&format!("# {}\n\n", doc.title));
    if let Some(author) = &doc.author {
        out.push_str(&format!("*{author}*\n\n"));
    }
    out.push_str(&format!("Exported {}\n", doc.exported));

    for section in &doc.sections {
        out.push_str(&format!("\n## {}\n\n", section.heading));
        if let Some(blurb) = &section.blurb {
            out.push_str(&format!("{blurb}\n\n"));
        }

        for entry in &section.entries {
            if let Some(title) = &entry.title {
                out.push_str(&format!("### {title}\n\n"));
            }
            for paragraph in &entry.body {
                out.push_str(&format!("{paragraph}\n\n"));
            }
            for bullet in &entry.bullets {
                out.push_str(&format!("- {bullet}\n"));
            }
            if !entry.bullets.is_empty() {
                out.push('\n');
            }
            if let Some(note) = &entry.note {
                out.push_str(&format!("*{note}*\n\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_whole_document() {
        let md = render(&super::super::sample());
        assert!(md.starts_with("# Ethics"));
        assert!(md.contains("*Spinoza*"));
        assert!(md.contains("## Terms"));
        assert!(md.contains("### substance"));
        assert!(md.contains("- earlier: a thing & its parts"));
    }

    #[test]
    fn markdown_keeps_the_text_verbatim() {
        // Unlike the XML formats, `<and>` is not markup here and must survive
        // untouched — escaping it would put literal entities in the file.
        let md = render(&super::super::sample());
        assert!(md.contains("in itself <and> conceived"), "{md}");
    }
}
