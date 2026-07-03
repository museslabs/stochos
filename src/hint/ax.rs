//! macos accessibility detector: walks the AXUIElement tree of the frontmost
//! app and emits a [`HintCandidate`] per actionable control, mirroring
//! [`super::atspi`] on linux.
//!
//! a node is a candidate if its `AXRole` is in [`HINT_ROLES`] or it advertises
//! an action in [`ACTIONABLE_ACTIONS`]. the action path is what surfaces
//! Electron and ARIA web widgets that show up as `AXGroup`/`AXImage`.
//!
//! `AXPosition`/`AXSize` are already screen-space, so the shared remap in
//! [`super`] is a no-op.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use objc2_app_kit::NSWorkspace;
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    kCFBooleanTrue, CFArray, CFRetained, CFString, CFType, CGPoint, CGSize, Type,
};

use super::detect::{DetectorOutput, HintCandidate, HintDetector};
use crate::backend::Backend;

pub struct AxHintDetector;

impl HintDetector for AxHintDetector {
    fn name(&self) -> &'static str {
        "ax"
    }

    fn detect(&self, backend: &mut dyn Backend) -> Result<DetectorOutput> {
        let (screen_w, screen_h) = backend.screen_size();
        let (candidates, focus_rect) = collect_candidates(screen_w, screen_h)?;
        Ok(DetectorOutput {
            candidates,
            capture_w: screen_w,
            capture_h: screen_h,
            focus_rect,
        })
    }
}

/// upper bound on returned targets. generous because Electron pages routinely
/// blow past 1k actionable elements and we rank-and-cap downstream.
const MAX_TARGETS: usize = 8192;

/// wall-clock cap on the whole walk. even with `set_messaging_timeout` a deep
/// tree of slow children can drag on, so we bail and let the caller fall back
/// to cv.
const COLLECT_BUDGET: Duration = Duration::from_secs(2);

/// per-element ax message timeout. the 6 s default lets one frozen Electron
/// app ruin hint mode. 0.4 s is generous for healthy trees.
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.4;

/// cycle guard for tree walks, matching the at-spi counterpart.
const MAX_TREE_DEPTH: usize = 100;

/// roles that are intrinsically clickable. anything outside this list can
/// still be a hint via [`ACTIONABLE_ACTIONS`].
const HINT_ROLES: &[&str] = &[
    "AXButton",
    "AXLink",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXTab",
    "AXSearchField",
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXDisclosureTriangle",
    "AXIncrementor",
    "AXSlider",
    "AXSwitch",
    "AXRow",
    "AXOutlineRow",
    "AXCell",
    "AXToolbarButton",
    "AXSegmentedControl",
    "AXStepper",
];

/// actions that mean "does something when clicked". an element advertising
/// any of these is a hint regardless of role, which is what makes Electron
/// and ARIA web widgets work.
const ACTIONABLE_ACTIONS: &[&str] = &["AXPress", "AXShowMenu", "AXPick", "AXOpen"];

/// structural roles that never carry an action and are only walked for their
/// children. probing `AXActions` on each is one mach ipc per node, a
/// measurable slice of walk time on busy trees, so skip the probe.
const SKIP_ACTION_PROBE_ROLES: &[&str] = &[
    "AXWindow",
    "AXSheet",
    "AXDrawer",
    "AXSplitter",
    "AXSplitGroup",
    "AXScrollBar",
    "AXScrollArea",
    "AXLayoutArea",
    "AXLayoutItem",
    "AXToolbar",
    "AXMenuBar",
    "AXMenu",
    "AXTable",
    "AXOutline",
    "AXList",
    "AXBrowser",
    "AXMatte",
    "AXRuler",
    "AXRulerMarker",
    "AXUnknown",
];

fn collect_candidates(
    screen_w: u32,
    screen_h: u32,
) -> Result<(Vec<HintCandidate>, Option<(u32, u32, u32, u32)>)> {
    let pid = frontmost_pid()?;
    // SAFETY: pid is a valid pid_t (32-bit signed int) from NSWorkspace.
    let app = unsafe { AXUIElement::new_application(pid) };
    let _ = unsafe { app.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
    wake_renderer_accessibility(&app);

    let roots = collect_roots(&app);
    // first root's rect scopes the cv fallback. it's the focused/main window
    // when we have one, or the first window in the all-windows fallback.
    let focus_rect = roots
        .first()
        .and_then(|root| rect_of(root))
        .and_then(|r| clamp_rect(r, screen_w, screen_h));

    let mut candidates = Vec::new();
    let deadline = Instant::now() + COLLECT_BUDGET;
    for root in &roots {
        if candidates.len() >= MAX_TARGETS || Instant::now() >= deadline {
            break;
        }
        walk(root, 0, &mut candidates, screen_w, screen_h, deadline);
    }

    Ok((candidates, focus_rect))
}

fn frontmost_pid() -> Result<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace
        .frontmostApplication()
        .ok_or_else(|| anyhow!("NSWorkspace has no frontmost application"))?;
    Ok(app.processIdentifier())
}

