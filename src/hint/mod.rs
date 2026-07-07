//! Hint mode: detect likely click targets on screen, label them with
//! prefix-free key sequences, and move the cursor to a chosen target.
//!
//! Detection is pluggable behind the [`HintDetector`] trait, with two sources:
//! the AT-SPI accessibility tree ([`atspi`], Linux-only) and a pure-CV
//! screenshot scan ([`cv`]); the default `auto` detector cascades from the
//! first to the second. Everything downstream of a detector (remapping,
//! scoring, deduplication, label assignment) is shared and lives here.

#[cfg(target_os = "linux")]
mod atspi;
#[cfg(target_os = "macos")]
mod ax;
#[cfg(target_os = "linux")]
mod compositor;
mod cv;
mod detect;

pub use detect::{select_detector, HintCandidate, HintDetector};

use crate::{backend::Backend, config::config};

#[derive(Clone, Debug)]
pub struct HintElement {
    pub cx: u32,
    pub cy: u32,
    pub bbox: (u32, u32, u32, u32),
    pub label: String,
}

/// Prefix test over `label`'s chars without allocating a `Vec<char>` (run for
/// every element on every keystroke).
pub fn label_starts_with(label: &str, prefix: &[char]) -> bool {
    let mut chars = label.chars();
    prefix.iter().all(|&c| chars.next() == Some(c))
}

/// Build hint mode using the detector selected in config (`hint.detector`).
pub fn build_hint_mode<B: Backend>(
    backend: &mut B,
    screen_w: u32,
    screen_h: u32,
) -> anyhow::Result<Vec<HintElement>> {
    let detector = select_detector()?;
    build_hint_mode_with_detector(backend, screen_w, screen_h, detector.as_ref())
}

pub fn build_hint_mode_with_detector<B: Backend>(
    backend: &mut B,
    screen_w: u32,
    screen_h: u32,
    detector: &dyn HintDetector,
) -> anyhow::Result<Vec<HintElement>> {
    use anyhow::Context;
    let output = detector
        .detect(backend)
        .with_context(|| format!("hint {} detection failed", detector.name()))?;
    let candidates = remap_to_screen(
        output.candidates,
        output.capture_w,
        output.capture_h,
        screen_w,
        screen_h,
    );
    Ok(assign_hint_elements(candidates, screen_w, screen_h))
}

/// Rescale candidate boxes from capture pixels into the overlay's logical
/// space: on HiDPI/fractional-scaled Wayland outputs the screencopy buffer is
/// physical pixels while the pointer and overlay are logical. A no-op when
/// the sizes already match.
fn remap_to_screen(
    mut candidates: Vec<HintCandidate>,
    capture_w: u32,
    capture_h: u32,
    screen_w: u32,
    screen_h: u32,
) -> Vec<HintCandidate> {
    if capture_w == 0
        || capture_h == 0
        || screen_w == 0
        || screen_h == 0
        || (capture_w == screen_w && capture_h == screen_h)
    {
        return candidates;
    }
    let sx = screen_w as f32 / capture_w as f32;
    let sy = screen_h as f32 / capture_h as f32;
    for candidate in &mut candidates {
        let (x, y, w, h) = candidate.bbox;
        let nx = ((x as f32 * sx).round() as u32).min(screen_w - 1);
        let ny = ((y as f32 * sy).round() as u32).min(screen_h - 1);
        let nw = ((w as f32 * sx).round() as u32).max(1).min(screen_w - nx);
        let nh = ((h as f32 * sy).round() as u32).max(1).min(screen_h - ny);
        candidate.bbox = (nx, ny, nw, nh);
    }
    candidates
}

