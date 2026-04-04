use anyhow::{Context, Result};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use super::{Backend, KeyEvent};
use crate::config::{config, Key};

const BTN_LEFT: u8 = 1;
const BTN_RIGHT: u8 = 3;
const BTN_SCROLL_UP: u8 = 4;
const BTN_SCROLL_DOWN: u8 = 5;
const BTN_SCROLL_LEFT: u8 = 6;
const BTN_SCROLL_RIGHT: u8 = 7;

pub struct X11Backend {
    conn: RustConnection,
    window: Window,
    gc: Gcontext,
    root: Window,
    screen_w: u32,
    screen_h: u32,
    depth: u8,
    mapped: bool,
    shift_held: bool,
    /// Screenshot of the desktop captured before mapping the overlay.
    /// Used to alpha-blend the overlay on top (X11 has no compositor).
    background: Vec<u8>,
}

impl X11Backend {
    pub fn new() -> Result<Self> {
        let (conn, screen_num) = RustConnection::connect(None).context("connect to X11 display")?;

        // Verify XTest extension is available
        conn.xtest_get_version(2, 2)
            .context("XTest extension not available")?
            .reply()
            .context("XTest extension query failed")?;

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let screen_w = screen.width_in_pixels as u32;
        let screen_h = screen.height_in_pixels as u32;
        let depth = screen.root_depth;

        // Capture the desktop before we cover it with the overlay.
        let background = capture_root(&conn, root, screen_w, screen_h)?;

        let window = conn.generate_id()?;
        let gc = conn.generate_id()?;

        conn.create_window(
            depth,
            window,
            root,
            0,
            0,
            screen_w as u16,
            screen_h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            0, // CopyFromParent visual
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE)
                .background_pixel(0),
        )
        .context("create window")?;

        conn.create_gc(gc, window, &CreateGCAux::new())
            .context("create GC")?;

        // Map the window and grab keyboard
        conn.map_window(window).context("map window")?;

        // Raise above everything
        conn.configure_window(
            window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;

        conn.flush().context("flush after map")?;

        // Grab keyboard so all keys go to our overlay
        let grab = conn
            .grab_keyboard(true, window, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
            .context("grab keyboard request")?
            .reply()
            .context("grab keyboard reply")?;

        if grab.status != GrabStatus::SUCCESS {
            anyhow::bail!("failed to grab keyboard: {:?}", grab.status);
        }

        Ok(X11Backend {
            conn,
            window,
            gc,
            root,
            screen_w,
            screen_h,
            depth,
            mapped: true,
            shift_held: false,
            background,
        })
    }

    fn teardown(&mut self) -> Result<()> {
        if self.mapped {
            self.conn
                .ungrab_keyboard(CURRENT_TIME)
                .context("ungrab keyboard")?;
            self.conn.unmap_window(self.window).context("unmap")?;
            self.conn.flush().context("flush after teardown")?;
            // Sync to ensure the server has processed the unmap before we
            // simulate input — otherwise the overlay may intercept our own
            // fake events.
            self.conn.sync().context("sync after teardown")?;
            self.mapped = false;
        }
        Ok(())
    }

    fn warp_and_sync(&self, x: u32, y: u32) -> Result<()> {
        self.conn
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x as i16, y as i16)
            .context("warp pointer")?;
        self.conn.flush().context("flush after warp")?;
        self.conn.sync().context("sync after warp")?;
        Ok(())
    }

    fn fake_button_click(&self, button: u8) -> Result<()> {
        xtest::fake_input(
            &self.conn,
            BUTTON_PRESS_EVENT,
            button,
            CURRENT_TIME,
            self.root,
            0,
            0,
            0,
        )
        .context("fake button press")?;
        xtest::fake_input(
            &self.conn,
            BUTTON_RELEASE_EVENT,
            button,
            CURRENT_TIME,
            self.root,
            0,
            0,
            0,
        )
        .context("fake button release")?;
        self.conn.flush().context("flush after click")?;
        self.conn.sync().context("sync after click")?;
        Ok(())
    }

    fn scroll(&mut self, button: u8) -> Result<()> {
        self.teardown()?;
        self.fake_button_click(button)?;
        // Give the underlying app time to process the scroll and redraw
        // before we recapture the background.
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.reopen()
    }
}

impl Backend for X11Backend {
    fn screen_size(&self) -> (u32, u32) {
        (self.screen_w, self.screen_h)
    }

