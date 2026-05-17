//! macOS backend.
//!
//! Requires Accessibility permission (System Settings → Privacy & Security
//! → Accessibility). Mouse synthesis and global key capture via CGEventTap
//! both depend on it.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, define_class, msg_send, MainThreadOnly};

use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent,
    NSEventMask, NSPanel, NSScreen, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSDate, NSPoint, NSRect, NSSize, NSString};

use super::{Backend, KeyEvent};
use crate::config::{config, Key};

type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;
type CGImageRef = *mut c_void;
type CGColorSpaceRef = *mut c_void;
type CGDataProviderRef = *mut c_void;
type CFTypeRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    type_: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[repr(C)]
#[derive(Copy, Clone)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(transparent)]
#[derive(Copy, Clone)]
struct CGDirectDisplayID(u32);

const K_CGEVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CGEVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CGEVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const K_CGEVENT_RIGHT_MOUSE_UP: u32 = 4;
const K_CGEVENT_MOUSE_MOVED: u32 = 5;
const K_CGEVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const K_CGEVENT_KEY_DOWN: u32 = 10;
const K_CGEVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFFFFFE;
const K_CGEVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFFFFFF;

const K_CGMOUSE_BUTTON_LEFT: u32 = 0;
const K_CGMOUSE_BUTTON_RIGHT: u32 = 1;

const K_CGHIDEVENT_TAP: u32 = 0;
const K_CGANNOTATED_SESSION_EVENT_TAP: u32 = 2;

const K_CGHEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CGEVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CGEVENT_SOURCE_STATE_HIDSYSTEM_STATE: i32 = 1;

const K_CGMOUSE_EVENT_CLICK_STATE: u32 = 1;
const K_CGKEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CGEVENT_FLAG_MASK_COMMAND: u64 = 0x100000;

const K_CGSCROLL_EVENT_UNIT_LINE: u32 = 1;
const K_CGIMAGE_ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
const K_CGBITMAP_BYTE_ORDER32_LITTLE: u32 = 2 << 12;
const K_CGRENDERING_INTENT_DEFAULT: u32 = 0;

const K_CGMAXIMUM_WINDOW_LEVEL_KEY: i32 = 14;
const K_CFSTRING_ENCODING_UTF8: u32 = 0x08000100;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    fn CGDisplayMoveCursorToPoint(display: CGDirectDisplayID, point: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;

    fn CGEventSourceCreate(state_id: i32) -> CGEventSourceRef;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        location: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    );
    fn CGEventPost(tap_location: u32, event: CGEventRef);
    fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);

    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;

    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGWindowLevelForKey(key: i32) -> i32;

    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const u8,
        size: usize,
        release_callback: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize)>,
    ) -> CGDataProviderRef;
    fn CGDataProviderRelease(dp: CGDataProviderRef);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
        provider: CGDataProviderRef,
        decode: *const f64,
        should_interpolate: bool,
        intent: u32,
    ) -> CGImageRef;
    fn CGImageRelease(img: CGImageRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreate(
        allocator: *mut c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *mut c_void;
    fn CFRelease(cf: CFTypeRef);
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFStringCreateWithCString(
        allocator: *mut c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *const c_void;

    fn CFMachPortCreateRunLoopSource(
        allocator: *mut c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRunInMode(
        mode: *const c_void,
        seconds: f64,
        return_after_source_handled: bool,
    ) -> i32;
    static kCFRunLoopDefaultMode: *const c_void;
    static kCFRunLoopCommonModes: *const c_void;
}

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "StochosOverlayWindow"]
    #[derive(Debug)]
    struct OverlayWindow;

    impl OverlayWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            true
        }
    }
);

pub struct MacosBackend {
    app: Retained<NSApplication>,
    window: Retained<OverlayWindow>,
    view: Retained<NSView>,
    screen_w: u32,
    screen_h: u32,
    visible: bool,
    current_image: Option<CGImageHandle>,
    event_source: CGEventSourceHandle,
    event_tap: EventTapHandle,
}

