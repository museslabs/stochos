//! Hyprland provider: window geometry over its Unix IPC socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::{CompositorSnapshot, WindowRect};

pub fn snapshot() -> Option<CompositorSnapshot> {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let clients = clients_from_json(&ipc_json("clients")?, true);
    // `clients` and `activewindow` are separate IPC calls, so volatile fields
    // (the title, above all) can change in between; the stable address is what
    // identifies the active window in the list.
    let active = ipc_json("activewindow")
        .map(|json| clients_from_json(&json, false))
        .and_then(|mut active| (!active.is_empty()).then(|| active.remove(0)))
        .and_then(|active| active_index(&clients, &active));
    Some(CompositorSnapshot {
        windows: clients.into_iter().map(window_rect).collect(),
        active,
    })
}

fn clients_from_json(json: &str, require_mapped: bool) -> Vec<HyprClient> {
    // `clients` returns an array, `activewindow` a single object.
    let clients: Vec<HyprClient> = serde_json::from_str(json)
        .or_else(|_| serde_json::from_str::<HyprClient>(json).map(|client| vec![client]))
        .unwrap_or_default();
    clients
        .into_iter()
        .filter(|c| (!require_mapped || c.mapped) && !c.hidden && c.size.0 > 0 && c.size.1 > 0)
        .collect()
}

fn active_index(clients: &[HyprClient], active: &HyprClient) -> Option<usize> {
    clients
        .iter()
        .position(|c| match (&c.address, &active.address) {
            (Some(a), Some(b)) => a == b,
            _ => c.at == active.at && c.size == active.size && c.title == active.title,
        })
}

fn window_rect(c: HyprClient) -> WindowRect {
    WindowRect {
        x: c.at.0,
        y: c.at.1,
        w: c.size.0,
        h: c.size.1,
        title: c.title,
        pid: c.pid.filter(|&pid| pid > 0),
    }
}

fn ipc_json(command: &str) -> Option<String> {
    let socket = socket_path()?;
    let mut stream = UnixStream::connect(socket).ok()?;
    // `j/` requests JSON; the suffix is the query name, as in hyprctl.
    stream.write_all(format!("j/{command}").as_bytes()).ok()?;
    let mut out = String::new();
    stream.read_to_string(&mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

fn socket_path() -> Option<PathBuf> {
    let sig = PathBuf::from(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
    let mut candidates = Vec::with_capacity(2);
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(runtime).join("hypr").join(&sig));
    }
    candidates.push(PathBuf::from("/tmp/hypr").join(&sig));
    candidates
        .into_iter()
        .map(|dir| dir.join(".socket.sock"))
        .find(|path| path.exists())
}

#[derive(serde::Deserialize)]
struct HyprClient {
    at: (i32, i32),
    size: (i32, i32),
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    pid: Option<i32>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clients_and_activewindow_payloads() {
        let clients = r#"[
            {"at": [4, 28], "size": [1912, 1048], "title": "browser", "mapped": true, "pid": 4242, "address": "0xabc"},
            {"at": [0, 0], "size": [800, 600], "title": "hidden", "mapped": true, "hidden": true},
            {"at": [0, 0], "size": [800, 600], "title": "unmapped"}
        ]"#;
        let clients = clients_from_json(clients, true);
        assert_eq!(clients.len(), 1);
        let window = window_rect(clients.into_iter().next().unwrap());
        assert_eq!(
            (window.x, window.y, window.w, window.h),
            (4, 28, 1912, 1048)
        );
        assert_eq!(window.title, "browser");
        assert_eq!(window.pid, Some(4242));

        let active = r#"{"at": [966, 28], "size": [950, 1048], "title": "term"}"#;
        let clients = clients_from_json(active, false);
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].title, "term");
    }

    #[test]
    fn active_is_matched_by_address_despite_a_title_change() {
        // `clients` and `activewindow` are separate IPC calls; the browser's
        // title changed in between, but the address pins down the window.
        let clients = clients_from_json(
            r#"[
                {"at": [4, 28], "size": [956, 1048], "title": "old title", "mapped": true, "address": "0xa"},
                {"at": [964, 28], "size": [956, 1048], "title": "notes", "mapped": true, "address": "0xb"}
            ]"#,
            true,
        );
        let active = clients_from_json(
            r#"{"at": [4, 28], "size": [956, 1048], "title": "new title", "address": "0xa"}"#,
            false,
        )
        .remove(0);
        assert_eq!(active_index(&clients, &active), Some(0));
    }
}