    fn present(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<()> {
        // Alpha-blend overlay pixels over the captured desktop background.
        // X11 without a compositor cannot blend for us.
        // Pixel format: BGRA in memory (little-endian ARGB8888).
        let mut composited = self.background.clone();
        for i in (0..composited.len()).step_by(4) {
            let a = pixels[i + 3] as u32;
            if a == 255 {
                composited[i] = pixels[i];
                composited[i + 1] = pixels[i + 1];
                composited[i + 2] = pixels[i + 2];
            } else if a > 0 {
                let inv = 255 - a;
                composited[i] = ((pixels[i] as u32 * a + composited[i] as u32 * inv) / 255) as u8;
                composited[i + 1] =
                    ((pixels[i + 1] as u32 * a + composited[i + 1] as u32 * inv) / 255) as u8;
                composited[i + 2] =
                    ((pixels[i + 2] as u32 * a + composited[i + 2] as u32 * inv) / 255) as u8;
            }
            // a == 0: keep background as-is
        }

        // X11 has a maximum request size, so split into row bands.
        let stride = (width * 4) as usize;
        let max_data = self.conn.maximum_request_bytes() - 32;
        let rows_per_chunk = (max_data / stride).max(1) as u32;

        let mut y = 0u32;
        while y < height {
            let chunk_h = rows_per_chunk.min(height - y);
            let start = (y as usize) * stride;
            let end = start + (chunk_h as usize) * stride;
            self.conn
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    self.window,
                    self.gc,
                    width as u16,
                    chunk_h as u16,
                    0,
                    y as i16,
                    0,
                    self.depth,
                    &composited[start..end],
                )
                .context("put_image")?;
            y += chunk_h;
        }
        self.conn.flush().context("flush after present")?;
        Ok(())
    }

    fn move_mouse(&mut self, x: u32, y: u32) -> Result<()> {
        self.warp_and_sync(x, y)
    }

    fn click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown()?;
        self.warp_and_sync(x, y)?;
        self.fake_button_click(BTN_LEFT)
    }

    fn double_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown()?;
        self.warp_and_sync(x, y)?;
        self.fake_button_click(BTN_LEFT)?;
        self.fake_button_click(BTN_LEFT)
    }

    fn right_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown()?;
        self.warp_and_sync(x, y)?;
        self.fake_button_click(BTN_RIGHT)
    }

    fn drag_select(&mut self, x1: u32, y1: u32, x2: u32, y2: u32) -> Result<()> {
        self.teardown()?;
        self.warp_and_sync(x1, y1)?;

        xtest::fake_input(
            &self.conn,
            BUTTON_PRESS_EVENT,
            BTN_LEFT,
            CURRENT_TIME,
            self.root,
            0,
            0,
            0,
        )
        .context("fake drag press")?;
        self.conn.flush()?;
        self.conn.sync()?;

        xtest::fake_input(
            &self.conn,
            MOTION_NOTIFY_EVENT,
            0,
            CURRENT_TIME,
            self.root,
            x2 as i16,
            y2 as i16,
            0,
        )
        .context("fake drag motion")?;
        self.conn.flush()?;
        self.conn.sync()?;

        xtest::fake_input(
            &self.conn,
            BUTTON_RELEASE_EVENT,
            BTN_LEFT,
            CURRENT_TIME,
            self.root,
            0,
            0,
            0,
        )
        .context("fake drag release")?;
        self.conn.flush()?;
        self.conn.sync()?;
        Ok(())
    }

    fn scroll_up(&mut self) -> Result<()> {
        self.scroll(BTN_SCROLL_UP)
    }

    fn scroll_down(&mut self) -> Result<()> {
        self.scroll(BTN_SCROLL_DOWN)
    }

    fn scroll_left(&mut self) -> Result<()> {
        self.scroll(BTN_SCROLL_LEFT)
    }

    fn scroll_right(&mut self) -> Result<()> {
        self.scroll(BTN_SCROLL_RIGHT)
    }

    fn exit(&mut self) -> Result<()> {
        self.teardown()
    }

    fn next_key(&mut self) -> Result<Option<KeyEvent>> {
        if !self.mapped {
            return Ok(None);
        }

        loop {
            let event = self.conn.wait_for_event().context("wait for event")?;
            match event {
                Event::KeyPress(ev) => {
                    let keycode = ev.detail;
                    // Shift keys (left=50, right=62 in X11 keycodes)
                    if keycode == 50 || keycode == 62 {
                        self.shift_held = true;
                        continue;
                    }
                    // X11 keycodes are evdev + 8
                    let evdev_kc = (keycode as u32).wrapping_sub(8);
                    if let Some(key_event) =
                        keycode_to_key(evdev_kc, self.shift_held).and_then(|k| {
                            config().keys.to_event(k).or(match k {
                                Key::Char(c) => Some(KeyEvent::Char(c)),
                                _ => None,
                            })
                        })
                    {
                        return Ok(Some(key_event));
                    }
                }
                Event::KeyRelease(ev) => {
                    let keycode = ev.detail;
                    if keycode == 50 || keycode == 62 {
                        self.shift_held = false;
                    }
                }
                _ => {}
            }
        }
    }

    fn reopen(&mut self) -> Result<()> {
        if self.mapped {
            return Ok(());
        }
        // Re-capture desktop since it may have changed after our action.
        self.background = capture_root(&self.conn, self.root, self.screen_w, self.screen_h)?;
        self.conn.map_window(self.window).context("remap window")?;
        self.conn
            .configure_window(
                self.window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .context("raise window")?;
        self.conn.flush().context("flush after remap")?;

        let grab = self
            .conn
            .grab_keyboard(
                true,
                self.window,
                CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .context("regrab keyboard")?
            .reply()
            .context("regrab keyboard reply")?;

        if grab.status != GrabStatus::SUCCESS {
            anyhow::bail!("failed to regrab keyboard: {:?}", grab.status);
        }

        self.mapped = true;
        Ok(())
    }
}

/// Capture the root window contents as a BGRA pixel buffer.
fn capture_root(conn: &RustConnection, root: Window, w: u32, h: u32) -> Result<Vec<u8>> {
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w as u16, h as u16, !0)
        .context("get_image on root")?
        .reply()
        .context("get_image reply")?;
    Ok(reply.data)
}

