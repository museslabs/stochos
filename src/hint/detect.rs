//! The detector seam. A [`HintDetector`] turns the current screen into a list
//! of candidate click targets in *capture-pixel* coordinates; the shared code
//! in [`super`] handles everything after that.

use crate::backend::Backend;
use crate::config::{config, HintDetectorKind};

#[derive(Clone, Copy, Debug)]
pub struct HintCandidate {
    /// Box in capture-pixel coordinates: `(x, y, w, h)`.
    pub bbox: (u32, u32, u32, u32),
    /// Detector-provided quality, roughly 0..1; blended with the shared
    /// size/center preferences during ranking.
    pub score: f32,
}

/// Detector output plus the pixel dimensions of the capture the boxes are
/// expressed in; the caller rescales to the overlay's logical space.
pub struct DetectorOutput {
    pub candidates: Vec<HintCandidate>,
    pub capture_w: u32,
    pub capture_h: u32,
    /// The window the candidates are scoped to (screen coordinates), when the
    /// detector knows it; lets `auto` keep CV supplements in the active window.
    pub focus_rect: Option<(u32, u32, u32, u32)>,
}

pub trait HintDetector {
    fn name(&self) -> &'static str;
    fn detect(&self, backend: &mut dyn Backend) -> anyhow::Result<DetectorOutput>;
}

/// Pick the detector named by `hint.detector`. Selecting `atspi` on a
/// platform without it is an error rather than a silent fall back to CV;
/// `auto` uses whatever the platform offers.
pub fn select_detector() -> anyhow::Result<Box<dyn HintDetector>> {
    match config().hint.detector {
        HintDetectorKind::Auto => Ok(Box::new(AutoDetector)),
        HintDetectorKind::Cv => Ok(Box::new(super::cv::CvHintDetector)),
        HintDetectorKind::Atspi => atspi_detector(),
    }
}

fn atspi_detector() -> anyhow::Result<Box<dyn HintDetector>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(super::atspi::AtspiHintDetector))
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("hint.detector = \"atspi\" is only available on Linux")
    }
}

/// Fewest AT-SPI targets the cascade accepts without falling back to CV;
/// fewer means the accessibility tree is absent or a stub.
const CASCADE_MIN_ATSPI_TARGETS: usize = 5;

/// The default detector: prefer semantic AT-SPI targets, but supplement them
/// with CV for small visual controls a browser may omit from accessibility.
struct AutoDetector;

impl HintDetector for AutoDetector {
    fn name(&self) -> &'static str {
        "auto"
    }

    fn detect(&self, backend: &mut dyn Backend) -> anyhow::Result<DetectorOutput> {
        #[cfg(target_os = "linux")]
        {
            match super::atspi::AtspiHintDetector.detect(backend) {
                Ok(output) if output.candidates.len() >= CASCADE_MIN_ATSPI_TARGETS => {
                    return Ok(supplement_with_cv(backend, output));
                }
                Ok(output) => eprintln!(
                    "hint: atspi found only {} targets, falling back to cv",
                    output.candidates.len()
                ),
                Err(e) => eprintln!("hint: atspi detection failed, falling back to cv: {e:#}"),
            }
            super::cv::CvHintDetector.detect(backend)
        }

        // On macOS the AX tree is often thin (Electron/Firefox), so when the
        // semantic pass is too sparse to use directly we still keep its focus
        // rect to scope the CV fallback to the active window rather than
        // painting hints across the whole desktop.
        #[cfg(target_os = "macos")]
        {
            let mut fallback_focus: Option<(u32, u32, u32, u32)> = None;
            match super::ax::AxHintDetector.detect(backend) {
                Ok(output) if output.candidates.len() >= CASCADE_MIN_ATSPI_TARGETS => {
                    return Ok(supplement_with_cv(backend, output));
                }
                Ok(output) => {
                    eprintln!(
                        "hint: ax found only {} targets, falling back to cv",
                        output.candidates.len()
                    );
                    fallback_focus = output.focus_rect;
                }
                Err(e) => eprintln!("hint: ax detection failed, falling back to cv: {e:#}"),
            }
            let cv_output = super::cv::CvHintDetector.detect(backend)?;
            Ok(scope_cv_output(
                cv_output,
                fallback_focus,
                backend.screen_size(),
            ))
        }
    }
}

