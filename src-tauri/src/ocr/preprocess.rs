//! Preparing a photograph for the OCR model.
//!
//! Deliberately light. Traditional OCR engines needed aggressive deskewing and
//! dewarping to cope with a curved page shot on a phone; a vision-language
//! model tolerates that geometry natively. Skipping it keeps OpenCV — and a
//! genuinely painful Windows build dependency — out of the project entirely.
//!
//! What is left is what actually pays: correcting orientation so the text is
//! the right way up, and bounding the resolution so we are not shipping a
//! 12-megapixel image into the model for no accuracy gain.

use std::io::Cursor;

use image::DynamicImage;

use crate::error::{AppError, Result};

/// Longest edge, in pixels, of the image handed to the model.
///
/// Book text stays comfortably legible at this size and the payload is small
/// enough to keep per-page latency in the low seconds.
pub const MAX_EDGE: u32 = 1600;

#[derive(Debug, Clone)]
pub struct PreprocessOptions {
    pub max_edge: u32,
    /// Off by default: colour costs nothing at this resolution and greyscaling
    /// can flatten faint ink on aged paper.
    pub grayscale: bool,
    pub jpeg_quality: u8,
}

impl Default for PreprocessOptions {
    fn default() -> Self {
        Self {
            max_edge: MAX_EDGE,
            grayscale: false,
            jpeg_quality: 90,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

/// Target dimensions preserving aspect ratio, never upscaling.
///
/// Enlarging a photograph invents no detail for the model to read and costs
/// latency, so an image already within bounds is left alone.
pub fn fit_within(width: u32, height: u32, max_edge: u32) -> Dimensions {
    let longest = width.max(height);
    if longest <= max_edge || longest == 0 {
        return Dimensions { width, height };
    }
    let scale = max_edge as f64 / longest as f64;
    Dimensions {
        width: ((width as f64 * scale).round() as u32).max(1),
        height: ((height as f64 * scale).round() as u32).max(1),
    }
}

/// Apply an EXIF orientation value (1-8) to an image.
///
/// Phones almost always record orientation in metadata rather than rotating
/// the pixels. Ignoring it hands the model a sideways page.
pub fn apply_orientation(img: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Turn a photographed page upright by looking at which way its text runs.
///
/// EXIF says how the phone was held, not how the book sat in the frame, so a
/// page shot sideways arrives "correctly oriented" and still unreadable — and
/// worse, a landscape photograph of one sideways page has the same proportions
/// as a photograph of an open book, which is how a single page came to be
/// split down the middle of its text.
fn upright_page(img: DynamicImage) -> DynamicImage {
    crate::ocr::orient::upright(img).0
}

/// Read the EXIF orientation tag, defaulting to 1 (upright) when absent.
pub fn read_orientation(bytes: &[u8]) -> u32 {
    let mut cursor = Cursor::new(bytes);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut cursor) else {
        return 1;
    };
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1)
}

/// What kind of file was handed to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// HEIC/HEIF, as produced by every iPhone by default.
    Heic,
    /// Anything the `image` crate handles.
    Standard(image::ImageFormat),
    Unrecognised,
}

/// Brands that appear in the `ftyp` box of a HEIF file carrying HEVC-coded
/// stills. `mif1`/`msf1` are the generic HEIF brands; AVIF uses `avif`/`avis`
/// instead and is left to the `image` crate.
const HEIF_BRANDS: [&[u8; 4]; 10] = [
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1",
];

/// Identify the file from its magic bytes rather than its extension.
///
/// A phone photo copied around gains and loses extensions freely, and a file
/// named `.jpg` that is really HEIC is common enough to be worth handling.
pub fn sniff(bytes: &[u8]) -> SourceKind {
    // ISO base media format: [4..8] is "ftyp", [8..12] is the major brand.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand: &[u8] = &bytes[8..12];
        if HEIF_BRANDS.iter().any(|b| b.as_slice() == brand) {
            return SourceKind::Heic;
        }
    }
    match image::guess_format(bytes) {
        Ok(f) => SourceKind::Standard(f),
        Err(_) => SourceKind::Unrecognised,
    }
}

