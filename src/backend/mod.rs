use anyhow::Result;

/// A decoded key event, platform-agnostic.
pub enum KeyEvent {
    Char(u8),
    Space,
    Escape,
}

/// Platform backend — one implementation per OS/display-server.
///
/// `render.rs` produces a raw ARGB pixel buffer that every backend receives
/// unchanged via `present()`. All other methods are input/pointer control.
pub trait Backend {
    /// Screen dimensions in pixels.
    fn screen_size(&self) -> (u32, u32);

    /// Display a rendered ARGB8888 pixel buffer on the overlay.
    fn present(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<()>;

    /// Move the mouse pointer to an absolute position.
    fn move_mouse(&mut self, x: u32, y: u32) -> Result<()>;

    /// Tear down the overlay, click at (x, y), then return.
    /// Kept as one method because some backends (Wayland) need surface
    /// destruction to happen before the click lands on the right window.
    fn click_and_exit(&mut self, x: u32, y: u32) -> Result<()>;

    /// Close the overlay without clicking.
    fn exit(&mut self) -> Result<()>;

    /// Block until the next key event. Returns None when the overlay closes.
    fn next_key(&mut self) -> Result<Option<KeyEvent>>;
}

pub mod wayland;
