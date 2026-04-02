use std::io::Write;
use std::os::fd::AsFd;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::protocol::wl_pointer::{Axis, AxisSource};
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry,
        wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1},
};

use anyhow::{Context, Result};

use super::{Backend, KeyEvent};
use crate::config::{config, Key};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Wayland backend. Holds the event queue alongside the state so that
/// `Backend` methods can call `roundtrip` / `blocking_dispatch` freely
/// without borrowing conflicts.
pub struct WaylandBackend {
    state: WaylandState,
    eq: EventQueue<WaylandState>,
    qh: QueueHandle<WaylandState>,
}

impl WaylandBackend {
    pub fn new() -> Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();

        conn.display().get_registry(&qh, ());

        let mut state = WaylandState {
            conn,
            compositor: None,
            shm: None,
            surface: None,
            layer_shell: None,
            layer_surface: None,
            seat: None,
            vp_manager: None,
            vp: None,
            screen_w: 0,
            screen_h: 0,
            configured: false,
            pending_key: None,
            shift_held: false,
            current_output: None,
        };

        eq.roundtrip(&mut state).context("initial roundtrip")?;

        state.init_layer_surface(&qh);
        eq.roundtrip(&mut state)
            .context("roundtrip after layer surface")?;

        while !state.configured {
            eq.blocking_dispatch(&mut state)
                .context("waiting for configure")?;
        }

        Ok(WaylandBackend { state, eq, qh })
    }

    fn ensure_vp(&mut self) {
        if self.state.vp.is_some() {
            return;
        }
        if let Some(manager) = &self.state.vp_manager {
            self.state.vp = Some(manager.create_virtual_pointer_with_output(
                self.state.seat.as_ref(),
                self.state.current_output.as_ref(),
                &self.qh,
                (),
            ));
        }
    }

    fn scroll(&mut self, axis: Axis, value: f64, discrete: i32) -> Result<()> {
        self.teardown_surface()?;

        if let Some(vp) = &self.state.vp {
            // axis_source identifies this as a wheel, not touchpad continuous scroll.
            // axis_discrete sends both the continuous value and the notch count —
            // bare axis() alone is often ignored by compositors expecting wheel events.
            vp.axis_source(AxisSource::Wheel);
            vp.axis_discrete(timestamp(), axis, value, discrete);
            vp.frame();
        }
        self.state.conn.flush().context("flush after axis")?;

        self.reopen()
    }

    fn teardown_surface(&mut self) -> Result<()> {
        if let Some(ls) = self.state.layer_surface.take() {
            ls.destroy();
        }
        if let Some(s) = self.state.surface.take() {
            s.destroy();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after surface destroy")?;
        Ok(())
    }
}

impl Backend for WaylandBackend {
    fn screen_size(&self) -> (u32, u32) {
        (self.state.screen_w, self.state.screen_h)
    }

    fn present(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<()> {
        let stride = width * 4;
        let mut file = tempfile::tempfile().context("create shm tempfile")?;
        file.write_all(pixels).context("write pixel buffer")?;

        let shm = self.state.shm.as_ref().context("wl_shm not available")?;
        let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &self.qh, ());
        let buf = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            &self.qh,
            (),
        );

        let surface = self
            .state
            .surface
            .as_ref()
            .context("wl_surface not available")?;
        surface.attach(Some(&buf), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        pool.destroy();
        Ok(())
    }

