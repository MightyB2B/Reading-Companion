//! Word documents, via `docx-rs`.

use docx_rs::{AlignmentType, Docx, Paragraph, Run, RunFonts};

use super::Document;
use crate::error::{AppError, Result};

/// A paragraph in one of the document's few styles.
///
/// Built here rather than leaning on Word's named styles: a `.docx` that
/// refers to styles it does not define renders differently depending on the
/// user's template, and an export should look the same wherever it is opened.
fn styled(text: &str, size: usize, bold: bool, italic: bool, serif: bool) -> Paragraph {
    let mut run = Run::new().add_text(text).size(size);
    if bold {
        run = run.bold();
    }
    if italic {
        run = run.italic();
    }
    run = run.fonts(
        RunFonts::new().ascii(if serif { "Georgia" } else { "Calibri" }),
    );
    Paragraph::new().add_run(run)
}

pub fn render(doc: &Document) -> Result<Vec<u8>> {
    // docx-rs sizes in half-points, so 48 is 24pt.
    let mut file = Docx::new()
        .add_paragraph(styled(&doc.title, 48, true, false, true).align(AlignmentType::Center));

    if let Some(author) = &doc.author {
        file = file.add_paragraph(
            styled(author, 26, false, true, true).align(AlignmentType::Center),
        );
    }
    file = file.add_paragraph(
        styled(&format!("Exported {}", doc.exported), 18, false, false, false)
            .align(AlignmentType::Center),
    );

    for section in &doc.sections {
        file = file.add_paragraph(Paragraph::new());
        file = file.add_paragraph(styled(&section.heading, 32, true, false, true));
        if let Some(blurb) = &section.blurb {
            file = file.add_paragraph(styled(blurb, 20, false, true, false));
        }

        for entry in &section.entries {
            if let Some(title) = &entry.title {
                file = file.add_paragraph(styled(title, 24, true, false, true));
            }
            for text in &entry.body {
                file = file.add_paragraph(styled(text, 22, false, false, true));
            }
            for bullet in &entry.bullets {
                // A bullet glyph rather than a numbering definition: numbering
                // in OOXML needs an abstract list, an instance, and a style,
                // and none of that survives being pasted anywhere else.
                file = file.add_paragraph(
                    styled(&format!("• {bullet}"), 22, false, false, true)
                        .indent(Some(360), None, None, None),
                );
            }
            if let Some(note) = &entry.note {
                file = file.add_paragraph(styled(note, 18, false, true, false));
            }
        }
    }

    let mut out = std::io::Cursor::new(Vec::new());
    file.build()
        .pack(&mut out)
        .map_err(|e| AppError::Invalid(format!("writing docx: {e}")))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn produces_a_readable_word_package() {
        let bytes = render(&super::super::sample()).unwrap();

        // A .docx is a zip; if this is not one, Word will not open it.
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(zip.by_name("[Content_Types].xml").is_ok());

        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();

        assert!(xml.contains("Ethics"));
        assert!(xml.contains("substance"));
        // The library escapes for us; check it really did, because an
        // unescaped ampersand makes the file unopenable.
        assert!(xml.contains("&amp;") || !xml.contains(" & "));
        assert!(!xml.contains("<and>"));
    }
}
