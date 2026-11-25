//! The compositor state (`Desk`) and cross-cutting helpers: clients, globals,
//! outputs, damage, serials, surface tree geometry.
use crate::geom::Rect;
use crate::input::Seat;
use crate::slab::Slab;
use crate::state::*;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use wl::protocol::*;
use wl::{Conn, Iface, ObjId};

pub const TOK_LISTENER: u64 = 1;
pub const TOK_IPC: u64 = 2;
pub const TOK_TIMER: u64 = 3;
pub const TOK_BACKEND: u64 = 1 << 56;
pub const TOK_CLIENT: u64 = 2 << 56;
pub const TOK_IPC_CLIENT: u64 = 3 << 56;
pub const TOK_CLIP: u64 = 4 << 56;
pub const TOK_XWM: u64 = 5 << 56;
/// The PulseAudio connection (tray.rs).
pub const TOK_PULSE: u64 = 6 << 56;
pub const TOK_KIND_MASK: u64 = 0xff << 56;

pub fn client_token(k: CKey) -> u64 {
    TOK_CLIENT | ((k.gen as u64 & 0xff_ffff) << 24) | (k.idx as u64 & 0xff_ffff)
}
pub fn token_client(t: u64) -> CKey {
    CKey { idx: (t & 0xff_ffff) as u32, gen: ((t >> 24) & 0xff_ffff) as u32 }
}

pub struct Config {
    pub terminal: String,
    pub launcher: String,
    pub files: String,
    pub nested: bool,
}

#[derive(Default)]
pub struct Overlay {
    pub snap_preview: Option<Rect>,
    /// Alt+Tab: windows in MRU order and the highlighted index.
    pub switcher: Option<(Vec<WKey>, usize)>,
    pub start_menu: Option<usize>,
    /// Window preview over a hovered taskbar button.
    pub peek: Option<crate::shell::Peek>,
    /// Taskbar springs per output, last tick time, and whether anything still moves.
    pub tb_anim: Vec<crate::shell::TbAnim>,
    /// Task view (Win+Tab).
    pub taskview: Option<crate::taskview::TaskView>,
    pub anim_ms: u32,
    pub animating: bool,
    /// Animations' clock rate (`ctl slowmo 0.2`: five times slower, for recording); 0: normal.
    pub slowmo: f32,
    /// The start-up animation, while it runs (boot.rs).
    pub boot: Option<Box<crate::boot::Boot>>,
    /// Programs coming and going: taskbar buttons, launches, closed windows (life.rs).
    pub life: crate::life::Life,
    /// Alt+Tab's drops (alttab.rs; GPU path).
    pub alttab: Option<Box<crate::alttab::At>>,
}

