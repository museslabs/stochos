//! macos accessibility detector, mirroring [`super::atspi`] on linux. web areas
//! are queried with search predicates instead of walked. `AXFrame` is already
//! screen-space, so the shared remap in [`super`] is a no-op.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Once;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNull, CFNumber, CFRetained, CFString, CFType, CGPoint,
    CGRect, CGSize, Type,
};

use super::detect::{DetectorOutput, HintCandidate, HintDetector};
use crate::backend::Backend;

pub struct AxHintDetector;

impl AxHintDetector {
    /// AX already covers web content, and a CV pass costs a full capture while
    /// merging real controls away.
    pub const SUPPLEMENT_WITH_CV: bool = false;

    /// A sparse AX result is kept, since a small dialog's few buttons are exact,
    /// and CV only fills in the rest of the focused window.
    pub const SCOPE_CV_FALLBACK: bool = true;
}

impl HintDetector for AxHintDetector {
    fn name(&self) -> &'static str {
        "ax"
    }

    fn detect(&self, backend: &mut dyn Backend) -> Result<DetectorOutput> {
        let (screen_w, screen_h) = backend.screen_size();
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            w: f64::from(screen_w),
            h: f64::from(screen_h),
        };
        let pass = collect(frontmost_pid()?, screen);
        Ok(DetectorOutput {
            candidates: pass.targets,
            capture_w: screen_w,
            capture_h: screen_h,
            focus_rect: pass.focus_rect,
        })
    }
}

/// upper bound on returned targets. generous because Electron pages routinely
/// blow past 1k actionable elements and we rank-and-cap downstream.
const MAX_TARGETS: usize = 8192;

/// wall-clock cap on the whole collection. even with a messaging timeout a
/// deep tree of slow children can drag on, so we bail and let the caller
/// fall back to cv.
const COLLECT_BUDGET: Duration = Duration::from_secs(2);

/// ax message timeout. the 6 s default lets one frozen app ruin hint mode.
/// it has to be set on the system-wide element, since on an app element it
/// only covers that one element and not its children.
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.25;

/// cycle guard for tree walks, matching the at-spi counterpart.
const MAX_TREE_DEPTH: usize = 100;

/// how long a web engine gets to publish its content after the wake-up read,
/// and how often we look.
const READY_POLL_BUDGET: Duration = Duration::from_millis(300);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// WebKit fills a page's view only after something has read it, leaving the tab
/// pane empty on the first walk and the page's scroll area off its window on
/// the next.
const MAX_WEBKIT_REWALKS: usize = 2;

/// boxes thinner than this are scrolled-out slivers, not targets.
const MIN_SIDE: u32 = 2;

/// share of the window above which an action-promoted element is a wrapper
/// rather than a control. its center click lands somewhere arbitrary.
const MAX_PROMOTED_COVERAGE: f64 = 0.25;

/// roles that are intrinsically clickable. anything outside this list can
/// still be a hint via [`PRESS_ACTIONS`].
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

/// hint roles that contain other controls. they only become hints when the
/// targets inside them cover less than [`ROW_COVERAGE`] of them, so a sidebar
/// row yields to its cell but not to the small eject button inside it.
const ROW_ROLES: &[&str] = &["AXRow", "AXOutlineRow", "AXCell"];
const ROW_COVERAGE: f64 = 0.5;

/// actions that mean "does something when clicked". an element advertising
/// any of these is a hint regardless of role, which is what surfaces ARIA
/// widgets that show up as `AXGroup`/`AXImage`. `AXShowMenu` is left out on
/// purpose, since Chromium advertises it on every web node, text runs included.
const PRESS_ACTIONS: &[&str] = &["AXPress", "AXOpen", "AXPick"];

/// structural roles that never take a click themselves and are only walked
/// for their children, which also saves the `AXActions` round-trip on each.
const STRUCTURAL_ROLES: &[&str] = &[
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
    "AXWebArea",
];

const CLIP_ROLES: &[&str] = &["AXWindow", "AXScrollArea", "AXWebArea"];

/// roles drawn outside the window that hosts them, so only the screen clips
/// them.
const FLOATING_ROLES: &[&str] = &["AXPopover", "AXMenu", "AXDrawer"];

/// roles whose children are never targets on their own.
const LEAF_ROLES: &[&str] = &["AXStaticText", "AXImage"];

/// one batched read per node. `AXFrame` is position and size in one value.
const NODE_ATTRS: [&str; 3] = ["AXRole", "AXFrame", "AXChildren"];
const RESULT_ATTRS: [&str; 2] = ["AXRole", "AXFrame"];

