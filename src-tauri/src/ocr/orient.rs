//! Working out which way up a photographed page is.
//!
//! EXIF orientation is not enough. It records how the phone was held, not how
//! the *book* sat in the frame, so a page photographed sideways arrives
//! "correctly oriented" and still unreadable.
//!
//! This matters beyond legibility. A landscape photograph of one page held
//! sideways has the same proportions as a photograph of an open book, so
//! spread detection cannot tell them apart by shape — and got it wrong on a
//! real import, splitting a single page straight down the middle of its text.
//! Text direction is the signal that distinguishes them, so it has to be
//! settled before anything else looks at the geometry.
//!
//! The method is a projection profile — the variance of brightness along each
//! row and each column — scored by how *periodically* it repeats. Lines of
//! type recur at a fixed pitch and nothing else in a photograph does, so
//! whichever axis carries the period is the one running across the lines.
//!
//! Two simpler measures were tried against real photographs first and both
//! failed, which is why this one is more elaborate than it looks. Counting
//! dark pixels measures the desk, not the type. Measuring how much a profile
//! varies measures the edge of the text block. Only periodicity picks out
//! text, and each rejected approach is noted where it was used.
//!
//! What this cannot do is tell which of two quarter turns is the right way
//! up — a page and the same page upside down are geometrically alike. That is
//! settled after transcription by [`crate::ocr::legible`], on the evidence
//! that an upside-down page does not read as language.

use image::DynamicImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    /// Turn a quarter clockwise.
    Cw90,
    /// Turn a quarter anticlockwise.
    Ccw90,
    /// Upside down.
    Half,
}

impl Rotation {
    pub fn apply(self, img: DynamicImage) -> DynamicImage {
        match self {
            Rotation::None => img,
            Rotation::Cw90 => img.rotate90(),
            Rotation::Ccw90 => img.rotate270(),
            Rotation::Half => img.rotate180(),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Rotation::None => "none",
            Rotation::Cw90 => "90° clockwise",
            Rotation::Ccw90 => "90° anticlockwise",
            Rotation::Half => "180°",
        }
    }
}

/// Longest edge used for analysis. The profile only needs the shape of the
/// text, and working small keeps this to a few milliseconds.
const ANALYSIS_EDGE: u32 = 1000;

/// How much harder one axis must swing before the page is called sideways.
///
/// Comfortably above 1: a page of prose is not a marginal case, and the cost
/// of a wrong call here is rotating a perfectly good page.
const AXIS_RATIO: f32 = 1.35;

/// Minimum swing on the winning axis before the ratio means anything.
///
/// A ratio on its own is easily fooled. A lighting gradient, or the single
/// dark band of a book's spine, gives one axis a tiny swing and the other
/// none at all — an enormous ratio built on no evidence. Lines of type swing
/// an order of magnitude harder than that, so requiring an absolute floor
/// separates real text from a single edge.
const MIN_PERIODICITY: f32 = 0.08;

/// Minimum mean brightness variance before the image is judged at all.
///
/// In intensity-squared units, so this is a standard deviation of about four
/// levels — far below anything with text on it, and above a blank wall.
const MIN_CONTRAST: f32 = 16.0;

/// Contrast profiles along both axes: the variance of brightness within each
/// row and each column.
///
/// Variance rather than a count of dark pixels. Thresholding for "ink" needs
/// to know what counts as dark, and in a photograph of a book on a desk the
/// darkest things in frame are the desk and the shadow under the cover, not
/// the type — so a global threshold measures the furniture and misses the
/// text. It read a real sideways page as upright for exactly that reason.
///
/// Variance has no such problem. A flat expanse of desk has none however dark
/// it is; a line of type, being ink and paper together, has a great deal.
struct Profiles {
    rows: Vec<f32>,
    cols: Vec<f32>,
}

/// Share of each dimension kept for analysis.
///
/// A photograph of a page contains a good deal that is not the page: the desk,
/// a thumb, the shadowed edge of the cover. Those swamp the measurement —
/// every row crosses the page boundary, so every row has high variance and the
/// alternation of the text is lost, while the boundary itself gives the column
/// profile a large step that has nothing to do with type. Measured on a real
/// photograph, an upright page came out looking sideways by a factor of three
/// for exactly this reason. Working from the middle keeps the analysis on
/// paper.
const ANALYSIS_CROP: f32 = 0.62;