struct CGImageHandle(CGImageRef);
impl Drop for CGImageHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CGImageRelease(self.0) };
        }
    }
}

struct CGEventSourceHandle(CGEventSourceRef);
impl Drop for CGEventSourceHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

struct EventTapHandle {
    tap: CFMachPortRef,
    source: CFRunLoopSourceRef,
}
impl Drop for EventTapHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.tap.is_null() {
                CGEventTapEnable(self.tap, false);
            }
            if !self.source.is_null() {
                CFRunLoopRemoveSource(CFRunLoopGetCurrent(), self.source, kCFRunLoopCommonModes);
                CFRelease(self.source);
            }
            if !self.tap.is_null() {
                EVENT_TAP.store(ptr::null_mut(), Ordering::Release);
                CFRelease(self.tap);
            }
        }
    }
}

static EVENT_QUEUE: Mutex<VecDeque<KeyEvent>> = Mutex::new(VecDeque::new());

static EVENT_TAP: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

impl MacosBackend {
    pub fn new() -> Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow!("MacosBackend must be constructed on the main thread"))?;

        ensure_accessibility_permission()?;

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        // Skipping `app.finishLaunching()` on purpose. It posts
        // `AppDidFinishLaunching`, which system menus listen to as a
        // cue to auto-dismiss. The CGEventTap path doesn't need it.

        let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        let screen_w = bounds.size.width as u32;
        let screen_h = bounds.size.height as u32;

        let main_screen = NSScreen::mainScreen(mtm)
            .ok_or_else(|| anyhow!("NSScreen::mainScreen returned nil"))?;
        let screen_frame = main_screen.frame();

        let window: Retained<OverlayWindow> = unsafe {
            let alloc = OverlayWindow::alloc(mtm);
            msg_send![
                alloc,
                initWithContentRect: screen_frame,
                styleMask: NSWindowStyleMask::Borderless
                    | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };

        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setOpaque(false);
        window.setHasShadow(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        let max_level = unsafe { CGWindowLevelForKey(K_CGMAXIMUM_WINDOW_LEVEL_KEY) } as isize;
        window.setLevel(max_level);
        window.setIgnoresMouseEvents(true);
        window.setWorksWhenModal(true);
        window.setHidesOnDeactivate(false);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // The Retained reference lives for the whole lifetime of MacosBackend,
        // so opting out of release-on-close is sound.
        unsafe { window.setReleasedWhenClosed(false) };

        let view_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(screen_w as f64, screen_h as f64),
        );
        let view = {
            let alloc = NSView::alloc(mtm);
            NSView::initWithFrame(alloc, view_frame)
        };
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            unsafe {
                // Top-down pixel buffer, so flip the layer so row 0 is at the top.
                let _: () = msg_send![&*layer, setGeometryFlipped: true];
                let gravity = NSString::from_str("resize");
                let _: () = msg_send![&*layer, setContentsGravity: &*gravity];
            }
        }
        window.setContentView(Some(&view));

        // `orderFrontRegardless` shows the panel without firing the
        // active-app / key-window / main-window notifications that
        // trigger menu auto-dismiss.
        window.orderFrontRegardless();

        let event_source = unsafe { CGEventSourceCreate(K_CGEVENT_SOURCE_STATE_HIDSYSTEM_STATE) };
        if event_source.is_null() {
            return Err(anyhow!("CGEventSourceCreate failed"));
        }
        // Drops the default 250 ms suppression window after a synthesized
        // event. Without this, a fast click+drag sequence can lose events.
        unsafe { CGEventSourceSetLocalEventsSuppressionInterval(event_source, 0.0) };

        let event_tap = {
            let mask: u64 = 1u64 << K_CGEVENT_KEY_DOWN;
            let tap = unsafe {
                CGEventTapCreate(
                    K_CGANNOTATED_SESSION_EVENT_TAP,
                    K_CGHEAD_INSERT_EVENT_TAP,
                    K_CGEVENT_TAP_OPTION_DEFAULT,
                    mask,
                    event_tap_callback,
                    ptr::null_mut(),
                )
            };
            if tap.is_null() {
                return Err(anyhow!(
                    "CGEventTapCreate failed — Accessibility or Input Monitoring permission missing"
                ));
            }
            let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null_mut(), tap, 0) };
            if source.is_null() {
                unsafe { CFRelease(tap) };
                return Err(anyhow!("CFMachPortCreateRunLoopSource failed"));
            }
            EVENT_TAP.store(tap, Ordering::Release);
            unsafe {
                CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
                CGEventTapEnable(tap, true);
            }
            EventTapHandle { tap, source }
        };

        Ok(Self {
            app,
            window,
            view,
            screen_w,
            screen_h,
            visible: true,
            current_image: None,
            event_source: CGEventSourceHandle(event_source),
            event_tap,
        })
    }

    fn focus_target_app(&self) {
        let target: Option<Retained<AnyObject>> = unsafe {
            let ws: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            if ws.is_null() {
                None
            } else {
                let app_ptr: *mut AnyObject = msg_send![ws, frontmostApplication];
                if app_ptr.is_null() {
                    None
                } else {
                    Retained::retain(app_ptr)
                }
            }
        };
        let Some(app) = target.as_deref() else {
            return;
        };
        // NSApplicationActivateAllWindows = 1 << 0.
        const ACTIVATE_ALL_WINDOWS: u64 = 1;
        let _: bool = unsafe { msg_send![app, activateWithOptions: ACTIVATE_ALL_WINDOWS] };
        // Synthesized clicks need the target app actually focused, otherwise
        // macOS click-through-to-focus eats the first event.
        self.pump_events_briefly();
        thread::sleep(Duration::from_millis(40));
        self.pump_events_briefly();
    }

    fn pump_events_briefly(&self) {
        let past = NSDate::distantPast();
        loop {
            let event: Option<Retained<NSEvent>> =
                self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&past),
                    &default_run_loop_mode(),
                    true,
                );
            let Some(event) = event else { break };
            self.app.sendEvent(&event);
        }
    }

    fn hide_overlay(&mut self) {
        if !self.visible {
            return;
        }
        unsafe { CGEventTapEnable(self.event_tap.tap, false) };
        self.window.orderOut(None);
        self.pump_events_briefly();
        self.visible = false;
    }

    fn show_overlay(&mut self) {
        if self.visible {
            return;
        }
        self.window.orderFrontRegardless();
        unsafe { CGEventTapEnable(self.event_tap.tap, true) };
        self.pump_events_briefly();
        self.visible = true;
    }

    fn post_mouse(&self, ty: u32, button: u32, x: u32, y: u32, click_count: i64) {
        unsafe {
            let pt = CGPoint {
                x: x as f64,
                y: y as f64,
            };
            let evt = CGEventCreateMouseEvent(self.event_source.0, ty, pt, button);
            if evt.is_null() {
                return;
            }
            // ClickState defaults to 0, which apps interpret as a non-click
            // (mouse-button-held move). Set it for every Down/Up/Dragged.
            if click_count > 0 {
                CGEventSetIntegerValueField(evt, K_CGMOUSE_EVENT_CLICK_STATE, click_count);
            }
            CGEventPost(K_CGHIDEVENT_TAP, evt);
            CFRelease(evt);
        }
    }

    fn move_cursor(&self, x: u32, y: u32) {
        let pt = CGPoint {
            x: x as f64,
            y: y as f64,
        };
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(true);
            CGDisplayMoveCursorToPoint(CGMainDisplayID(), pt);
        }
    }

    fn click_at(&self, ty_down: u32, ty_up: u32, button: u32, x: u32, y: u32, count: i64) {
        // mouseMoved before the click so apps see the cursor at the target.
        // CGWarpMouseCursorPosition is avoided because its 250 ms suppression
        // window can swallow the synthesized click that follows.
        self.post_mouse(K_CGEVENT_MOUSE_MOVED, 0, x, y, 0);
        thread::sleep(Duration::from_millis(10));
        self.post_mouse(ty_down, button, x, y, count);
        thread::sleep(Duration::from_millis(20));
        self.post_mouse(ty_up, button, x, y, count);
        thread::sleep(Duration::from_millis(20));
    }
}

