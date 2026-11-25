//! A small Wayland client for resc-desk's own apps (`resc-desk settings`, ...): one
//! xdg_toplevel window, double-buffered shm, frames paced by frame callbacks, pointer,
//! wheel and raw key events. The window frame (title bar, moving, snapping, the taskbar
//! button) comes from the compositor like for any other program.
use crate::render::Canvas;
use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wl::protocol::*;
use wl::{Conn, ObjId};

#[derive(Clone, Copy)]
enum K {
    Registry,
    Sync,
    Frame,
    Xdg,
    Toplevel,
    Buffer(usize),
    WmBase,
    Seat,
    Pointer,
    Keyboard,
    Other,
}

pub enum Ev {
    /// The size changed (logical px); redraw everything.
    Resized,
    Close,
    Motion(f64, f64),
    Leave,
    Button(u32, bool),
    /// Vertical wheel, in the protocol's units (≈ 10–15 per notch).
    Wheel(f64),
    /// evdev key code, pressed.
    Key(u32, bool),
    /// The modifiers changed (in order with the keys: Alt down, Left, Alt up).
    Mods(crate::textedit::Mods),
}

struct Buf {
    buffer: ObjId,
    map: sys::Mmap,
    _fd: OwnedFd,
    busy: bool,
}

pub struct Window {
    conn: Conn,
    next_id: u32,
    objs: HashMap<ObjId, K>,
    globals: Vec<(u32, String, u32)>,
    compositor: ObjId,
    shm: ObjId,
    wm_base: ObjId,
    seat: ObjId,
    pointer: ObjId,
    keyboard: ObjId,
    surface: ObjId,
    xdg: ObjId,
    top: ObjId,
    bufs: Vec<Buf>,
    pub w: i32,
    pub h: i32,
    pending: Option<(i32, i32)>,
    configured: bool,
    frame_cb: ObjId,
    synced: bool,
    events: Vec<Ev>,
    /// Modifiers (xkb bits: 1 Shift, 2 Lock, 4 Control, 8 Alt) and the layout group.
    mods: (u32, u32),
    /// Key repeat: (rate per second, delay ms), the held key and when it repeats next.
    repeat: (i32, i32),
    held: Option<(u32, u64)>,
    /// More fds that wake `poll` (inotify, worker pipes).
    extra: Vec<i32>,
    /// The last pointer button press (move / resize requests need it).
    btn_serial: u32,
    pending_max: bool,
    /// Maximized by the compositor (the caption button shows "restore").
    pub maximized: bool,
}

/// evdev codes of the modifier keys (they don't repeat).
fn is_modifier(key: u32) -> bool {
    matches!(key, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126 | 58)
}

fn err(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, m.to_string())
}