/// roots to walk, in priority order. the focused/main window is preferred
/// because it scopes cv fallback nicely. the all-windows fallback catches
/// apps that don't expose a focused window while the cursor hovers content.
fn collect_roots(app: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let mut roots = Vec::new();
    for attr in ["AXFocusedWindow", "AXMainWindow"] {
        if let Some(value) = copy_attribute(app, &ax_str(attr)) {
            if let Ok(window) = value.downcast::<AXUIElement>() {
                roots.push(window);
                break;
            }
        }
    }
    if roots.is_empty() {
        if let Some(value) = copy_attribute(app, &ax_str("AXWindows")) {
            if let Some(arr) = cf_array(value) {
                for child in arr.iter() {
                    if let Some(window) = child.downcast_ref::<AXUIElement>() {
                        roots.push(window.retain());
                    }
                }
            }
        }
    }
    if roots.is_empty() {
        roots.push(app.retain());
    }
    roots
}

fn walk(
    element: &AXUIElement,
    depth: usize,
    out: &mut Vec<HintCandidate>,
    screen_w: u32,
    screen_h: u32,
    deadline: Instant,
) {
    if depth >= MAX_TREE_DEPTH || out.len() >= MAX_TARGETS || Instant::now() >= deadline {
        return;
    }

    // fast path is the role match. slow path is `AXActions`, one extra ipc
    // per element. it's what makes web content work, but we skip it for
    // structural roles where no real control lives.
    let role = role_of(element);
    let role_str = role.as_deref().unwrap_or("");
    let role_match = HINT_ROLES.contains(&role_str);
    let skip_action_probe = SKIP_ACTION_PROBE_ROLES.contains(&role_str);
    let is_hint = role_match || (!skip_action_probe && has_actionable_action(element));
    if debug() {
        let indent = "  ".repeat(depth.min(40));
        eprintln!(
            "{:>3} {}{}{}",
            depth,
            indent,
            role.as_deref().unwrap_or("?"),
            if is_hint { " *" } else { "" }
        );
    }
    if is_hint {
        if let Some(rect) = rect_of(element) {
            if let Some(bbox) = clamp_rect(rect, screen_w, screen_h) {
                out.push(HintCandidate { bbox, score: 1.0 });
            }
        }
    }

    let Some(children) = children_of(element) else {
        return;
    };
    for child in children.iter() {
        if out.len() >= MAX_TARGETS || Instant::now() >= deadline {
            break;
        }
        if let Some(child_el) = child.downcast_ref::<AXUIElement>() {
            walk(child_el, depth + 1, out, screen_w, screen_h, deadline);
        }
    }
}

fn debug() -> bool {
    // cache the env lookup: the walk runs this on every node and `var_os` is
    // a syscall each call.
    use std::sync::OnceLock;
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var_os("STOCHOS_AX_DEBUG").is_some())
}

/// nudge Chromium/Electron into building its renderer accessibility tree.
/// Chrome gates that tree behind an explicit consumer signal, without which
/// only the native window chrome is exposed. two attributes cover it:
///
/// - `AXManualAccessibility=true`: Chrome's own opt-in, honoured by Slack,
///   Discord, VS Code, Notion.
/// - `AXEnhancedUserInterface=true`: broader AppKit signal that VoiceOver
///   sets. some apps expose extra structure when they see it.
///
/// both calls are advisory and ignore errors. we then give the renderer a
/// brief moment to publish, paid only on the first walk.
fn wake_renderer_accessibility(app: &AXUIElement) {
    let Some(boolean_true) = (unsafe { kCFBooleanTrue.as_ref() }) else {
        return;
    };
    let truthy: &CFType = boolean_true.as_ref();
    for attr in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
        let name = ax_str(attr);
        let _ = unsafe { app.set_attribute_value(&name, truthy) };
    }
    // first call enables the tree, so wait for it to populate. skipped after
    // that: the toggle persists on the app element until quit.
    use std::sync::atomic::{AtomicBool, Ordering};
    static FIRST_WAKE: AtomicBool = AtomicBool::new(true);
    if FIRST_WAKE.swap(false, Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn role_of(element: &AXUIElement) -> Option<String> {
    let value = copy_attribute(element, &ax_str("AXRole"))?;
    Some(value.downcast_ref::<CFString>()?.to_string())
}

fn children_of(element: &AXUIElement) -> Option<CFRetained<CFArray<CFType>>> {
    let value = copy_attribute(element, &ax_str("AXChildren"))?;
    cf_array(value)
}

/// true if any action this element advertises is in [`ACTIONABLE_ACTIONS`].
fn has_actionable_action(element: &AXUIElement) -> bool {
    let mut raw: *const CFArray = std::ptr::null();
    let Some(out_ptr) = NonNull::new(&mut raw as *mut *const CFArray) else {
        return false;
    };
    let err = unsafe { element.copy_action_names(out_ptr) };
    if err != AXError::Success {
        return false;
    }
    let Some(nn) = NonNull::new(raw as *mut CFArray) else {
        return false;
    };
    let arr = unsafe { CFRetained::from_raw(nn) };
    // SAFETY: CFArray of CFString action names; both subclass CFType.
    let arr = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(arr) };
    for entry in arr.iter() {
        if let Some(name) = entry.downcast_ref::<CFString>() {
            let s = name.to_string();
            if ACTIONABLE_ACTIONS.contains(&s.as_str()) {
                return true;
            }
        }
    }
    false
}

