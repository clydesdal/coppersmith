//! Trackpad pinches on Linux. The window library only passes pinches on from
//! Macs and iPhones, so this listens for them on the window's own connection to
//! the Wayland desktop, on a small thread of its own.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use wayland_client::backend::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer::WlPointer, wl_registry::WlRegistry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{
    self as pinch, ZwpPointerGesturePinchV1,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;

/// How often the listener looks up to see whether it's time to stop.
const LISTEN_MS: i32 = 100;

/// What's been pinched since the last frame.
#[derive(Clone, Copy)]
struct Gesture {
    zoom: f32,
    pan: [f32; 2],
}

impl Default for Gesture {
    fn default() -> Self {
        Gesture { zoom: 1.0, pan: [0.0, 0.0] }
    }
}

#[derive(Default)]
pub struct Pinch {
    gesture: Arc<Mutex<Gesture>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pinch {
    /// Starts listening, if this window is on Wayland and the desktop offers pinches.
    pub fn listen(&mut self, display: *mut std::ffi::c_void, ctx: &egui::Context) {
        // A pointer can't cross threads by itself; it's only used to attach below.
        let address = display as usize;
        let (gesture, stop, ctx) = (self.gesture.clone(), self.stop.clone(), ctx.clone());
        self.thread = Some(std::thread::spawn(move || {
            // The display stays open until stop() has joined this thread.
            let backend = unsafe { Backend::from_foreign_display(address as *mut _) };
            let conn = Connection::from_backend(backend);
            // No gestures on this desktop just means no pinching: scrolling still zooms.
            let _ = run(&conn, gesture, stop, ctx);
        }));
    }

    #[allow(dead_code)]
    pub fn add(&self, zoom: f32) {
        if let Ok(mut g) = self.gesture.lock() {
            g.zoom *= zoom;
        }
    }

    /// The zoom factor and movement since last asked.
    pub fn take(&self) -> (f32, [f32; 2]) {
        let Ok(mut g) = self.gesture.lock() else { return (1.0, [0.0, 0.0]) };
        let out = (g.zoom, g.pan);
        *g = Gesture::default();
        out
    }

    /// Must happen before the window closes: the thread is using its connection.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Listener {
    gesture: Arc<Mutex<Gesture>>,
    ctx: egui::Context,
    gestures: ZwpPointerGesturesV1,
    pinch: Option<ZwpPointerGesturePinchV1>,
    last_scale: f64,
    two_fingers: bool,
}

fn run(
    conn: &Connection,
    gesture: Arc<Mutex<Gesture>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) -> Result<(), Box<dyn std::error::Error>> {
    let (globals, mut queue): (_, EventQueue<Bare>) = registry_queue_init(conn)?;
    let qh = queue.handle();
    let gestures: ZwpPointerGesturesV1 = globals.bind(&qh, 1..=3, ())?;
    let _seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let mut bare = Bare { gestures: Some(gestures), listener: None, gesture, ctx };
    queue.roundtrip(&mut bare)?;
    while !stop.load(Ordering::Relaxed) {
        queue.dispatch_pending(&mut bare)?;
        conn.flush()?;
        let Some(guard) = queue.prepare_read() else { continue };
        let mut fd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&guard.connection_fd()),
            events: libc::POLLIN,
            revents: 0,
        };
        // Reading has to be given up in good time, or the window's own thread waits.
        let ready = unsafe { libc::poll(&mut fd, 1, LISTEN_MS) };
        if ready > 0 {
            guard.read()?;
        }
    }
    Ok(())
}

/// Holds the listener until the seat says it has a pointer.
struct Bare {
    gestures: Option<ZwpPointerGesturesV1>,
    listener: Option<Listener>,
    gesture: Arc<Mutex<Gesture>>,
    ctx: egui::Context,
}

impl Dispatch<WlRegistry, GlobalListContents> for Bare {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Bare {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else { return };
        if !caps.contains(wl_seat::Capability::Pointer) || state.listener.is_some() {
            return;
        }
        let Some(gestures) = state.gestures.take() else { return };
        let pointer = seat.get_pointer(qh, ());
        let pinch = gestures.get_pinch_gesture(&pointer, qh, ());
        state.listener = Some(Listener {
            gesture: state.gesture.clone(),
            ctx: state.ctx.clone(),
            gestures,
            pinch: Some(pinch),
            last_scale: 1.0,
            two_fingers: false,
        });
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for Bare {
    fn event(
        state: &mut Self,
        _: &ZwpPointerGesturePinchV1,
        event: pinch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(l) = state.listener.as_mut() else { return };
        match event {
            pinch::Event::Begin { fingers, .. } => {
                l.last_scale = 1.0;
                // More fingers usually means the desktop has its own plans.
                l.two_fingers = fingers == 2;
            }
            pinch::Event::Update { dx, dy, scale, .. } if l.two_fingers && scale > 0.0 => {
                let step = scale / l.last_scale;
                l.last_scale = scale;
                if let Ok(mut g) = l.gesture.lock() {
                    g.zoom *= step as f32;
                    g.pan[0] += dx as f32;
                    g.pan[1] += dy as f32;
                }
                l.ctx.request_repaint();
            }
            _ => {}
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(pinch) = self.pinch.take()
            && pinch.is_alive()
        {
            pinch.destroy();
        }
        if self.gestures.version() >= 2 {
            self.gestures.release();
        }
    }
}

delegate_noop!(Bare: ignore WlPointer);
delegate_noop!(Bare: ignore ZwpPointerGesturesV1);
