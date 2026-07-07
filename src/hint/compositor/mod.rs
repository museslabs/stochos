//! Real window geometry from the compositor, for the AT-SPI detector.
//!
//! On Wayland a client cannot know its window's on-screen position (no standard
//! protocol exposes it), so AT-SPI coordinates need shifting by it. Each
//! compositor with its own IPC gets a provider file here, returning `Some` with
//! absolute-screen rectangles when detected, else `None`. With no provider,
//! [`snapshot`] is empty and coordinates are used unshifted (correct on X11).

mod hyprland;
mod sway;

/// A compositor-reported window rectangle, in absolute screen coordinates.
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
    /// Owning process, when the compositor reports one; the primary key for
    /// matching a window to its AT-SPI application.
    pub pid: Option<i32>,
}

#[derive(Default)]
pub struct CompositorSnapshot {
    /// Every mapped, visible window.
    pub windows: Vec<WindowRect>,
    /// Index into `windows` of the focused window, when the compositor
    /// reports one.
    pub active: Option<usize>,
}

impl CompositorSnapshot {
    pub fn active_window(&self) -> Option<&WindowRect> {
        self.windows.get(self.active?)
    }
}

/// Window geometry from the first provider whose compositor is running.
pub fn snapshot() -> CompositorSnapshot {
    hyprland::snapshot()
        .or_else(sway::snapshot)
        .unwrap_or_default()
}