/// Clip a pure-CV pass to the active-window rect that the semantic detector
/// did expose, even when its candidate list was too sparse to use directly.
/// Capture coordinates are physical pixels; focus rects are screen-logical, so
/// we filter in logical space after the shared remap.
#[cfg(target_os = "macos")]
fn scope_cv_output(
    output: DetectorOutput,
    focus_rect: Option<(u32, u32, u32, u32)>,
    screen: (u32, u32),
) -> DetectorOutput {
    let Some(rect) = focus_rect else {
        return output;
    };
    let (screen_w, screen_h) = screen;
    let logical = super::remap_to_screen(
        output.candidates,
        output.capture_w,
        output.capture_h,
        screen_w,
        screen_h,
    );
    let kept: Vec<HintCandidate> = logical
        .into_iter()
        .filter(|c| {
            let cx = c.bbox.0 + c.bbox.2 / 2;
            let cy = c.bbox.1 + c.bbox.3 / 2;
            cx >= rect.0 && cx < rect.0 + rect.2 && cy >= rect.1 && cy < rect.1 + rect.3
        })
        .collect();
    DetectorOutput {
        candidates: kept,
        capture_w: screen_w,
        capture_h: screen_h,
        focus_rect: Some(rect),
    }
}

/// Add visual candidates that are not already covered by a precise semantic box.
fn supplement_with_cv(backend: &mut dyn Backend, mut semantic: DetectorOutput) -> DetectorOutput {
    let Some(window) = semantic.focus_rect else {
        return semantic;
    };
    let Ok(cv) = super::cv::CvHintDetector.detect(backend) else {
        return semantic;
    };
    let cv_candidates = super::remap_to_screen(
        cv.candidates,
        cv.capture_w,
        cv.capture_h,
        semantic.capture_w,
        semantic.capture_h,
    );
    let raw_visual: Vec<HintCandidate> = cv_candidates
        .into_iter()
        .filter(|c| contains_point(window, center(c.bbox)))
        .collect();
    let semantic_boxes: Vec<(u32, u32, u32, u32)> =
        semantic.candidates.iter().map(|c| c.bbox).collect();
    let visual: Vec<HintCandidate> = raw_visual
        .iter()
        .copied()
        .filter(|c| {
            !semantic_boxes
                .iter()
                .any(|&b| precise_semantic_box_covers(b, c.bbox))
        })
        .collect();

    merge_visual_supplements(&mut semantic.candidates, visual, &raw_visual);
    semantic
}

fn merge_visual_supplements(
    semantic: &mut Vec<HintCandidate>,
    visual: Vec<HintCandidate>,
    raw_visual: &[HintCandidate],
) {
    let semantic_snapshot = semantic.clone();
    let mut remove_semantic = vec![false; semantic_snapshot.len()];
    let mut drop_visual = vec![false; visual.len()];
    let mut replacements = Vec::new();

    for (semantic_idx, semantic_candidate) in semantic_snapshot.iter().enumerate() {
        let text_visual: Vec<usize> = visual
            .iter()
            .enumerate()
            .filter(|(_, v)| broad_semantic_box_contains(semantic_candidate.bbox, v.bbox))
            .filter(|(_, v)| !is_icon_like(v.bbox))
            .map(|(idx, _)| idx)
            .collect();
        let anchor = leftmost_anchor_icon(semantic_candidate.bbox, raw_visual, &semantic_snapshot);

        if text_visual.is_empty() && anchor.is_none() {
            continue;
        }

        remove_semantic[semantic_idx] = true;
        for (idx, candidate) in semantic_snapshot.iter().enumerate() {
            if idx != semantic_idx && is_left_anchor_icon(semantic_candidate.bbox, candidate.bbox) {
                remove_semantic[idx] = true;
            }
        }
        for (idx, candidate) in visual.iter().enumerate() {
            if broad_semantic_box_contains(semantic_candidate.bbox, candidate.bbox)
                && is_icon_like(candidate.bbox)
            {
                drop_visual[idx] = true;
            }
        }

        if text_visual.is_empty() {
            if let Some(icon) = anchor {
                replacements.push(text_target_after_icon(semantic_candidate.bbox, icon));
            }
        }
    }

    let mut remove_semantic = remove_semantic.into_iter();
    semantic.retain(|_| !remove_semantic.next().unwrap_or(false));
    semantic.extend(
        visual
            .into_iter()
            .enumerate()
            .filter_map(|(idx, candidate)| (!drop_visual[idx]).then_some(candidate)),
    );
    semantic.extend(replacements);
}

fn contains_point((x, y, w, h): (u32, u32, u32, u32), (px, py): (u32, u32)) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

