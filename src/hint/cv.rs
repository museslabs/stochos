//! Pure computer-vision detector (always-available fallback; `image` +
//! `imageproc` only). Pipeline: downscale → grayscale → Canny → morphological
//! close → connected components → shape filtering/scoring → drop icon-in-hit-area
//! nesting. Sees visual structure, not semantic clickability.

use anyhow::{bail, Context};
use image::{GrayImage, Luma};
use imageproc::{
    distance_transform::Norm,
    edges::canny,
    morphology::close,
    region_labelling::{connected_components, Connectivity},
};
use std::collections::HashMap;

use super::detect::{DetectorOutput, HintCandidate, HintDetector};
use crate::{backend::Backend, config::config};

#[derive(Default)]
pub struct CvHintDetector;

impl HintDetector for CvHintDetector {
    fn name(&self) -> &'static str {
        "cv"
    }

    fn detect(&self, backend: &mut dyn Backend) -> anyhow::Result<DetectorOutput> {
        let capture = backend.capture_screen().context(
            "hint mode requires screen capture; the CV detector cannot run on this backend",
        )?;
        let candidates = detect_candidates_cv(&capture.bgra, capture.w, capture.h)?;
        Ok(DetectorOutput {
            candidates,
            capture_w: capture.w,
            capture_h: capture.h,
            focus_rect: None,
        })
    }
}

/// A component in the downscaled working image: its box plus how many mask
/// pixels it actually occupies (used for the solidity score).
struct Component {
    bbox: (u32, u32, u32, u32),
    pixels: u32,
}

fn detect_candidates_cv(
    bgra: &[u8],
    capture_w: u32,
    capture_h: u32,
) -> anyhow::Result<Vec<HintCandidate>> {
    if capture_w == 0 || capture_h == 0 {
        bail!("cannot run hint CV detector on an empty capture");
    }
    if bgra.len() < (capture_w as usize * capture_h as usize * 4) {
        bail!("screen capture buffer is smaller than declared dimensions");
    }

    let cfg = &config().hint;
    let longest = capture_w.max(capture_h);
    let scale = if cfg.downscale_longest > 0 && longest > cfg.downscale_longest {
        cfg.downscale_longest as f32 / longest as f32
    } else {
        1.0
    };
    let ww = ((capture_w as f32 * scale).round() as u32).max(1);
    let wh = ((capture_h as f32 * scale).round() as u32).max(1);

    let gray = grayscale_resized(bgra, capture_w as usize, capture_h as usize, ww, wh);
    let edges = canny(
        &gray,
        cfg.edge_low_threshold as f32,
        cfg.edge_high_threshold as f32,
    );
    // Closing (dilate + erode) reconnects the broken strokes of one element
    // without inflating boxes or bridging neighbours.
    const CLOSE_RADIUS: u8 = 2;
    let closed = close(&edges, Norm::LInf, CLOSE_RADIUS);

    let components = connected_component_boxes(&closed);

    // Shape filters, relative to the working image so behaviour matches at
    // 1080p and 4K: minimum side (sub-glyph noise), maximum side/area
    // fractions (bars, panels, banners), maximum elongation (separators,
    // scrollbars, underlines).
    const MIN_SIZE: u32 = 8;
    const MAX_SIZE_FRAC: f32 = 0.6;
    const MAX_AREA_FRAC: f32 = 0.25;
    const MAX_ASPECT: f32 = 12.0;

    let work_area = (ww as f32) * (wh as f32);
    let max_w = (MAX_SIZE_FRAC * ww as f32).max(MIN_SIZE as f32);
    let max_h = (MAX_SIZE_FRAC * wh as f32).max(MIN_SIZE as f32);
    let max_area = (MAX_AREA_FRAC * work_area).max(1.0);

    let kept: Vec<(Component, f32)> = components
        .into_iter()
        .filter_map(|component| {
            let (_, _, w, h) = component.bbox;
            if w < MIN_SIZE || h < MIN_SIZE {
                return None;
            }
            if (w as f32) > max_w || (h as f32) > max_h {
                return None;
            }
            let area = (w as f32) * (h as f32);
            if area > max_area {
                return None;
            }
            let aspect = (w.max(h) as f32) / (w.min(h).max(1) as f32);
            if aspect > MAX_ASPECT {
                return None;
            }
            // Coherent controls fill their box; gradient/text speckle scores low.
            let solidity = (component.pixels as f32 / area).clamp(0.0, 1.0);
            Some((component, solidity))
        })
        .collect();

    let kept = drop_icon_inner_boxes(kept);

    let inv_scale = 1.0 / scale;
    Ok(kept
        .into_iter()
        .map(|(component, solidity)| {
            let bbox = scale_box_to_capture(component.bbox, inv_scale, capture_w, capture_h);
            HintCandidate {
                bbox,
                score: solidity,
            }
        })
        .collect())
}