    fn move_mouse(&mut self, x: u32, y: u32) -> Result<()> {
        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x, y, self.state.screen_w, self.state.screen_h);
            vp.frame();
        }
        self.state.conn.flush().context("flush after move_mouse")?;
        Ok(())
    }

    /// Destroy the overlay so the compositor removes it from the surface stack,
    /// then re-send motion to trigger a focus update, then click.
    fn click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown_surface()?;

        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x, y, self.state.screen_w, self.state.screen_h);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after motion")?;

        if let Some(vp) = &self.state.vp {
            let ts = timestamp();
            vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Pressed);
            vp.frame();
            vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Released);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after click")?;
        Ok(())
    }

    fn double_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown_surface()?;

        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x, y, self.state.screen_w, self.state.screen_h);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after motion")?;

        if let Some(vp) = &self.state.vp {
            for _ in 0..2 {
                let ts = timestamp();
                vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Pressed);
                vp.frame();
                vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Released);
                vp.frame();
            }
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after click")?;
        Ok(())
    }

    /// Destroy the overlay so the compositor removes it from the surface stack,
    /// then re-send motion to trigger a focus update, then right click.
    fn right_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.teardown_surface()?;

        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x, y, self.state.screen_w, self.state.screen_h);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after motion")?;

        if let Some(vp) = &self.state.vp {
            let ts = timestamp();
            vp.button(ts, BTN_RIGHT, wl_pointer::ButtonState::Pressed);
            vp.frame();
            vp.button(ts, BTN_RIGHT, wl_pointer::ButtonState::Released);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after click")?;
        Ok(())
    }

    fn scroll_up(&mut self) -> Result<()> {
        self.scroll(Axis::VerticalScroll, -15.0, -1)
    }

    fn scroll_down(&mut self) -> Result<()> {
        self.scroll(Axis::VerticalScroll, 15.0, 1)
    }

    fn scroll_left(&mut self) -> Result<()> {
        self.scroll(Axis::HorizontalScroll, -15.0, -1)
    }

    fn scroll_right(&mut self) -> Result<()> {
        self.scroll(Axis::HorizontalScroll, 15.0, 1)
    }

    fn drag_select(&mut self, x1: u32, y1: u32, x2: u32, y2: u32) -> Result<()> {
        self.teardown_surface()?;

        let (sw, sh) = (self.state.screen_w, self.state.screen_h);

        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x1, y1, sw, sh);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after motion to start")?;

        if let Some(vp) = &self.state.vp {
            vp.button(timestamp(), BTN_LEFT, wl_pointer::ButtonState::Pressed);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after press")?;

        if let Some(vp) = &self.state.vp {
            vp.motion_absolute(timestamp(), x2, y2, sw, sh);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after motion to end")?;

        if let Some(vp) = &self.state.vp {
            vp.button(timestamp(), BTN_LEFT, wl_pointer::ButtonState::Released);
            vp.frame();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after release")?;
        Ok(())
    }

    fn exit(&mut self) -> Result<()> {
        self.teardown_surface()
    }

    fn reopen(&mut self) -> Result<()> {
        self.state.configured = false;
        self.state.init_layer_surface(&self.qh);
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after reopen")?;
        while !self.state.configured {
            self.eq
                .blocking_dispatch(&mut self.state)
                .context("waiting for configure after reopen")?;
        }
        Ok(())
    }

    fn next_key(&mut self) -> Result<Option<KeyEvent>> {
        loop {
            if let Some(key) = self.state.pending_key.take() {
                self.ensure_vp();
                return Ok(Some(key));
            }
            if self.state.surface.is_none() {
                return Ok(None);
            }
            self.eq
                .blocking_dispatch(&mut self.state)
                .context("blocking_dispatch")?;
        }
    }
}

struct WaylandState {
    conn: Connection,

    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    surface: Option<wl_surface::WlSurface>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    seat: Option<wl_seat::WlSeat>,
    vp_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    vp: Option<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,

    screen_w: u32,
    screen_h: u32,
    configured: bool,
    pending_key: Option<KeyEvent>,
    shift_held: bool,
    current_output: Option<wl_output::WlOutput>,
}

impl WaylandState {
    fn init_layer_surface(&mut self, qh: &QueueHandle<Self>) {
        let compositor = self.compositor.as_ref().expect("wl_compositor missing");
        let layer_shell = self
            .layer_shell
            .as_ref()
            .expect("zwlr_layer_shell_v1 missing");

        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "stochos".to_string(),
            qh,
            (),
        );

        layer_surface.set_size(0, 0);
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Bottom
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer_surface.set_exclusive_zone(-1);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);

        surface.commit();

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "wl_shm" => {
                state.shm = Some(registry.bind(name, 1, qh, ()));
            }
            "wl_seat" => {
                state.seat = Some(registry.bind(name, version.min(7), qh, ()));
            }
            "wl_output" => {
                // Bind outputs so the compositor can reference them in wl_surface::enter.
                let _: wl_output::WlOutput = registry.bind(name, version.min(4), qh, ());
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.vp_manager = Some(registry.bind(name, version.min(2), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(key_state),
            ..
        } = event
        {
            match key_state {
                wl_keyboard::KeyState::Pressed => match key {
                    42 | 54 => state.shift_held = true,
                    _ => {
                        state.pending_key = keycode_to_key(key, state.shift_held).and_then(|k| {
                            config().keys.to_event(k).or(match k {
                                Key::Char(c) => Some(KeyEvent::Char(c)),
                                _ => None,
                            })
                        });
                    }
                },
                wl_keyboard::KeyState::Released => {
                    if key == 42 || key == 54 {
                        state.shift_held = false;
                    }
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                if width > 0 && height > 0 {
                    state.screen_w = width;
                    state.screen_h = height;
                }
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.pending_key = Some(KeyEvent::Close);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::Enter { output } = event {
            state.current_output = Some(output);
        }
    }
}

delegate_noop!(WaylandState: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandState: ignore wl_region::WlRegion);
delegate_noop!(WaylandState: ignore wl_output::WlOutput);
delegate_noop!(WaylandState: ignore wl_shm::WlShm);
delegate_noop!(WaylandState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WaylandState: ignore wl_buffer::WlBuffer);
delegate_noop!(WaylandState: ignore wl_pointer::WlPointer);
delegate_noop!(WaylandState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(WaylandState: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(WaylandState: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);

fn timestamp() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32
}

/// Maps a Wayland key code to a platform-agnostic Key.
fn keycode_to_key(kc: u32, shift_held: bool) -> Option<Key> {
    // Special (non-character) keys — checked first, unaffected by shift
    match kc {
        1 => return Some(Key::Escape),
        14 => return Some(Key::Backspace),
        15 => return Some(Key::Tab),
        28 => return Some(Key::Enter),
        57 => return Some(Key::Space),
        // Navigation
        102 => return Some(Key::Home),
        103 => return Some(Key::End),
        104 => return Some(Key::Up),
        105 => return Some(Key::Left),
        106 => return Some(Key::Right),
        107 => return Some(Key::PageUp),
        108 => return Some(Key::PageDown),
        109 => return Some(Key::Down),
        110 => return Some(Key::Insert),
        111 => return Some(Key::Delete),
        // Function keys
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
        // Lock / toggle
        58 => return Some(Key::CapsLock),
        69 => return Some(Key::NumLock),
        70 => return Some(Key::ScrollLock),
        // System
        99 => return Some(Key::PrintScreen),
        119 => return Some(Key::Pause),
        127 => return Some(Key::ContextMenu),
        // Numpad
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

    // Character keys — shift changes the produced character
    let ch = if shift_held {
        match kc {
            // Shifted digits → symbols
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
            // Shifted punctuation
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
            // Shifted letters → uppercase
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
            // Digits
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
            // Punctuation
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
            // Letters
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