const CONTROL_SEARCH_KEYS: &[&str] = &[
    "AXButtonSearchKey",
    "AXLinkSearchKey",
    "AXControlSearchKey",
    "AXTextFieldSearchKey",
];

/// Chromium answers keys it doesn't know as if they were
/// `AXAnyTypeSearchKey`, so it must never see this one.
const FOCUSABLE_SEARCH_KEY: &str = "AXKeyboardFocusableSearchKey";

/// roles Chromium gives non-focusable click handlers (a `div` with an
/// `onclick`, or a tree item), which no search key matches.
const CLICK_HANDLER_ROLES: &[&str] = &["AXGroup", "AXImage", "AXRow"];

const ENHANCED_UI: &str = "AXEnhancedUserInterface";
const MANUAL_ACCESSIBILITY: &str = "AXManualAccessibility";

fn collect(pid: i32, screen: Rect) -> Pass {
    static TIMEOUT: Once = Once::new();
    TIMEOUT.call_once(|| {
        let _ = unsafe {
            AXUIElement::new_system_wide().set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS)
        };
    });

    let deadline = Instant::now() + COLLECT_BUDGET;
    // pid is a valid pid_t (32-bit signed int), and a dead one only yields an
    // element whose reads fail.
    let app = unsafe { AXUIElement::new_application(pid) };
    // the first attribute read is what switches Chromium and Electron into
    // their basic accessibility mode, so it must come before the walk.
    let _ = copy_attribute(&app, &ax_str("AXRole"));
    let engine = Engine::of_pid(pid);
    let _enhanced_ui = match engine {
        Engine::Gecko => EnhancedUi::enable(&app),
        _ => None,
    };

    let keys = Keys::new();
    let mut pass = walk_app(&app, &keys, engine, screen, deadline);
    let mut rewalks = 0;
    while engine == Engine::Native && pass.unready_web_view && rewalks < MAX_WEBKIT_REWALKS {
        std::thread::sleep(READY_POLL_INTERVAL);
        pass = walk_app(&app, &keys, engine, screen, deadline);
        rewalks += 1;
    }
    if engine != Engine::Native {
        let poll_until = deadline.min(Instant::now() + READY_POLL_BUDGET);
        // Gecko has its tree once enhanced UI is on, so without a web area
        // there is nothing to wait for. Chromium may not have published one
        // yet.
        while !pass.web_ready
            && (pass.web_seen || engine != Engine::Gecko)
            && Instant::now() + READY_POLL_INTERVAL <= poll_until
        {
            std::thread::sleep(READY_POLL_INTERVAL);
            pass = walk_app(&app, &keys, engine, screen, deadline);
        }
        if !pass.web_ready && engine == Engine::Electron {
            // Electron's own opt-in, as a last resort. it arms a full mode
            // only after a ~2 s debounce, so it serves the next collection
            // rather than this one, and window managers flipping
            // `AXEnhancedUserInterface` keep tearing that mode down.
            let _ = set_bool(&app, MANUAL_ACCESSIBILITY, true);
        }
    }
    pass
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

/// rendering engine of an app, from its bundle layout rather than a list of
/// app names. the markers follow neru's `NeruDetectBundleType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Native,
    Chromium,
    Electron,
    Gecko,
}

impl Engine {
    fn of_pid(pid: i32) -> Engine {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|app| app.bundleURL())
            .and_then(|url| url.path())
            .map_or(Engine::Native, |path| {
                Engine::of_bundle(Path::new(&path.to_string()))
            })
    }

    fn of_bundle(bundle: &Path) -> Engine {
        let contents = bundle.join("Contents");
        if contents
            .join("Frameworks/Electron Framework.framework")
            .exists()
        {
            Engine::Electron
        } else if contents.join("MacOS/XUL").exists() {
            Engine::Gecko
        } else if is_chromium_browser(&contents) {
            Engine::Chromium
        } else {
            Engine::Native
        }
    }
}

/// Chromium browsers ship `app_mode_loader` in a framework's helpers (some
/// nest the real browser under `Contents/Support/*.app`), and their web-app
/// shims carry `CrAppMode*` keys in Info.plist.
fn is_chromium_browser(contents: &Path) -> bool {
    let entries = |dir: &Path| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
    };
    let nested = entries(&contents.join("Support"))
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(|app| app.join("Contents/Frameworks"));
    let has_loader = std::iter::once(contents.join("Frameworks"))
        .chain(nested)
        .any(|dir| entries(&dir).any(|fw| fw.join("Helpers/app_mode_loader").exists()));
    // binary plists store key strings verbatim, so a byte search covers both
    // plist formats.
    has_loader
        || std::fs::read(contents.join("Info.plist"))
            .is_ok_and(|plist| plist.windows(9).any(|w| w == b"CrAppMode"))
}