/// Can a WebView render this directly?
///
/// Anything that cannot has to be transcoded on import, or the reader would
/// import a page and then be unable to look at it.
fn is_web_displayable(fmt: image::ImageFormat) -> bool {
    use image::ImageFormat::*;
    matches!(fmt, Jpeg | Png | Gif | WebP | Bmp | Ico | Avif)
}

/// Decode any supported source into an image, routing HEIC to the pure-Rust
/// HEVC decoder.
pub fn decode_any(bytes: &[u8]) -> Result<DynamicImage> {
    match sniff(bytes) {
        SourceKind::Heic => {
            let out = heic::DecoderConfig::new()
                .decode(bytes, heic::PixelLayout::Rgb8)
                .map_err(|e| AppError::Invalid(format!("could not decode HEIC image: {e}")))?;

            let buf = image::RgbImage::from_raw(out.width, out.height, out.data).ok_or_else(
                || AppError::Invalid("HEIC decoder returned a malformed pixel buffer".into()),
            )?;
            Ok(DynamicImage::ImageRgb8(buf))
        }
        SourceKind::Standard(_) => Ok(image::load_from_memory(bytes)?),
        SourceKind::Unrecognised => Err(AppError::Invalid(
            "this file is not an image format the app can read".into(),
        )),
    }
}

/// Which half of a photographed spread a page came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// Two portrait pages side by side. Raised from 1.15 after a landscape
/// photograph of a *single* page — 4033x3024, aspect 1.33 — was taken for a
/// spread and cut in half through its text. Orientation correction is the real
/// defence against that, but the shape test should not have been so eager
/// either.
const SPREAD_MIN_ASPECT: f32 = 1.25;

/// How much darker than the page the gutter must be to count as one.
///
/// Measured, not guessed. A real photograph of an open book gives 0.751 — a
/// spine in ordinary room light is not dramatically dark, and 0.72 missed it,
/// importing two pages of text as one.
///
/// This was briefly tightened to 0.72 as a second line of defence against a
/// single sideways page being split. That defence turned out to belong
/// upstream: orientation correction turns such a page back to portrait, so it
/// never reaches this test at all. Tightening here only cost real spreads.
pub const GUTTER_DARKNESS: f32 = 0.80;

/// The spine is a narrow band, not a broad dim region. If a wide swathe of the
/// middle is equally dark, the picture is unevenly lit rather than folded.
pub const GUTTER_MAX_WIDTH_FRACTION: f32 = 0.10;

/// Find the spine of an open book, as an x coordinate.
///
/// Returns `None` when the photograph shows a single page, which is the case
/// this must not get wrong: splitting a single page in half would cut every
/// line of text down the middle.
///
/// The signal is a dark vertical band near the centre. Only the middle
/// vertical band of the image is sampled, so the reader's fingers along the
/// top and bottom edges do not drag the average around.
/// The measurements behind a spread decision: (gutter x, darkest column mean,
/// page mean, darkest/page ratio, share of the middle that is that dark).
///
/// Exposed so the thresholds can be calibrated against real photographs.
pub fn gutter_diagnostics(img: &DynamicImage) -> Option<(u32, f32, f32, f32, f32)> {
    measure_gutter(img).map(|m| {
        (
            m.x,
            m.darkest,
            m.page_mean,
            m.darkest / m.page_mean.max(f32::EPSILON),
            m.dark_fraction,
        )
    })
}

struct GutterMeasure {
    x: u32,
    darkest: f32,
    page_mean: f32,
    dark_fraction: f32,
}

