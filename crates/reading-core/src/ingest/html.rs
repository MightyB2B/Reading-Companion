//! Pulling readable prose out of markup.
//!
//! Shared by EPUB (whose chapters are XHTML) and by web import. The job is to
//! recover the author's paragraphs and headings while leaving out navigation,
//! scripts, cookie banners, and the rest of the furniture — the reader is here
//! to work through an argument, and being asked to summarise a cookie notice
//! would be worse than useless.

use std::collections::HashMap;

use scraper::{Html, Selector};

use crate::error::{AppError, Result};

use super::{clean_paragraph, paginate, Document};

/// Fetch a web page and extract the article from it.
///
/// This is the one part of the application that reaches the network for
/// content, and only ever at the reader's explicit request with a URL they
/// typed. Everything else — transcription, coaching, the dictionary — stays on
/// the machine.
pub async fn fetch(url: &str) -> Result<Document> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::Invalid(
            "a web address must begin with http:// or https://".into(),
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        // Identify honestly rather than impersonating a browser.
        .user_agent("ReadingCompanion/0.1 (personal reading tool)")
        .build()
        .map_err(|e| AppError::Other(anyhow::anyhow!("could not build the http client: {e}")))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Invalid(format!("could not fetch {url}: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::Invalid(format!(
            "{url} returned {}",
            resp.status()
        )));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Invalid(format!("could not read {url}: {e}")))?;

    let paragraphs = paragraphs(&body);
    if paragraphs.is_empty() {
        return Err(AppError::Invalid(format!(
            "no readable article was found at {url}"
        )));
    }

    let title = title(&body);
    Ok(Document {
        pages: paginate(paragraphs, None),
        title,
        author: None,
    })
}

/// Elements that never contain prose the reader wants.
const STRIP: [&str; 12] = [
    "script", "style", "nav", "header", "footer", "aside", "form", "noscript", "svg", "iframe",
    "figure", "button",
];

/// Where an article's body is likely to live, in descending order of
/// confidence. Falling back to `body` is what makes this work on the many
/// pages that use none of these.
const CONTAINERS: [&str; 6] = ["article", "main", "[role=main]", "#content", ".post", "body"];

/// Block-level elements worth keeping, in document order.
const BLOCKS: &str = "p, h1, h2, h3, h4, h5, h6, blockquote, li, pre";

/// Extract the title, if the document declares one.
pub fn title(document: &str) -> Option<String> {
    let doc = Html::parse_document(document);

    // An article's own <h1> is usually better than <title>, which often
    // carries the site name as well.
    for sel in ["article h1", "h1", "title"] {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(el) = doc.select(&selector).next() {
            if let Some(text) = clean_paragraph(&el.text().collect::<String>()) {
                return Some(text);
            }
        }
    }
    None
}

/// A piece of text pulled out of a document, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub text: String,
    pub is_heading: bool,
}

/// Share of an element's text that must be inside links before it counts as
/// navigation rather than prose.
///
/// A table of contents is a list of links and nothing else. Importing one as
/// reading matter turned sixty-six entries of a real EPUB into sixty-six
/// chapter headings and eight pages of text that says nothing.
const NAVIGATION_LINK_SHARE: f32 = 0.8;

fn is_navigation(el: &scraper::ElementRef) -> bool {
    let Ok(anchor) = Selector::parse("a") else {
        return false;
    };
    let total = el.text().collect::<String>();
    let total_len = total.split_whitespace().collect::<Vec<_>>().join(" ").len();
    if total_len == 0 {
        return true;
    }
    let linked: usize = el
        .select(&anchor)
        .map(|a| {
            a.text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .len()
        })
        .sum();
    linked as f32 / total_len as f32 >= NAVIGATION_LINK_SHARE
}

