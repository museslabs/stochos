mod app;
mod backend;
mod cli;
mod config;
mod input;
mod macro_store;
mod mode;
mod render;

use backend::Backend;
use clap::Parser;
use cli::{Action, Invocation};
use macro_store::{MacroEntry, MacroStore};
use nix::fcntl::{Flock, FlockArg};
use std::fs::OpenOptions;

struct LockGuard {
    _lock: Flock<std::fs::File>, // RAII → lock held for lifetime
}

fn acquire_lock(allow_multiple: bool) -> anyhow::Result<Option<LockGuard>> {
    if allow_multiple {
        return Ok(None);
    }

    let lock_path = "/tmp/stochos.lock";

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(lock_path)?;

    let lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => lock,

        Err((_, nix::errno::Errno::EWOULDBLOCK)) => {
            eprintln!("App already running (use --allow-multiple to override)");
            std::process::exit(1);
        }

        Err((_, e)) => return Err(anyhow::anyhow!(e)),
    };

    Ok(Some(LockGuard { _lock: lock }))
}

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();
    let inv = args.invocation();

    match &inv {
        // These touch neither the config nor a display backend, so handle them
        // up front without taking the single-instance lock.
        Invocation::PrintConfig => {
            print!("{}", config::Config::default_toml()?);
            return Ok(());
        }
        Invocation::ListMacros => {
            list_macros(&MacroStore::load_strict()?);
            return Ok(());
        }
        // Resolve the macro before spinning up a backend so a bad name fails
        // fast, without flashing the overlay or prompting for permissions.
        Invocation::Macro(query) => {
            resolve_macro(&MacroStore::load(), query)?;
        }
        Invocation::Overlay(_) | Invocation::Action(_) => {}
    }

    let _lock = acquire_lock(args.allow_multiple)?; // keep lock alive
    config::init();
    run(inv)
}

/// Construct the backend for the current display server and dispatch to it.
fn run(inv: Invocation) -> anyhow::Result<()> {
    #[cfg(all(feature = "wayland", target_os = "linux"))]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(mut b) = backend::wayland::WaylandBackend::new() {
            return dispatch(&mut b, inv);
        }
    }

    #[cfg(all(feature = "x11", target_os = "linux"))]
    if std::env::var_os("DISPLAY").is_some() {
        let mut b = backend::x11::X11Backend::new()?;
        return dispatch(&mut b, inv);
    }

    #[cfg(target_os = "macos")]
    {
        let mut b = backend::macos::MacosBackend::new()?;
        return dispatch(&mut b, inv);
    }

    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("no display server found (need WAYLAND_DISPLAY or DISPLAY)")
}

fn dispatch<B: Backend>(backend: &mut B, inv: Invocation) -> anyhow::Result<()> {
    match inv {
        Invocation::Overlay(mode) => app::run(backend, mode),
        Invocation::Action(action) => run_action(backend, action),
        Invocation::Macro(query) => run_macro(backend, &query),
        // Handled in main() before any backend is created.
        Invocation::PrintConfig | Invocation::ListMacros => unreachable!(),
    }
}

/// Synthesize a single pointer action at the current cursor position, then exit.
/// No grid is drawn; clicks land wherever the cursor already is.
fn run_action<B: Backend>(backend: &mut B, action: Action) -> anyhow::Result<()> {
    match action {
        Action::Click => {
            let (x, y) = backend.mouse_pos()?;
            backend.click(x, y)?;
        }
        Action::DoubleClick => {
            let (x, y) = backend.mouse_pos()?;
            backend.double_click(x, y)?;
        }
        Action::RightClick => {
            let (x, y) = backend.mouse_pos()?;
            backend.right_click(x, y)?;
        }
        Action::ScrollUp => backend.scroll_up()?,
        Action::ScrollDown => backend.scroll_down()?,
        Action::ScrollLeft => backend.scroll_left()?,
        Action::ScrollRight => backend.scroll_right()?,
    }
    backend.exit()
}

/// Replay a saved macro by name or bind key, then exit. Macros carry their own
/// resolution-independent targets, so no interactive selection is needed.
fn run_macro<B: Backend>(backend: &mut B, query: &str) -> anyhow::Result<()> {
    let store = MacroStore::load();
    let entry = resolve_macro(&store, query)?;
    let (w, h) = backend.screen_size();
    mode::replay_macro(&entry.actions, w, h, backend)?;
    backend.exit()
}

/// A single-character query matching a bind key wins; otherwise fall back to a
/// case-insensitive name match.
fn resolve_macro<'a>(store: &'a MacroStore, query: &str) -> anyhow::Result<&'a MacroEntry> {
    let mut chars = query.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some(entry) = store.find_by_key(c) {
            return Ok(entry);
        }
    }
    store
        .macros
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(query))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no macro matching '{query}' (by name or bind key); run --list-macros to see saved macros"
            )
        })
}

fn list_macros(store: &MacroStore) {
    if store.macros.is_empty() {
        println!("No macros saved.");
        return;
    }
    for m in &store.macros {
        let bind = m
            .bind_key
            .map_or_else(|| "-".to_string(), |c| c.to_string());
        let n = m.actions.len();
        let plural = if n == 1 { "" } else { "s" };
        println!("{bind}\t{name}\t({n} action{plural})", name = m.name);
    }
}
