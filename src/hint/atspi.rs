//! AT-SPI accessibility detector (Linux): semantic click targets with exact
//! boxes from the desktop accessibility tree. Whole trees come from one
//! `Cache.GetItems` per app (fetched concurrently) rather than per-node D-Bus
//! roundtrips; only actionable visible nodes get a `GetExtents`. On Wayland,
//! AT-SPI coordinates arrive pinned to (0,0) and are corrected with [`compositor`]
//! window geometry, hinting only the active window; without it (X11) coordinates
//! are already absolute and used unshifted. Async (zbus) collection runs behind
//! a caller-side timeout so a wedged accessibility bus cannot hold the overlay.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::cache::CacheProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::zbus::{fdo::DBusProxy, names::BusName};
use atspi::{AccessibilityConnection, CoordType, Role, State, StateSet};
use futures_util::future::join_all;

use super::compositor::{self, CompositorSnapshot, WindowRect};
use super::detect::{DetectorOutput, HintCandidate, HintDetector};
use crate::backend::Backend;

static ACCESSIBILITY_ENABLE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Upper bound on returned targets; matches the shared ranking/dedup cap.
const MAX_TARGETS: usize = 1024;

const COLLECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Cycle guard for parent-chain walks over cache data we don't control.
const MAX_ANCESTOR_DEPTH: usize = 128;

pub struct AtspiHintDetector;

impl HintDetector for AtspiHintDetector {
    fn name(&self) -> &'static str {
        "atspi"
    }

    fn detect(&self, backend: &mut dyn Backend) -> Result<DetectorOutput> {
        let (screen_w, screen_h) = backend.screen_size();
        let (candidates, focus_rect) = collect_candidates_with_timeout(screen_w, screen_h)?;
        // Boxes are already in screen coordinates, so the shared remap is a no-op.
        Ok(DetectorOutput {
            candidates,
            capture_w: screen_w,
            capture_h: screen_h,
            focus_rect,
        })
    }
}

/// Roles a user would plausibly want to click; containers are walked for
/// their children but never labelled themselves.
fn is_actionable(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::PushButtonMenu
            | Role::ToggleButton
            | Role::Link
            | Role::Menu
            | Role::MenuItem
            | Role::CheckMenuItem
            | Role::RadioMenuItem
            | Role::CheckBox
            | Role::RadioButton
            | Role::ComboBox
            | Role::Entry
            | Role::PasswordText
            | Role::PageTab
            | Role::ListItem
            | Role::TreeItem
            | Role::TableRow
            | Role::TableCell
            | Role::TableColumnHeader
            | Role::Slider
            | Role::SpinButton
            | Role::Icon
    )
}

/// A top-level window node, the anchor for position correction.
fn is_window(role: Role) -> bool {
    matches!(role, Role::Frame | Role::Window | Role::Dialog)
}

struct Node {
    path: String,
    parent: String,
    role: Role,
    states: StateSet,
    name: String,
    /// Child count the node claims to have, `-1` when unknown; a mismatch with
    /// the children actually present marks a lazily-materialized subtree.
    children: i32,
}

/// Candidates plus the compositor's active-window rectangle, which the `auto`
/// cascade uses to spot regions the accessibility tree left empty.
type Collected = (Vec<HintCandidate>, Option<(u32, u32, u32, u32)>);

fn collect_candidates_with_timeout(screen_w: u32, screen_h: u32) -> Result<Collected> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = futures_executor::block_on(collect_candidates(screen_w, screen_h));
        let _ = tx.send(result);
    });
    match rx.recv_timeout(COLLECT_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            anyhow::bail!("AT-SPI collection timed out after {COLLECT_TIMEOUT:?}")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("AT-SPI collection worker exited before sending a result")
        }
    }
}

