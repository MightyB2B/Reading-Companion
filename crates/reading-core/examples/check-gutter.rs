//! Check spread detection against a real photograph.
//!
//! Synthetic fixtures prove the arithmetic; only a real photograph of a real
//! book proves the thresholds. Uneven lighting, a hand at the edge of frame,
//! and a book that will not lie flat are all things a generated test image
//! does not have.
//!
//! Usage: cargo run --example check-gutter -- <image> [<image> ...]

use reading_core::ocr::orient;
use reading_core::ocr::preprocess::{decode_any, detect_gutter, split_at_gutter};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: cargo run --example check-gutter -- <image> [...]");
        std::process::exit(2);
    }

    for path in &args {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("{path}: cannot read ({e})");
                continue;
            }
        };
        let img = match decode_any(&bytes) {
            Ok(i) => i,
            Err(e) => {
                println!("{path}: cannot decode ({e})");
                continue;
            }
        };

        let (w, h) = (img.width(), img.height());
        let aspect = w as f32 / h as f32;
        println!("{path}\n  as shot : {w}x{h}  aspect {aspect:.2}");

        // Orientation first: a sideways page has the proportions of a spread,
        // so the shape means nothing until the text is the right way up.
        let sideways = orient::text_is_sideways(&img);
        let (rs, cs, contrast) = orient::diagnostics(&img);
        let (img, turn) = orient::upright(img);
        println!(
            "  text    : {}  -> turn {}\n            swing rows {rs:.3}  cols {cs:.3}  \
             ratio {:.2}  baseline {contrast:+.2}",
            if sideways { "runs down the page" } else { "runs across" },
            turn.as_str(),
            if rs > 0.0 { cs / rs } else { f32::INFINITY },
        );

        let (w, h) = (img.width(), img.height());
        let aspect = w as f32 / h as f32;
        print!("  upright : {w}x{h}  aspect {aspect:.2}  -> ");

        if let Some((gx, darkest, page, ratio, frac)) =
            reading_core::ocr::preprocess::gutter_diagnostics(&img)
        {
            println!(
                "\n  gutter  : darkest {darkest:.0} at x={gx}, page {page:.0}, \
                 ratio {ratio:.3} (need <{:.2}), dark span {:.1}% (need <{:.0}%)",
                reading_core::ocr::preprocess::GUTTER_DARKNESS,
                frac * 100.0,
                reading_core::ocr::preprocess::GUTTER_MAX_WIDTH_FRACTION * 100.0,
            );
        }

        match detect_gutter(&img) {
            Some(x) => {
                let (l, r) = split_at_gutter(&img, x);
                let offset = (x as f32 / w as f32) * 100.0;
                println!(
                    "SPREAD, gutter at x={x} ({offset:.1}% across); \
                     left {}x{}, right {}x{}",
                    l.width(),
                    l.height(),
                    r.width(),
                    r.height()
                );
                // Written out so the halves can be fed to the OCR model, which
                // is the only way to know whether splitting recovers the page
                // numbers the whole spread lost.
                if let Ok(dir) = std::env::var("SPLIT_OUT") {
                    for (half, name) in [(l, "left"), (r, "right")] {
                        let out = format!("{dir}/{name}.jpg");
                        match half.save(&out) {
                            Ok(()) => println!("    wrote {out}"),
                            Err(e) => println!("    could not write {out}: {e}"),
                        }
                    }
                }
            }
            None => println!("single page"),
        }
    }
}