fn assign_hint_elements(
    mut candidates: Vec<HintCandidate>,
    screen_w: u32,
    screen_h: u32,
) -> Vec<HintElement> {
    let cfg = config();
    let mut alphabet = cfg.hint_alphabet().to_vec();
    // A repeated alphabet key would generate two identical labels and make
    // one element permanently unreachable.
    let mut seen = std::collections::HashSet::new();
    alphabet.retain(|ch| seen.insert(*ch));
    if alphabet.is_empty() {
        return Vec::new();
    }

    for candidate in &mut candidates {
        let (x, y, w, h) = candidate.bbox;
        candidate.score =
            W_DETECTOR * candidate.score + importance_for(x, y, w, h, screen_w, screen_h);
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Bounds the O(n^2) dedup on pathologically cluttered screens.
    const MAX_DEDUP_CANDIDATES: usize = 1024;
    candidates.truncate(MAX_DEDUP_CANDIDATES);
    candidates = dedup_candidates(candidates);

    let max = alphabet
        .len()
        .saturating_pow(cfg.hint.max_label_len.max(1) as u32)
        .max(1);
    candidates.truncate(max);

    let labels = assign_labels(candidates.len(), &alphabet, cfg.hint.max_label_len.max(1));
    candidates
        .into_iter()
        .zip(labels)
        .map(|(candidate, label)| {
            let (x, y, w, h) = candidate.bbox;
            HintElement {
                cx: x + w / 2,
                cy: y + h / 2,
                bbox: candidate.bbox,
                label,
            }
        })
        .collect()
}

/// Ranking weights: detector quality, then a preference for mid-sized
/// targets, then a mild center bias. They only decide which targets get the
/// shortest labels, so they are implementation detail, not configuration.
const W_DETECTOR: f32 = 0.6;
const W_SIZE: f32 = 0.5;
const W_CENTER: f32 = 0.2;

/// Resolution-independent ranking bonus for mid-sized, roughly centered targets.
fn importance_for(x: u32, y: u32, w: u32, h: u32, screen_w: u32, screen_h: u32) -> f32 {
    let area = (w as f32) * (h as f32);
    let screen_area = (screen_w as f32 * screen_h as f32).max(1.0);
    let area_ratio = area / screen_area;
    let size_pref = if area_ratio < 0.00005 {
        0.0
    } else if area_ratio > 0.05 {
        0.2
    } else {
        1.0 - (area_ratio - 0.004).abs().min(0.004) / 0.004
    };

    let cx = x as f32 + w as f32 / 2.0;
    let cy = y as f32 + h as f32 / 2.0;
    let dx = (cx / screen_w.max(1) as f32 - 0.5).abs();
    let dy = (cy / screen_h.max(1) as f32 - 0.5).abs();
    let center_bias = 1.0 - (dx * dx + dy * dy).sqrt().min(1.0);

    W_SIZE * size_pref + W_CENTER * center_bias
}

/// Greedy non-maximum suppression over score-sorted candidates. Conservative
/// on purpose: distinct controls can legitimately overlap heavily (tab +
/// close button, row + child action), so only collapse boxes that are
/// effectively the same target.
fn dedup_candidates(candidates: Vec<HintCandidate>) -> Vec<HintCandidate> {
    let mut kept: Vec<HintCandidate> = Vec::new();
    'outer: for candidate in candidates {
        for existing in &kept {
            if is_duplicate(candidate.bbox, existing.bbox) {
                continue 'outer;
            }
        }
        kept.push(candidate);
    }
    kept
}

fn is_duplicate(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
    let overlap = iou(a, b);
    if overlap > 0.9 {
        return true;
    }
    if overlap <= 0.7 {
        return false;
    }

    // High-but-not-near-total overlap also needs matching centers and sizes,
    // so adjacent/nested controls survive.
    let acx = a.0 as i64 + a.2 as i64 / 2;
    let acy = a.1 as i64 + a.3 as i64 / 2;
    let bcx = b.0 as i64 + b.2 as i64 / 2;
    let bcy = b.1 as i64 + b.3 as i64 / 2;
    let center_close = (acx - bcx).abs() <= 4 && (acy - bcy).abs() <= 4;
    let width_ratio = a.2.min(b.2) as f32 / a.2.max(b.2).max(1) as f32;
    let height_ratio = a.3.min(b.3) as f32 / a.3.max(b.3).max(1) as f32;
    center_close && width_ratio >= 0.8 && height_ratio >= 0.8
}