fn profiles(img: &DynamicImage) -> Profiles {
    let (fw, fh) = (img.width(), img.height());
    let keep_w = ((fw as f32 * ANALYSIS_CROP) as u32).max(1);
    let keep_h = ((fh as f32 * ANALYSIS_CROP) as u32).max(1);
    let cropped = img.crop_imm((fw - keep_w) / 2, (fh - keep_h) / 2, keep_w, keep_h);

    let small = cropped.resize(
        ANALYSIS_EDGE,
        ANALYSIS_EDGE,
        image::imageops::FilterType::Triangle,
    );
    let luma = small.to_luma8();
    let (w, h) = (luma.width(), luma.height());
    if w == 0 || h == 0 {
        return Profiles {
            rows: Vec::new(),
            cols: Vec::new(),
        };
    }

    // Running sums and sums of squares, one pass over the image.
    let mut row_sum = vec![0f64; h as usize];
    let mut row_sq = vec![0f64; h as usize];
    let mut col_sum = vec![0f64; w as usize];
    let mut col_sq = vec![0f64; w as usize];

    for y in 0..h {
        for x in 0..w {
            let v = luma.get_pixel(x, y).0[0] as f64;
            row_sum[y as usize] += v;
            row_sq[y as usize] += v * v;
            col_sum[x as usize] += v;
            col_sq[x as usize] += v * v;
        }
    }

    let variance = |sum: f64, sq: f64, n: f64| -> f32 {
        let mean = sum / n;
        ((sq / n) - mean * mean).max(0.0) as f32
    };

    Profiles {
        rows: (0..h as usize)
            .map(|i| variance(row_sum[i], row_sq[i], w as f64))
            .collect(),
        cols: (0..w as usize)
            .map(|i| variance(col_sum[i], col_sq[i], h as f64))
            .collect(),
    }
}

/// How *regularly* a profile repeats — the strength of its strongest period.
///
/// Not how much it varies. Amount of variation was tried and does not work:
/// the edge of a text block is one enormous step in the profile running along
/// the lines, so a cleanly typeset upright page measured as sideways by a
/// factor of 1.7. Any measure of magnitude is dominated by boundaries.
///
/// Lines of type are distinguished by being *periodic*: they repeat at a fixed
/// pitch down the page, and nothing else in a photograph does. So the profile
/// is differenced first — turning each edge into a single impulse and each run
/// of text lines into a regular train of them — and then autocorrelated. A
/// lone block boundary contributes one impulse, which correlates with nothing.
fn periodicity(profile: &[f32]) -> f32 {
    let n = profile.len();
    if n < 48 {
        return 0.0;
    }

    // Differencing removes gradients and block edges alike.
    let d: Vec<f32> = profile.windows(2).map(|w| w[1] - w[0]).collect();
    let mean: f32 = d.iter().sum::<f32>() / d.len() as f32;
    let centred: Vec<f32> = d.iter().map(|v| v - mean).collect();

    let energy: f32 = centred.iter().map(|v| v * v).sum();
    if energy <= f32::EPSILON {
        return 0.0;
    }

    // Search only pitches that could be lines of text: between roughly four
    // and fifty lines across the measured span. Going finer than this picks up
    // the spacing of letters, which is periodic along a line as well as across
    // it and therefore tells us nothing about direction.
    let span = centred.len();
    let min_lag = (span / 50).max(4);
    let max_lag = (span / 4).max(min_lag + 1);

    let mut best = 0f32;
    for lag in min_lag..max_lag {
        let corr: f32 = centred[..centred.len() - lag]
            .iter()
            .zip(&centred[lag..])
            .map(|(a, b)| a * b)
            .sum();
        best = best.max(corr / energy);
    }
    best
}

/// Is there enough detail here to be a page of text?
fn has_text(p: &Profiles) -> bool {
    if p.rows.is_empty() || p.cols.is_empty() {
        return false;
    }
    let mean = p.rows.iter().sum::<f32>() / p.rows.len() as f32;
    mean >= MIN_CONTRAST
}

/// The raw measurements behind a decision: (row swing, column swing, mean
/// contrast). Exposed so thresholds can be calibrated against real
/// photographs rather than guessed at.
pub fn diagnostics(img: &DynamicImage) -> (f32, f32, f32) {
    let p = profiles(img);
    let contrast = if p.rows.is_empty() {
        0.0
    } else {
        p.rows.iter().sum::<f32>() / p.rows.len() as f32
    };
    (periodicity(&p.rows), periodicity(&p.cols), contrast)
}

/// Is the text running down the page rather than across it?
pub fn text_is_sideways(img: &DynamicImage) -> bool {
    let p = profiles(img);
    if !has_text(&p) {
        return false;
    }
    let across_rows = periodicity(&p.rows);
    let across_cols = periodicity(&p.cols);

    across_cols >= MIN_PERIODICITY && across_cols > across_rows * AXIS_RATIO
}

/// Work out the turn that puts a photographed page upright.
///
/// Only the *axis* is decided here, and only from geometry. Which of the two
/// quarter turns is the right way up cannot be settled this way: the obvious
/// signal — that Latin type hangs from its baseline, so ink sits high within
/// each line — was implemented and measured against real photographs, and got
/// three of six wrong, scoring a page verified to be upside down at +0.93.
/// Half the time is not a heuristic, so it is gone.
///
/// The turn below is therefore a *proposal*. It is checked after
/// transcription, where there is real evidence to check it against: an
/// upside-down page transcribes to nonsense, and the dictionary can say so.
/// See [`crate::ocr::legible`].
pub fn detect(img: &DynamicImage) -> Rotation {
    if img.width() == 0 || img.height() == 0 {
        return Rotation::None;
    }
    if text_is_sideways(img) {
        // Anticlockwise, which is correct for a book held in the left hand and
        // shot in landscape — the case that prompted all this. When it is
        // wrong the legibility check catches it.
        return Rotation::Ccw90;
    }
    Rotation::None
}