/// Extract text from an HTML or XHTML document, in reading order.
///
/// `anchors` maps element ids to the labels an EPUB's table of contents gives
/// them. Where a document carries such an id, a heading is emitted before the
/// text that follows it — which is how a book with no headings of its own gets
/// its structure back. A real EPUB's contents pointed *into* its files with
/// `#pgepubid00003` fragments, and its chapters were nameless without this.
pub fn extract(document: &str, anchors: &HashMap<String, String>) -> Vec<Extracted> {
    let doc = Html::parse_document(document);

    let Ok(block_sel) = Selector::parse(BLOCKS) else {
        return Vec::new();
    };

    let inside_furniture = |el: &scraper::ElementRef| {
        el.ancestors().any(|n| {
            n.value()
                .as_element()
                .is_some_and(|e| STRIP.contains(&e.name()))
        })
    };

    for container in CONTAINERS {
        let Ok(container_sel) = Selector::parse(container) else {
            continue;
        };
        let Some(root) = doc.select(&container_sel).next() else {
            continue;
        };

        let mut out: Vec<Extracted> = Vec::new();

        // One ordered walk rather than a selector pass: an anchor's heading
        // has to land in front of the text it introduces, which means seeing
        // both in the order the document sets them.
        for node in root.descendants() {
            let Some(el) = scraper::ElementRef::wrap(node) else {
                continue;
            };

            if !anchors.is_empty() {
                if let Some(label) = el.value().attr("id").and_then(|id| anchors.get(id)) {
                    if !out.iter().any(|e| e.is_heading && &e.text == label) {
                        out.push(Extracted {
                            text: label.clone(),
                            is_heading: true,
                        });
                    }
                }
            }

            if !block_sel.matches(&el) || inside_furniture(&el) {
                continue;
            }
            // A <li> inside a <p> would otherwise be emitted twice; take only
            // the leaf-most block.
            if el.select(&block_sel).next().is_some() {
                continue;
            }
            // A line that is entirely a link belongs to a table of contents.
            if is_navigation(&el) {
                continue;
            }
            if let Some(text) = clean_paragraph(&el.text().collect::<String>()) {
                let heading = matches!(
                    el.value().name(),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                );
                out.push(Extracted {
                    text,
                    is_heading: heading,
                });
            }
        }

        // A container that yielded almost nothing is the wrong container —
        // try the next, less specific one.
        if out.len() >= 2 {
            return out;
        }
    }
    Vec::new()
}