/// downcast a `CFType` into a `CFArray<CFType>`. the default `CFArray`
/// parameterization is `Opaque`, and reinterpreting as `CFType` is a
/// zero-cost marker swap since both are zero-sized.
fn cf_array(value: CFRetained<CFType>) -> Option<CFRetained<CFArray<CFType>>> {
    let arr = value.downcast::<CFArray>().ok()?;
    Some(unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(arr) })
}

/// read AXPosition + AXSize and convert to a screen-pixel `(x, y, w, h)`.
fn rect_of(element: &AXUIElement) -> Option<(i32, i32, i32, i32)> {
    let pos_value = copy_attribute(element, &ax_str("AXPosition"))?;
    let size_value = copy_attribute(element, &ax_str("AXSize"))?;
    let pos_ax = pos_value.downcast_ref::<AXValue>()?;
    let size_ax = size_value.downcast_ref::<AXValue>()?;
    let point = read_point(pos_ax)?;
    let size = read_size(size_ax)?;
    let w = size.width.round() as i32;
    let h = size.height.round() as i32;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((point.x.round() as i32, point.y.round() as i32, w, h))
}

/// clip a signed screen-rect into the on-screen `u32` box used downstream.
/// off-screen elements are dropped rather than wrapped to the corner.
fn clamp_rect(
    (x, y, w, h): (i32, i32, i32, i32),
    screen_w: u32,
    screen_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    let sw = screen_w as i32;
    let sh = screen_h as i32;
    let x2 = (x + w).min(sw);
    let y2 = (y + h).min(sh);
    let cx = x.max(0);
    let cy = y.max(0);
    let cw = (x2 - cx).max(0);
    let ch = (y2 - cy).max(0);
    if cw <= 0 || ch <= 0 {
        return None;
    }
    Some((cx as u32, cy as u32, cw as u32, ch as u32))
}

fn read_point(value: &AXValue) -> Option<CGPoint> {
    let mut out = CGPoint { x: 0.0, y: 0.0 };
    let ptr = NonNull::from(&mut out).cast::<c_void>();
    if unsafe { value.value(AXValueType::CGPoint, ptr) } {
        Some(out)
    } else {
        None
    }
}

fn read_size(value: &AXValue) -> Option<CGSize> {
    let mut out = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ptr = NonNull::from(&mut out).cast::<c_void>();
    if unsafe { value.value(AXValueType::CGSize, ptr) } {
        Some(out)
    } else {
        None
    }
}

/// safe wrapper over `AXUIElement::copy_attribute_value`. returns `None` on
/// any error (unsupported attribute, timeout, invalid element), which is what
/// every call site wants anyway.
fn copy_attribute(element: &AXUIElement, name: &CFString) -> Option<CFRetained<CFType>> {
    let mut raw: *const CFType = std::ptr::null();
    let err = unsafe {
        element.copy_attribute_value(name, NonNull::new(&mut raw as *mut *const CFType)?)
    };
    if err != AXError::Success {
        return None;
    }
    let nn = NonNull::new(raw as *mut CFType)?;
    Some(unsafe { CFRetained::from_raw(nn) })
}

/// construct an attribute-name `CFString`. we allocate once per call, which
/// is cheap next to the round-trip ax rpc. static caching would need a `Sync`
/// `CFString`, which the `objc2-core-foundation` wrapper rules out.
fn ax_str(name: &str) -> CFRetained<CFString> {
    CFString::from_str(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_on_screen_rect() {
        assert_eq!(
            clamp_rect((10, 20, 30, 40), 1920, 1080),
            Some((10, 20, 30, 40))
        );
    }

    #[test]
    fn clamp_clips_overflowing_rect() {
        assert_eq!(
            clamp_rect((1900, 1070, 100, 100), 1920, 1080),
            Some((1900, 1070, 20, 10))
        );
    }

    #[test]
    fn clamp_drops_offscreen_rect() {
        assert_eq!(clamp_rect((-200, -50, 100, 30), 1920, 1080), None);
        assert_eq!(clamp_rect((2000, 1100, 100, 30), 1920, 1080), None);
    }

    #[test]
    fn role_whitelist_covers_common_controls() {
        for role in ["AXButton", "AXLink", "AXMenuItem", "AXCheckBox"] {
            assert!(HINT_ROLES.contains(&role), "missing role: {role}");
        }
        for role in ["AXWindow", "AXGroup", "AXScrollArea"] {
            assert!(!HINT_ROLES.contains(&role), "unexpected role: {role}");
        }
    }

    #[test]
    fn action_list_includes_press_and_show_menu() {
        assert!(ACTIONABLE_ACTIONS.contains(&"AXPress"));
        assert!(ACTIONABLE_ACTIONS.contains(&"AXShowMenu"));
    }
}
