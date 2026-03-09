use std::io::Write;
use std::os::fd::AsFd;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1},
};

use anyhow::{Context, Result};

use super::{Backend, KeyEvent};
use crate::input::keycode_to_char;

const BTN_LEFT: u32 = 0x110;

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
            _keep: Vec::new(),
        };

        eq.roundtrip(&mut state).context("initial roundtrip")?;

        if let Some(manager) = state.vp_manager.clone() {
            state.vp = Some(manager.create_virtual_pointer(state.seat.as_ref(), &qh, ()));
        }

        state.init_layer_surface(&qh);
        eq.roundtrip(&mut state)
            .context("roundtrip after layer surface")?;

        while !state.configured {
            eq.blocking_dispatch(&mut state)
                .context("waiting for configure")?;
        }

        Ok(WaylandBackend { state, eq, qh })
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

        self.state._keep.push(buf);
        drop(pool);
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
    /// See CLAUDE.md for why the order and roundtrips are critical.
    fn click_and_exit(&mut self, x: u32, y: u32) -> Result<()> {
        if let Some(ls) = self.state.layer_surface.take() {
            ls.destroy();
        }
        if let Some(s) = self.state.surface.take() {
            s.destroy();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip after surface destroy")?;

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

    fn exit(&mut self) -> Result<()> {
        if let Some(ls) = self.state.layer_surface.take() {
            ls.destroy();
        }
        if let Some(s) = self.state.surface.take() {
            s.destroy();
        }
        self.eq
            .roundtrip(&mut self.state)
            .context("roundtrip on exit")?;
        Ok(())
    }

    fn next_key(&mut self) -> Result<Option<KeyEvent>> {
        loop {
            if let Some(key) = self.state.pending_key.take() {
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

    _keep: Vec<wl_buffer::WlBuffer>,
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
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            state.pending_key = match key {
                1 => Some(KeyEvent::Escape),
                57 => Some(KeyEvent::Space),
                _ => keycode_to_char(key).map(KeyEvent::Char),
            };
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
                state.pending_key = Some(KeyEvent::Escape);
            }
            _ => {}
        }
    }
}

delegate_noop!(WaylandState: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandState: ignore wl_surface::WlSurface);
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