fn measure_gutter(img: &DynamicImage) -> Option<GutterMeasure> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || (w as f32) < (h as f32) * SPREAD_MIN_ASPECT {
        return None;
    }

    let luma = img.to_luma8();

    // Vertical middle half only: away from the hands and the table.
    let y0 = h / 4;
    let y1 = h - h / 4;
    let rows = (y1 - y0).max(1);

    let column_mean = |x: u32| -> f32 {
        let mut total = 0u32;
        for y in y0..y1 {
            total += luma.get_pixel(x, y).0[0] as u32;
        }
        total as f32 / rows as f32
    };

    // The spine is near the centre, never at the edges.
    let x0 = w * 35 / 100;
    let x1 = w * 65 / 100;

    let mut darkest_x = x0;
    let mut darkest = f32::MAX;
    for x in x0..x1 {
        let m = column_mean(x);
        if m < darkest {
            darkest = m;
            darkest_x = x;
        }
    }

    // Compare against the pages themselves, sampled well away from the spine.
    let mut page_total = 0.0;
    let mut page_count = 0.0;
    for x in (w / 10..w * 3 / 10).chain(w * 7 / 10..w * 9 / 10) {
        page_total += column_mean(x);
        page_count += 1.0;
    }
    if page_count == 0.0 {
        return None;
    }
    let page_mean = page_total / page_count;
    let threshold = page_mean * GUTTER_DARKNESS;
    let dark_columns = (x0..x1).filter(|&x| column_mean(x) < threshold).count();

    Some(GutterMeasure {
        x: darkest_x,
        darkest,
        page_mean,
        dark_fraction: dark_columns as f32 / w as f32,
    })
}

/// Find the spine of an open book, as an x coordinate.
pub fn detect_gutter(img: &DynamicImage) -> Option<u32> {
    let m = measure_gutter(img)?;

    if m.darkest >= m.page_mean * GUTTER_DARKNESS {
        return None;
    }
    // The shadow must be a narrow band. A single page curling away from the
    // lens shades a broad region; a spine does not.
    if m.dark_fraction > GUTTER_MAX_WIDTH_FRACTION {
        return None;
    }
    Some(m.x)
}

/// Cut a spread into its two pages at the gutter.
///
/// A little of the gutter is trimmed from the inner edge of each half, since
/// the curl of the paper there carries no readable text.
pub fn split_at_gutter(img: &DynamicImage, gutter_x: u32) -> (DynamicImage, DynamicImage) {
    let (w, h) = (img.width(), img.height());
    let trim = (w / 100).max(2); // ~1% of the width

    let left_w = gutter_x.saturating_sub(trim).max(1);
    let right_x = (gutter_x + trim).min(w.saturating_sub(1));
    let right_w = w.saturating_sub(right_x).max(1);

    (
        img.crop_imm(0, 0, left_w, h),
        img.crop_imm(right_x, 0, right_w, h),
    )
}

/// The two copies produced by an import.
pub struct ImportedImage {
    /// Full resolution and viewable in the app. For a HEIC or TIFF source this
    /// is a JPEG transcode; for a JPEG or PNG it is the untouched original.
    pub archival: Vec<u8>,
    pub archival_ext: &'static str,
    /// Bounded-resolution JPEG sent to the OCR model.
    pub processed: Vec<u8>,
    /// Set when the source had to be converted, so the UI can say so.
    pub converted_from: Option<&'static str>,
    /// Set when this came from one half of a photographed spread.
    pub side: Option<Side>,
}

/// Quality for the archival copy.
///
/// Higher than the processed copy: this one stands in for the original
/// photograph, which is the only faithful record of what was actually printed
/// once the OCR model has modernised the text.
const ARCHIVAL_QUALITY: u8 = 95;

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    // JPEG has no alpha channel; flatten rather than fail on an RGBA source.
    encoder.encode_image(&DynamicImage::ImageRgb8(img.to_rgb8()))?;
    Ok(out.into_inner())
}