/// Map a working-image box back to clamped capture-pixel coordinates.
fn scale_box_to_capture(
    (x, y, w, h): (u32, u32, u32, u32),
    inv_scale: f32,
    capture_w: u32,
    capture_h: u32,
) -> (u32, u32, u32, u32) {
    let nx = ((x as f32 * inv_scale).round() as u32).min(capture_w.saturating_sub(1));
    let ny = ((y as f32 * inv_scale).round() as u32).min(capture_h.saturating_sub(1));
    let nw = ((w as f32 * inv_scale).round() as u32)
        .max(1)
        .min(capture_w - nx);
    let nh = ((h as f32 * inv_scale).round() as u32)
        .max(1)
        .min(capture_h - ny);
    (nx, ny, nw, nh)
}

/// Drop a box sitting concentrically inside a small, near-square box — an
/// icon glyph inside its own hit area, where both labels would point at the
/// same place. Deliberately narrow so container children are preserved.
/// Thresholds are in working-image pixels.
fn drop_icon_inner_boxes(mut items: Vec<(Component, f32)>) -> Vec<(Component, f32)> {
    const MAX_PARENT_SIDE: u32 = 48;
    const SQUARE_TOLERANCE: u32 = 6;
    const CONCENTRIC_TOLERANCE: u32 = 6;

    let boxes: Vec<(u32, u32, u32, u32)> = items.iter().map(|(c, _)| c.bbox).collect();
    let mut drop = vec![false; boxes.len()];

    for (i, &child) in boxes.iter().enumerate() {
        let smallest_parent = boxes
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i && !drop[*j])
            .filter(|(_, parent)| contains(**parent, child))
            .min_by_key(|(_, parent)| parent.2 * parent.3)
            .map(|(_, parent)| *parent);

        let Some(parent) = smallest_parent else {
            continue;
        };
        let near_square = parent.2.abs_diff(parent.3) <= SQUARE_TOLERANCE;
        let small = parent.2 <= MAX_PARENT_SIDE && parent.3 <= MAX_PARENT_SIDE;
        let child_center = (child.0 + child.2 / 2, child.1 + child.3 / 2);
        let parent_center = (parent.0 + parent.2 / 2, parent.1 + parent.3 / 2);
        let concentric = child_center.0.abs_diff(parent_center.0) <= CONCENTRIC_TOLERANCE
            && child_center.1.abs_diff(parent_center.1) <= CONCENTRIC_TOLERANCE;
        if small && near_square && concentric {
            drop[i] = true;
        }
    }

    let mut keep = drop.into_iter();
    items.retain(|_| !keep.next().unwrap_or(false));
    items
}

fn contains(parent: (u32, u32, u32, u32), child: (u32, u32, u32, u32)) -> bool {
    parent.0 <= child.0
        && parent.1 <= child.1
        && parent.0 + parent.2 >= child.0 + child.2
        && parent.1 + parent.3 >= child.1 + child.3
}

/// Nearest-neighbour downscale of a BGRA capture straight into grayscale —
/// edges survive it, and it is far cheaper than a filtered resize.
fn grayscale_resized(bgra: &[u8], src_w: usize, src_h: usize, dst_w: u32, dst_h: u32) -> GrayImage {
    let dst_w_usize = dst_w as usize;
    let dst_h_usize = dst_h as usize;
    let mut out = vec![0u8; dst_w_usize * dst_h_usize];
    for y in 0..dst_h_usize {
        let src_y = (y * src_h / dst_h_usize).min(src_h - 1);
        for x in 0..dst_w_usize {
            let src_x = (x * src_w / dst_w_usize).min(src_w - 1);
            let off = (src_y * src_w + src_x) * 4;
            let b = bgra[off] as u32;
            let g = bgra[off + 1] as u32;
            let r = bgra[off + 2] as u32;
            out[y * dst_w_usize + x] = ((29 * b + 150 * g + 77 * r) / 256) as u8;
        }
    }
    GrayImage::from_vec(dst_w, dst_h, out).expect("grayscale buffer size matches image dimensions")
}