fn iou(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> f32 {
    let ax2 = a.0 + a.2;
    let ay2 = a.1 + a.3;
    let bx2 = b.0 + b.2;
    let by2 = b.1 + b.3;
    let ix = ax2.min(bx2).saturating_sub(a.0.max(b.0));
    let iy = ay2.min(by2).saturating_sub(a.1.max(b.1));
    let intersection = ix * iy;
    if intersection == 0 {
        return 0.0;
    }
    let union = a.2 * a.3 + b.2 * b.3 - intersection;
    intersection as f32 / union.max(1) as f32
}

pub fn assign_labels(n: usize, alphabet: &[char], max_len: usize) -> Vec<String> {
    if n == 0 || alphabet.is_empty() || max_len == 0 {
        return Vec::new();
    }
    let k = alphabet.len();
    if k == 1 {
        return labels_of_len(alphabet, 1).take(n.min(1)).collect();
    }
    let mut len = 1usize;
    while len < max_len && k.saturating_pow(len as u32) < n {
        len += 1;
    }
    if len == 1 {
        return labels_of_len(alphabet, 1).take(n.min(k)).collect();
    }

    let capacity = k.saturating_pow(len as u32);
    let short_count =
        ((capacity.saturating_sub(n)) / (k - 1)).min(k.saturating_pow((len - 1) as u32));
    // Short labels are reserved prefixes; a full-length label is kept only if it
    // starts with none of them.
    let short: Vec<String> = labels_of_len(alphabet, len - 1).take(short_count).collect();
    let mut long: Vec<String> = Vec::new();
    for label in labels_of_len(alphabet, len) {
        if short.iter().any(|prefix| label.starts_with(prefix)) {
            continue;
        }
        long.push(label);
        if short.len() + long.len() == n {
            break;
        }
    }
    short.into_iter().chain(long).collect()
}

fn labels_of_len(alphabet: &[char], len: usize) -> impl Iterator<Item = String> + '_ {
    let total = alphabet.len().saturating_pow(len as u32);
    (0..total).map(move |mut idx| {
        let mut chars = vec![alphabet[0]; len];
        for pos in (0..len).rev() {
            chars[pos] = alphabet[idx % alphabet.len()];
            idx /= alphabet.len();
        }
        chars.into_iter().collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_prefix_free(labels: &[String]) -> bool {
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j && b.starts_with(a.as_str()) {
                    return false;
                }
            }
        }
        true
    }

    fn assert_label_invariants(n: usize, alphabet: &[char], max_len: usize) {
        let labels = assign_labels(n, alphabet, max_len);
        assert_eq!(labels.len(), n, "label count for n={n}");

        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "labels not unique for n={n}");

        assert!(is_prefix_free(&labels), "labels not prefix-free for n={n}");

        assert!(
            labels.iter().all(|l| l.chars().count() <= max_len),
            "label exceeds max_len for n={n}"
        );

        // Shorter labels (more important) must precede longer ones.
        let lengths: Vec<usize> = labels.iter().map(|l| l.chars().count()).collect();
        assert!(
            lengths.windows(2).all(|w| w[0] <= w[1]),
            "labels not length-ordered for n={n}: {lengths:?}"
        );
    }

    #[test]
    fn labels_are_well_formed_across_counts() {
        let alphabet: Vec<char> = "asdfjkl".chars().collect();
        for n in 0..=alphabet.len().pow(3) {
            assert_label_invariants(n, &alphabet, 3);
        }
    }

    #[test]
    fn single_key_alphabet_yields_one_label() {
        let alphabet = ['a'];
        assert_eq!(assign_labels(5, &alphabet, 3), vec!["a".to_string()]);
    }

    #[test]
    fn empty_inputs_yield_no_labels() {
        assert!(assign_labels(0, &['a', 'b'], 3).is_empty());
        assert!(assign_labels(5, &[], 3).is_empty());
        assert!(assign_labels(5, &['a', 'b'], 0).is_empty());
    }

    #[test]
    fn iou_matches_known_cases() {
        // Identical boxes overlap fully.
        assert!((iou((0, 0, 10, 10), (0, 0, 10, 10)) - 1.0).abs() < 1e-6);
        // Disjoint boxes do not overlap.
        assert_eq!(iou((0, 0, 10, 10), (20, 20, 10, 10)), 0.0);
        // Half-overlapping boxes: intersection 50, union 150.
        assert!((iou((0, 0, 10, 10), (5, 0, 10, 10)) - (50.0 / 150.0)).abs() < 1e-6);
    }

    #[test]
    fn dedup_drops_near_duplicate_boxes() {
        // Two nearly identical boxes (IoU > 0.6) collapse to the higher-scored
        // one; a disjoint box survives.
        let candidates = vec![
            HintCandidate {
                bbox: (0, 0, 40, 40),
                score: 1.0,
            },
            HintCandidate {
                bbox: (2, 2, 40, 40),
                score: 0.5,
            },
            HintCandidate {
                bbox: (200, 200, 30, 30),
                score: 0.4,
            },
        ];
        let kept = dedup_candidates(candidates);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].bbox, (0, 0, 40, 40));
        assert_eq!(kept[1].bbox, (200, 200, 30, 30));
    }

    #[test]
    fn dedup_keeps_overlapping_distinct_targets() {
        let candidates = vec![
            HintCandidate {
                bbox: (0, 0, 100, 30),
                score: 1.0,
            },
            HintCandidate {
                bbox: (18, 0, 100, 30),
                score: 0.9,
            },
            HintCandidate {
                bbox: (70, 4, 20, 20),
                score: 0.8,
            },
        ];
        let kept = dedup_candidates(candidates);
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn dedup_drops_same_center_same_size_boxes() {
        let candidates = vec![
            HintCandidate {
                bbox: (10, 10, 40, 20),
                score: 1.0,
            },
            HintCandidate {
                bbox: (11, 10, 40, 20),
                score: 0.9,
            },
        ];
        let kept = dedup_candidates(candidates);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].bbox, (10, 10, 40, 20));
    }

    #[test]
    fn remap_scales_into_logical_space() {
        let candidates = vec![HintCandidate {
            bbox: (100, 200, 40, 20),
            score: 0.5,
        }];
        // Capture is 2x the logical screen (HiDPI).
        let out = remap_to_screen(candidates, 3840, 2160, 1920, 1080);
        assert_eq!(out[0].bbox, (50, 100, 20, 10));
    }

    #[test]
    fn remap_is_noop_when_sizes_match() {
        let candidates = vec![HintCandidate {
            bbox: (10, 20, 30, 40),
            score: 0.5,
        }];
        let out = remap_to_screen(candidates, 1920, 1080, 1920, 1080);
        assert_eq!(out[0].bbox, (10, 20, 30, 40));
    }
}