/// Prepare an imported file: transcode if the format is not viewable, and
/// produce the bounded copy for the model.
pub fn prepare_import(bytes: &[u8], opts: &PreprocessOptions) -> Result<ImportedImage> {
    let kind = sniff(bytes);
    let orientation = read_orientation(bytes);

    let (archival, archival_ext, converted_from) = match kind {
        SourceKind::Standard(fmt) if is_web_displayable(fmt) => {
            let decoded = apply_orientation(decode_any(bytes)?, orientation);
            let turn = crate::ocr::orient::detect(&decoded);

            if turn == crate::ocr::orient::Rotation::None && orientation == 1 {
                // Nothing to correct, so keep the file byte for byte rather
                // than re-encoding it and losing a generation of quality.
                let ext = fmt.extensions_str().first().copied().unwrap_or("img");
                (bytes.to_vec(), ext, None)
            } else {
                (
                    encode_jpeg(&turn.apply(decoded), ARCHIVAL_QUALITY)?,
                    "jpg",
                    None,
                )
            }
        }
        SourceKind::Heic => {
            let img = upright_page(apply_orientation(decode_any(bytes)?, orientation));
            (encode_jpeg(&img, ARCHIVAL_QUALITY)?, "jpg", Some("HEIC"))
        }
        SourceKind::Standard(fmt) => {
            // TIFF and friends: decodable but not renderable in a WebView.
            let img = upright_page(apply_orientation(decode_any(bytes)?, orientation));
            let label: &'static str = match fmt {
                image::ImageFormat::Tiff => "TIFF",
                image::ImageFormat::Farbfeld => "Farbfeld",
                image::ImageFormat::Pnm => "PNM",
                image::ImageFormat::Tga => "TGA",
                image::ImageFormat::Dds => "DDS",
                image::ImageFormat::OpenExr => "OpenEXR",
                _ => "an unsupported format",
            };
            (encode_jpeg(&img, ARCHIVAL_QUALITY)?, "jpg", Some(label))
        }
        SourceKind::Unrecognised => {
            return Err(AppError::Invalid(
                "this file is not an image format the app can read".into(),
            ))
        }
    };

    // The processed copy is derived from the archival bytes, so orientation is
    // applied exactly once for transcoded sources.
    let processed = preprocess(&archival, opts)?;

    Ok(ImportedImage {
        archival,
        archival_ext,
        processed,
        converted_from,
        side: None,
    })
}

/// Prepare an import, splitting a photographed spread into its two pages.
///
/// A photograph of an open book contains two numbered pages. Treating it as
/// one runs both pages' text together, doubles the transcription past the
/// model's token budget — which is what made it stop mid-sentence and start
/// apologising in prose — and leaves the page number ambiguous between the
/// two. Splitting at the spine restores the one-photo-one-page assumption that
/// everything downstream is built on.
pub fn prepare_import_pages(
    bytes: &[u8],
    opts: &PreprocessOptions,
) -> Result<Vec<ImportedImage>> {
    let single = prepare_import(bytes, opts)?;

    // Detect on the archival copy: it has been oriented, so a sideways
    // photograph is upright by now and the spine really is vertical.
    let img = decode_any(&single.archival)?;
    let Some(gutter) = detect_gutter(&img) else {
        return Ok(vec![single]);
    };

    let (left, right) = split_at_gutter(&img, gutter);
    let mut out = Vec::with_capacity(2);
    for (half, side) in [(left, Side::Left), (right, Side::Right)] {
        let archival = encode_jpeg(&half, ARCHIVAL_QUALITY)?;
        let processed = preprocess(&archival, opts)?;
        out.push(ImportedImage {
            archival,
            archival_ext: "jpg",
            processed,
            converted_from: single.converted_from,
            side: Some(side),
        });
    }
    Ok(out)
}