fn center((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32) {
    (x + w / 2, y + h / 2)
}

fn precise_semantic_box_covers(
    semantic: (u32, u32, u32, u32),
    visual: (u32, u32, u32, u32),
) -> bool {
    contains_point(semantic, center(visual)) && !is_broad_horizontal_container(semantic, visual)
}

fn broad_semantic_box_contains(
    semantic: (u32, u32, u32, u32),
    visual: (u32, u32, u32, u32),
) -> bool {
    contains_point(semantic, center(visual)) && is_broad_horizontal_container(semantic, visual)
}

fn is_broad_horizontal_container(
    semantic: (u32, u32, u32, u32),
    visual: (u32, u32, u32, u32),
) -> bool {
    let (_, _, semantic_w, semantic_h) = semantic;
    let (_, _, visual_w, visual_h) = visual;
    u64::from(semantic_w) >= u64::from(visual_w.max(1)) * 3
        && u64::from(semantic_h) <= u64::from(visual_h.max(1)) * 3
}

fn leftmost_anchor_icon(
    semantic: (u32, u32, u32, u32),
    raw_visual: &[HintCandidate],
    semantic_snapshot: &[HintCandidate],
) -> Option<(u32, u32, u32, u32)> {
    raw_visual
        .iter()
        .map(|c| c.bbox)
        .chain(semantic_snapshot.iter().map(|c| c.bbox))
        .filter(|&bbox| is_left_anchor_icon(semantic, bbox))
        .min_by_key(|&(x, _, _, _)| x)
}

fn is_left_anchor_icon(semantic: (u32, u32, u32, u32), icon: (u32, u32, u32, u32)) -> bool {
    if !broad_semantic_box_contains(semantic, icon) || !is_icon_like(icon) {
        return false;
    }
    let (x, _, w, _) = semantic;
    center(icon).0 <= x + w / 3
}

fn is_icon_like((_, _, w, h): (u32, u32, u32, u32)) -> bool {
    let short = w.min(h).max(1);
    let long = w.max(h);
    long <= 32 && long <= short * 2
}

fn text_target_after_icon(
    semantic: (u32, u32, u32, u32),
    icon: (u32, u32, u32, u32),
) -> HintCandidate {
    let (sx, sy, sw, sh) = semantic;
    let (ix, _, iw, ih) = icon;
    let right = sx + sw;
    let x = (ix + iw + 6).clamp(sx, right.saturating_sub(1));
    let w = sh.max(ih).clamp(16, 40).min(right - x);
    HintCandidate {
        bbox: (x, sy, w.max(1), sh.max(1)),
        score: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cv_supplement_never_doubles_a_semantic_target() {
        let semantic = (500u32, 500u32, 40u32, 40u32);
        assert!(contains_point(semantic, (520, 520)));
        assert!(!contains_point(semantic, (560, 520)));
    }

    #[test]
    fn cv_supplement_stays_inside_focus_window() {
        let window = (100, 100, 800, 600);
        assert!(contains_point(window, (405, 305)));
        assert!(!contains_point(window, (15, 305)));
    }

    #[test]
    fn broad_semantic_row_does_not_cover_precise_visual_target() {
        let semantic_row = (10, 100, 900, 24);
        let file_link = (20, 102, 120, 20);

        assert!(!precise_semantic_box_covers(semantic_row, file_link));
        assert!(broad_semantic_box_contains(semantic_row, file_link));
    }

    #[test]
    fn tight_semantic_box_covers_matching_visual_target() {
        let semantic_link = (20, 100, 140, 24);
        let visual_link = (25, 102, 120, 20);

        assert!(precise_semantic_box_covers(semantic_link, visual_link));
        assert!(!broad_semantic_box_contains(semantic_link, visual_link));
    }

    #[test]
    fn broad_semantic_with_text_visual_drops_icon_anchor() {
        let mut semantic = vec![
            HintCandidate {
                bbox: (10, 100, 900, 24),
                score: 1.0,
            },
            HintCandidate {
                bbox: (22, 104, 16, 16),
                score: 1.0,
            },
        ];
        let visual = vec![HintCandidate {
            bbox: (48, 105, 80, 14),
            score: 1.0,
        }];
        let raw_visual = [HintCandidate {
            bbox: (22, 104, 16, 16),
            score: 1.0,
        }];

        merge_visual_supplements(&mut semantic, visual, &raw_visual);

        let boxes: Vec<_> = semantic.iter().map(|c| c.bbox).collect();
        assert_eq!(boxes, vec![(48, 105, 80, 14)]);
    }

    #[test]
    fn broad_semantic_with_only_icon_anchor_clicks_text_side() {
        let mut semantic = vec![
            HintCandidate {
                bbox: (10, 100, 900, 24),
                score: 1.0,
            },
            HintCandidate {
                bbox: (22, 104, 16, 16),
                score: 1.0,
            },
        ];
        let raw_visual = [HintCandidate {
            bbox: (22, 104, 16, 16),
            score: 1.0,
        }];

        merge_visual_supplements(&mut semantic, Vec::new(), &raw_visual);

        let boxes: Vec<_> = semantic.iter().map(|c| c.bbox).collect();
        assert_eq!(boxes, vec![(44, 100, 24, 24)]);
    }

    #[test]
    fn icon_without_broad_parent_stays_available() {
        let mut semantic = vec![HintCandidate {
            bbox: (22, 104, 16, 16),
            score: 1.0,
        }];
        let raw_visual = [HintCandidate {
            bbox: (22, 104, 16, 16),
            score: 1.0,
        }];

        merge_visual_supplements(&mut semantic, Vec::new(), &raw_visual);

        assert_eq!(semantic[0].bbox, (22, 104, 16, 16));
    }
}