impl Backend for MacosBackend {
    fn screen_size(&self) -> (u32, u32) {
        (self.screen_w, self.screen_h)
    }

    fn present(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<()> {
        if !self.visible {
            self.show_overlay();
        }

        // Source is BGRA, straight alpha. CG's 32-bit-little-endian ARGB
        // format requires premultiplied alpha. Premultiply into an owned
        // buffer that the data provider releases on drop.
        let mut buf: Vec<u8> = pixels.to_vec();
        for px in buf.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a < 255 {
                px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
                px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
                px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
            }
        }

        let boxed: Box<Vec<u8>> = Box::new(buf);
        let len = boxed.len();
        let data_ptr = boxed.as_ptr();
        let raw: *mut Vec<u8> = Box::into_raw(boxed);

        let image = unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            if cs.is_null() {
                drop(Box::from_raw(raw));
                return Err(anyhow!("CGColorSpaceCreateDeviceRGB failed"));
            }
            let provider = CGDataProviderCreateWithData(
                raw as *mut c_void,
                data_ptr,
                len,
                Some(release_pixel_buffer),
            );
            if provider.is_null() {
                CGColorSpaceRelease(cs);
                drop(Box::from_raw(raw));
                return Err(anyhow!("CGDataProviderCreateWithData failed"));
            }
            let img = CGImageCreate(
                width as usize,
                height as usize,
                8,
                32,
                (width as usize) * 4,
                cs,
                K_CGIMAGE_ALPHA_PREMULTIPLIED_FIRST | K_CGBITMAP_BYTE_ORDER32_LITTLE,
                provider,
                ptr::null(),
                false,
                K_CGRENDERING_INTENT_DEFAULT,
            );
            CGColorSpaceRelease(cs);
            CGDataProviderRelease(provider);
            if img.is_null() {
                return Err(anyhow!("CGImageCreate failed"));
            }
            img
        };

