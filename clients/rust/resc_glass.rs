//! resc-desk's glass under parts of the window (protocols/resc-glass-v1.xml): rounded
//! panels where the compositor blurs, tints and bends what is under the window. Panels
//! are drawn in the order given, each over the ones before; they take effect with the
//! surface's next commit (the next vkQueuePresentKHR).
//!
//! Needs wayland-client 0.31, wayland-scanner 0.31, wayland-backend 0.3 with the
//! `client_system` feature (winit's wayland connection is libwayland's: the same one).

use std::ffi::c_void;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

pub mod proto {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables, non_upper_case_globals, non_snake_case, unused_imports, missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        // The protocol file copied next to Cargo.toml (resc-desk: protocols/resc-glass-v1.xml).
        wayland_scanner::generate_interfaces!("protocols/resc-glass-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/resc-glass-v1.xml");
}
use proto::resc_glass_manager_v1::RescGlassManagerV1;
use proto::resc_glass_v1::RescGlassV1;

/// One panel, surface-local logical px (physical / scale).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Panel {
    pub rect: [f32; 4],
    /// Top left, top right, bottom right, bottom left.
    pub radii: [f32; 4],
    /// Shown only inside it (x, y, w, h).
    pub clip: Option<[f32; 4]>,
    /// Gaussian sigma, px; negative: the desk's own (what «Параметры» → «Стекло» sets).
    pub blur: f32,
    /// 0xAARRGGBB straight alpha; 0: the desk's window tint.
    pub tint: u32,
    /// The bending band along the edge, px; negative: the desk's own.
    pub bezel: f32,
    /// The desk's panel look: soft shadow, hairline rim, inset highlight.
    pub rim: bool,
}

struct St;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for St {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<RescGlassManagerV1, ()> for St {
    fn event(_: &mut Self, _: &RescGlassManagerV1, _: proto::resc_glass_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<RescGlassV1, ()> for St {
    fn event(_: &mut Self, _: &RescGlassV1, _: proto::resc_glass_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

pub struct Glass {
    conn: Connection,
    _queue: EventQueue<St>,
    _manager: RescGlassManagerV1,
    glass: RescGlassV1,
    told: Vec<Panel>,
}

impl Glass {
    /// The glass of the window's surface. `display` and `surface`: the `wl_display*` and
    /// `wl_surface*` of raw-window-handle's WaylandDisplayHandle / WaylandWindowHandle.
    /// None: not on resc-desk (no such global).
    ///
    /// # Safety
    /// Both pointers live as long as the value.
    pub unsafe fn new(display: *mut c_void, surface: *mut c_void) -> Option<Glass> {
        let backend = Backend::from_foreign_display(display as *mut _);
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<St>(&conn).ok()?;
        let qh = queue.handle();
        let manager: RescGlassManagerV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        let id = ObjectId::from_ptr(WlSurface::interface(), surface as *mut _).ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        let glass = manager.get_glass(&surface, &qh, ());
        conn.flush().ok()?;
        Some(Glass { conn, _queue: queue, _manager: manager, glass, told: Vec::new() })
    }

    /// The panels from the surface's next commit on, bottom to top (nothing sent when they
    /// are as last told).
    pub fn set(&mut self, panels: &[Panel]) {
        if panels == self.told.as_slice() {
            return;
        }
        self.glass.clear();
        for p in panels {
            let [x, y, w, h] = p.rect;
            let [a, b, c, d] = p.radii;
            self.glass.add_panel(x as f64, y as f64, w as f64, h as f64, a as f64, b as f64, c as f64, d as f64, p.blur as f64, p.tint, p.bezel as f64, p.rim as u32);
            if let Some([x, y, w, h]) = p.clip {
                self.glass.clip_panel(x as f64, y as f64, w as f64, h as f64);
            }
        }
        let _ = self.conn.flush();
        self.told = panels.to_vec();
    }
}

impl Drop for Glass {
    fn drop(&mut self) {
        self.glass.destroy();
        let _ = self.conn.flush();
    }
}