/// keeps `AXEnhancedUserInterface` on for one collection. Gecko only builds
/// its tree while it is set, but left on it makes AppKit animate every frame
/// change, which window managers fight, so it is undone if we turned it on.
struct EnhancedUi(CFRetained<AXUIElement>);

impl EnhancedUi {
    fn enable(app: &AXUIElement) -> Option<Self> {
        let already_on = copy_attribute(app, &ax_str(ENHANCED_UI))
            .is_some_and(|v| v.downcast_ref::<CFBoolean>().is_some_and(|b| b.as_bool()));
        if already_on || set_bool(app, ENHANCED_UI, true) != AXError::Success {
            return None;
        }
        Some(Self(app.retain()))
    }
}

impl Drop for EnhancedUi {
    fn drop(&mut self) {
        let _ = set_bool(&self.0, ENHANCED_UI, false);
    }
}

/// screen-space rectangle in points, as `AXFrame` reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn has_area(&self) -> bool {
        self.w > 0.0 && self.h > 0.0
    }

    fn area(&self) -> f64 {
        self.w * self.h
    }

    fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let w = (self.x + self.w).min(other.x + other.w) - x;
        let h = (self.y + self.h).min(other.y + other.h) - y;
        (w > 0.0 && h > 0.0).then_some(Rect { x, y, w, h })
    }

    /// pixel box with each edge rounded on its own, so boxes that touch stay
    /// touching.
    fn to_bbox(self) -> (u32, u32, u32, u32) {
        let x1 = self.x.round().max(0.0);
        let y1 = self.y.round().max(0.0);
        let x2 = (self.x + self.w).round().max(x1);
        let y2 = (self.y + self.h).round().max(y1);
        (x1 as u32, y1 as u32, (x2 - x1) as u32, (y2 - y1) as u32)
    }
}

fn visible_part(frame: Option<Rect>, clip: Option<Rect>) -> Option<Rect> {
    frame?.intersect(&clip?)
}

/// a subtree is skipped only when its frame has area and none of it is
/// visible. zero-size frames are never pruned, because Gecko roots whole
/// pages in 0-height wrappers.
fn prunes(frame: Option<Rect>, clip: Option<Rect>) -> bool {
    frame.is_some_and(|f| f.has_area() && visible_part(Some(f), clip).is_none())
}

/// clip for a node's children. windows, scroll areas and web areas hide
/// whatever lies outside their frame, unless that frame is zero-size and so
/// says nothing, as in [`prunes`].
fn child_clip(clip: Option<Rect>, role: &str, frame: Option<Rect>) -> Option<Rect> {
    match frame {
        Some(frame) if frame.has_area() && CLIP_ROLES.contains(&role) => {
            visible_part(Some(frame), clip)
        }
        _ => clip,
    }
}

fn target_box(visible: Rect) -> Option<(u32, u32, u32, u32)> {
    let bbox = visible.to_bbox();
    (bbox.2 >= MIN_SIDE && bbox.3 >= MIN_SIDE).then_some(bbox)
}

fn too_large(visible: Rect, window_area: f64) -> bool {
    visible.area() > MAX_PROMOTED_COVERAGE * window_area
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    Hint,
    Row,
    Probe,
    Matched,
    Skip,
}

impl Rule {
    fn for_role(role: &str, engine: Engine) -> Rule {
        if ROW_ROLES.contains(&role) {
            Rule::Row
        } else if HINT_ROLES.contains(&role) {
            Rule::Hint
        } else if STRUCTURAL_ROLES.contains(&role) || engine == Engine::Gecko {
            // every Gecko node advertises AXPress, its window chrome included,
            // so it tells nothing.
            Rule::Skip
        } else {
            Rule::Probe
        }
    }

    fn promoted(self) -> bool {
        matches!(self, Rule::Probe | Rule::Matched)
    }
}

fn row_is_covered(inside: f64, visible: Rect) -> bool {
    inside >= ROW_COVERAGE * visible.area()
}

fn presses<S: AsRef<str>>(actions: &[S]) -> bool {
    actions
        .iter()
        .any(|action| PRESS_ACTIONS.contains(&action.as_ref()))
}