/// Reuse the same evdev keycode → Key mapping as the Wayland backend.
/// X11 keycodes are evdev + 8, so callers subtract 8 before calling this.
fn keycode_to_key(kc: u32, shift_held: bool) -> Option<Key> {
    match kc {
        1 => return Some(Key::Escape),
        14 => return Some(Key::Backspace),
        15 => return Some(Key::Tab),
        28 => return Some(Key::Enter),
        57 => return Some(Key::Space),
        102 => return Some(Key::Home),
        103 => return Some(Key::Up),
        104 => return Some(Key::PageUp),
        105 => return Some(Key::Left),
        106 => return Some(Key::Right),
        107 => return Some(Key::End),
        108 => return Some(Key::Down),
        109 => return Some(Key::PageDown),
        110 => return Some(Key::Insert),
        111 => return Some(Key::Delete),
        59 => return Some(Key::F1),
        60 => return Some(Key::F2),
        61 => return Some(Key::F3),
        62 => return Some(Key::F4),
        63 => return Some(Key::F5),
        64 => return Some(Key::F6),
        65 => return Some(Key::F7),
        66 => return Some(Key::F8),
        67 => return Some(Key::F9),
        68 => return Some(Key::F10),
        87 => return Some(Key::F11),
        88 => return Some(Key::F12),
        58 => return Some(Key::CapsLock),
        69 => return Some(Key::NumLock),
        70 => return Some(Key::ScrollLock),
        99 => return Some(Key::PrintScreen),
        119 => return Some(Key::Pause),
        127 => return Some(Key::ContextMenu),
        82 => return Some(Key::NumPad0),
        79 => return Some(Key::NumPad1),
        80 => return Some(Key::NumPad2),
        81 => return Some(Key::NumPad3),
        75 => return Some(Key::NumPad4),
        76 => return Some(Key::NumPad5),
        77 => return Some(Key::NumPad6),
        71 => return Some(Key::NumPad7),
        72 => return Some(Key::NumPad8),
        73 => return Some(Key::NumPad9),
        78 => return Some(Key::NumPadAdd),
        74 => return Some(Key::NumPadSubtract),
        55 => return Some(Key::NumPadMultiply),
        98 => return Some(Key::NumPadDivide),
        83 => return Some(Key::NumPadDecimal),
        96 => return Some(Key::NumPadEnter),
        _ => {}
    }

    let ch = if shift_held {
        match kc {
            2 => '!',
            3 => '@',
            4 => '#',
            5 => '$',
            6 => '%',
            7 => '^',
            8 => '&',
            9 => '*',
            10 => '(',
            11 => ')',
            12 => '_',
            13 => '+',
            26 => '{',
            27 => '}',
            43 => '|',
            39 => ':',
            40 => '"',
            51 => '<',
            52 => '>',
            53 => '?',
            41 => '~',
            16 => 'Q',
            17 => 'W',
            18 => 'E',
            19 => 'R',
            20 => 'T',
            21 => 'Y',
            22 => 'U',
            23 => 'I',
            24 => 'O',
            25 => 'P',
            30 => 'A',
            31 => 'S',
            32 => 'D',
            33 => 'F',
            34 => 'G',
            35 => 'H',
            36 => 'J',
            37 => 'K',
            38 => 'L',
            44 => 'Z',
            45 => 'X',
            46 => 'C',
            47 => 'V',
            48 => 'B',
            49 => 'N',
            50 => 'M',
            _ => return None,
        }
    } else {
        match kc {
            2 => '1',
            3 => '2',
            4 => '3',
            5 => '4',
            6 => '5',
            7 => '6',
            8 => '7',
            9 => '8',
            10 => '9',
            11 => '0',
            12 => '-',
            13 => '=',
            26 => '[',
            27 => ']',
            43 => '\\',
            39 => ';',
            40 => '\'',
            51 => ',',
            52 => '.',
            53 => '/',
            41 => '`',
            16 => 'q',
            17 => 'w',
            18 => 'e',
            19 => 'r',
            20 => 't',
            21 => 'y',
            22 => 'u',
            23 => 'i',
            24 => 'o',
            25 => 'p',
            30 => 'a',
            31 => 's',
            32 => 'd',
            33 => 'f',
            34 => 'g',
            35 => 'h',
            36 => 'j',
            37 => 'k',
            38 => 'l',
            44 => 'z',
            45 => 'x',
            46 => 'c',
            47 => 'v',
            48 => 'b',
            49 => 'n',
            50 => 'm',
            _ => return None,
        }
    };
    Some(Key::Char(ch))
}