pub struct Desk {
    pub ep: sys::Epoll,
    pub listener: UnixListener,
    pub socket_name: String,
    pub socket_path: String,
    pub clients: Slab<Client>,
    pub surfaces: Slab<Surface>,
    pub windows: Slab<Window>,
    pub sources: Slab<DataSource>,
    pub outputs: Vec<Output>,
    pub globals: Vec<Global>,
    /// Stacking order bottom → top (all workspaces).
    pub stack: Vec<WKey>,
    /// Most-recently-used focus order, front = most recent.
    pub mru: Vec<WKey>,
    pub workspace: u32,
    pub workspaces: u32,
    pub seat: Seat,
    pub serial: u32,
    pub keymap: Option<Keymap>,
    pub repeat: (i32, i32),
    pub selection: Option<DKey>,
    /// A drag and drop in progress (dnd.rs).
    pub dnd: Option<crate::dnd::Dnd>,
    /// The next window of this app id opens with point (x, y) of it under the pointer
    /// (a tab torn off «Проводник»); until the deadline (ms).
    pub place_next: Option<(String, i32, i32, u32)>,
    /// Primary selection: the last text selected, pasted with the middle button.
    pub primary: Option<DKey>,
    pub children: Vec<std::process::Child>,
    pub running: bool,
    /// Start again (the program file anew) once stopped: `ctl restart`, after an update.
    pub restart: bool,
    pub next_output_id: u32,
    pub start_ns: u64,
    pub cfg: Config,
    pub overlay: Overlay,
    pub cursor_drawn: Rect,
    pub show_desktop: Vec<WKey>,
    /// (output, taskbar item) under the pointer.
    pub overlay_hover: Option<(usize, usize)>,
    pub start_hover: Option<usize>,
    pub start_field: crate::textedit::TextField,
    pub start_sel: usize,
    pub start: crate::start::Start,
    /// The taskbar's right island: sound, calendar.
    pub tray: crate::tray::Tray,
    pub apps: Vec<crate::apps::App>,
    pub pending_screenshot: Option<String>,
    pub display_cfg: crate::layout::DisplayConfig,
    /// Logical px per millimetre of the monitor plane, and the plane origin (mm).
    pub mm_k: f64,
    pub mm_origin: (f64, f64),
    pub calib: Option<crate::layout::Calib>,
    /// Logical crop for the next "all" screenshot (IPC).
    pub shot_crop: Option<Rect>,
    pub wall: crate::wallpaper::Wall,
    pub clip: crate::clipboard::Clip,
    /// Monitors: the Win+P panel, a reconfiguration to do (monitors.rs).
    pub mons: crate::monitors::State,
    /// Vulkan renderer; None: the CPU draws everything (gpu.rs `init`).
    pub gpu: Option<Box<crate::gpu::Gpu>>,
    /// X programs through Xwayland (xwm.rs); None when Xwayland is not installed or off.
    pub xwm: Option<Box<crate::xwm::Xwm>>,
    /// Wayland programs asking for what an X program copied: (source, type, where to write).
    /// xwm takes them on its next turn.
    pub x_receives: Vec<(DKey, String, OwnedFd)>,
    /// What clients may hand over as dma-bufs (dmabuf.rs); None: not offered.
    pub dmabuf: Option<crate::dmabuf::Support>,
}