        unsafe {
            if let Some(layer) = self.view.layer() {
                let _: () = msg_send![&*layer, setContents: image as *mut AnyObject];
            }
        }
        // Install the new image before dropping the previous handle.
        self.current_image = Some(CGImageHandle(image));

        self.window.displayIfNeeded();
        self.pump_events_briefly();
        Ok(())
    }

    fn mouse_pos(&mut self) -> Result<(u32, u32)> {
        let pt = unsafe {
            let evt = CGEventCreate(std::ptr::null_mut());
            let loc = CGEventGetLocation(evt);
            CFRelease(evt);
            loc
        };
        Ok((pt.x as u32, pt.y as u32))
    }

    fn move_mouse(&mut self, x: u32, y: u32) -> Result<()> {
        self.move_cursor(x, y);
        Ok(())
    }

    fn click(&mut self, x: u32, y: u32) -> Result<()> {
        self.hide_overlay();
        self.focus_target_app();
        self.click_at(
            K_CGEVENT_LEFT_MOUSE_DOWN,
            K_CGEVENT_LEFT_MOUSE_UP,
            K_CGMOUSE_BUTTON_LEFT,
            x,
            y,
            1,
        );
        Ok(())
    }

    fn double_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.hide_overlay();
        self.focus_target_app();
        self.click_at(
            K_CGEVENT_LEFT_MOUSE_DOWN,
            K_CGEVENT_LEFT_MOUSE_UP,
            K_CGMOUSE_BUTTON_LEFT,
            x,
            y,
            1,
        );
        self.click_at(
            K_CGEVENT_LEFT_MOUSE_DOWN,
            K_CGEVENT_LEFT_MOUSE_UP,
            K_CGMOUSE_BUTTON_LEFT,
            x,
            y,
            2,
        );
        Ok(())
    }

    fn right_click(&mut self, x: u32, y: u32) -> Result<()> {
        self.hide_overlay();
        self.focus_target_app();
        self.click_at(
            K_CGEVENT_RIGHT_MOUSE_DOWN,
            K_CGEVENT_RIGHT_MOUSE_UP,
            K_CGMOUSE_BUTTON_RIGHT,
            x,
            y,
            1,
        );
        Ok(())
    }

    fn drag_select(&mut self, x1: u32, y1: u32, x2: u32, y2: u32) -> Result<()> {
        self.hide_overlay();
        self.focus_target_app();
        self.post_mouse(K_CGEVENT_MOUSE_MOVED, 0, x1, y1, 0);
        thread::sleep(Duration::from_millis(10));
        self.post_mouse(K_CGEVENT_LEFT_MOUSE_DOWN, K_CGMOUSE_BUTTON_LEFT, x1, y1, 1);

        // Interpolate so apps that distinguish click-vs-drag (e.g. text
        // selection in some editors) recognize this as a drag.
        const STEPS: u32 = 24;
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let x = x1 as f64 + (x2 as f64 - x1 as f64) * t;
            let y = y1 as f64 + (y2 as f64 - y1 as f64) * t;
            self.post_mouse(
                K_CGEVENT_LEFT_MOUSE_DRAGGED,
                K_CGMOUSE_BUTTON_LEFT,
                x as u32,
                y as u32,
                1,
            );
            thread::sleep(Duration::from_millis(8));
        }

        self.post_mouse(K_CGEVENT_LEFT_MOUSE_UP, K_CGMOUSE_BUTTON_LEFT, x2, y2, 1);
        Ok(())
    }

    fn scroll_up(&mut self) -> Result<()> {
        post_scroll(self.event_source.0, 3, 0);
        Ok(())
    }
    fn scroll_down(&mut self) -> Result<()> {
        post_scroll(self.event_source.0, -3, 0);
        Ok(())
    }
    fn scroll_left(&mut self) -> Result<()> {
        post_scroll(self.event_source.0, 0, 3);
        Ok(())
    }
    fn scroll_right(&mut self) -> Result<()> {
        post_scroll(self.event_source.0, 0, -3);
        Ok(())
    }

    fn exit(&mut self) -> Result<()> {
        self.hide_overlay();
        Ok(())
    }

    fn next_key(&mut self) -> Result<Option<KeyEvent>> {
        if !self.visible {
            return Ok(None);
        }

        loop {
            if let Some(ev) = EVENT_QUEUE.lock().unwrap().pop_front() {
                return Ok(Some(ev));
            }
            // The tap dispatches via the CFRunLoop. AppKit's queue is
            // separate, so drain it explicitly for window redraws.
            self.pump_events_briefly();
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, true);
            }
        }
    }

    fn reopen(&mut self) -> Result<()> {
        self.show_overlay();
        Ok(())
    }
}