struct Pass {
    targets: Vec<HintCandidate>,
    focus_rect: Option<(u32, u32, u32, u32)>,
    web_seen: bool,
    web_ready: bool,
    /// a WebKit page that has not been read before, see [`MAX_WEBKIT_REWALKS`].
    unready_web_view: bool,
}

fn walk_app(
    app: &AXUIElement,
    keys: &Keys,
    engine: Engine,
    screen: Rect,
    deadline: Instant,
) -> Pass {
    let mut walk = Walk {
        keys,
        engine,
        screen,
        deadline,
        window_area: screen.area(),
        targets: Vec::new(),
        seen: HashSet::new(),
        web_seen: false,
        web_ready: false,
        unready_web_view: false,
    };
    let mut focus_rect = None;
    for (i, root) in collect_roots(app).iter().enumerate() {
        if walk.exhausted() {
            break;
        }
        let node = fetch_node(root, keys);
        let window = visible_part(node.frame, Some(screen));
        if i == 0 {
            focus_rect = window
                .map(Rect::to_bbox)
                .filter(|&(_, _, w, h)| w > 0 && h > 0);
        }
        walk.window_area = window.unwrap_or(screen).area();
        walk.visit_node(root, node, 0, Some(screen), false);
    }
    Pass {
        targets: walk.targets,
        focus_rect,
        web_seen: walk.web_seen,
        web_ready: walk.web_ready,
        unready_web_view: walk.unready_web_view,
    }
}

struct Walk<'a> {
    keys: &'a Keys,
    engine: Engine,
    screen: Rect,
    deadline: Instant,
    window_area: f64,
    targets: Vec<HintCandidate>,
    seen: HashSet<(u32, u32, u32, u32)>,
    web_seen: bool,
    web_ready: bool,
    unready_web_view: bool,
}