impl Desk {
    pub fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }
    pub fn now_ms(&self) -> u32 {
        ((sys::now_ns() - self.start_ns) / 1_000_000) as u32
    }

    // ---------------------------------------------------------------- clients

    pub fn accept_clients(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    self.add_client(stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("accept: {e}");
                    break;
                }
            }
        }
    }

    /// A new Wayland client on `stream` (accepted, or a socket pair end given to Xwayland).
    pub fn add_client(&mut self, stream: std::os::unix::net::UnixStream) -> CKey {
        let _ = stream.set_nonblocking(true);
        let fd: OwnedFd = stream.into();
        let raw = fd.as_raw_fd();
        let ck = self.clients.insert(Client::new(Conn::new(fd)));
        // wl_display is always object 1.
        self.clients[ck].insert(1, Iface::WlDisplay, 1, Data::None);
        if let Err(e) = self.ep.add(raw, client_token(ck), sys::EPOLLIN) {
            eprintln!("epoll add client: {e}");
        }
        ck
    }

    pub fn flush_clients(&mut self) {
        for ck in self.clients.keys() {
            let c = &mut self.clients[ck];
            if c.conn.pending_out() == 0 {
                continue;
            }
            match c.conn.flush() {
                Ok(true) => {
                    let _ = self.ep.modify(c.conn.raw_fd(), client_token(ck), sys::EPOLLIN);
                }
                Ok(false) => {
                    if c.conn.pending_out() > wl::wire::OUT_LIMIT {
                        c.kill("client not reading its socket".into());
                    } else {
                        let _ = self.ep.modify(c.conn.raw_fd(), client_token(ck), sys::EPOLLIN | sys::EPOLLOUT);
                    }
                }
                Err(e) => c.kill(format!("write: {e}")),
            }
        }
        // Reap dead clients.
        let dead: Vec<CKey> = self.clients.iter().filter(|(_, c)| c.dead.is_some()).map(|(k, _)| k).collect();
        for ck in dead {
            self.destroy_client(ck);
        }
    }

    pub fn destroy_client(&mut self, ck: CKey) {
        if let Some(c) = self.clients.get_mut(ck) {
            if let Some(why) = &c.dead {
                if why != "eof" {
                    eprintln!("client {}: disconnected: {why}", ck.idx);
                }
            }
            let _ = c.conn.flush();
            self.ep.remove(c.conn.raw_fd());
        }
        let surfs: Vec<SKey> = self.surfaces.iter().filter(|(_, s)| s.client == ck).map(|(k, _)| k).collect();
        for sk in surfs {
            crate::server::destroy_surface(self, sk);
        }
        let srcs: Vec<DKey> = self.sources.iter().filter(|(_, s)| s.client == ck && s.own.is_none()).map(|(k, _)| k).collect();
        for k in srcs {
            crate::server::destroy_source(self, k);
        }
        self.clients.remove(ck);
        if self.xwm.as_ref().map_or(false, |x| x.client == Some(ck)) {
            crate::xwm::xwayland_gone(self, "its Wayland connection closed");
        }
    }

    /// Protocol error on object `obj`, then disconnect.
    pub fn post_error(&mut self, ck: CKey, obj: ObjId, code: u32, msg: &str) {
        if let Some(c) = self.clients.get_mut(ck) {
            if c.dead.is_none() {
                wl_display::evt::error(&mut c.conn, 1, obj, code, msg);
                c.kill(format!("protocol error on {obj}: {msg}"));
            }
        }
    }

    // ---------------------------------------------------------------- globals

    pub fn add_global(&mut self, iface: Iface, version: u32, output: Option<u32>) -> u32 {
        let name = self.globals.len() as u32 + 1;
        let g = Global { name, iface, version, output, alive: true };
        self.globals.push(g);
        for ck in self.clients.keys() {
            let regs = self.clients[ck].registries.clone();
            for r in regs {
                wl_registry::evt::global(&mut self.clients[ck].conn, r, name, iface.name(), version);
            }
        }
        name
    }

    pub fn remove_global(&mut self, name: u32) {
        if let Some(g) = self.globals.get_mut(name as usize - 1) {
            g.alive = false;
        }
        for ck in self.clients.keys() {
            let regs = self.clients[ck].registries.clone();
            for r in regs {
                wl_registry::evt::global_remove(&mut self.clients[ck].conn, r, name);
            }
        }
    }

    // ---------------------------------------------------------------- outputs

    pub fn output_index(&self, id: u32) -> Option<usize> {
        self.outputs.iter().position(|o| o.id == id)
    }

    /// Adds an output; its place and scale come from the physical plane (layout.rs).
    pub fn add_output(&mut self, name: String, pw: i32, ph: i32, spec: &crate::backend::OutSpec, refresh_mhz: i32) -> usize {
        let id = self.next_output_id;
        self.next_output_id += 1;
        let mut plane = crate::layout::initial_plane(self, &name, pw, ph, spec.mm, spec.scale, spec.pos_mm);
        // Remembered upright, back turned (or the other way round): the plane turns too.
        if (pw > ph) != (plane[2] > plane[3]) && pw != ph {
            plane.swap(2, 3);
        }
        let mut o = Output {
            id,
            name,
            x: 0,
            y: 0,
            pw,
            ph,
            scale: 1.0,
            plane,
            shift_mm: 0.0,
            refresh_mhz,
            global: 0,
            damage: Default::default(),
            frame_pending: false,
            wants_frame: true,
            last_frame_ns: 0,
            frames: 0,
        };
        self.outputs.push(o);
        let i = self.outputs.len() - 1;
        crate::layout::relayout(self);
        let r = self.outputs[i].rect();
        self.outputs[i].damage.add(r);
        let g = self.add_global(Iface::WlOutput, 4, Some(id));
        self.outputs[i].global = g;
        let o = &self.outputs[i];
        eprintln!(
            "output {}: {}x{} px, {:.0}x{:.0} mm at {:.0},{:.0} mm -> scale {:.3}, logical {},{} {}x{}",
            o.name, pw, ph, o.plane[2], o.plane[3], o.plane[0], o.plane[1], o.scale, o.x, o.y, o.lw(), o.lh()
        );
        i
    }

    pub fn remove_output(&mut self, idx: usize) {
        let o = self.outputs.remove(idx);
        // Its place stays for when it comes back (Win+P, switched on again, plugged in).
        self.display_cfg.planes.insert(o.name.clone(), o.plane);
        self.remove_global(o.global);
        for (_, w) in self.windows.iter_mut() {
            if w.output > idx {
                w.output -= 1;
            } else if w.output == idx {
                w.output = 0;
            }
        }
        if let Some(c) = self.calib.as_mut() {
            c.sel = 0;
            c.backup = self.outputs.iter().map(|o| o.plane).collect();
        }
        crate::layout::relayout(self);
        crate::wm::outputs_changed(self);
    }

    /// A monitor turned (`swap`: by 90°, its plane's width and height trade places) and/or
    /// given a new mode: the output becomes `pw`×`ph`. The monitor is the same size on the
    /// desk, so its plane stays (its pixels get bigger or smaller).
    pub fn reshape_output(&mut self, idx: usize, pw: i32, ph: i32, swap: bool) {
        let o = &mut self.outputs[idx];
        if swap {
            o.plane.swap(2, 3);
        }
        o.pw = pw;
        o.ph = ph;
        crate::layout::relayout(self);
        crate::layout::broadcast_output(self, idx);
        crate::wm::outputs_changed(self);
    }

    /// Mode change: the monitor's pixel density stays, so its plane grows/shrinks.
    pub fn set_output_mode(&mut self, idx: usize, pw: i32, ph: i32) {
        let o = &mut self.outputs[idx];
        if o.pw == pw && o.ph == ph {
            return;
        }
        let mm_per_px = o.plane[2] / o.pw.max(1) as f64;
        o.plane[2] = pw as f64 * mm_per_px;
        o.plane[3] = ph as f64 * mm_per_px;
        o.pw = pw;
        o.ph = ph;
        crate::layout::relayout(self);
        crate::layout::broadcast_output(self, idx);
        crate::wm::outputs_changed(self);
    }

    pub fn output_at(&self, x: f64, y: f64) -> Option<usize> {
        self.outputs.iter().position(|o| o.rect().contains(x, y))
    }

    /// Output with the largest overlap with `r` (falls back to nearest / first).
    pub fn output_for_rect(&self, r: &Rect) -> usize {
        let mut best = (0usize, -1i64);
        for (i, o) in self.outputs.iter().enumerate() {
            let a = o.rect().intersect(r).area();
            if a > best.1 {
                best = (i, a);
            }
        }
        if best.1 <= 0 {
            let (cx, cy) = r.center();
            let mut nd = i64::MAX;
            for (i, o) in self.outputs.iter().enumerate() {
                let (ox, oy) = o.rect().center();
                let d = (ox - cx) as i64 * (ox - cx) as i64 + (oy - cy) as i64 * (oy - cy) as i64;
                if d < nd {
                    nd = d;
                    best.0 = i;
                }
            }
        }
        best.0
    }

    /// Bounding box of all outputs.
    pub fn layout_bounds(&self) -> Rect {
        self.outputs.iter().fold(Rect::default(), |a, o| a.union(&o.rect()))
    }

    // ---------------------------------------------------------------- damage

    /// Damage a rect in global logical coords on every output it touches.
    pub fn damage(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        for o in self.outputs.iter_mut() {
            let i = o.rect().intersect(&r);
            if !i.is_empty() {
                o.damage.add(i);
            }
        }
    }
    pub fn damage_all(&mut self) {
        for o in self.outputs.iter_mut() {
            let r = o.rect();
            o.damage.add(r);
        }
    }

    // ---------------------------------------------------------------- surface tree

    /// Window owning `sk` (walking up subsurfaces and popups).
    pub fn window_of(&self, mut sk: SKey) -> Option<WKey> {
        for _ in 0..64 {
            let s = self.surfaces.get(sk)?;
            match s.role {
                Role::Toplevel(w) => return Some(w),
                Role::Subsurface => sk = s.parent?,
                Role::Popup => sk = s.popup.as_ref()?.parent,
                _ => return None,
            }
        }
        None
    }

    /// Global logical position of the surface's (0,0), if it is placed in the scene.
    pub fn surface_origin(&self, sk: SKey) -> Option<(i32, i32)> {
        let s = self.surfaces.get(sk)?;
        match s.role {
            Role::Toplevel(wk) => {
                let w = self.windows.get(wk)?;
                let g = s.geom();
                Some((w.x - g.x, w.y - g.y))
            }
            Role::Subsurface => {
                let (px, py) = self.surface_origin(s.parent?)?;
                Some((px + s.sub_pos.0, py + s.sub_pos.1))
            }
            Role::Popup => {
                // X menus and tooltips: X itself says where they are.
                if let Some(at) = s.x_pos {
                    return Some(at);
                }
                let p = s.popup.as_ref()?;
                let parent = self.surfaces.get(p.parent)?;
                let (px, py) = self.surface_origin(p.parent)?;
                let pg = parent.geom();
                let g = s.geom();
                Some((px + pg.x + p.x - g.x, py + pg.y + p.y - g.y))
            }
            Role::Cursor => {
                let (hx, hy) = self.seat.cursor_hotspot;
                Some((self.seat.x as i32 - hx, self.seat.y as i32 - hy))
            }
            Role::DndIcon => {
                let o = self.dnd.as_ref()?.icon_off;
                Some((self.seat.x as i32 + o.0, self.seat.y as i32 + o.1))
            }
            _ => None,
        }
    }

    /// Whether the surface is currently shown (mapped window on current workspace, not minimized).
    pub fn surface_visible(&self, sk: SKey) -> bool {
        let Some(s) = self.surfaces.get(sk) else { return false };
        if s.buffer.is_none() {
            return false;
        }
        if s.role == Role::Cursor || s.role == Role::DndIcon {
            return true;
        }
        match self.window_of(sk) {
            Some(wk) => {
                let w = &self.windows[wk];
                !w.minimized && w.workspace == self.workspace && self.surfaces.get(w.surf).map_or(false, |s| s.mapped)
            }
            None => false,
        }
    }

    /// Damage the full extent of a surface and its subsurfaces/popups.
    pub fn damage_surface_tree(&mut self, sk: SKey) {
        let mut stack = vec![sk];
        let mut rects = vec![];
        while let Some(k) = stack.pop() {
            let Some(s) = self.surfaces.get(k) else { continue };
            if let Some((x, y)) = self.surface_origin(k) {
                rects.push(Rect::new(x, y, s.w, s.h));
            }
            stack.extend(s.subs.iter().filter(|c| **c != SKey::default()));
            stack.extend(s.popups.iter());
        }
        for r in rects {
            self.damage(r);
        }
    }

    pub fn spawn(&mut self, cmd: &str) {
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg(cmd).env("WAYLAND_DISPLAY", &self.socket_name);
        c.env("XDG_SESSION_TYPE", "wayland");
        c.env("GDK_BACKEND", "wayland");
        c.env("QT_QPA_PLATFORM", "wayland");
        c.env("MOZ_ENABLE_WAYLAND", "1");
        // Our own apps (resc-desk settings, ...) talk back through the control socket.
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        c.env("RESC_DESK_CTL", format!("{runtime}/resc-desk-{}.ctl", self.socket_name));
        match &self.xwm {
            // X programs (Steam, games) find Xwayland here; it starts when the first connects.
            // SDL games stay on X: that is the path they are made and tested for.
            Some(x) => {
                c.env("DISPLAY", format!(":{}", x.display));
                c.env_remove("SDL_VIDEODRIVER");
            }
            None => {
                c.env_remove("DISPLAY");
                c.env("SDL_VIDEODRIVER", "wayland");
            }
        }
        c.stdin(std::process::Stdio::null());
        match c.spawn() {
            Ok(ch) => self.children.push(ch),
            Err(e) => eprintln!("spawn {cmd}: {e}"),
        }
    }

    pub fn reap_children(&mut self) {
        self.children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }
}
