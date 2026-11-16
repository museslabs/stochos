//! Mango provider: window geometry through Mango's documented IPC socket.
//!
//! Mango exports the Unix socket path in `MANGO_INSTANCE_SIGNATURE` to
//! processes it launches. The IPC protocol accepts one newline-terminated
//! command and returns one JSON response. Querying the socket directly avoids
//! spawning the `mmsg` executable on every hint invocation.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use super::{CompositorSnapshot, WindowRect};

/// Bound time spent writing the request and reading Mango's IPC response.
///
/// The request is small and local to the compositor, so 500 ms is generous.
/// The standard library does not expose a configurable timeout for Unix-domain
/// `connect`; the read/write phases are bounded here, and Stochos also runs
/// detector collection behind its existing overall timeout.
const IPC_TIMEOUT: Duration = Duration::from_millis(500);

/// Read Mango's current client list, if this process belongs to a Mango session.
pub fn snapshot() -> Option<CompositorSnapshot> {
    let started = Instant::now();
    let socket_path = match std::env::var_os("MANGO_INSTANCE_SIGNATURE") {
        Some(path) => path,
        None => {
            debug("MANGO_INSTANCE_SIGNATURE is absent; not a Mango session");
            return None;
        }
    };

    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) => {
            debug(format!("could not connect to Mango IPC socket: {error}"));
            return None;
        }
    };

    if let Err(error) = stream.set_write_timeout(Some(IPC_TIMEOUT)) {
        debug(format!("could not set Mango IPC write timeout: {error}"));
        return None;
    }
    if let Err(error) = stream.set_read_timeout(Some(IPC_TIMEOUT)) {
        debug(format!("could not set Mango IPC read timeout: {error}"));
        return None;
    }

    // This is the same one-shot request syntax used by `mmsg get all-clients`.
    if let Err(error) = stream.write_all(b"get all-clients\n") {
        debug(format!("Mango IPC request failed: {error}"));
        return None;
    }

    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        debug(format!("reading Mango IPC response failed: {error}"));
        return None;
    }
    if response.is_empty() {
        debug("Mango IPC returned an empty response");
        return None;
    }

    let snapshot = parse_snapshot(&response);
    match &snapshot {
        Some(snapshot) => debug(format!(
            "parsed {} visible clients; focused client index: {:?}; IPC + parse: {:?}",
            snapshot.windows.len(),
            snapshot.active,
            started.elapsed()
        )),
        None => debug(format!(
            "Mango IPC response contained no usable clients; IPC + parse: {:?}",
            started.elapsed()
        )),
    }
    snapshot
}

fn debug(message: impl AsRef<str>) {
    if std::env::var_os("STOCHOS_MANGO_DEBUG").is_some() {
        eprintln!("[stochos:mango] {}", message.as_ref());
    }
}

/// Parse Mango's `{ "clients": [...] }` response. Accept a bare array too.
fn parse_snapshot(json: &str) -> Option<CompositorSnapshot> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let clients = value
        .as_array()
        .or_else(|| value.get("clients").and_then(serde_json::Value::as_array))?;

    let mut snapshot = CompositorSnapshot::default();
    for value in clients {
        let client = match serde_json::from_value::<MangoClient>(value.clone()) {
            Ok(client) => client,
            Err(_) => continue,
        };

        if !client.is_visible || client.is_minimized || client.width <= 0 || client.height <= 0 {
            continue;
        }

        let index = snapshot.windows.len();
        if client.is_focused && snapshot.active.is_none() {
            snapshot.active = Some(index);
        }

        snapshot.windows.push(WindowRect {
            x: client.x,
            y: client.y,
            w: client.width,
            h: client.height,
            title: client.title,
            pid: client.pid.filter(|&pid| pid > 0),
        });
    }

    if snapshot.windows.is_empty() {
        None
    } else {
        Some(snapshot)
    }
}

#[derive(serde::Deserialize)]
struct MangoClient {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    pid: Option<i32>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    is_visible: bool,
    #[serde(default)]
    is_minimized: bool,
    #[serde(default)]
    is_focused: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mango_clients_across_multiple_outputs() {
        let json = r#"{
            "clients": [
                {
                    "id": 8, "pid": 3435, "title": "foot",
                    "monitor": "DP-2", "is_visible": true,
                    "is_focused": true, "is_minimized": false,
                    "x": 5, "y": 35, "width": 1053, "height": 1040
                },
                {
                    "id": 2, "pid": 3586, "title": "Sonora",
                    "monitor": "DP-1", "is_visible": true,
                    "is_focused": false, "is_minimized": false,
                    "x": 1925, "y": 35, "width": 1910, "height": 1040
                }
            ]
        }"#;

        let snapshot = parse_snapshot(json).expect("visible clients should be parsed");
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.active, Some(0));
        assert_eq!(snapshot.windows[0].pid, Some(3435));
        assert_eq!(snapshot.windows[1].x, 1925);
        assert_eq!(snapshot.windows[1].title, "Sonora");
    }

    #[test]
    fn accepts_a_bare_client_array() {
        let json = r#"[
            {
                "pid": 123, "title": "app", "is_visible": true,
                "is_focused": true, "x": 10, "y": 20,
                "width": 800, "height": 600
            }
        ]"#;

        let snapshot = parse_snapshot(json).expect("bare array should be parsed");
        assert_eq!(snapshot.active, Some(0));
        assert_eq!(snapshot.windows[0].x, 10);
    }

    #[test]
    fn ignores_hidden_minimized_and_empty_clients() {
        let json = r#"{"clients":[
            {"is_visible":false,"x":0,"y":0,"width":10,"height":10},
            {"is_visible":true,"is_minimized":true,"x":0,"y":0,"width":10,"height":10},
            {"is_visible":true,"x":0,"y":0,"width":0,"height":10}
        ]}"#;

        assert!(parse_snapshot(json).is_none());
    }

    #[test]
    fn rejects_invalid_json_and_empty_client_lists() {
        assert!(parse_snapshot("not json").is_none());
        assert!(parse_snapshot(r#"{"clients":[]}"#).is_none());
    }
}