unsafe extern "C" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    type_: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if type_ == K_CGEVENT_TAP_DISABLED_BY_TIMEOUT {
        let tap = EVENT_TAP.load(Ordering::Acquire);
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, true) };
        }
        return event;
    }
    if type_ == K_CGEVENT_TAP_DISABLED_BY_USER_INPUT {
        return event;
    }
    if type_ != K_CGEVENT_KEY_DOWN {
        return event;
    }

    let keycode = unsafe { CGEventGetIntegerValueField(event, K_CGKEYBOARD_EVENT_KEYCODE) } as u16;
    let flags = unsafe { CGEventGetFlags(event) };

    // Cmd+Q / Cmd+. → unconditional close, independent of bindings.
    if flags & K_CGEVENT_FLAG_MASK_COMMAND != 0 && (keycode == 0x0C || keycode == 0x2F) {
        EVENT_QUEUE.lock().unwrap().push_back(KeyEvent::Close);
        return ptr::null_mut();
    }

    let key = if let Some(k) = vk_to_key(keycode) {
        Some(k)
    } else {
        let mut buf = [0u16; 8];
        let mut actual: usize = 0;
        unsafe { CGEventKeyboardGetUnicodeString(event, buf.len(), &mut actual, buf.as_mut_ptr()) };
        if actual == 0 {
            None
        } else {
            String::from_utf16_lossy(&buf[..actual.min(buf.len())])
                .chars()
                .next()
                .filter(|&c| (c as u32) < 0xE000)
                .map(Key::Char)
        }
    };

    let Some(k) = key else {
        return event;
    };

    let mapped = config().keys.to_event(k).or(match k {
        Key::Char(c) => Some(KeyEvent::Char(c)),
        _ => None,
    });

    if let Some(ev) = mapped {
        EVENT_QUEUE.lock().unwrap().push_back(ev);
    }
    // Consume even unmapped keys, so they don't leak to focused apps.
    ptr::null_mut()
}

