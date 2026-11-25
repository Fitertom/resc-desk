//! Nested backend: we are a Wayland client of a host compositor (e.g. WSLg).
//! Each output is a host toplevel window; rendering goes into double-buffered
//! shm buffers with per-buffer damage history; host frame callbacks pace us.
use super::{take_damage, Backend};
use crate::desk::{Desk, TOK_BACKEND};
use crate::geom::{Damage, Rect};
use crate::render::{render_output, Canvas};
use crate::state::Keymap;
use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wl::protocol::*;
use wl::{Conn, Iface, ObjId};

#[derive(Clone, Copy, Debug)]
enum HObj {
    Registry,
    Sync,
    Frame(usize),
    Surface(usize),
    Xdg(usize),
    Toplevel(usize),
    Buffer(usize, usize),
    WmBase,
    Seat,
    Pointer,
    Keyboard,
    Other,
}

struct HBuf {
    buffer: ObjId,
    map: sys::Mmap,
    _fd: OwnedFd,
    busy: bool,
    damage: Damage,
}

struct HostOut {
    out_id: u32,
    surface: ObjId,
    xdg: ObjId,
    toplevel: ObjId,
    bufs: Vec<HBuf>,
    w: i32,
    h: i32,
    pending_size: Option<(i32, i32)>,
    configured: bool,
    frame_cb: ObjId,
    last: usize,
}

pub struct Nested {
    conn: Conn,
    next_id: u32,
    objs: HashMap<ObjId, HObj>,
    globals: Vec<(u32, String, u32)>,
    compositor: ObjId,
    shm: ObjId,
    wm_base: ObjId,
    seat: ObjId,
    pointer: ObjId,
    keyboard: ObjId,
    seat_ver: u32,
    outs: Vec<HostOut>,
    ptr_out: Option<usize>,
    synced: bool,
    axis_v120: [Option<i32>; 2],
    axis_source: Option<u32>,
}