/// Full preprocessing pass: orient, bound the resolution, optionally
/// greyscale, and re-encode as JPEG for a compact payload.
pub fn preprocess(bytes: &[u8], opts: &PreprocessOptions) -> Result<Vec<u8>> {
    let orientation = read_orientation(bytes);
    let img = decode_any(bytes)?;
    let img = apply_orientation(img, orientation);

    let target = fit_within(img.width(), img.height(), opts.max_edge);
    let img = if target.width != img.width() || target.height != img.height() {
        img.resize(
            target.width,
            target.height,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let img = if opts.grayscale {
        DynamicImage::ImageLuma8(img.to_luma8())
    } else {
        img
    };

    // JPEG rather than PNG: at this resolution the quality difference is
    // invisible to the model and the payload is several times smaller.
    let mut out = Cursor::new(Vec::new());
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, opts.jpeg_quality);
    encoder.encode_image(&img)?;
    Ok(out.into_inner())
}

/// Can this file be imported at all? Accepts HEIC, which `image::guess_format`
/// alone would reject.
pub fn is_supported_image(bytes: &[u8]) -> bool {
    !matches!(sniff(bytes), SourceKind::Unrecognised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgb};

    /// A page-like image: light paper with horizontal lines of "type".
    ///
    /// Not an abstract gradient. Import now corrects orientation, and a
    /// synthetic pattern periodic in both axes aliases under downsampling into
    /// something that reads as sideways — these fixtures were being silently
    /// rotated, which is the sort of thing that makes a test fail for a reason
    /// having nothing to do with what it is testing.
    fn sample(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            let in_line = (y % 40) >= 8 && (y % 40) < 26;
            // Irregular along the line, as real type is. A well-mixed hash,
            // because a single multiply leaves a short-period ramp that the
            // orientation detector reads as structure.
            let stroke = {
                let mut h = x.wrapping_mul(0x9E37_79B1);
                h ^= h >> 15;
                h = h.wrapping_mul(0x85EB_CA6B);
                h ^= h >> 13;
                h % 100 < 55
            };
            let margin = width / 10;
            if in_line && stroke && x > margin && x < width.saturating_sub(margin) {
                Rgb([38, 36, 33])
            } else {
                Rgb([244, 241, 234])
            }
        }))
    }

    #[test]
    fn scales_a_large_page_down_preserving_aspect() {
        let d = fit_within(4000, 3000, 1600);
        assert_eq!(d.width, 1600);
        assert_eq!(d.height, 1200);
    }

    #[test]
    fn scales_by_the_longest_edge_when_portrait() {
        let d = fit_within(3000, 4000, 1600);
        assert_eq!(d.height, 1600);
        assert_eq!(d.width, 1200);
    }

    #[test]
    fn never_upscales_a_small_image() {
        let d = fit_within(800, 600, 1600);
        assert_eq!((d.width, d.height), (800, 600));
    }

    #[test]
    fn rotating_by_ninety_swaps_the_axes() {
        let img = sample(40, 20);
        // Orientation 6 is the common "phone held upright" value.
        let out = apply_orientation(img, 6);
        assert_eq!((out.width(), out.height()), (20, 40));
    }

    #[test]
    fn orientation_one_is_a_no_op() {
        let img = sample(40, 20);
        let out = apply_orientation(img.clone(), 1);
        assert_eq!((out.width(), out.height()), (40, 20));
    }

    #[test]
    fn missing_exif_defaults_to_upright() {
        // A bare PNG carries no EXIF block.
        let mut buf = Cursor::new(Vec::new());
        sample(10, 10).write_to(&mut buf, ImageFormat::Png).unwrap();
        assert_eq!(read_orientation(&buf.into_inner()), 1);
    }

    #[test]
    fn preprocess_bounds_the_longest_edge() {
        let mut buf = Cursor::new(Vec::new());
        sample(3000, 2000)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        let out = preprocess(&buf.into_inner(), &PreprocessOptions::default()).unwrap();

        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.width().max(decoded.height()), MAX_EDGE);
        assert_eq!(image::guess_format(&out).unwrap(), ImageFormat::Jpeg);
    }

    #[test]
    fn rejects_input_that_is_not_an_image() {
        assert!(!is_supported_image(b"this is not an image"));
        assert_eq!(sniff(b"this is not an image"), SourceKind::Unrecognised);
    }

    /// Minimal ISO-BMFF header: a `ftyp` box carrying the given brand. Enough
    /// to exercise format detection without a real 3MB photo in the repo.
    fn ftyp_header(brand: &[u8; 4]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x18];
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(brand);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(brand);
        v
    }

    #[test]
    fn recognises_heic_from_its_magic_bytes() {
        // The brands an iPhone actually writes.
        for brand in [b"heic", b"heix", b"mif1"] {
            assert_eq!(
                sniff(&ftyp_header(brand)),
                SourceKind::Heic,
                "brand {} should be detected as HEIC",
                String::from_utf8_lossy(brand)
            );
        }
        assert!(is_supported_image(&ftyp_header(b"heic")));
    }

    #[test]
    fn does_not_mistake_avif_for_heic() {
        // AVIF shares the container but the `image` crate decodes it, so it
        // must not be routed to the HEVC decoder.
        assert_ne!(sniff(&ftyp_header(b"avif")), SourceKind::Heic);
    }

    #[test]
    fn detection_ignores_the_file_extension() {
        // A HEIC named .jpg is common when photos get copied around; sniffing
        // the bytes is what makes that survivable.
        let mut buf = Cursor::new(Vec::new());
        sample(20, 20).write_to(&mut buf, ImageFormat::Png).unwrap();
        assert_eq!(
            sniff(&buf.into_inner()),
            SourceKind::Standard(ImageFormat::Png)
        );
    }

    #[test]
    fn a_jpeg_import_is_kept_byte_for_byte() {
        let mut buf = Cursor::new(Vec::new());
        sample(400, 300).write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let original = buf.into_inner();

        let out = prepare_import(&original, &PreprocessOptions::default()).unwrap();

        // Already viewable, so nothing is re-encoded and no quality is lost.
        assert_eq!(out.archival, original);
        assert_eq!(out.archival_ext, "jpg");
        assert!(out.converted_from.is_none());
    }

    #[test]
    fn a_tiff_import_is_transcoded_to_viewable_jpeg() {
        // TIFF decodes fine but a WebView cannot display it, so importing one
        // without conversion would leave the reader unable to see their page.
        let mut buf = Cursor::new(Vec::new());
        sample(400, 300).write_to(&mut buf, ImageFormat::Tiff).unwrap();

        let out = prepare_import(&buf.into_inner(), &PreprocessOptions::default()).unwrap();

        assert_eq!(out.archival_ext, "jpg");
        assert_eq!(out.converted_from, Some("TIFF"));
        assert_eq!(image::guess_format(&out.archival).unwrap(), ImageFormat::Jpeg);
        assert_eq!(image::guess_format(&out.processed).unwrap(), ImageFormat::Jpeg);
    }

    #[test]
    fn import_produces_a_bounded_copy_for_the_model() {
        let mut buf = Cursor::new(Vec::new());
        sample(3200, 2400).write_to(&mut buf, ImageFormat::Png).unwrap();

        let out = prepare_import(&buf.into_inner(), &PreprocessOptions::default()).unwrap();

        // Archival keeps full resolution; the model copy is bounded.
        let archival = image::load_from_memory(&out.archival).unwrap();
        assert_eq!(archival.width(), 3200);

        let processed = image::load_from_memory(&out.processed).unwrap();
        assert_eq!(processed.width().max(processed.height()), MAX_EDGE);
    }

    #[test]
    fn transcoding_flattens_alpha_rather_than_failing() {
        // JPEG has no alpha channel, so an RGBA PNG must not blow up the
        // encoder on its way to becoming the model's copy.
        let rgba = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(50, 50, |x, _| {
            image::Rgba([255, 0, 0, (x * 5) as u8])
        }));
        let jpeg = encode_jpeg(&rgba, ARCHIVAL_QUALITY).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), ImageFormat::Jpeg);
    }

    /// A bright page with an optional dark vertical band standing in for the
    /// spine of an open book.
    fn page_image(width: u32, height: u32, gutter: Option<u32>) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, _| {
            match gutter {
                // Spine shadow, ~2% of the width.
                Some(g) if x.abs_diff(g) < width / 50 => Rgb([40, 38, 35]),
                _ => Rgb([245, 242, 235]),
            }
        }))
    }

    /// A spine as dark as a real one, rather than as dark as convenient.
    ///
    /// Measured from a photograph of an open book in ordinary room light: the
    /// gutter came out at 0.751 of the page's brightness. A threshold of 0.72
    /// missed it and imported two pages of text as one, so this pins the real
    /// figure.
    #[test]
    fn finds_a_gutter_only_as_dark_as_a_real_one() {
        let page = 200u8;
        let gutter = (page as f32 * 0.751) as u8;
        let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(1600, 1200, |x, _| {
            if x.abs_diff(800) < 16 {
                Rgb([gutter, gutter, gutter])
            } else {
                Rgb([page, page, page])
            }
        }));

        assert!(
            detect_gutter(&img).is_some(),
            "a spine at 0.751 of page brightness is a real spread, and was missed once"
        );
    }

    #[test]
    fn finds_the_spine_of_an_open_book() {
        let img = page_image(1600, 1100, Some(800));
        let found = detect_gutter(&img).expect("spread should be detected");
        // Within a few percent of where the shadow actually is.
        assert!(found.abs_diff(800) < 40, "found gutter at {found}");
    }

    /// The failure that must never happen: splitting a single page in half
    /// would cut every line of text down the middle.
    #[test]
    fn does_not_split_a_single_page() {
        // Portrait, as a single page is normally shot.
        assert_eq!(detect_gutter(&page_image(1100, 1600, None)), None);
        // Landscape but with no spine shadow.
        assert_eq!(detect_gutter(&page_image(1600, 1100, None)), None);
    }

    #[test]
    fn does_not_split_a_portrait_photo_even_with_a_dark_band() {
        // A shadow down a single page is not a spine. Aspect ratio settles it.
        assert_eq!(detect_gutter(&page_image(1100, 1600, Some(550))), None);
    }

    #[test]
    fn splitting_yields_two_halves_that_omit_the_gutter() {
        let img = page_image(1600, 1100, Some(800));
        let (left, right) = split_at_gutter(&img, 800);

        assert!(left.width() < 800);
        assert!(right.width() < 800);
        // Together they account for nearly the whole width, less the trim.
        let total = left.width() + right.width();
        assert!(total > 1500 && total <= 1600, "total width {total}");
        assert_eq!(left.height(), 1100);
    }

    #[test]
    fn a_spread_import_produces_two_pages() {
        let mut buf = Cursor::new(Vec::new());
        page_image(1600, 1100, Some(790))
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();

        let pages = prepare_import_pages(&buf.into_inner(), &PreprocessOptions::default()).unwrap();

        assert_eq!(pages.len(), 2, "a spread should become two pages");
        assert_eq!(pages[0].side, Some(Side::Left));
        assert_eq!(pages[1].side, Some(Side::Right));
        // Each half must be independently readable.
        for p in &pages {
            assert_eq!(image::guess_format(&p.processed).unwrap(), ImageFormat::Jpeg);
        }
    }

    #[test]
    fn a_single_page_import_produces_one_page() {
        let mut buf = Cursor::new(Vec::new());
        page_image(1100, 1600, None)
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();

        let pages = prepare_import_pages(&buf.into_inner(), &PreprocessOptions::default()).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].side, None);
    }

    #[test]
    fn a_truncated_heic_reports_a_readable_error() {
        // Correct magic bytes, no actual image behind them.
        let err = decode_any(&ftyp_header(b"heic")).unwrap_err();
        assert!(
            err.to_string().contains("HEIC"),
            "error should name the format: {err}"
        );
    }
}