unsafe extern "C" fn release_pixel_buffer(info: *mut c_void, _data: *const u8, _size: usize) {
    if !info.is_null() {
        unsafe { drop(Box::from_raw(info as *mut Vec<u8>)) };
    }
}

fn post_scroll(source: CGEventSourceRef, vertical: i32, horizontal: i32) {
    unsafe {
        let evt = CGEventCreateScrollWheelEvent2(
            source,
            K_CGSCROLL_EVENT_UNIT_LINE,
            2,
            vertical,
            horizontal,
            0,
        );
        if !evt.is_null() {
            CGEventPost(K_CGHIDEVENT_TAP, evt);
            CFRelease(evt);
        }
    }
}

fn default_run_loop_mode() -> Retained<NSString> {
    NSString::from_str("kCFRunLoopDefaultMode")
}

/// macOS virtual keycodes (from `<Carbon/HIToolbox/Events.h>`).
mod vk {
    pub const RETURN: u16 = 0x24;
    pub const TAB: u16 = 0x30;
    pub const SPACE: u16 = 0x31;
    /// Labeled "delete" on Apple keyboards, semantically backspace.
    pub const DELETE: u16 = 0x33;
    pub const ESCAPE: u16 = 0x35;
    pub const KEYPAD_ENTER: u16 = 0x4C;

    pub const F1: u16 = 0x7A;
    pub const F2: u16 = 0x78;
    pub const F3: u16 = 0x63;
    pub const F4: u16 = 0x76;
    pub const F5: u16 = 0x60;
    pub const F6: u16 = 0x61;
    pub const F7: u16 = 0x62;
    pub const F8: u16 = 0x64;
    pub const F9: u16 = 0x65;
    pub const F10: u16 = 0x6D;
    pub const F11: u16 = 0x67;
    pub const F12: u16 = 0x6F;

    pub const HOME: u16 = 0x73;
    pub const PAGE_UP: u16 = 0x74;
    pub const FORWARD_DELETE: u16 = 0x75;
    pub const END: u16 = 0x77;
    pub const PAGE_DOWN: u16 = 0x79;
    pub const LEFT_ARROW: u16 = 0x7B;
    pub const RIGHT_ARROW: u16 = 0x7C;
    pub const DOWN_ARROW: u16 = 0x7D;
    pub const UP_ARROW: u16 = 0x7E;

    pub const CAPS_LOCK: u16 = 0x39;

    pub const KEYPAD_0: u16 = 0x52;
    pub const KEYPAD_1: u16 = 0x53;
    pub const KEYPAD_2: u16 = 0x54;
    pub const KEYPAD_3: u16 = 0x55;
    pub const KEYPAD_4: u16 = 0x56;
    pub const KEYPAD_5: u16 = 0x57;
    pub const KEYPAD_6: u16 = 0x58;
    pub const KEYPAD_7: u16 = 0x59;
    pub const KEYPAD_8: u16 = 0x5B;
    pub const KEYPAD_9: u16 = 0x5C;
    pub const KEYPAD_PLUS: u16 = 0x45;
    pub const KEYPAD_MINUS: u16 = 0x4E;
    pub const KEYPAD_MULTIPLY: u16 = 0x43;
    pub const KEYPAD_DIVIDE: u16 = 0x4B;
    pub const KEYPAD_DECIMAL: u16 = 0x41;
}