impl Nested {
    pub fn new(d: &mut Desk, spec: &str) -> std::io::Result<Self> {
        let disp = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = if disp.starts_with('/') {
            disp
        } else {
            format!("{}/{}", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()), disp)
        };
        let stream = UnixStream::connect(&path)?;
        stream.set_nonblocking(true)?;
        let fd: OwnedFd = stream.into();
        let mut n = Nested {
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
            seat_ver: 0,
            outs: vec![],
            ptr_out: None,
            synced: false,
            axis_v120: [None; 2],
            axis_source: None,
        };
        let reg = n.new_id(HObj::Registry);
        wl_display::req::get_registry(&mut n.conn, 1, reg);
        n.roundtrip(d)?;
        let find = |n: &Nested, name: &str| n.globals.iter().find(|g| g.1 == name).map(|g| (g.0, g.2));
        let (cname, cver) = find(&n, "wl_compositor").ok_or_else(|| err("host has no wl_compositor"))?;
        let (sname, _) = find(&n, "wl_shm").ok_or_else(|| err("host has no wl_shm"))?;
        let (wname, wver) = find(&n, "xdg_wm_base").ok_or_else(|| err("host has no xdg_wm_base"))?;
        n.compositor = n.bind(reg, cname, "wl_compositor", cver.min(4), HObj::Other);
        n.shm = n.bind(reg, sname, "wl_shm", 1, HObj::Other);
        n.wm_base = n.bind(reg, wname, "xdg_wm_base", wver.min(3), HObj::WmBase);
        if let Some((name, ver)) = find(&n, "wl_seat") {
            n.seat_ver = ver.min(7);
            n.seat = n.bind(reg, name, "wl_seat", n.seat_ver, HObj::Seat);
        }
        n.roundtrip(d)?;
        for (i, s) in super::parse_outputs(spec).into_iter().enumerate() {
            let idx = d.add_output(format!("WL-{}", i + 1), s.w, s.h, &s, 60000);
            n.create_window(d.outputs[idx].id, s.w, s.h, i);
        }
        n.roundtrip(d)?;
        d.ep.add(n.conn.raw_fd(), TOK_BACKEND, sys::EPOLLIN)?;
        Ok(n)
    }

    fn new_id(&mut self, kind: HObj) -> ObjId {
        let id = self.next_id;
        self.next_id += 1;
        self.objs.insert(id, kind);
        id
    }

    fn bind(&mut self, reg: ObjId, name: u32, iface: &str, ver: u32, kind: HObj) -> ObjId {
        let id = self.new_id(kind);
        wl_registry::req::bind(&mut self.conn, reg, name, (iface, ver, id));
        id
    }

    fn roundtrip(&mut self, d: &mut Desk) -> std::io::Result<()> {
        let cb = self.new_id(HObj::Sync);
        wl_display::req::sync(&mut self.conn, 1, cb);
        self.synced = false;
        let mut ep = sys::Epoll::new()?;
        ep.add(self.conn.raw_fd(), 1, sys::EPOLLIN)?;
        let mut evs = Vec::new();
        let deadline = sys::now_ns() + 5_000_000_000;
        while !self.synced {
            self.conn.flush()?;
            ep.wait(100, &mut evs)?;
            self.read_events(d);
            if sys::now_ns() > deadline {
                return Err(err("host compositor did not answer"));
            }
        }
        Ok(())
    }

    fn create_window(&mut self, out_id: u32, w: i32, h: i32, i: usize) {
        let idx = self.outs.len();
        let surface = self.new_id(HObj::Surface(idx));
        wl_compositor::req::create_surface(&mut self.conn, self.compositor, surface);
        let xdg = self.new_id(HObj::Xdg(idx));
        xdg_wm_base::req::get_xdg_surface(&mut self.conn, self.wm_base, xdg, surface);
        let top = self.new_id(HObj::Toplevel(idx));
        xdg_surface::req::get_toplevel(&mut self.conn, xdg, top);
        xdg_toplevel::req::set_title(&mut self.conn, top, &format!("resc-desk — monitor {}", i + 1));
        xdg_toplevel::req::set_app_id(&mut self.conn, top, "resc-desk");
        xdg_toplevel::req::set_min_size(&mut self.conn, top, 320, 240);
        wl_surface::req::commit(&mut self.conn, surface);
        self.outs.push(HostOut {
            out_id,
            surface,
            xdg,
            toplevel: top,
            bufs: vec![],
            w,
            h,
            pending_size: None,
            configured: false,
            frame_cb: 0,
            last: 0,
        });
    }

    fn make_buffers(&mut self, oi: usize) {
        let (w, h) = (self.outs[oi].w, self.outs[oi].h);
        for b in self.outs[oi].bufs.drain(..) {
            wl_buffer::req::destroy(&mut self.conn, b.buffer);
        }
        let stride = w * 4;
        let size = (stride * h) as usize;
        for _ in 0..2 {
            let Ok(fd) = sys::memfd("resc-desk-out", size) else { continue };
            let Ok(map) = sys::Mmap::map(fd.as_fd(), size, true, true) else { continue };
            let pool = self.next_id;
            self.next_id += 1;
            self.objs.insert(pool, HObj::Other);
            wl_shm::req::create_pool(&mut self.conn, self.shm, pool, fd.as_fd(), size as i32);
            let buffer = self.new_id(HObj::Buffer(oi, self.outs[oi].bufs.len()));
            wl_shm_pool::req::create_buffer(&mut self.conn, pool, buffer, 0, w, h, stride, crate::shm::FMT_XRGB8888);
            wl_shm_pool::req::destroy(&mut self.conn, pool);
            let mut damage = Damage::default();
            damage.add(Rect::new(0, 0, w, h));
            self.outs[oi].bufs.push(HBuf { buffer, map, _fd: fd, busy: false, damage });
        }
    }

    fn out_by_id(&self, d: &Desk, ho: usize) -> Option<usize> {
        d.output_index(self.outs.get(ho)?.out_id)
    }

    fn global_pos(&self, d: &Desk, ho: usize, sx: f64, sy: f64) -> Option<(f64, f64)> {
        let oi = self.out_by_id(d, ho)?;
        let o = &d.outputs[oi];
        Some((o.x as f64 + sx / o.scale, o.y as f64 + sy / o.scale))
    }

    fn read_events(&mut self, d: &mut Desk) {
        loop {
            match self.conn.read() {
                Ok(0) => {
                    eprintln!("nested: host compositor closed the connection");
                    d.running = false;
                    return;
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("nested: read: {e}");
                    d.running = false;
                    return;
                }
            }
            loop {
                let h = match self.conn.next_msg() {
                    Ok(Some(h)) => h,
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("nested: framing {e:?}");
                        d.running = false;
                        return;
                    }
                };
                if h.obj == 1 {
                    let mut a = self.conn.args(&h);
                    match wl_display::Event::parse(h.op, &mut a) {
                        Ok(wl_display::Event::Error { object_id, code, message }) => {
                            eprintln!("nested: host protocol error on {object_id} code {code}: {message}");
                            d.running = false;
                        }
                        Ok(wl_display::Event::DeleteId { id }) => {
                            self.objs.remove(&id);
                        }
                        _ => {}
                    }
                    continue;
                }
                let Some(kind) = self.objs.get(&h.obj).copied() else { continue };
                self.event(d, kind, h);
            }
        }
    }

    fn event(&mut self, d: &mut Desk, kind: HObj, h: wl::wire::MsgHdr) {
        macro_rules! ev {
            ($m:ident) => {{
                let mut a = self.conn.args(&h);
                match $m::Event::parse(h.op, &mut a) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("nested: bad {} event: {e:?}", stringify!($m));
                        return;
                    }
                }
            }};
        }
        match kind {
            HObj::Registry => {
                if let wl_registry::Event::Global { name, interface, version } = ev!(wl_registry) {
                    self.globals.push((name, interface, version));
                }
            }
            HObj::Sync => self.synced = true,
            HObj::Frame(ho) => {
                self.outs[ho].frame_cb = 0;
                if let Some(oi) = self.out_by_id(d, ho) {
                    d.outputs[oi].frame_pending = false;
                }
            }
            HObj::WmBase => {
                if let xdg_wm_base::Event::Ping { serial } = ev!(xdg_wm_base) {
                    xdg_wm_base::req::pong(&mut self.conn, self.wm_base, serial);
                }
            }
            HObj::Xdg(ho) => {
                if let xdg_surface::Event::Configure { serial } = ev!(xdg_surface) {
                    xdg_surface::req::ack_configure(&mut self.conn, self.outs[ho].xdg, serial);
                    let resize = self.outs[ho].pending_size.take().filter(|s| *s != (self.outs[ho].w, self.outs[ho].h));
                    if let Some((w, h)) = resize {
                        self.outs[ho].w = w;
                        self.outs[ho].h = h;
                        self.outs[ho].bufs.clear();
                        if let Some(oi) = self.out_by_id(d, ho) {
                            d.set_output_mode(oi, w, h);
                        }
                    }
                    if self.outs[ho].bufs.is_empty() {
                        self.make_buffers(ho);
                    }
                    self.outs[ho].configured = true;
                    if let Some(oi) = self.out_by_id(d, ho) {
                        let r = d.outputs[oi].rect();
                        d.outputs[oi].damage.add(r);
                        if self.outs[ho].frame_cb == 0 {
                            d.outputs[oi].frame_pending = false;
                        }
                    }
                }
            }
            HObj::Toplevel(ho) => match ev!(xdg_toplevel) {
                xdg_toplevel::Event::Configure { width, height, .. } => {
                    if width > 0 && height > 0 {
                        self.outs[ho].pending_size = Some((width, height));
                    }
                }
                xdg_toplevel::Event::Close => {
                    if let Some(oi) = self.out_by_id(d, ho) {
                        d.remove_output(oi);
                    }
                    let o = &self.outs[ho];
                    xdg_toplevel::req::destroy(&mut self.conn, o.toplevel);
                    xdg_surface::req::destroy(&mut self.conn, o.xdg);
                    wl_surface::req::destroy(&mut self.conn, o.surface);
                    self.outs[ho].bufs.clear();
                    self.outs[ho].configured = false;
                    if d.outputs.is_empty() {
                        d.running = false;
                    }
                }
                _ => {}
            },
            HObj::Buffer(ho, bi) => {
                if let Some(b) = self.outs.get_mut(ho).and_then(|o| o.bufs.get_mut(bi)) {
                    b.busy = false;
                }
                if self.outs.get(ho).map_or(false, |o| o.frame_cb == 0) {
                    if let Some(oi) = self.out_by_id(d, ho) {
                        d.outputs[oi].frame_pending = false;
                    }
                }
            }
            HObj::Seat => {
                if let wl_seat::Event::Capabilities { capabilities } = ev!(wl_seat) {
                    if capabilities & 1 != 0 && self.pointer == 0 {
                        self.pointer = self.new_id(HObj::Pointer);
                        wl_seat::req::get_pointer(&mut self.conn, self.seat, self.pointer);
                    }
                    if capabilities & 2 != 0 && self.keyboard == 0 {
                        self.keyboard = self.new_id(HObj::Keyboard);
                        wl_seat::req::get_keyboard(&mut self.conn, self.seat, self.keyboard);
                    }
                }
            }
            HObj::Pointer => {
                let e = ev!(wl_pointer);
                self.pointer_event(d, e)
            }
            HObj::Keyboard => match ev!(wl_keyboard) {
                wl_keyboard::Event::Keymap { format: _, fd, size } => {
                    d.keymap = Some(Keymap { fd, size });
                    crate::input::broadcast_keymap(d);
                }
                wl_keyboard::Event::Leave { .. } => crate::input::release_all_keys(d),
                wl_keyboard::Event::Key { time, key, state, .. } => crate::input::key(d, key, state == 1, time),
                wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                    crate::input::host_modifiers(d, mods_depressed, mods_latched, mods_locked, group)
                }
                wl_keyboard::Event::RepeatInfo { rate, delay } => d.repeat = (rate, delay),
                _ => {}
            },
            HObj::Surface(_) | HObj::Other => {}
        }
    }

    fn pointer_event(&mut self, d: &mut Desk, e: wl_pointer::Event) {
        use wl_pointer::Event as E;
        match e {
            E::Enter { serial, surface, surface_x, surface_y } => {
                self.ptr_out = self.outs.iter().position(|o| o.surface == surface);
                // Hide the host cursor; we draw our own.
                wl_pointer::req::set_cursor(&mut self.conn, self.pointer, serial, 0, 0, 0);
                if let Some(ho) = self.ptr_out {
                    if let Some((x, y)) = self.global_pos(d, ho, surface_x.to_f64(), surface_y.to_f64()) {
                        let t = d.now_ms();
                        crate::input::pointer_motion(d, x, y, t);
                    }
                }
            }
            E::Leave { .. } => {
                // Keep ptr_out while buttons are held (host implicit grab continues).
                if d.seat.buttons.is_empty() {
                    self.ptr_out = None;
                }
            }
            E::Motion { time, surface_x, surface_y } => {
                if let Some(ho) = self.ptr_out {
                    if let Some((x, y)) = self.global_pos(d, ho, surface_x.to_f64(), surface_y.to_f64()) {
                        crate::input::pointer_motion(d, x, y, time);
                    }
                }
            }
            E::Button { time, button, state, .. } => crate::input::pointer_button(d, button, state == 1, time),
            E::AxisSource { axis_source } => self.axis_source = Some(axis_source),
            E::AxisDiscrete { axis, discrete } => {
                if let Some(s) = self.axis_v120.get_mut(axis as usize) {
                    *s = Some(discrete * 120);
                }
            }
            E::AxisValue120 { axis, value120 } => {
                if let Some(s) = self.axis_v120.get_mut(axis as usize) {
                    *s = Some(value120);
                }
            }
            E::Axis { time, axis, value } => {
                let v120 = self.axis_v120.get_mut(axis as usize).and_then(|s| s.take());
                crate::input::pointer_axis(d, time, axis, value.to_f64(), v120, self.axis_source);
            }
            E::AxisStop { time, axis } => crate::input::pointer_axis(d, time, axis, 0.0, None, self.axis_source),
            E::Frame => {
                self.axis_source = None;
                self.axis_v120 = [None; 2];
            }
            _ => {}
        }
    }
}

