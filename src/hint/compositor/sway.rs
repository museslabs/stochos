//! Sway provider: window geometry over the i3 IPC socket (`$SWAYSOCK`).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use super::{CompositorSnapshot, WindowRect};

pub fn snapshot() -> Option<CompositorSnapshot> {
    let socket = std::env::var_os("SWAYSOCK")?;
    let tree = get_tree(&socket)?;
    let mut snapshot = CompositorSnapshot::default();
    collect_windows(&tree, &mut snapshot);
    Some(snapshot)
}

/// One i3-IPC `GET_TREE` request/reply. The wire format is `"i3-ipc"` +
/// payload length (u32 LE) + message type (u32 LE) + JSON payload.
fn get_tree(socket: &std::ffi::OsStr) -> Option<serde_json::Value> {
    const MAGIC: &[u8] = b"i3-ipc";
    const GET_TREE: u32 = 4;

    let mut stream = UnixStream::connect(socket).ok()?;
    let mut request = Vec::with_capacity(14);
    request.extend_from_slice(MAGIC);
    request.extend_from_slice(&0u32.to_le_bytes());
    request.extend_from_slice(&GET_TREE.to_le_bytes());
    stream.write_all(&request).ok()?;

    let mut header = [0u8; 14];
    stream.read_exact(&mut header).ok()?;
    if &header[..6] != MAGIC {
        return None;
    }
    let len = u32::from_le_bytes(header[6..10].try_into().ok()?) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    serde_json::from_slice(&payload).ok()
}

/// Walk the layout tree collecting visible application windows. A window node
/// carries a `pid`; its on-screen content rect is the node `rect` shifted by
/// `window_rect` (which excludes borders and title bar).
fn collect_windows(node: &serde_json::Value, out: &mut CompositorSnapshot) {
    if node.get("pid").is_some_and(|pid| !pid.is_null())
        && node.get("visible").and_then(|v| v.as_bool()) == Some(true)
    {
        if let Some(window) = window_rect_of(node) {
            if node.get("focused").and_then(|v| v.as_bool()) == Some(true) {
                out.active = Some(out.windows.len());
            }
            out.windows.push(window);
        }
    }
    for key in ["nodes", "floating_nodes"] {
        for child in node
            .get(key)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            collect_windows(child, out);
        }
    }
}

fn window_rect_of(node: &serde_json::Value) -> Option<WindowRect> {
    let field = |rect: &serde_json::Value, key: &str| rect.get(key)?.as_i64().map(|v| v as i32);
    let rect = node.get("rect")?;
    let content = node.get("window_rect")?;
    let (w, h) = (field(content, "width")?, field(content, "height")?);
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(WindowRect {
        x: field(rect, "x")? + field(content, "x")?,
        y: field(rect, "y")? + field(content, "y")?,
        w,
        h,
        title: node
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        pid: node
            .get("pid")
            .and_then(|v| v.as_i64())
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|&pid| pid > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_visible_windows_from_a_tree() {
        let tree: serde_json::Value = serde_json::from_str(
            r#"{
              "type": "root", "nodes": [{
                "type": "output", "nodes": [{
                  "type": "workspace", "nodes": [
                    {
                      "type": "con", "pid": 100, "visible": true, "focused": false,
                      "name": "editor",
                      "rect": {"x": 0, "y": 23, "width": 960, "height": 1057},
                      "window_rect": {"x": 2, "y": 2, "width": 956, "height": 1053},
                      "nodes": []
                    },
                    {
                      "type": "con", "pid": 200, "visible": true, "focused": true,
                      "name": "browser",
                      "rect": {"x": 960, "y": 23, "width": 960, "height": 1057},
                      "window_rect": {"x": 0, "y": 0, "width": 960, "height": 1057},
                      "nodes": []
                    },
                    {
                      "type": "con", "pid": 300, "visible": false, "focused": false,
                      "name": "scratchpad",
                      "rect": {"x": 0, "y": 0, "width": 800, "height": 600},
                      "window_rect": {"x": 0, "y": 0, "width": 800, "height": 600},
                      "nodes": []
                    }
                  ],
                  "floating_nodes": [{
                    "type": "floating_con", "pid": 400, "visible": true, "focused": false,
                    "name": "popup",
                    "rect": {"x": 500, "y": 300, "width": 404, "height": 304},
                    "window_rect": {"x": 2, "y": 2, "width": 400, "height": 300},
                    "nodes": []
                  }]
                }]
              }]
            }"#,
        )
        .unwrap();

        let mut snapshot = CompositorSnapshot::default();
        collect_windows(&tree, &mut snapshot);

        let summary: Vec<_> = snapshot
            .windows
            .iter()
            .map(|w| (w.title.as_str(), w.x, w.y, w.w, w.h, w.pid))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("editor", 2, 25, 956, 1053, Some(100)),
                ("browser", 960, 23, 960, 1057, Some(200)),
                ("popup", 502, 302, 400, 300, Some(400)),
            ]
        );
        let active = snapshot.active_window().expect("focused window");
        assert_eq!(active.title, "browser");
        assert_eq!((active.x, active.y), (960, 23));
    }

    #[test]
    fn container_without_pid_is_not_a_window() {
        let tree: serde_json::Value = serde_json::from_str(
            r#"{"type": "workspace", "visible": true,
                "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
                "window_rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
                "nodes": []}"#,
        )
        .unwrap();
        let mut snapshot = CompositorSnapshot::default();
        collect_windows(&tree, &mut snapshot);
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.active.is_none());
    }
}
