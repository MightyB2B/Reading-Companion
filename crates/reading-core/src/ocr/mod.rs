//! Photo -> text -> paragraph blocks.

pub mod legible;
pub mod orient;
pub mod outline;
pub mod page_number;
pub mod preprocess;
pub mod segment;

use crate::models::{DraftBlock, Normalization};
use segment::WordOracle;

/// The full post-OCR pipeline, in the order the evidence says it must run.
///
/// Deduplication happens *after* segmentation because the repeat is a
/// repetition of paragraphs, not of characters — comparing blocks is both
/// simpler and more robust than string surgery on the raw output. Furniture
/// removal comes last, once the blocks it needs to compare actually exist.
///
/// `book_title` is used to recognise the running head on a single page,
/// without waiting for a second page to prove it repeats.
pub fn blocks_from_raw(
    raw: &str,
    book_title: &str,
    level: Normalization,
    oracle: Option<&dyn WordOracle>,
) -> Vec<DraftBlock> {
    let mut blocks = segment::segment_page(raw, level, oracle);
    segment::dedup_repeated_pass(&mut blocks);
    // Before furniture: a trailing apology from the model would otherwise sit
    // where the footer check expects to find a footer.
    segment::strip_model_commentary(&mut blocks);
    segment::strip_single_page_furniture(&mut blocks, book_title);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_turns_looped_output_into_clean_paragraphs() {
        let raw = "\
This is the first paragraph of the page and it is long enough to count as real prose rather than furniture.

This is the second paragraph of the page, likewise long enough to be treated as substantial content.
This is the first paragraph of the page and it is long enough to count as real prose rather than furniture.

This is the second paragraph of the page, likewise long enough to be treated as substantial content.
12";
        let blocks = blocks_from_raw(raw, "Some Book", Normalization::Off, None);
        assert_eq!(blocks.len(), 2, "got {blocks:#?}");
    }

    /// Verbatim `glm-ocr` output for page 1 of Charles Hodge's *Systematic
    /// Theology*, the first real page imported into the app.
    ///
    /// Before this pipeline existed, all eight blocks came back as
    /// `paragraph` — so the reader was invited to write a one-sentence summary
    /// of "CHAPTER I." and of the running head.
    const HODGE_PAGE_1: &str = "\
SYSTEMATIC THEOLOGY

INTRODUCTION.

CHAPTER I.

ON METHOD.

§ 1. Theology a Science.

In every science there are two factors: facts and ideas; or, facts and the mind. Science is more than knowledge. Knowledge is the persuasion of what is true on adequate evidence. But the facts of astronomy, chemistry, or history do not constitute the science of those departments of knowledge. Nor does the mere orderly arrangement of facts amount to science.

The Bible is no more a system of theology, than nature is a system of chemistry or of mechanics. We find in nature the facts which the chemist or the mechanical philosopher has to examine, and from them to ascertain the laws by which they are determined.

1. Theology a Science.";

    #[test]
    fn hodge_page_yields_only_real_prose_as_paragraphs() {
        let blocks = blocks_from_raw(
            HODGE_PAGE_1,
            "Systematic Theology",
            Normalization::Off,
            None,
        );

        let paragraphs: Vec<&DraftBlock> = blocks
            .iter()
            .filter(|b| b.kind == crate::models::BlockKind::Paragraph)
            .collect();

        assert_eq!(
            paragraphs.len(),
            2,
            "only the two body paragraphs should be summarisable, got {:#?}",
            blocks.iter().map(|b| (b.kind, &b.text_raw)).collect::<Vec<_>>()
        );
        assert!(paragraphs[0].text_raw.starts_with("In every science"));
        assert!(paragraphs[1].text_raw.starts_with("The Bible is no more"));
    }

    #[test]
    fn hodge_page_drops_the_running_head_and_footer_echo() {
        let blocks = blocks_from_raw(
            HODGE_PAGE_1,
            "Systematic Theology",
            Normalization::Off,
            None,
        );
        let texts: Vec<&str> = blocks.iter().map(|b| b.text_raw.as_str()).collect();

        assert!(
            !texts.contains(&"SYSTEMATIC THEOLOGY"),
            "the running head should not survive: {texts:#?}"
        );
        assert!(
            !texts.contains(&"1. Theology a Science."),
            "the footer echo of the section head should not survive: {texts:#?}"
        );
        // The real section head above it must stay.
        assert!(texts.contains(&"§ 1. Theology a Science."));
    }

    /// Verbatim tail of glm-ocr's output on a photograph of the open book at
    /// pages 2-3. Twice the text overran the token budget, so the model
    /// stopped mid-sentence and explained itself — in prose, which became a
    /// paragraph the reader was invited to summarise.
    #[test]
    fn model_commentary_never_becomes_a_paragraph() {
        let raw = "\
Every science has its own method, determined by its peculiar nature. This is a matter of so much \
importance that it has been erected into a distinct department.
The text is cut off before it can be fully transcribed. The continuation is missing, so no complete \
transcription can be provided.";

        let blocks = blocks_from_raw(raw, "Systematic Theology", Normalization::Off, None);

        assert_eq!(blocks.len(), 1, "got {blocks:#?}");
        assert!(blocks[0].text_raw.starts_with("Every science has its own method"));
        assert!(
            !blocks.iter().any(|b| b.text_raw.contains("cut off before")),
            "the model's apology survived as book text"
        );
    }

    /// Verbatim from a second import — an entirely different formulation from
    /// the first, which is why phrase-matching alone was not enough.
    #[test]
    fn a_self_congratulatory_trailer_is_removed() {
        let raw = "\
Every science has its own method, determined by its peculiar nature. Modern literature abounds in \
works on Methodology, that is, on the science of method.

The transcription is complete and follows the instructions exactly as they are printed. No \
additional text is present. The image contains only the text from the book.";

        let blocks = blocks_from_raw(raw, "Systematic Theology", Normalization::Off, None);

        assert_eq!(blocks.len(), 1, "got {blocks:#?}");
        assert!(blocks[0].text_raw.starts_with("Every science"));
    }

    #[test]
    fn a_page_ending_in_ordinary_prose_is_untouched() {
        // The positional rule must not eat a real final paragraph.
        let raw = "\
First paragraph of the page, long enough to be treated as real prose by every stage of this pipeline.

And the page ends here, mid-argument, as pages generally do when the paper runs out.";
        let blocks = blocks_from_raw(raw, "Some Book", Normalization::Off, None);
        assert_eq!(blocks.len(), 2, "the closing paragraph was deleted");
    }

    #[test]
    fn real_prose_is_not_mistaken_for_commentary() {
        // Long prose that happens to mention images and text must survive.
        let raw = "The image shows nothing to the untrained eye, and yet the naturalist reads in \
it a whole history; for the text of nature is written in a hand that must be learned before it can \
be construed, and the student who cannot transcribe it faithfully will never read it at all, nor \
will he understand why the attempt is worth making in the first place.";
        let blocks = blocks_from_raw(raw, "Some Book", Normalization::Off, None);
        assert_eq!(blocks.len(), 1, "long prose was deleted as commentary");
    }

    #[test]
    fn hodge_page_classifies_its_headings() {
        let blocks = blocks_from_raw(
            HODGE_PAGE_1,
            "Systematic Theology",
            Normalization::Off,
            None,
        );
        let headings: Vec<&str> = blocks
            .iter()
            .filter(|b| b.kind == crate::models::BlockKind::Heading)
            .map(|b| b.text_raw.as_str())
            .collect();

        for expected in [
            "INTRODUCTION.",
            "CHAPTER I.",
            "ON METHOD.",
            "§ 1. Theology a Science.",
        ] {
            assert!(
                headings.contains(&expected),
                "{expected:?} should be a heading, got {headings:#?}"
            );
        }
    }
}