impl Window {
    pub fn open(title: &str, app_id: &str, w: i32, h: i32, min: (i32, i32)) -> std::io::Result<Window> {
        let disp = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = if disp.starts_with('/') {
            disp
        } else {
            format!("{}/{}", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()), disp)
        };
        let stream = UnixStream::connect(&path)?;
        stream.set_nonblocking(true)?;
        let fd: OwnedFd = stream.into();
        let mut n = Window {
            conn: Conn::new(fd),
            next_id: 2,
            objs: HashMap::new(),
            globals: vec![],
            compositor: 0,
            shm: 0,
            wm_base: 0,
            seat: 0,
            pointer: 0,
            keyboard: 0,
            surface: 0,
            xdg: 0,
            top: 0,
            bufs: vec![],
            w,
            h,
            pending: None,
            configured: false,
            frame_cb: 0,
            synced: false,
            events: vec![],
            mods: (0, 0),
            repeat: (25, 600),
            held: None,
            extra: vec![],
            btn_serial: 0,
            pending_max: false,
            maximized: false,
        };
        let reg = n.new_id(K::Registry);
        wl_display::req::get_registry(&mut n.conn, 1, reg);
        n.roundtrip()?;
        let find = |n: &Window, name: &str| n.globals.iter().find(|g| g.1 == name).map(|g| (g.0, g.2));
        let (cname, cver) = find(&n, "wl_compositor").ok_or_else(|| err("no wl_compositor"))?;
        let (sname, _) = find(&n, "wl_shm").ok_or_else(|| err("no wl_shm"))?;
        let (wname, wver) = find(&n, "xdg_wm_base").ok_or_else(|| err("no xdg_wm_base"))?;
        n.compositor = n.bind(reg, cname, "wl_compositor", cver.min(4), K::Other);
        n.shm = n.bind(reg, sname, "wl_shm", 1, K::Other);
        n.wm_base = n.bind(reg, wname, "xdg_wm_base", wver.min(3), K::WmBase);
        if let Some((name, ver)) = find(&n, "wl_seat") {
            n.seat = n.bind(reg, name, "wl_seat", ver.min(5), K::Seat);
        }
        n.surface = n.new_id(K::Other);
        wl_compositor::req::create_surface(&mut n.conn, n.compositor, n.surface);
        n.xdg = n.new_id(K::Xdg);
        xdg_wm_base::req::get_xdg_surface(&mut n.conn, n.wm_base, n.xdg, n.surface);
        n.top = n.new_id(K::Toplevel);
        xdg_surface::req::get_toplevel(&mut n.conn, n.xdg, n.top);
        xdg_toplevel::req::set_title(&mut n.conn, n.top, title);
        xdg_toplevel::req::set_app_id(&mut n.conn, n.top, app_id);
        xdg_toplevel::req::set_min_size(&mut n.conn, n.top, min.0, min.1);
        // The compositor's frame (title bar, buttons, shadow) rather than our own.
        if let Some((name, _)) = find(&n, "zxdg_decoration_manager_v1") {
            let mgr = n.bind(reg, name, "zxdg_decoration_manager_v1", 1, K::Other);
            let deco = n.new_id(K::Other);
            zxdg_decoration_manager_v1::req::get_toplevel_decoration(&mut n.conn, mgr, deco, n.top);
            zxdg_toplevel_decoration_v1::req::set_mode(&mut n.conn, deco, 2);
        }
        wl_surface::req::commit(&mut n.conn, n.surface);
        n.roundtrip()?;
        Ok(n)
    }

    fn new_id(&mut self, kind: K) -> ObjId {
        let id = self.next_id;
        self.next_id += 1;
        self.objs.insert(id, kind);
        id
    }

    fn bind(&mut self, reg: ObjId, name: u32, iface: &str, ver: u32, kind: K) -> ObjId {
        let id = self.new_id(kind);
        wl_registry::req::bind(&mut self.conn, reg, name, (iface, ver, id));
        id
    }

    fn roundtrip(&mut self) -> std::io::Result<()> {
        let cb = self.new_id(K::Sync);
        wl_display::req::sync(&mut self.conn, 1, cb);
        self.synced = false;
        let deadline = sys::now_ns() + 5_000_000_000;
        while !self.synced {
            self.wait(100)?;
            if sys::now_ns() > deadline {
                return Err(err("compositor did not answer"));
            }
        }
        Ok(())
    }

    /// Waits up to `ms` (-1: forever) for the compositor, then handles what came.
    fn wait(&mut self, ms: i32) -> std::io::Result<()> {
        self.conn.flush()?;
        let mut ep = sys::Epoll::new()?;
        ep.add(self.conn.raw_fd(), 1, sys::EPOLLIN)?;
        for fd in &self.extra {
            let _ = ep.add(*fd, 2, sys::EPOLLIN);
        }
        let mut evs = Vec::new();
        ep.wait(ms, &mut evs)?;
        self.read()
    }

    fn read(&mut self) -> std::io::Result<()> {
        loop {
            match self.conn.read() {
                Ok(0) => return Err(err("compositor closed the connection")),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
            while let Some(h) = self.conn.next_msg().map_err(|e| err(&format!("{e:?}")))? {
                if h.obj == 1 {
                    let mut a = self.conn.args(&h);
                    match wl_display::Event::parse(h.op, &mut a) {
                        Ok(wl_display::Event::Error { object_id, code, message }) => {
                            return Err(err(&format!("protocol error on {object_id} code {code}: {message}")));
                        }
                        Ok(wl_display::Event::DeleteId { id }) => {
                            self.objs.remove(&id);
                        }
                        _ => {}
                    }
                    continue;
                }
                let Some(kind) = self.objs.get(&h.obj).copied() else { continue };
                self.event(kind, h);
            }
        }
    }

    fn event(&mut self, kind: K, h: wl::wire::MsgHdr) {
        macro_rules! ev {
            ($m:ident) => {{
                let mut a = self.conn.args(&h);
                match $m::Event::parse(h.op, &mut a) {
                    Ok(e) => e,
                    Err(_) => return,
                }
            }};
        }
        match kind {
            K::Registry => {
                if let wl_registry::Event::Global { name, interface, version } = ev!(wl_registry) {
                    self.globals.push((name, interface, version));
                }
            }
            K::Sync => self.synced = true,
            K::Frame => self.frame_cb = 0,
            K::WmBase => {
                if let xdg_wm_base::Event::Ping { serial } = ev!(xdg_wm_base) {
                    xdg_wm_base::req::pong(&mut self.conn, self.wm_base, serial);
                }
            }
            K::Xdg => {
                if let xdg_surface::Event::Configure { serial } = ev!(xdg_surface) {
                    xdg_surface::req::ack_configure(&mut self.conn, self.xdg, serial);
                    if let Some((w, h)) = self.pending.take().filter(|s| *s != (self.w, self.h)) {
                        self.w = w;
                        self.h = h;
                        self.drop_buffers();
                    }
                    self.configured = true;
                    self.maximized = self.pending_max;
                    self.events.push(Ev::Resized);
                }
            }
            K::Toplevel => match ev!(xdg_toplevel) {
                xdg_toplevel::Event::Configure { width, height, states } => {
                    self.pending_max = states.chunks_exact(4).any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == 1);
                    if width > 0 && height > 0 {
                        self.pending = Some((width, height));
                    }
                }
                xdg_toplevel::Event::Close => self.events.push(Ev::Close),
                _ => {}
            },
            K::Buffer(i) => {
                if let Some(b) = self.bufs.get_mut(i) {
                    b.busy = false;
                }
            }
            K::Seat => {
                if let wl_seat::Event::Capabilities { capabilities } = ev!(wl_seat) {
                    if capabilities & 1 != 0 && self.pointer == 0 {
                        self.pointer = self.new_id(K::Pointer);
                        wl_seat::req::get_pointer(&mut self.conn, self.seat, self.pointer);
                    }
                    if capabilities & 2 != 0 && self.keyboard == 0 {
                        self.keyboard = self.new_id(K::Keyboard);
                        wl_seat::req::get_keyboard(&mut self.conn, self.seat, self.keyboard);
                    }
                }
            }
            K::Pointer => match ev!(wl_pointer) {
                wl_pointer::Event::Enter { surface_x, surface_y, .. } | wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                    self.events.push(Ev::Motion(surface_x.to_f64(), surface_y.to_f64()))
                }
                wl_pointer::Event::Leave { .. } => self.events.push(Ev::Leave),
                wl_pointer::Event::Button { serial, button, state, .. } => {
                    if state == 1 {
                        self.btn_serial = serial;
                    }
                    self.events.push(Ev::Button(button, state == 1))
                }
                wl_pointer::Event::Axis { axis: 0, value, .. } => self.events.push(Ev::Wheel(value.to_f64())),
                _ => {}
            },
            K::Keyboard => match ev!(wl_keyboard) {
                wl_keyboard::Event::Key { key, state, .. } => {
                    let down = state == 1;
                    if down && !is_modifier(key) && self.repeat.0 > 0 {
                        self.held = Some((key, sys::now_ns() + self.repeat.1.max(1) as u64 * 1_000_000));
                    } else if !down && self.held.map_or(false, |h| h.0 == key) {
                        self.held = None;
                    }
                    self.events.push(Ev::Key(key, down));
                }
                wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                    self.mods = (mods_depressed | mods_latched | mods_locked, group);
                    let m = self.mods();
                    self.events.push(Ev::Mods(m));
                }
                wl_keyboard::Event::RepeatInfo { rate, delay } => self.repeat = (rate, delay),
                wl_keyboard::Event::Leave { .. } => {
                    self.held = None;
                    self.mods.0 &= 2;
                }
                _ => {}
            },
            K::Other => {}
        }
    }

    fn drop_buffers(&mut self) {
        for b in self.bufs.drain(..) {
            wl_buffer::req::destroy(&mut self.conn, b.buffer);
        }
    }

    fn make_buffers(&mut self) {
        let (w, h) = (self.w, self.h);
        let stride = w * 4;
        let size = (stride * h) as usize;
        for _ in 0..2 {
            let Ok(fd) = sys::memfd("resc-app", size) else { continue };
            let Ok(map) = sys::Mmap::map(fd.as_fd(), size, true, true) else { continue };
            let pool = self.new_id(K::Other);
            wl_shm::req::create_pool(&mut self.conn, self.shm, pool, fd.as_fd(), size as i32);
            let buffer = self.new_id(K::Buffer(self.bufs.len()));
            // With alpha: our apps are see-through where the compositor's glass should show.
            wl_shm_pool::req::create_buffer(&mut self.conn, pool, buffer, 0, w, h, stride, crate::shm::FMT_ARGB8888);
            wl_shm_pool::req::destroy(&mut self.conn, pool);
            self.bufs.push(Buf { buffer, map, _fd: fd, busy: false });
        }
    }

    /// Waits for input (up to `ms`, -1 forever) and returns what happened.
    pub fn poll(&mut self, ms: i32) -> std::io::Result<Vec<Ev>> {
        if self.events.is_empty() {
            // A held key repeats: wake up for it.
            let ms = match self.held {
                Some((_, at)) => {
                    let left = (at.saturating_sub(sys::now_ns()) / 1_000_000) as i32;
                    if ms < 0 { left } else { ms.min(left) }
                }
                None => ms,
            };
            self.wait(ms)?;
        }
        if let Some((key, at)) = self.held {
            let now = sys::now_ns();
            if now >= at {
                self.events.push(Ev::Key(key, true));
                self.held = Some((key, now + 1_000_000_000 / self.repeat.0.max(1) as u64));
            }
        }
        Ok(std::mem::take(&mut self.events))
    }

    /// Wake `poll` when `fd` is readable too (`None`: stop watching it).
    pub fn watch_fd(&mut self, fd: i32, on: bool) {
        self.extra.retain(|f| *f != fd);
        if on {
            self.extra.push(fd);
        }
    }

    /// The compositor moves the window with the pointer (a press on our own title row).
    pub fn begin_move(&mut self) {
        xdg_toplevel::req::r#move(&mut self.conn, self.top, self.seat, self.btn_serial);
        let _ = self.conn.flush();
    }

    pub fn minimize(&mut self) {
        xdg_toplevel::req::set_minimized(&mut self.conn, self.top);
        let _ = self.conn.flush();
    }

    pub fn toggle_maximize(&mut self) {
        if self.maximized {
            xdg_toplevel::req::unset_maximized(&mut self.conn, self.top);
        } else {
            xdg_toplevel::req::set_maximized(&mut self.conn, self.top);
        }
        let _ = self.conn.flush();
    }

    pub fn set_title(&mut self, t: &str) {
        xdg_toplevel::req::set_title(&mut self.conn, self.top, t);
        let _ = self.conn.flush();
    }

    /// The modifier state for text editing and shortcuts.
    pub fn mods(&self) -> crate::textedit::Mods {
        let m = self.mods.0;
        crate::textedit::Mods { shift: m & 1 != 0, caps: m & 2 != 0, ctrl: m & 4 != 0, alt: m & 8 != 0, group: self.mods.1 }
    }

    /// Ready for a new frame (configured, the last one shown, a buffer free).
    pub fn ready(&self) -> bool {
        self.configured && self.frame_cb == 0 && (self.bufs.is_empty() || self.bufs.iter().any(|b| !b.busy))
    }

    /// Draws a whole frame with `f` and shows it.
    pub fn draw(&mut self, f: impl FnOnce(&mut Canvas)) {
        if !self.ready() {
            return;
        }
        if self.bufs.is_empty() {
            self.make_buffers();
        }
        let Some(i) = self.bufs.iter().position(|b| !b.busy) else { return };
        let (w, h) = (self.w, self.h);
        {
            let b = &mut self.bufs[i];
            let bytes = b.map.as_mut_slice();
            // memfd mappings are page aligned: a u32 view is fine.
            let px = unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u32, (w * h) as usize) };
            let mut cv = Canvas { px, w, h, stride: w as usize };
            f(&mut cv);
            b.busy = true;
        }
        let buffer = self.bufs[i].buffer;
        wl_surface::req::attach(&mut self.conn, self.surface, buffer, 0, 0);
        wl_surface::req::damage_buffer(&mut self.conn, self.surface, 0, 0, w, h);
        self.frame_cb = self.new_id(K::Frame);
        wl_surface::req::frame(&mut self.conn, self.surface, self.frame_cb);
        wl_surface::req::commit(&mut self.conn, self.surface);
        let _ = self.conn.flush();
    }
}