impl Walk<'_> {
    fn exhausted(&self) -> bool {
        self.targets.len() >= MAX_TARGETS || Instant::now() >= self.deadline
    }

    /// walk one subtree. returns the combined area of the targets in it.
    fn visit(
        &mut self,
        element: &AXUIElement,
        depth: usize,
        clip: Option<Rect>,
        in_web: bool,
    ) -> f64 {
        if depth >= MAX_TREE_DEPTH || self.exhausted() {
            return 0.0;
        }
        let node = fetch_node(element, self.keys);
        self.visit_node(element, node, depth, clip, in_web)
    }

    fn visit_node(
        &mut self,
        element: &AXUIElement,
        node: Node,
        depth: usize,
        clip: Option<Rect>,
        mut in_web: bool,
    ) -> f64 {
        let role = node.role.as_str();
        let clip = if FLOATING_ROLES.contains(&role) {
            Some(self.screen)
        } else {
            clip
        };
        let has_children = node.children.as_ref().is_some_and(|c| !c.is_empty());
        if prunes(node.frame, clip) {
            self.unready_web_view |= role == "AXScrollArea" && has_children;
            return 0.0;
        }
        let visible = visible_part(node.frame, clip);
        self.unready_web_view |= role == "AXGroup"
            && !has_children
            && visible.is_some_and(|v| too_large(v, self.window_area))
            && parent_role(element).as_deref() == Some("AXTabGroup");
        let rule = Rule::for_role(role, self.engine);
        let mut covered = match rule {
            Rule::Row | Rule::Skip => None,
            _ => self.emit(element, visible, rule),
        };
        if debug() {
            debug_node(depth, role, covered.is_some());
        }

        // a combo box's list drops out of the window too.
        let clip = if role == "AXComboBox" {
            Some(self.screen)
        } else {
            child_clip(clip, role, node.frame)
        };
        if role == "AXWebArea" && !in_web {
            self.web_seen = true;
            if let Some(found) = self.search_web_area(element, depth, clip) {
                return covered.unwrap_or(0.0) + found;
            }
            in_web = true;
        }

        let mut inside = 0.0;
        if let Some(children) = node.children.filter(|_| !LEAF_ROLES.contains(&role)) {
            for child in children.iter() {
                if self.exhausted() {
                    break;
                }
                if let Some(child) = child.downcast_ref::<AXUIElement>() {
                    inside += self.visit(child, depth + 1, clip, in_web);
                }
            }
        }
        if rule == Rule::Row && visible.is_some_and(|v| !row_is_covered(inside, v)) {
            covered = self.emit(element, visible, rule);
            if covered.is_some() && debug() {
                debug_node(depth, role, true);
            }
        }
        covered.unwrap_or(0.0) + inside
    }

    /// record a visible element as a target. returns the target's area when
    /// there is now one at its box, including one an earlier element claimed.
    fn emit(&mut self, element: &AXUIElement, visible: Option<Rect>, rule: Rule) -> Option<f64> {
        let visible = visible?;
        let bbox = target_box(visible)?;
        if self.seen.contains(&bbox) {
            return Some(visible.area());
        }
        if rule.promoted() && too_large(visible, self.window_area) {
            return None;
        }
        if rule == Rule::Probe && !action_names(element).is_some_and(|a| presses(&a)) {
            return None;
        }
        self.seen.insert(bbox);
        self.targets.push(HintCandidate { bbox, score: 1.0 });
        Some(visible.area())
    }

    /// collect a web area's controls with search queries instead of walking
    /// it. `None` when the area doesn't answer them.
    fn search_web_area(
        &mut self,
        area: &AXUIElement,
        depth: usize,
        clip: Option<Rect>,
    ) -> Option<f64> {
        let keys = self.keys;
        // Chromium's private node id also marks its views embedded in other apps.
        let query = if copy_attribute(area, &keys.chrome_node_id).is_some() {
            &keys.chromium_controls
        } else {
            &keys.web_controls
        };
        search(area, keys, query).map(|controls| {
            let mut covered = self.emit_results(&controls, depth, clip, false);
            // the recover pass. Gecko puts AXPress on every node, so for it
            // this would turn the whole page into targets.
            if self.engine != Engine::Gecko {
                if let Some(visible) = search(area, keys, &keys.any_visible) {
                    covered += self.emit_results(&visible, depth, clip, true);
                }
            }
            self.web_ready |= !controls.is_empty()
                || search(area, keys, &keys.any_first).is_some_and(|any| !any.is_empty());
            covered
        })
    }

    /// emit search results. with `click_handlers` only plain groups and
    /// images that press are kept, for click handlers no key matches.
    fn emit_results(
        &mut self,
        results: &CFArray<CFType>,
        depth: usize,
        clip: Option<Rect>,
        click_handlers: bool,
    ) -> f64 {
        let mut covered = 0.0;
        for result in results.iter() {
            if self.exhausted() {
                break;
            }
            let Some(result) = result.downcast_ref::<AXUIElement>() else {
                continue;
            };
            let Some([role, frame]) = read_attributes(result, &self.keys.result)
                .and_then(|slots| <[_; RESULT_ATTRS.len()]>::try_from(slots).ok())
            else {
                continue;
            };
            let role = role.as_deref().and_then(cf_string).unwrap_or_default();
            let rule = if click_handlers {
                if !CLICK_HANDLER_ROLES.contains(&role.as_str()) {
                    continue;
                }
                Rule::Probe
            } else if HINT_ROLES.contains(&role.as_str()) {
                Rule::Hint
            } else if STRUCTURAL_ROLES.contains(&role.as_str()) {
                // focusable containers and splitters match the control and
                // focusable keys too.
                continue;
            } else {
                Rule::Matched
            };
            let visible = visible_part(frame_of(result, frame.as_deref(), self.keys), clip);
            if let Some(area) = self.emit(result, visible, rule) {
                covered += area;
                if debug() {
                    debug_node(depth + 1, &role, true);
                }
            }
        }
        covered
    }
}

fn debug() -> bool {
    // cache the env lookup: the walk runs this on every node and `var_os` is
    // a syscall each call.
    use std::sync::OnceLock;
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var_os("STOCHOS_AX_DEBUG").is_some())
}

fn debug_node(depth: usize, role: &str, hint: bool) {
    eprintln!(
        "{:>3} {}{}{}",
        depth,
        "  ".repeat(depth.min(40)),
        if role.is_empty() { "?" } else { role },
        if hint { " *" } else { "" }
    );
}

struct Keys {
    node: AttributeSet,
    result: AttributeSet,
    position: CFRetained<CFString>,
    size: CFRetained<CFString>,
    chrome_node_id: CFRetained<CFString>,
    search: CFRetained<CFString>,
    chromium_controls: CFRetained<CFDictionary<CFString, CFType>>,
    web_controls: CFRetained<CFDictionary<CFString, CFType>>,
    any_visible: CFRetained<CFDictionary<CFString, CFType>>,
    any_first: CFRetained<CFDictionary<CFString, CFType>>,
}