/// Detect and apply in one step.
pub fn upright(img: DynamicImage) -> (DynamicImage, Rotation) {
    let rotation = detect(&img);
    (rotation.apply(img), rotation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// A well-mixed hash, so "irregular" really is.
    ///
    /// A single multiply is not enough: consecutive inputs then differ by a
    /// constant, which after a modulo is a smooth ramp with a short period —
    /// structure the periodicity measure duly found, and reported as text.
    fn scramble(x: u32) -> u32 {
        let mut h = x.wrapping_mul(0x9E37_79B1);
        h ^= h >> 15;
        h = h.wrapping_mul(0x85EB_CA6B);
        h ^= h >> 13;
        h
    }

    /// A page of "type" on a light ground.
    ///
    /// The ink must break up *along* each line as well as between lines. Solid
    /// bars were tried and are useless here: a solid bar has no variance
    /// within it, so a column lying along one measures as flat as blank paper
    /// and the periodicity vanishes. Real type is ink and paper interleaved at
    /// letter scale, and that texture is what the measurement reads.
    fn typeset_page(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            let line_pitch = 40;
            let within = y % line_pitch;
            let margin = width / 10;
            let in_column = x > margin && x < width - margin;

            // Letters, not a picket fence. Real type is irregular along the
            // line — words and spaces of varying width — and a perfectly
            // periodic stroke pattern would give the column profile a period
            // of its own, which is precisely the confusion being guarded
            // against.
            let stroke = scramble(x) % 100 < 55;
            let in_line = within >= 6 && within < 24;

            if in_column && in_line && stroke {
                Rgb([35, 33, 30])
            } else {
                Rgb([243, 240, 233])
            }
        }))
    }

    #[test]
    fn an_upright_page_needs_no_turning() {
        assert_eq!(detect(&typeset_page(1200, 1600)), Rotation::None);
    }

    #[test]
    fn recognises_text_running_down_the_page() {
        let up = typeset_page(1200, 1600);
        let sideways = up.rotate90();

        let (ur, uc, _) = diagnostics(&up);
        let (sr, sc, _) = diagnostics(&sideways);

        assert!(
            text_is_sideways(&sideways),
            "a quarter-turned page should read as sideways \
             (rows {sr:.3}, cols {sc:.3}, ratio {:.2})",
            sc / sr.max(f32::EPSILON)
        );
        assert!(
            !text_is_sideways(&up),
            "an upright page should not \
             (rows {ur:.3}, cols {uc:.3}, ratio {:.2})",
            uc / ur.max(f32::EPSILON)
        );
    }

    #[test]
    fn turns_a_sideways_page_back_to_portrait() {
        let sideways = typeset_page(1200, 1600).rotate90();
        let (fixed, rotation) = upright(sideways);

        assert_eq!(rotation, Rotation::Ccw90);
        assert_eq!((fixed.width(), fixed.height()), (1200, 1600));
        // Whichever way it was turned, the lines now run across the page.
        assert!(!text_is_sideways(&fixed));
    }

    /// Both quarter turns are corrected to portrait. Which of the two is the
    /// right way up is not decided here — see the module docs — so the
    /// guarantee is only that the lines end up horizontal.
    #[test]
    fn a_page_turned_the_other_way_is_also_made_portrait() {
        let sideways = typeset_page(1200, 1600).rotate270();
        let (fixed, _) = upright(sideways);

        assert_eq!((fixed.width(), fixed.height()), (1200, 1600));
        assert!(!text_is_sideways(&fixed));
    }

    /// An upside-down page has horizontal lines already, so geometry has
    /// nothing to say about it. The legibility check catches this case after
    /// transcription instead.
    #[test]
    fn an_upside_down_page_is_left_to_the_legibility_check() {
        assert_eq!(detect(&typeset_page(1200, 1600).rotate180()), Rotation::None);
    }

    #[test]
    fn a_blank_page_is_left_alone() {
        let blank = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            800,
            1000,
            Rgb([245, 242, 235]),
        ));
        assert_eq!(detect(&blank), Rotation::None);
    }

    #[test]
    fn a_lighting_gradient_does_not_read_as_text() {
        // Variance would be fooled by this; the step measure should not be.
        let gradient = DynamicImage::ImageRgb8(ImageBuffer::from_fn(1000, 1000, |x, _| {
            let v = 140 + (x * 100 / 1000) as u8;
            Rgb([v, v, v])
        }));
        assert_eq!(detect(&gradient), Rotation::None);
    }
}