async fn collect_candidates(screen_w: u32, screen_h: u32) -> Result<Collected> {
    // Best-effort, once per process: ask the session to enable accessibility
    // so toolkits publish their trees.
    if !ACCESSIBILITY_ENABLE_REQUESTED.swap(true, Ordering::Relaxed) {
        let _ = atspi::connection::set_session_accessibility(true).await;
    }

    let a11y = AccessibilityConnection::new()
        .await
        .context("connect to the AT-SPI accessibility bus (is it running?)")?;
    let root = a11y
        .root_accessible_on_registry()
        .await
        .context("read the AT-SPI registry root")?;

    let compositor = compositor::snapshot();

    // (destination, app root object path) per application.
    let apps: Vec<(String, String)> = root
        .get_children()
        .await
        .context("list applications on the accessibility bus")?
        .iter()
        .filter_map(|app| {
            let dest = app.name_as_str()?.to_owned();
            Some((dest, app.path_as_str().to_owned()))
        })
        .collect();

    // Tree fetch and pid resolution are independent D-Bus batches, so overlap
    // them instead of paying two serial round-trips at hint-mode entry.
    let (trees, pids) = futures_util::future::join(
        join_all(
            apps.iter()
                .map(|(dest, root_path)| fetch_app_tree(&a11y, dest, root_path)),
        ),
        app_pids(&a11y, &apps, &compositor),
    )
    .await;

    let mut out = Vec::new();
    for (((dest, _), nodes), pid) in apps.iter().zip(trees).zip(pids) {
        let Some(mut nodes) = nodes else { continue };
        // Match the app's top-level frames first: they live in the raw cache,
        // so this needs no tree completion. An app whose frames are all
        // skipped (not the active window) contributes nothing, so skip the
        // expensive lazy-child expansion entirely.
        let offsets = frame_offsets(&a11y, dest, &nodes, pid, &compositor).await;
        if offsets.values().all(Option::is_none) {
            continue;
        }
        complete_tree(&a11y, dest, &mut nodes).await;
        collect_targets(&a11y, dest, &nodes, &offsets, screen_w, screen_h, &mut out).await;
        if out.len() >= MAX_TARGETS {
            out.truncate(MAX_TARGETS);
            break;
        }
    }
    let focus_rect = compositor
        .active_window()
        .and_then(|w| clamp_to_screen(w.x, w.y, w.w, w.h, screen_w, screen_h));
    Ok((out, focus_rect))
}

/// The pid behind each application's bus connection, from the a11y bus
/// daemon. The compositor reports the same pid on the app's windows, making
/// it the primary window-matching key; only needed when there is compositor
/// geometry to match against. Sandboxed apps reach the bus through a proxy
/// process, so a pid here may match no window — matching then falls back to
/// size and title.
async fn app_pids(
    a11y: &AccessibilityConnection,
    apps: &[(String, String)],
    compositor: &CompositorSnapshot,
) -> Vec<Option<i32>> {
    if compositor.windows.is_empty() {
        return vec![None; apps.len()];
    }
    let Ok(dbus) = DBusProxy::new(a11y.connection()).await else {
        return vec![None; apps.len()];
    };
    join_all(apps.iter().map(|(dest, _)| {
        let dbus = &dbus;
        async move {
            let name = BusName::try_from(dest.as_str()).ok()?;
            let pid = dbus.get_connection_unix_process_id(name).await.ok()?;
            i32::try_from(pid).ok()
        }
    }))
    .await
}

/// At or below this many cache items, assume the app has not built its tree.
const STUB_TREE_MAX_ITEMS: usize = 10;

/// Fetch an application's entire tree in one `Cache.GetItems` call (legacy
/// wire format as fallback). Toolkits build their tree lazily, on the first
/// sign of a client *walking* them — a cache read is not such a sign — so a
/// stub-sized answer gets one live `get_children` poke on the root and one
/// re-read; by the next hint launch the cache is fully populated.
async fn fetch_app_tree(
    a11y: &AccessibilityConnection,
    dest: &str,
    root_path: &str,
) -> Option<Vec<Node>> {
    let cache = CacheProxy::builder(a11y.connection())
        .destination(dest.to_owned())
        .ok()?
        .build()
        .await
        .ok()?;
    let items = cache_nodes(&cache).await?;
    if items.len() > STUB_TREE_MAX_ITEMS {
        return Some(items);
    }
    let _ = children_of(a11y, dest, root_path).await;
    match cache_nodes(&cache).await {
        Some(retried) if retried.len() > items.len() => Some(retried),
        _ => Some(items),
    }
}