impl Keys {
    fn new() -> Self {
        Keys {
            node: AttributeSet::new(&NODE_ATTRS),
            result: AttributeSet::new(&RESULT_ATTRS),
            position: ax_str("AXPosition"),
            size: ax_str("AXSize"),
            chrome_node_id: ax_str("ChromeAXNodeId"),
            search: ax_str("AXUIElementsForSearchPredicate"),
            chromium_controls: search_query(CONTROL_SEARCH_KEYS, true, -1),
            web_controls: search_query(
                &[&[FOCUSABLE_SEARCH_KEY], CONTROL_SEARCH_KEYS].concat(),
                true,
                -1,
            ),
            any_visible: search_query(&["AXAnyTypeSearchKey"], true, -1),
            any_first: search_query(&["AXAnyTypeSearchKey"], false, 1),
        }
    }
}

struct AttributeSet {
    array: CFRetained<CFArray<CFString>>,
    names: Vec<CFRetained<CFString>>,
}

impl AttributeSet {
    fn new(names: &[&str]) -> Self {
        let names: Vec<_> = names.iter().map(|name| ax_str(name)).collect();
        AttributeSet {
            array: CFArray::from_retained_objects(&names),
            names,
        }
    }
}

/// `limit` -1 means unlimited.
fn search_query(
    search_keys: &[&str],
    visible_only: bool,
    limit: i64,
) -> CFRetained<CFDictionary<CFString, CFType>> {
    let search_keys: Vec<_> = search_keys.iter().map(|key| ax_str(key)).collect();
    let search_keys = CFArray::from_retained_objects(&search_keys);
    let limit = CFNumber::new_i64(limit);
    let direction = ax_str("AXDirectionNext");
    let names = [
        "AXSearchKey",
        "AXVisibleOnly",
        "AXResultsLimit",
        "AXDirection",
    ]
    .map(ax_str);
    let values: [&CFType; 4] = [
        search_keys.as_ref(),
        CFBoolean::new(visible_only).as_ref(),
        limit.as_ref(),
        direction.as_ref(),
    ];
    CFDictionary::from_slices(&names.each_ref().map(|name| &**name), &values)
}

fn search(
    area: &AXUIElement,
    keys: &Keys,
    query: &CFDictionary<CFString, CFType>,
) -> Option<CFRetained<CFArray<CFType>>> {
    let mut raw: *const CFType = std::ptr::null();
    let err = unsafe {
        area.copy_parameterized_attribute_value(
            &keys.search,
            query.as_ref(),
            NonNull::from(&mut raw),
        )
    };
    if err != AXError::Success {
        return None;
    }
    let value = unsafe { CFRetained::from_raw(NonNull::new(raw as *mut CFType)?) };
    cf_array(value)
}

#[derive(Default)]
struct Node {
    role: String,
    frame: Option<Rect>,
    children: Option<CFRetained<CFArray<CFType>>>,
}

fn fetch_node(element: &AXUIElement, keys: &Keys) -> Node {
    let Some([role, frame, children]) = read_attributes(element, &keys.node)
        .and_then(|slots| <[_; NODE_ATTRS.len()]>::try_from(slots).ok())
    else {
        return Node::default();
    };
    Node {
        role: role.as_deref().and_then(cf_string).unwrap_or_default(),
        frame: frame_of(element, frame.as_deref(), keys),
        children: children.and_then(cf_array),
    }
}

/// apps that predate `AXFrame` still answer position and size.
fn frame_of(element: &AXUIElement, frame: Option<&CFType>, keys: &Keys) -> Option<Rect> {
    frame.and_then(ax_rect).or_else(|| {
        let position = ax_point(&*copy_attribute(element, &keys.position)?)?;
        let size = ax_size(&*copy_attribute(element, &keys.size)?)?;
        Some(Rect {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
        })
    })
}

type Slots = Vec<Option<CFRetained<CFType>>>;

/// read several attributes in one round-trip, one slot per name. falls back
/// to one read each when the app doesn't implement the batched call. `None`
/// when the element is gone or the app is not answering at all.
fn read_attributes(element: &AXUIElement, set: &AttributeSet) -> Option<Slots> {
    let mut raw: *const CFArray = std::ptr::null();
    let err = unsafe {
        element.copy_multiple_attribute_values(
            set.array.as_ref(),
            AXCopyMultipleAttributeOptions::empty(),
            NonNull::from(&mut raw),
        )
    };
    if err == AXError::InvalidUIElement || err == AXError::CannotComplete {
        return None;
    }
    let mut slots: Slots = match NonNull::new(raw as *mut CFArray) {
        Some(values) if err == AXError::Success => {
            let values = unsafe { CFRetained::from_raw(values) };
            // a CFArray of CFType values, and both are zero-sized markers.
            let values = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(values) };
            values
                .iter()
                .map(|value| (!is_missing(&value)).then_some(value))
                .collect()
        }
        _ => set
            .names
            .iter()
            .map(|name| copy_attribute(element, name))
            .collect(),
    };
    slots.resize(set.names.len(), None);
    Some(slots)
}