/// Extract paragraphs from an HTML or XHTML document, in reading order.
pub fn paragraphs(document: &str) -> Vec<String> {
    extract(document, &HashMap::new())
        .into_iter()
        .map(|e| e.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_paragraphs_in_document_order() {
        let html = r#"
            <html><body><article>
              <h1>On Method</h1>
              <p>The first paragraph of the argument.</p>
              <p>The second paragraph, which follows from it.</p>
            </article></body></html>"#;

        let paras = paragraphs(html);
        assert_eq!(paras.len(), 3);
        assert_eq!(paras[0], "On Method");
        assert!(paras[1].starts_with("The first paragraph"));
        assert!(paras[2].starts_with("The second paragraph"));
    }

    #[test]
    fn leaves_out_navigation_and_scripts() {
        let html = r#"
            <html><body>
              <nav><a href="/">Home</a><a href="/about">About the site</a></nav>
              <script>console.log("tracking pixel goes here");</script>
              <article>
                <p>The actual prose the reader came for, first paragraph.</p>
                <p>And the second paragraph of the actual prose.</p>
              </article>
              <footer>Copyright some publisher, all rights reserved.</footer>
            </body></html>"#;

        let paras = paragraphs(html);
        assert_eq!(paras.len(), 2, "got {paras:#?}");
        assert!(paras.iter().all(|p| !p.contains("Home")));
        assert!(paras.iter().all(|p| !p.contains("Copyright")));
        assert!(paras.iter().all(|p| !p.contains("tracking")));
    }

    /// The fault that made a real EPUB import eight pages of nothing: a table
    /// of contents is a list of links, and it was read as text.
    #[test]
    fn a_table_of_contents_is_not_reading_matter() {
        let html = r#"
            <html><body>
              <p><a href="c1.html">The First Book of Moses: Called Genesis</a></p>
              <p><a href="c2.html">The Second Book of Moses: Called Exodus</a></p>
              <p>In the beginning of the actual prose, which is not a link.</p>
              <p>A second real paragraph, so the container is not rejected.</p>
            </body></html>"#;

        let paras = paragraphs(html);
        assert_eq!(paras.len(), 2, "got {paras:#?}");
        assert!(paras.iter().all(|p| !p.contains("Called Genesis")));
    }

    #[test]
    fn a_link_inside_a_sentence_is_kept() {
        // Only a line that is *entirely* a link is navigation.
        let html = r#"<html><body>
            <p>See <a href="x">the appendix</a> for the full table of figures and other matter.</p>
            <p>A second paragraph so the container is accepted.</p>
        </body></html>"#;
        let paras = paragraphs(html);
        assert_eq!(paras.len(), 2);
        assert!(paras[0].contains("the appendix"));
    }

    /// An EPUB's contents can point into the middle of a file. Where it does,
    /// the label it gives becomes a heading in front of the text it names —
    /// otherwise a book whose chapters carry no headings of their own arrives
    /// with no structure at all.
    #[test]
    fn a_contents_anchor_becomes_a_heading() {
        let html = r#"<html><body>
            <p>Text before the anchor, belonging to what came earlier.</p>
            <div id="pgepubid00003"></div>
            <p>The first verse of the book the contents named.</p>
            <p>The second verse of that book.</p>
        </body></html>"#;

        let mut anchors = HashMap::new();
        anchors.insert(
            "pgepubid00003".to_string(),
            "The Third Book of Moses: Called Leviticus".to_string(),
        );

        let out = extract(html, &anchors);
        let heading_at = out.iter().position(|e| e.is_heading).expect("a heading");

        assert_eq!(out[heading_at].text, "The Third Book of Moses: Called Leviticus");
        // It must land before the text it introduces, not after.
        assert!(out[heading_at + 1].text.starts_with("The first verse"));
        assert!(out[heading_at - 1].text.starts_with("Text before"));
    }

    #[test]
    fn html_headings_are_marked_as_headings() {
        let html = "<html><body><article><h2>On Method</h2>\
                    <p>The opening paragraph.</p><p>The second paragraph.</p>\
                    </article></body></html>";
        let out = extract(html, &HashMap::new());
        assert!(out[0].is_heading);
        assert!(!out[1].is_heading);
    }

    #[test]
    fn falls_back_to_the_body_when_there_is_no_article() {
        // Most of the web has no <article> element.
        let html = r#"
            <html><body>
              <p>A first paragraph in a plain old page.</p>
              <p>A second paragraph in the same plain page.</p>
            </body></html>"#;
        assert_eq!(paragraphs(html).len(), 2);
    }

    #[test]
    fn does_not_emit_nested_blocks_twice() {
        let html = r#"
            <html><body><article>
              <blockquote><p>A quoted paragraph inside a blockquote.</p></blockquote>
              <p>An ordinary paragraph following it.</p>
            </article></body></html>"#;

        let paras = paragraphs(html);
        assert_eq!(paras.len(), 2, "got {paras:#?}");
        assert!(paras[0].starts_with("A quoted paragraph"));
    }

    #[test]
    fn collapses_the_whitespace_xhtml_leaves_behind() {
        let html = "<html><body><article><p>Broken\n   across\n   lines.</p>\
                    <p>Another paragraph here.</p></article></body></html>";
        assert_eq!(paragraphs(html)[0], "Broken across lines.");
    }

    #[test]
    fn prefers_the_article_heading_for_the_title() {
        let html = r#"<html><head><title>Site Name — On Method</title></head>
            <body><article><h1>On Method</h1><p>Body text goes here.</p></article></body></html>"#;
        assert_eq!(title(html).as_deref(), Some("On Method"));
    }

    #[test]
    fn falls_back_to_the_title_element() {
        let html = "<html><head><title>A Paper</title></head><body><p>Text.</p></body></html>";
        assert_eq!(title(html).as_deref(), Some("A Paper"));
    }
}