fn err(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, m.to_string())
}

impl Backend for Nested {
    fn dispatch(&mut self, d: &mut Desk, _token: u64) {
        self.read_events(d);
        let _ = self.conn.flush();
    }

    fn present(&mut self, d: &mut Desk, oi: usize) {
        let id = d.outputs[oi].id;
        let Some(ho) = self.outs.iter().position(|o| o.out_id == id) else { return };
        // Not ready: park the output until configure / frame done / buffer release
        // (otherwise the main loop would spin on the pending damage).
        let free = (0..self.outs[ho].bufs.len()).find(|i| !self.outs[ho].bufs[*i].busy);
        if !self.outs[ho].configured || self.outs[ho].frame_cb != 0 || free.is_none() {
            d.outputs[oi].frame_pending = true;
            return;
        }
        let bi = free.unwrap();
        let dmg = take_damage(d, oi);
        // Every other buffer misses this frame's damage: remember it.
        for (i, b) in self.outs[ho].bufs.iter_mut().enumerate() {
            if i != bi {
                b.damage.extend(&dmg);
            }
        }
        let (w, h) = (self.outs[ho].w, self.outs[ho].h);
        let b = &mut self.outs[ho].bufs[bi];
        let mut full = std::mem::take(&mut b.damage);
        full.extend(&dmg);
        let px = unsafe { std::slice::from_raw_parts_mut(b.map.ptr as *mut u32, (w * h) as usize) };
        let mut cv = Canvas { px, w, h, stride: w as usize };
        let painted = render_output(d, oi, &mut cv, &full);
        let (surface, buffer) = (self.outs[ho].surface, self.outs[ho].bufs[bi].buffer);
        wl_surface::req::attach(&mut self.conn, surface, buffer, 0, 0);
        for r in &painted {
            wl_surface::req::damage_buffer(&mut self.conn, surface, r.x, r.y, r.w, r.h);
        }
        let cb = self.new_id(HObj::Frame(ho));
        wl_surface::req::frame(&mut self.conn, surface, cb);
        wl_surface::req::commit(&mut self.conn, surface);
        self.outs[ho].frame_cb = cb;
        self.outs[ho].bufs[bi].busy = true;
        self.outs[ho].last = bi;
        let o = &mut d.outputs[oi];
        o.frame_pending = true;
        o.last_frame_ns = sys::now_ns();
        o.frames += 1;
        let _ = self.conn.flush();
    }

    fn read_pixels(&mut self, d: &mut Desk, oi: usize) -> Option<(Vec<u32>, i32, i32)> {
        let id = d.outputs.get(oi)?.id;
        let o = self.outs.iter().find(|o| o.out_id == id)?;
        let b = o.bufs.get(o.last)?;
        let n = (o.w * o.h) as usize;
        let px = unsafe { std::slice::from_raw_parts(b.map.ptr as *const u32, n) };
        Some((px.to_vec(), o.w, o.h))
    }

    fn flush(&mut self) {
        let _ = self.conn.flush();
    }

    fn name(&self) -> &'static str {
        "nested"
    }
}

#[allow(dead_code)]
fn _iface_check() -> Iface {
    Iface::WlDisplay
}