/// batched reads report a missing attribute in its slot, as CFNull or as an
/// AXValue wrapping the AXError.
fn is_missing(value: &CFType) -> bool {
    value.downcast_ref::<CFNull>().is_some()
        || value
            .downcast_ref::<AXValue>()
            .is_some_and(|v| unsafe { v.r#type() } == AXValueType::AXError)
}

fn parent_role(element: &AXUIElement) -> Option<String> {
    let parent = copy_attribute(element, &ax_str("AXParent"))?
        .downcast::<AXUIElement>()
        .ok()?;
    cf_string(&*copy_attribute(&parent, &ax_str("AXRole"))?)
}

fn action_names(element: &AXUIElement) -> Option<Vec<String>> {
    let mut raw: *const CFArray = std::ptr::null();
    let err = unsafe { element.copy_action_names(NonNull::from(&mut raw)) };
    if err != AXError::Success {
        return None;
    }
    let arr = unsafe { CFRetained::from_raw(NonNull::new(raw as *mut CFArray)?) };
    // a CFArray of CFString names, and both subclass CFType.
    let arr = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(arr) };
    Some(arr.iter().filter_map(|name| cf_string(&name)).collect())
}

fn set_bool(element: &AXUIElement, attribute: &str, value: bool) -> AXError {
    unsafe { element.set_attribute_value(&ax_str(attribute), CFBoolean::new(value).as_ref()) }
}

/// downcast a `CFType` into a `CFArray<CFType>`. the default `CFArray`
/// parameterization is `Opaque`, and reinterpreting as `CFType` is a
/// zero-cost marker swap since both are zero-sized.
fn cf_array(value: CFRetained<CFType>) -> Option<CFRetained<CFArray<CFType>>> {
    let arr = value.downcast::<CFArray>().ok()?;
    Some(unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(arr) })
}

fn cf_string(value: &CFType) -> Option<String> {
    Some(value.downcast_ref::<CFString>()?.to_string())
}

fn ax_rect(value: &CFType) -> Option<Rect> {
    let mut out = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };
    unsafe { ax_value(value, AXValueType::CGRect, &mut out) }?;
    Some(Rect {
        x: out.origin.x,
        y: out.origin.y,
        w: out.size.width,
        h: out.size.height,
    })
}

fn ax_point(value: &CFType) -> Option<CGPoint> {
    let mut out = CGPoint { x: 0.0, y: 0.0 };
    unsafe { ax_value(value, AXValueType::CGPoint, &mut out) }?;
    Some(out)
}

fn ax_size(value: &CFType) -> Option<CGSize> {
    let mut out = CGSize {
        width: 0.0,
        height: 0.0,
    };
    unsafe { ax_value(value, AXValueType::CGSize, &mut out) }?;
    Some(out)
}

