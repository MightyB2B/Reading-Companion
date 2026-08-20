//! OpenDocument Text, for LibreOffice.
//!
//! Hand-written rather than taken from a crate: ODF is a zip of four small XML
//! files and there is no maintained Rust writer worth a dependency. The format
//! is genuinely simpler than OOXML, and writing it directly means the styles
//! are ours rather than whatever a library thought reasonable.
//!
//! One rule the specification is strict about: `mimetype` must be the first
//! entry in the archive and must be **stored uncompressed**, so the file can be
//! recognised by reading its first bytes. Compress it and LibreOffice refuses
//! the document, with no hint as to why.

use std::io::Write;

use zip::write::SimpleFileOptions;

use super::{escape_xml, Document};
use crate::error::{AppError, Result};

const MIMETYPE: &str = "application/vnd.oasis.opendocument.text";

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
 <manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3">
 <office:styles>
  <style:style style:name="Standard" style:family="paragraph"/>
 </office:styles>
</office:document-styles>"#;

fn paragraph(style: &str, text: &str) -> String {
    format!(
        "<text:p text:style-name=\"{style}\">{}</text:p>",
        escape_xml(text)
    )
}

fn content(doc: &Document) -> String {
    let mut body = String::new();

    body.push_str(&paragraph("Title", &doc.title));
    if let Some(author) = &doc.author {
        body.push_str(&paragraph("Subtitle", author));
    }
    body.push_str(&paragraph("Standard", &format!("Exported {}", doc.exported)));

    for section in &doc.sections {
        body.push_str(&paragraph("Heading_20_1", &section.heading));
        if let Some(blurb) = &section.blurb {
            body.push_str(&paragraph("Standard", blurb));
        }
        for entry in &section.entries {
            if let Some(title) = &entry.title {
                body.push_str(&paragraph("Heading_20_2", title));
            }
            for text in &entry.body {
                body.push_str(&paragraph("Standard", text));
            }
            if !entry.bullets.is_empty() {
                body.push_str("<text:list>");
                for bullet in &entry.bullets {
                    body.push_str(&format!(
                        "<text:list-item>{}</text:list-item>",
                        paragraph("Standard", bullet)
                    ));
                }
                body.push_str("</text:list>");
            }
            if let Some(note) = &entry.note {
                body.push_str(&paragraph("Standard", note));
            }
        }
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3">
 <office:body><office:text>{body}</office:text></office:body>
</office:document-content>"#
    )
}

pub fn render(doc: &Document) -> Result<Vec<u8>> {
    let buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buffer);

    let fail = |e: zip::result::ZipError| AppError::Invalid(format!("writing odt: {e}"));

    // Stored, not deflated, and first in the archive. This is the one part of
    // ODF that is not negotiable.
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .map_err(fail)?;
    zip.write_all(MIMETYPE.as_bytes())?;

    let deflated = SimpleFileOptions::default();
    for (name, text) in [
        ("META-INF/manifest.xml", MANIFEST.to_string()),
        ("styles.xml", STYLES.to_string()),
        ("content.xml", content(doc)),
    ] {
        zip.start_file(name, deflated).map_err(fail)?;
        zip.write_all(text.as_bytes())?;
    }

    Ok(zip.finish().map_err(fail)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn archive(doc: &Document) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        zip::ZipArchive::new(std::io::Cursor::new(render(doc).unwrap())).unwrap()
    }

    #[test]
    fn the_mimetype_is_first_and_uncompressed() {
        // LibreOffice refuses the document otherwise.
        let mut zip = archive(&super::super::sample());
        let entry = zip.by_index(0).unwrap();
        assert_eq!(entry.name(), "mimetype");
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn every_required_part_is_present() {
        let mut zip = archive(&super::super::sample());
        for name in ["mimetype", "META-INF/manifest.xml", "styles.xml", "content.xml"] {
            assert!(zip.by_name(name).is_ok(), "{name} missing");
        }
    }

    #[test]
    fn the_content_carries_the_document_and_escapes_it() {
        let mut zip = archive(&super::super::sample());
        let mut xml = String::new();
        zip.by_name("content.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();

        assert!(xml.contains("Ethics"));
        assert!(xml.contains("substance"));
        // The angle brackets in the gloss must not become markup.
        assert!(xml.contains("&lt;and&gt;"), "{xml}");
        assert!(!xml.contains("<and>"));
        assert!(xml.contains("a thing &amp; its parts"));
    }
}