async fn cache_nodes(cache: &CacheProxy<'_>) -> Option<Vec<Node>> {
    match cache.get_items().await {
        Ok(items) => Some(
            items
                .into_iter()
                .map(|item| Node {
                    path: item.object.path_as_str().to_owned(),
                    parent: item.parent.path_as_str().to_owned(),
                    role: item.role,
                    states: item.states,
                    name: item.name,
                    children: item.children,
                })
                .collect(),
        ),
        // Signature error: the app serves the legacy item layout.
        Err(_) => Some(
            cache
                .get_legacy_items()
                .await
                .ok()?
                .into_iter()
                .map(|item| Node {
                    path: item.object.path_as_str().to_owned(),
                    parent: item.parent.path_as_str().to_owned(),
                    role: item.role,
                    states: item.states,
                    name: item.name,
                    children: item.children.len() as i32,
                })
                .collect(),
        ),
    }
}

const MAX_EXPANSION_NODES: usize = 2000;
const MAX_EXPANSION_ROUNDS: usize = 16;

/// Fill in the parts of the tree the cache only knows *about*: GTK tree and
/// list views materialize items on demand, so the cache holds the container
/// but not its children. Expand exactly the visible containers whose child
/// count mismatches, each at most once, with batched live calls.
async fn complete_tree(a11y: &AccessibilityConnection, dest: &str, nodes: &mut Vec<Node>) {
    let mut expanded: HashSet<String> = HashSet::new();
    let mut added = 0usize;
    for _ in 0..MAX_EXPANSION_ROUNDS {
        let mut cached_children: HashMap<&str, i32> = HashMap::new();
        for node in nodes.iter() {
            *cached_children.entry(node.parent.as_str()).or_default() += 1;
        }
        let pending: Vec<String> = nodes
            .iter()
            .filter(|n| {
                n.states.contains(State::Showing)
                    && !expanded.contains(&n.path)
                    && (n.children < 0
                        || n.children > cached_children.get(n.path.as_str()).copied().unwrap_or(0))
            })
            .map(|n| n.path.clone())
            .collect();
        if pending.is_empty() || added >= MAX_EXPANSION_NODES {
            return;
        }

        let known: HashSet<&str> = nodes.iter().map(|n| n.path.as_str()).collect();
        let child_lists = join_all(pending.iter().map(|path| children_of(a11y, dest, path))).await;
        // (parent path, child path)
        let mut new_refs: Vec<(String, String)> = Vec::new();
        for (parent, children) in pending.iter().zip(child_lists) {
            for child in children {
                if !known.contains(child.as_str()) {
                    new_refs.push((parent.clone(), child));
                }
            }
        }
        expanded.extend(pending);
        new_refs.truncate(MAX_EXPANSION_NODES.saturating_sub(added));

        let described = join_all(new_refs.iter().map(|(_, path)| describe(a11y, dest, path))).await;
        for ((parent, path), description) in new_refs.into_iter().zip(described) {
            let Some((role, states)) = description else {
                continue;
            };
            nodes.push(Node {
                path,
                parent,
                role,
                states,
                name: String::new(),
                children: -1,
            });
            added += 1;
        }
    }
}

/// A node's live children, restricted to the same application.
async fn children_of(a11y: &AccessibilityConnection, dest: &str, path: &str) -> Vec<String> {
    let Ok(accessible) = accessible_at(a11y, dest, path).await else {
        return Vec::new();
    };
    let Ok(children) = accessible.get_children().await else {
        return Vec::new();
    };
    children
        .iter()
        .filter(|child| child.name_as_str() == Some(dest))
        .map(|child| child.path_as_str().to_owned())
        .collect()
}

async fn describe(
    a11y: &AccessibilityConnection,
    dest: &str,
    path: &str,
) -> Option<(Role, StateSet)> {
    let accessible = accessible_at(a11y, dest, path).await.ok()?;
    let role = accessible.get_role().await.ok()?;
    let states = accessible.get_state().await.ok()?;
    Some((role, states))
}

async fn accessible_at<'c>(
    a11y: &'c AccessibilityConnection,
    dest: &str,
    path: &str,
) -> Result<AccessibleProxy<'c>> {
    Ok(AccessibleProxy::builder(a11y.connection())
        .destination(dest.to_owned())?
        .path(path.to_owned())?
        .build()
        .await?)
}

