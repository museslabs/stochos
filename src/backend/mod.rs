use anyhow::Result;

/// A decoded key event, platform-agnostic.
pub enum KeyEvent {
    Char(char),
    Click,
    DoubleClick,
    RightClick,
    Close,
    Undo,
    MacroMenu,
    MacroRecord,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
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
    fn click(&mut self, x: u32, y: u32) -> Result<()>;

    /// Tear down the overlay, double click at (x, y), then return.
    fn double_click(&mut self, x: u32, y: u32) -> Result<()>;

    /// Tear down the overlay, right click at (x, y), then return.
    fn right_click(&mut self, x: u32, y: u32) -> Result<()>;

    /// Tear down the overlay, drag from (x1,y1) to (x2,y2), then return.
    fn drag_select(&mut self, x1: u32, y1: u32, x2: u32, y2: u32) -> Result<()>;

    /// Scroll the mouse wheel up.
    fn scroll_up(&mut self) -> Result<()>;

    /// Scroll the mouse wheel down.
    fn scroll_down(&mut self) -> Result<()>;

    /// Scroll the mouse wheel left (horizontal scroll).
    fn scroll_left(&mut self) -> Result<()>;

    /// Scroll the mouse wheel right (horizontal scroll).
    fn scroll_right(&mut self) -> Result<()>;

    /// Close the overlay without clicking.
    fn exit(&mut self) -> Result<()>;

    /// Block until the next key event. Returns None when the overlay closes.
    fn next_key(&mut self) -> Result<Option<KeyEvent>>;

    /// Recreate the overlay after a teardown (for macro recording).
    fn reopen(&mut self) -> Result<()>;
}

pub mod wayland;
