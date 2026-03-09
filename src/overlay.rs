use std::io::Write;
use std::os::fd::AsFd;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1},
};

use crate::input::{
    keycode_to_char, keycode_to_hint, InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS,
};
use crate::render::render_grid;

const BTN_LEFT: u32 = 0x110;

pub struct App {
    pub running: bool,
    pub screen_w: u32,
    pub screen_h: u32,

    pub compositor: Option<wl_compositor::WlCompositor>,
    pub shm: Option<wl_shm::WlShm>,
    pub surface: Option<wl_surface::WlSurface>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub seat: Option<wl_seat::WlSeat>,
    pub vp_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    pub vp: Option<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,

    pub configured: bool,
    pub needs_redraw: bool,
    pub input: InputState,

    /// Pixel coordinates the mouse was moved to after a combo
    pub target: Option<(u32, u32)>,

    /// Signal flags set inside Dispatch, consumed in the main event loop
    pub do_move: bool,
    pub do_click: bool,

    /// Buffers kept alive until the compositor releases them
    pub _keep: Vec<wl_buffer::WlBuffer>,
}

pub fn show_overlay() {
    let conn = Connection::connect_to_env().unwrap();
    let mut eq = conn.new_event_queue();
    let qh = eq.handle();

    conn.display().get_registry(&qh, ());

    let mut app = App {
        running: true,
        screen_w: 0,
        screen_h: 0,
        compositor: None,
        shm: None,
        surface: None,
        layer_shell: None,
        layer_surface: None,
        seat: None,
        vp_manager: None,
        vp: None,
        configured: false,
        needs_redraw: false,
        input: InputState::First,
        target: None,
        do_move: false,
        do_click: false,
        _keep: Vec::new(),
    };

    eq.roundtrip(&mut app).unwrap();

    if let Some(manager) = app.vp_manager.clone() {
        let vp = manager.create_virtual_pointer(app.seat.as_ref(), &qh, ());
        app.vp = Some(vp);
    }

    app.init_layer_surface(&qh);

    eq.roundtrip(&mut app).unwrap();

    loop {
        if app.do_move {
            app.do_move = false;
            if let (Some(vp), Some((cx, cy))) = (&app.vp, app.target) {
                vp.motion_absolute(timestamp(), cx, cy, app.screen_w, app.screen_h);
                vp.frame();
            }
            conn.flush().unwrap();
        }

        if app.do_click {
            app.do_click = false;
            perform_click(&mut app, &mut eq);
            break;
        }

        if !app.running {
            break;
        }

        if app.needs_redraw && app.configured {
            app.redraw(&qh);
            app.needs_redraw = false;
        }

        eq.blocking_dispatch(&mut app).unwrap();
    }
}

/// Destroys the overlay surface, moves the pointer to the target, then sends
/// a left-click. Must use roundtrips (not bare flush) so Hyprland processes
/// each step before we continue.
fn perform_click(app: &mut App, eq: &mut wayland_client::EventQueue<App>) {
    if let Some(ls) = app.layer_surface.take() {
        ls.destroy();
    }
    if let Some(s) = app.surface.take() {
        s.destroy();
    }
    eq.roundtrip(app).unwrap();

    if let (Some(vp), Some((cx, cy))) = (&app.vp, app.target) {
        vp.motion_absolute(timestamp(), cx, cy, app.screen_w, app.screen_h);
        vp.frame();
    }
    eq.roundtrip(app).unwrap();

    if let Some(vp) = &app.vp {
        let ts = timestamp();
        vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Pressed);
        vp.frame();
        vp.button(ts, BTN_LEFT, wl_pointer::ButtonState::Released);
        vp.frame();
    }
    eq.roundtrip(app).unwrap();
}

impl App {
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

    fn redraw(&mut self, qh: &QueueHandle<Self>) {
        let (w, h) = (self.screen_w, self.screen_h);
        if w == 0 || h == 0 {
            return;
        }

        let stride = w * 4;
        let size = (stride * h) as usize;
        let mut pixels = vec![0u8; size];
        render_grid(&mut pixels, w, h, &self.input);

        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&pixels).unwrap();

        let shm = self.shm.as_ref().unwrap();
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        let surface = self.surface.as_ref().unwrap();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();

        self._keep.push(buffer);
        drop(pool);
    }

    fn handle_key(&mut self, key: u32) {
        match key {
            1 => {
                // ESC — exit without clicking
                self.running = false;
            }
            57 => {
                // Space — click at the current target (available after any main cell is chosen)
                if self.target.is_some() {
                    self.do_click = true;
                }
            }
            _ => {
                // Sub-grid uses the full char set; main grid is filtered to HINTS
                let ch = if matches!(self.input, InputState::SubFirst { .. }) {
                    let Some(c) = keycode_to_char(key) else {
                        return;
                    };
                    c
                } else {
                    let Some(c) = keycode_to_hint(key) else {
                        return;
                    };
                    c
                };
                self.handle_hint_char(ch);
            }
        }
    }

    fn handle_hint_char(&mut self, ch: u8) {
        match self.input.clone() {
            InputState::First => {
                self.input = InputState::Second(ch);
                self.needs_redraw = true;
            }
            InputState::Second(first) => {
                let col = HINTS.iter().position(|&c| c == first).unwrap_or(0) as u32;
                let row = HINTS.iter().position(|&c| c == ch).unwrap_or(0) as u32;
                let cell_w = self.screen_w / COLS;
                let cell_h = self.screen_h / ROWS;
                self.target = Some((col * cell_w + cell_w / 2, row * cell_h + cell_h / 2));
                self.do_move = true;
                self.input = InputState::SubFirst { col, row };
                self.needs_redraw = true;
            }
            InputState::SubFirst { col, row } => {
                if let Some(idx) = SUB_HINTS.iter().position(|&c| c == ch) {
                    let sub_col = idx as u32 % SUB_COLS;
                    let sub_row = idx as u32 / SUB_COLS;
                    let cell_w = self.screen_w / COLS;
                    let cell_h = self.screen_h / ROWS;
                    let sub_cell_w = cell_w / SUB_COLS;
                    let sub_cell_h = cell_h / SUB_ROWS;
                    let cx = col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2;
                    let cy = row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2;
                    self.target = Some((cx, cy));
                    self.do_move = true;
                    self.input = InputState::Ready;
                    self.needs_redraw = true;
                }
            }
            InputState::Ready => {}
        }
    }
}

fn timestamp() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
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

impl Dispatch<wl_seat::WlSeat, ()> for App {
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

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
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
            state.handle_key(key);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
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
                state.redraw(qh);
                state.needs_redraw = false;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}

delegate_noop!(App: ignore wl_compositor::WlCompositor);
delegate_noop!(App: ignore wl_surface::WlSurface);
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
delegate_noop!(App: ignore wl_buffer::WlBuffer);
delegate_noop!(App: ignore wl_pointer::WlPointer);
delegate_noop!(App: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(App: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(App: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);