/// The screen-coordinate offset for each showing top-level frame: `Some` when
/// the frame should be hinted, `None` when skipped (a non-active window). Both
/// outcomes are recorded so a node's nearest frame ancestor decides whether it
/// is hinted, even when an outer frame would match.
async fn frame_offsets(
    a11y: &AccessibilityConnection,
    dest: &str,
    nodes: &[Node],
    pid: Option<i32>,
    compositor: &CompositorSnapshot,
) -> HashMap<String, Option<(i32, i32)>> {
    let mut offsets = HashMap::new();
    for frame in nodes
        .iter()
        .filter(|n| is_window(n.role) && n.states.contains(State::Showing))
    {
        let offset = match extents(a11y, dest, &frame.path).await {
            Ok(ext) => {
                // Cache items often carry an empty name for frames; the title
                // is needed as the window-matching tiebreak, so fetch it live.
                let name = if frame.name.is_empty() && !compositor.windows.is_empty() {
                    live_name(a11y, dest, &frame.path).await.unwrap_or_default()
                } else {
                    frame.name.clone()
                };
                frame_offset(ext, &name, pid, compositor)
            }
            Err(_) => None,
        };
        offsets.insert(frame.path.clone(), offset);
    }
    offsets
}

async fn collect_targets(
    a11y: &AccessibilityConnection,
    dest: &str,
    nodes: &[Node],
    offsets: &HashMap<String, Option<(i32, i32)>>,
    screen_w: u32,
    screen_h: u32,
    out: &mut Vec<HintCandidate>,
) {
    let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    let rows_with_child_targets = table_rows_with_actionable_descendants(nodes, &index);
    let targets: Vec<(&Node, (i32, i32))> = nodes
        .iter()
        .filter(|n| is_actionable(n.role) && n.states.contains(State::Showing))
        .filter(|n| n.role != Role::TableRow || !rows_with_child_targets.contains(n.path.as_str()))
        .filter_map(|n| owning_frame_offset(n, &index, offsets).map(|off| (n, off)))
        .take(MAX_TARGETS.saturating_sub(out.len()))
        .collect();

    let boxes = join_all(targets.iter().map(|(n, _)| extents(a11y, dest, &n.path))).await;
    let mut unnamed_cells_by_table: HashMap<String, Vec<(u32, u32, u32, u32)>> = HashMap::new();
    for ((node, (dx, dy)), ext) in targets.iter().zip(boxes) {
        let Ok((x, y, w, h)) = ext else { continue };
        let Some(bbox) = clamp_to_screen(x + dx, y + dy, w, h, screen_w, screen_h) else {
            continue;
        };
        push_table_cell_candidate(node, bbox, &mut unnamed_cells_by_table, out);
    }
    for cells in unnamed_cells_by_table.into_values() {
        for bbox in merge_cells_into_rows(cells) {
            out.push(HintCandidate { bbox, score: 1.0 });
        }
    }
}

fn table_rows_with_actionable_descendants(
    nodes: &[Node],
    index: &HashMap<&str, &Node>,
) -> HashSet<String> {
    let mut rows = HashSet::new();
    for node in nodes {
        if node.role == Role::TableRow
            || node.role == Role::Icon
            || !node.states.contains(State::Showing)
            || !is_actionable(node.role)
        {
            continue;
        }

        let mut current = node;
        for _ in 0..MAX_ANCESTOR_DEPTH {
            let Some(parent) = index.get(current.parent.as_str()) else {
                break;
            };
            if parent.role == Role::TableRow {
                rows.insert(parent.path.clone());
                break;
            }
            current = parent;
        }
    }
    rows
}

/// Union unnamed table cells that share a vertical band into one box per row.
fn merge_cells_into_rows(mut cells: Vec<(u32, u32, u32, u32)>) -> Vec<(u32, u32, u32, u32)> {
    cells.sort_by_key(|&(_, y, _, h)| (y, h));
    let mut rows: Vec<(u32, u32, u32, u32)> = Vec::new();
    for cell in cells {
        match rows.last_mut() {
            Some(row) if vertical_overlap_ratio(*row, cell) >= 0.5 => *row = union_box(*row, cell),
            _ => rows.push(cell),
        }
    }
    rows
}