/// Connected components of a binary mask, with occupied-pixel counts for the
/// solidity score.
fn connected_component_boxes(mask: &GrayImage) -> Vec<Component> {
    let labels = connected_components(mask, Connectivity::Eight, Luma([0u8]));
    // (min_x, min_y, max_x, max_y, pixel_count)
    let mut rects: HashMap<u32, (u32, u32, u32, u32, u32)> = HashMap::new();

    for (x, y, label) in labels.enumerate_pixels() {
        let label = label[0];
        if label == 0 {
            continue;
        }
        rects
            .entry(label)
            .and_modify(|rect| {
                rect.0 = rect.0.min(x);
                rect.1 = rect.1.min(y);
                rect.2 = rect.2.max(x);
                rect.3 = rect.3.max(y);
                rect.4 += 1;
            })
            .or_insert((x, y, x, y, 1));
    }

    rects
        .into_values()
        .map(|(min_x, min_y, max_x, max_y, pixels)| Component {
            bbox: (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1),
            pixels,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solidity_is_pixel_fill_ratio() {
        // A hollow 4x4 ring: 12 of 16 box pixels filled, so solidity = 12/16.
        let mut img = GrayImage::new(4, 4);
        for (x, y, p) in img.enumerate_pixels_mut() {
            if x == 0 || x == 3 || y == 0 || y == 3 {
                *p = Luma([255]);
            }
        }
        let comps = connected_component_boxes(&img);
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].pixels, 12);
        assert_eq!(comps[0].bbox, (0, 0, 4, 4));
        let (_, _, w, h) = comps[0].bbox;
        let solidity = comps[0].pixels as f32 / (w * h) as f32;
        assert!((solidity - 12.0 / 16.0).abs() < 1e-6);
    }

    #[test]
    fn icon_inner_box_is_dropped_but_container_children_survive() {
        let comp = |bbox: (u32, u32, u32, u32)| {
            (
                Component {
                    bbox,
                    pixels: bbox.2 * bbox.3,
                },
                1.0,
            )
        };
        // Small near-square icon (outer ring) with a concentric inner glyph.
        let icon_outer = comp((100, 100, 24, 24));
        let icon_inner = comp((106, 106, 12, 12));
        // A large container with two real buttons inside it.
        let container = comp((0, 0, 400, 120));
        let button_a = comp((10, 20, 80, 30));
        let button_b = comp((120, 20, 80, 30));

        let kept =
            drop_icon_inner_boxes(vec![icon_outer, icon_inner, container, button_a, button_b]);
        let boxes: Vec<_> = kept.iter().map(|(c, _)| c.bbox).collect();

        // The concentric inner glyph is gone.
        assert!(!boxes.contains(&(106, 106, 12, 12)));
        // Everything else — including the container's children — is preserved.
        assert!(boxes.contains(&(100, 100, 24, 24)));
        assert!(boxes.contains(&(0, 0, 400, 120)));
        assert!(boxes.contains(&(10, 20, 80, 30)));
        assert!(boxes.contains(&(120, 20, 80, 30)));
    }
}

/// Dev tool, not a test of invariants: run the CV detector over a real
/// screenshot and print every candidate box, to study behaviour on actual UIs:
///   grim -t ppm /tmp/shot.ppm
///   STOCHOS_CV_IMAGE=/tmp/shot.ppm cargo test --release -- --ignored --nocapture cv_boxes
#[cfg(test)]
mod probe {
    use super::*;

    /// Minimal binary PPM (P6, maxval 255) reader → BGRA buffer.
    fn read_ppm(path: &str) -> (Vec<u8>, u32, u32) {
        let data = std::fs::read(path).expect("read image file");
        let mut fields = Vec::new();
        let mut pos = 0;
        while fields.len() < 4 {
            // Skip whitespace and comments between header fields.
            while pos < data.len() && (data[pos].is_ascii_whitespace() || data[pos] == b'#') {
                if data[pos] == b'#' {
                    while pos < data.len() && data[pos] != b'\n' {
                        pos += 1;
                    }
                }
                pos += 1;
            }
            let start = pos;
            while pos < data.len() && !data[pos].is_ascii_whitespace() {
                pos += 1;
            }
            fields.push(std::str::from_utf8(&data[start..pos]).unwrap().to_owned());
        }
        pos += 1; // single whitespace after maxval
        assert_eq!(fields[0], "P6", "expected binary PPM (grim -t ppm)");
        let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
        let rgb = &data[pos..pos + (w * h * 3) as usize];
        let mut bgra = vec![255u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            bgra[i * 4] = rgb[i * 3 + 2];
            bgra[i * 4 + 1] = rgb[i * 3 + 1];
            bgra[i * 4 + 2] = rgb[i * 3];
        }
        (bgra, w, h)
    }

    #[test]
    #[ignore = "set STOCHOS_CV_IMAGE to a P6 .ppm screenshot"]
    fn cv_boxes_from_screenshot() {
        let path = std::env::var("STOCHOS_CV_IMAGE").expect("set STOCHOS_CV_IMAGE");
        std::env::set_var(
            "XDG_CONFIG_HOME",
            std::env::temp_dir().join("stochos-cv-probe"),
        );
        crate::config::init();
        let (bgra, w, h) = read_ppm(&path);
        let start = std::time::Instant::now();
        let candidates = detect_candidates_cv(&bgra, w, h).expect("cv detection");
        eprintln!(
            "cv: {} candidates in {:?} on {w}x{h}",
            candidates.len(),
            start.elapsed()
        );
        let mut sorted = candidates;
        sorted.sort_by_key(|c| (c.bbox.1, c.bbox.0));
        for c in &sorted {
            eprintln!("  bbox={:?} score={:.2}", c.bbox, c.score);
        }
    }
}