/// # Safety
///
/// `T` must be the C struct `ty` describes.
unsafe fn ax_value<T>(value: &CFType, ty: AXValueType, out: &mut T) -> Option<()> {
    let value = value.downcast_ref::<AXValue>()?;
    let ptr = NonNull::from(out).cast::<c_void>();
    unsafe { value.value(ty, ptr) }.then_some(())
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

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    #[test]
    fn intersect_keeps_overlap() {
        assert_eq!(
            rect(1900.0, 1070.0, 100.0, 100.0).intersect(&SCREEN),
            Some(rect(1900.0, 1070.0, 20.0, 10.0))
        );
        assert_eq!(
            rect(10.0, 20.0, 30.0, 40.0).intersect(&SCREEN),
            Some(rect(10.0, 20.0, 30.0, 40.0))
        );
    }

    #[test]
    fn intersect_drops_disjoint_and_touching() {
        assert_eq!(rect(-200.0, -50.0, 100.0, 30.0).intersect(&SCREEN), None);
        assert_eq!(rect(1920.0, 0.0, 10.0, 10.0).intersect(&SCREEN), None);
    }

    #[test]
    fn clip_chain_narrows_through_window_scroll_and_web_areas() {
        let window = rect(100.0, 100.0, 800.0, 600.0);
        let clip = child_clip(Some(SCREEN), "AXWindow", Some(window));
        assert_eq!(clip, Some(window));
        let clip = child_clip(clip, "AXGroup", Some(rect(0.0, 0.0, 2000.0, 2000.0)));
        assert_eq!(clip, Some(window));
        let clip = child_clip(clip, "AXScrollArea", Some(rect(100.0, 150.0, 800.0, 300.0)));
        assert_eq!(clip, Some(rect(100.0, 150.0, 800.0, 300.0)));
        let clip = child_clip(clip, "AXWebArea", Some(rect(50.0, 200.0, 400.0, 1000.0)));
        assert_eq!(clip, Some(rect(100.0, 200.0, 350.0, 250.0)));
        // a row scrolled below the scroll area is not visible.
        assert_eq!(
            visible_part(Some(rect(100.0, 500.0, 300.0, 20.0)), clip),
            None
        );
    }

    #[test]
    fn zero_size_clip_role_keeps_the_parent_clip() {
        let window = Some(rect(100.0, 100.0, 800.0, 600.0));
        for frame in [rect(200.0, 200.0, 800.0, 0.0), rect(0.0, 2000.0, 0.0, 0.0)] {
            assert_eq!(child_clip(window, "AXScrollArea", Some(frame)), window);
        }
    }

    #[test]
    fn prune_only_frames_with_area_that_miss_the_clip() {
        let clip = Some(rect(0.0, 0.0, 100.0, 100.0));
        assert!(prunes(Some(rect(200.0, 200.0, 10.0, 10.0)), clip));
        assert!(!prunes(Some(rect(50.0, 50.0, 10.0, 10.0)), clip));
        // zero-size wrappers far off the clip still get walked.
        assert!(!prunes(Some(rect(200.0, 200.0, 500.0, 0.0)), clip));
        assert!(!prunes(Some(rect(200.0, 200.0, 0.0, 0.0)), clip));
        assert!(!prunes(None, clip));
    }

    #[test]
    fn target_box_drops_slivers() {
        assert_eq!(
            target_box(rect(10.0, 20.0, 30.0, 40.0)),
            Some((10, 20, 30, 40))
        );
        assert_eq!(target_box(rect(10.0, 20.0, 30.0, 1.0)), None);
        assert_eq!(target_box(rect(10.0, 20.0, 1.4, 30.0)), None);
        assert_eq!(target_box(rect(10.0, 20.0, 2.0, 2.0)), Some((10, 20, 2, 2)));
    }

    #[test]
    fn bbox_rounds_edges_independently() {
        assert_eq!(rect(10.4, 10.6, 10.4, 10.0).to_bbox(), (10, 11, 11, 10));
    }

    #[test]
    fn show_menu_alone_is_not_actionable() {
        assert!(!presses(&["AXShowMenu"]));
        assert!(!presses(&["AXShowMenu", "AXScrollToVisible"]));
        assert!(presses(&["AXShowMenu", "AXPress"]));
        assert!(presses(&["AXOpen"]));
        assert!(presses(&["AXPick"]));
        assert!(!presses::<&str>(&[]));
    }

    #[test]
    fn role_rules() {
        assert_eq!(Rule::for_role("AXButton", Engine::Native), Rule::Hint);
        assert_eq!(Rule::for_role("AXRow", Engine::Native), Rule::Row);
        assert_eq!(Rule::for_role("AXCell", Engine::Native), Rule::Row);
        assert_eq!(Rule::for_role("AXGroup", Engine::Native), Rule::Probe);
        assert_eq!(Rule::for_role("AXStaticText", Engine::Native), Rule::Probe);
        assert_eq!(Rule::for_role("AXScrollArea", Engine::Native), Rule::Skip);
        assert_eq!(Rule::for_role("AXWebArea", Engine::Native), Rule::Skip);
        assert_eq!(Rule::for_role("AXGroup", Engine::Chromium), Rule::Probe);
        // in Gecko only the role whitelist counts, browser chrome included.
        assert_eq!(Rule::for_role("AXGroup", Engine::Gecko), Rule::Skip);
        assert_eq!(Rule::for_role("AXLink", Engine::Gecko), Rule::Hint);
    }

    #[test]
    fn rows_yield_only_to_targets_covering_them() {
        let row = rect(0.0, 0.0, 220.0, 28.0);
        // a sidebar row whose cell is already a target.
        assert!(row_is_covered(rect(0.0, 0.0, 220.0, 28.0).area(), row));
        // cells that together fill a list row.
        assert!(row_is_covered(3.0 * rect(0.0, 0.0, 70.0, 28.0).area(), row));
        // an eject button does not stand for the whole row.
        assert!(!row_is_covered(rect(190.0, 6.0, 16.0, 16.0).area(), row));
        assert!(!row_is_covered(0.0, row));
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
}