fn push_table_cell_candidate(
    node: &Node,
    bbox: (u32, u32, u32, u32),
    unnamed_cells_by_table: &mut HashMap<String, Vec<(u32, u32, u32, u32)>>,
    out: &mut Vec<HintCandidate>,
) {
    if node.role == Role::TableCell && node.name.trim().is_empty() {
        unnamed_cells_by_table
            .entry(node.parent.clone())
            .or_default()
            .push(bbox);
    } else {
        out.push(HintCandidate { bbox, score: 1.0 });
    }
}

/// Overlap height as a fraction of the shorter box's height.
fn vertical_overlap_ratio(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> f32 {
    let top = a.1.max(b.1);
    let bottom = (a.1 + a.3).min(b.1 + b.3);
    let overlap = bottom.saturating_sub(top);
    overlap as f32 / a.3.min(b.3).max(1) as f32
}

fn union_box(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let x0 = a.0.min(b.0);
    let y0 = a.1.min(b.1);
    let x1 = (a.0 + a.2).max(b.0 + b.2);
    let y1 = (a.1 + a.3).max(b.1 + b.3);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Walk the parent chain to the owning window frame and return its offset.
fn owning_frame_offset(
    node: &Node,
    index: &HashMap<&str, &Node>,
    offsets: &HashMap<String, Option<(i32, i32)>>,
) -> Option<(i32, i32)> {
    let mut current = node;
    for _ in 0..MAX_ANCESTOR_DEPTH {
        if let Some(offset) = offsets.get(current.path.as_str()) {
            return *offset;
        }
        current = index.get(current.parent.as_str())?;
    }
    None
}

/// Offset that turns a frame's subtree coordinates into screen coordinates,
/// `None` when the frame should not be hinted. Without compositor info (X11,
/// unqueryable Wayland compositors) every window is kept unshifted; with it,
/// only the active window is kept — skipping unmatched frames keeps
/// mispositioned hints off the screen.
fn frame_offset(
    extents: (i32, i32, i32, i32),
    name: &str,
    pid: Option<i32>,
    compositor: &CompositorSnapshot,
) -> Option<(i32, i32)> {
    if compositor.windows.is_empty() {
        return Some((0, 0));
    }
    let (fx, fy, fw, fh) = extents;
    let index = match_window(fw, fh, name, pid, &compositor.windows)?;
    if compositor.active.is_some_and(|active| active != index) {
        return None;
    }
    let window = &compositor.windows[index];
    Some((window.x - fx, window.y - fy))
}

/// Best compositor window for an AT-SPI frame. The application's pid is the
/// primary key; size within a small tolerance and longest title-prefix
/// overlap only disambiguate between one app's windows. When no window
/// carries the pid (sandboxed apps connect through a proxy process), the
/// size+title heuristic runs over all windows, and a size match is then
/// required — the only safety against shifting by an unrelated window. Within
/// a pid group it is merely preferred, since decoration handling can skew the
/// reported size.
fn match_window(
    fw: i32,
    fh: i32,
    name: &str,
    pid: Option<i32>,
    windows: &[WindowRect],
) -> Option<usize> {
    const TOLERANCE: i32 = 8;
    let same_pid: Vec<usize> = windows
        .iter()
        .enumerate()
        .filter(|(_, w)| pid.is_some() && w.pid == pid)
        .map(|(index, _)| index)
        .collect();
    if let [only] = same_pid[..] {
        return Some(only);
    }
    let (pool, require_size) = match same_pid.is_empty() {
        true => ((0..windows.len()).collect(), true),
        false => (same_pid, false),
    };
    let name = name.to_lowercase();
    let mut best: Option<(usize, (bool, usize))> = None;
    for index in pool {
        let window = &windows[index];
        let size_ok = (window.w - fw).abs() <= TOLERANCE && (window.h - fh).abs() <= TOLERANCE;
        if require_size && !size_ok {
            continue;
        }
        let title = window.title.to_lowercase();
        let title_prefix = title
            .chars()
            .zip(name.chars())
            .take_while(|(x, y)| x == y)
            .count();
        let score = (size_ok, title_prefix);
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

/// A node's live `name` property (for window frames: the title).
async fn live_name(a11y: &AccessibilityConnection, dest: &str, path: &str) -> Option<String> {
    accessible_at(a11y, dest, path)
        .await
        .ok()?
        .name()
        .await
        .ok()
}

async fn extents(
    a11y: &AccessibilityConnection,
    dest: &str,
    path: &str,
) -> Result<(i32, i32, i32, i32)> {
    let component = ComponentProxy::builder(a11y.connection())
        .destination(dest.to_owned())?
        .path(path.to_owned())?
        .build()
        .await?;
    Ok(component.get_extents(CoordType::Screen).await?)
}

/// Clamp a box to the screen; `None` for empty or fully offscreen boxes.
fn clamp_to_screen(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    screen_w: u32,
    screen_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(screen_w as i32);
    let y1 = (y + h).min(screen_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(x: i32, w: i32, h: i32, title: &str, pid: Option<i32>) -> WindowRect {
        WindowRect {
            x,
            y: 28,
            w,
            h,
            title: title.into(),
            pid,
        }
    }

    #[test]
    fn match_window_without_pid_requires_size_and_prefers_title() {
        let windows = vec![
            window(4, 1912, 1048, "github — zen browser", None),
            window(1000, 900, 1048, "notes", None),
        ];
        // Exact size match picks the right window even with a partial title.
        assert_eq!(
            match_window(1912, 1048, "github — zen browser", None, &windows),
            Some(0)
        );
        // No size within tolerance → no match.
        assert!(match_window(300, 200, "whatever", None, &windows).is_none());
    }

    #[test]
    fn pid_disambiguates_equal_size_tiled_windows() {
        // Two tiled halves: identical size, and the AT-SPI frame name shares
        // no prefix with either title — the heuristic alone is a coin toss.
        let windows = vec![
            window(0, 956, 1048, "inbox", Some(100)),
            window(964, 956, 1048, "scratch", Some(200)),
        ];
        assert_eq!(match_window(956, 1048, "", Some(200), &windows), Some(1));
        assert_eq!(match_window(956, 1048, "", Some(100), &windows), Some(0));
    }

    #[test]
    fn pid_match_survives_a_decoration_size_mismatch() {
        // Server-side decorations: the compositor's height includes a 30px
        // titlebar the AT-SPI frame knows nothing about. The size gate would
        // reject this window; the pid keeps it.
        let windows = vec![
            window(4, 1912, 1078, "editor", Some(100)),
            window(1000, 900, 1048, "notes", Some(200)),
        ];
        assert_eq!(
            match_window(1912, 1048, "editor", Some(100), &windows),
            Some(0)
        );
        // Without the pid, the same frame finds no window at all.
        assert_eq!(match_window(1912, 1048, "editor", None, &windows), None);
    }

    #[test]
    fn within_a_pid_group_size_then_title_break_the_tie() {
        // One app, two windows: size narrows first, title settles the rest.
        let windows = vec![
            window(0, 956, 1048, "main", Some(100)),
            window(964, 700, 500, "palette", Some(100)),
            window(0, 800, 600, "other app", Some(200)),
        ];
        assert_eq!(
            match_window(700, 500, "palette", Some(100), &windows),
            Some(1)
        );
        assert_eq!(
            match_window(956, 1048, "main", Some(100), &windows),
            Some(0)
        );
    }

    #[test]
    fn unmatched_pid_falls_back_to_the_size_heuristic() {
        // A sandboxed app's bus pid matches no compositor window; the size
        // gate still finds the right one.
        let windows = vec![
            window(4, 1912, 1048, "browser", Some(100)),
            window(1000, 900, 1048, "notes", Some(200)),
        ];
        assert_eq!(
            match_window(900, 1048, "notes", Some(99999), &windows),
            Some(1)
        );
    }

    #[test]
    fn clamp_drops_offscreen_and_empty() {
        assert_eq!(
            clamp_to_screen(10, 20, 30, 40, 1920, 1080),
            Some((10, 20, 30, 40))
        );
        // Straddling the left edge: clamp to the on-screen part.
        assert_eq!(
            clamp_to_screen(-20, 10, 50, 50, 1920, 1080),
            Some((0, 10, 30, 50))
        );
        // Fully off the left edge, or off the bottom-right → dropped.
        assert!(clamp_to_screen(-100, 0, 50, 50, 1920, 1080).is_none());
        assert!(clamp_to_screen(5000, 5000, 50, 50, 1920, 1080).is_none());
        // Zero area → dropped.
        assert!(clamp_to_screen(0, 0, 0, 10, 1920, 1080).is_none());
    }

    fn snapshot(active: Option<usize>) -> CompositorSnapshot {
        CompositorSnapshot {
            windows: vec![
                window(4, 1912, 1048, "browser", Some(100)),
                window(1000, 900, 1048, "notes", Some(200)),
            ],
            active,
        }
    }

    #[test]
    fn frame_offset_keeps_active_window_and_skips_the_rest() {
        let snap = snapshot(Some(0));
        // The frame matching the active window gets the position correction:
        // AT-SPI reports it pinned at (0,0), Hyprland knows it sits at (4,28).
        assert_eq!(
            frame_offset((0, 0, 1912, 1048), "browser", Some(100), &snap),
            Some((4, 28))
        );
        // A different window is skipped entirely.
        assert_eq!(
            frame_offset((0, 0, 900, 1048), "notes", Some(200), &snap),
            None
        );
        // An unmatchable frame (popup, odd size) is skipped, not mispositioned.
        assert_eq!(frame_offset((0, 0, 300, 200), "menu", None, &snap), None);
    }

    #[test]
    fn frame_offset_without_compositor_info_uses_raw_coordinates() {
        let snap = CompositorSnapshot {
            windows: Vec::new(),
            active: None,
        };
        // X11 / unknown compositor: every frame kept, coordinates unshifted.
        assert_eq!(
            frame_offset((40, 60, 800, 600), "any", None, &snap),
            Some((0, 0))
        );
    }

    fn node(path: &str, parent: &str, role: Role) -> Node {
        Node {
            path: path.into(),
            parent: parent.into(),
            role,
            states: StateSet::new(State::Showing),
            name: String::new(),
            children: 0,
        }
    }

    fn named_node(path: &str, parent: &str, role: Role, name: &str) -> Node {
        Node {
            name: name.into(),
            ..node(path, parent, role)
        }
    }

    #[test]
    fn owning_frame_offset_walks_to_the_right_frame() {
        let nodes = [
            node("/app", "/", Role::Application),
            node("/frame_a", "/app", Role::Frame),
            node("/panel", "/frame_a", Role::Panel),
            node("/button", "/panel", Role::Button),
            node("/frame_b", "/app", Role::Frame),
            node("/other_button", "/frame_b", Role::Button),
            node("/orphan", "/nowhere", Role::Button),
        ];
        let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();
        let offsets: HashMap<String, Option<(i32, i32)>> = [
            ("/frame_a".to_owned(), Some((10, 20))),
            ("/frame_b".to_owned(), None),
        ]
        .into();

        // Deep child resolves through intermediate containers to its frame.
        assert_eq!(
            owning_frame_offset(&nodes[3], &index, &offsets),
            Some((10, 20))
        );
        // A child of a skipped frame is skipped with it.
        assert_eq!(owning_frame_offset(&nodes[5], &index, &offsets), None);
        // A node with a broken parent chain finds no frame.
        assert_eq!(owning_frame_offset(&nodes[6], &index, &offsets), None);
    }

    #[test]
    fn owning_frame_offset_survives_parent_cycles() {
        let nodes = [
            node("/a", "/b", Role::Button),
            node("/b", "/a", Role::Panel),
        ];
        let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();
        let offsets: HashMap<String, Option<(i32, i32)>> = HashMap::new();
        assert_eq!(owning_frame_offset(&nodes[0], &index, &offsets), None);
    }

    #[test]
    fn table_row_with_actionable_children_is_not_a_target() {
        let nodes = [
            node("/table", "/frame", Role::Table),
            named_node("/row", "/table", Role::TableRow, "src initial commit"),
            named_node("/file", "/row", Role::Link, "src"),
            named_node("/commit", "/row", Role::Link, "initial commit"),
        ];
        let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();

        let rows = table_rows_with_actionable_descendants(&nodes, &index);
        assert!(rows.contains("/row"));
    }

    #[test]
    fn table_row_without_actionable_children_remains_a_target() {
        let nodes = [
            node("/table", "/frame", Role::Table),
            named_node("/row", "/table", Role::TableRow, "Documents"),
            node("/panel", "/row", Role::Panel),
        ];
        let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();

        let rows = table_rows_with_actionable_descendants(&nodes, &index);
        assert!(!rows.contains("/row"));
    }

    #[test]
    fn table_row_with_only_icon_child_remains_a_target() {
        let nodes = [
            node("/table", "/frame", Role::Table),
            named_node("/row", "/table", Role::TableRow, "src"),
            named_node("/icon", "/row", Role::Icon, "Directory"),
        ];
        let index: HashMap<&str, &Node> = nodes.iter().map(|n| (n.path.as_str(), n)).collect();

        let rows = table_rows_with_actionable_descendants(&nodes, &index);
        assert!(!rows.contains("/row"));
    }

    #[test]
    fn table_cells_merge_into_one_box_per_row() {
        // Thunar-style sidebar: each row is several cells (zero-width padding
        // cells are already dropped by clamping before merge).
        let cells = vec![
            // Row 1 at y=100.
            (2, 100, 12, 24),
            (16, 100, 24, 24),
            (42, 100, 130, 24),
            // Row 2 at y=126, slightly different heights.
            (2, 126, 12, 27),
            (16, 127, 24, 24),
            (42, 126, 130, 27),
        ];
        let rows = merge_cells_into_rows(cells);
        assert_eq!(rows, vec![(2, 100, 170, 24), (2, 126, 170, 27)]);
    }

    #[test]
    fn named_table_cells_keep_their_own_click_target() {
        let file_cell = named_node("/file", "/row", Role::TableCell, "src");
        let commit_cell = named_node("/commit", "/row", Role::TableCell, "initial commit");
        let mut unnamed = HashMap::new();
        let mut out = Vec::new();

        push_table_cell_candidate(&file_cell, (10, 100, 200, 24), &mut unnamed, &mut out);
        push_table_cell_candidate(&commit_cell, (600, 100, 300, 24), &mut unnamed, &mut out);

        assert!(unnamed.is_empty());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bbox, (10, 100, 200, 24));
        assert_eq!(out[1].bbox, (600, 100, 300, 24));
    }

    #[test]
    fn unnamed_table_cells_still_merge_by_row() {
        let icon = node("/icon", "/row", Role::TableCell);
        let text = node("/text", "/row", Role::TableCell);
        let mut unnamed = HashMap::new();
        let mut out = Vec::new();

        push_table_cell_candidate(&icon, (2, 100, 12, 24), &mut unnamed, &mut out);
        push_table_cell_candidate(&text, (16, 100, 130, 24), &mut unnamed, &mut out);

        assert!(out.is_empty());
        let rows = merge_cells_into_rows(unnamed.remove("/row").unwrap());
        assert_eq!(rows, vec![(2, 100, 144, 24)]);
    }

    #[test]
    fn single_cells_pass_through_unmerged() {
        let rows = merge_cells_into_rows(vec![(5, 10, 100, 20)]);
        assert_eq!(rows, vec![(5, 10, 100, 20)]);
        assert!(merge_cells_into_rows(Vec::new()).is_empty());
    }

    /// Exercises the full async collection against the live accessibility bus.
    /// Ignored by default since it needs a running AT-SPI bus and GUI apps:
    ///   cargo test -- --ignored --nocapture walks_accessibility_tree
    #[test]
    #[ignore = "requires a running AT-SPI bus and GUI apps"]
    fn walks_accessibility_tree() {
        let start = std::time::Instant::now();
        let (candidates, _) = futures_executor::block_on(collect_candidates(1920, 1080))
            .expect("accessibility collection should succeed");
        eprintln!(
            "atspi collect: {} candidates in {:?}",
            candidates.len(),
            start.elapsed()
        );
        for c in &candidates {
            eprintln!("  candidate bbox={:?}", c.bbox);
        }
    }
}