fn vk_to_key(kc: u16) -> Option<Key> {
    Some(match kc {
        vk::RETURN | vk::KEYPAD_ENTER => Key::Enter,
        vk::TAB => Key::Tab,
        vk::SPACE => Key::Space,
        vk::DELETE => Key::Backspace,
        vk::ESCAPE => Key::Escape,
        vk::FORWARD_DELETE => Key::Delete,
        vk::HOME => Key::Home,
        vk::END => Key::End,
        vk::PAGE_UP => Key::PageUp,
        vk::PAGE_DOWN => Key::PageDown,
        vk::UP_ARROW => Key::Up,
        vk::DOWN_ARROW => Key::Down,
        vk::LEFT_ARROW => Key::Left,
        vk::RIGHT_ARROW => Key::Right,
        vk::F1 => Key::F1,
        vk::F2 => Key::F2,
        vk::F3 => Key::F3,
        vk::F4 => Key::F4,
        vk::F5 => Key::F5,
        vk::F6 => Key::F6,
        vk::F7 => Key::F7,
        vk::F8 => Key::F8,
        vk::F9 => Key::F9,
        vk::F10 => Key::F10,
        vk::F11 => Key::F11,
        vk::F12 => Key::F12,
        vk::CAPS_LOCK => Key::CapsLock,
        vk::KEYPAD_0 => Key::NumPad0,
        vk::KEYPAD_1 => Key::NumPad1,
        vk::KEYPAD_2 => Key::NumPad2,
        vk::KEYPAD_3 => Key::NumPad3,
        vk::KEYPAD_4 => Key::NumPad4,
        vk::KEYPAD_5 => Key::NumPad5,
        vk::KEYPAD_6 => Key::NumPad6,
        vk::KEYPAD_7 => Key::NumPad7,
        vk::KEYPAD_8 => Key::NumPad8,
        vk::KEYPAD_9 => Key::NumPad9,
        vk::KEYPAD_PLUS => Key::NumPadAdd,
        vk::KEYPAD_MINUS => Key::NumPadSubtract,
        vk::KEYPAD_MULTIPLY => Key::NumPadMultiply,
        vk::KEYPAD_DIVIDE => Key::NumPadDivide,
        vk::KEYPAD_DECIMAL => Key::NumPadDecimal,
        _ => return None,
    })
}

fn ensure_accessibility_permission() -> Result<()> {
    // Returns true if the process is already trusted. Shows the system
    // prompt on first untrusted call.
    unsafe {
        let key = CFStringCreateWithCString(
            ptr::null_mut(),
            b"AXTrustedCheckOptionPrompt\0".as_ptr() as *const c_char,
            K_CFSTRING_ENCODING_UTF8,
        );
        if key.is_null() {
            return Err(anyhow!("CFStringCreateWithCString failed"));
        }
        let mut keys: [*const c_void; 1] = [key as *const c_void];
        let mut vals: [*const c_void; 1] = [kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            ptr::null_mut(),
            keys.as_mut_ptr(),
            vals.as_mut_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        CFRelease(key as *mut c_void);
        if dict.is_null() {
            return Err(anyhow!("CFDictionaryCreate failed"));
        }
        let trusted = AXIsProcessTrustedWithOptions(dict);
        CFRelease(dict);
        if !trusted {
            return Err(anyhow!(
                "stochos needs Accessibility permission to synthesize input.\n\
                 Open System Settings → Privacy & Security → Accessibility\n\
                 and enable the running binary (Terminal/iTerm or the stochos\n\
                 binary directly), then re-run."
            ));
        }
    }
    Ok(())
}
