//! Core data model: clients, protocol objects, surfaces, windows, outputs, seat.
use crate::geom::{Damage, Rect, Region};
use crate::shm::{Buffer, Pool};
use crate::slab::{Key, Slab};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use wl::{Conn, Iface, ObjId};

pub type CKey = Key; // client
pub type SKey = Key; // surface
pub type WKey = Key; // window
pub type DKey = Key; // data source

pub const SERVER_ID_BASE: u32 = 0xff00_0000;

pub struct Obj {
    pub iface: Iface,
    pub version: u32,
    pub data: Data,
}

#[derive(Default)]
pub enum Data {
    #[default]
    None,
    Surface(SKey),
    Region(Region),
    Pool(Rc<Pool>),
    Buffer(Rc<Buffer>),
    XdgSurface(SKey),
    Toplevel(SKey),
    Popup(SKey),
    Positioner(Box<Positioner>),
    Subsurface(SKey),
    Output(u32),
    XdgOutput(u32),
    DataSource(DKey),
    DataOffer(DKey),
    Decoration(SKey),
    Viewport(SKey),
    /// A surface's glass panels (resc_glass_v1).
    Glass(SKey),
    FracScale(SKey),
    LayerSurface(SKey),
    Params(Box<crate::dmabuf::Params>),
    ScreencopyFrame(Box<crate::screencopy::FrameInfo>),
}

pub struct Client {
    pub conn: Conn,
    pub objs: Vec<Option<Obj>>,
    pub sobjs: HashMap<u32, Obj>,
    pub next_sid: u32,
    pub dead: Option<String>,
    pub pointers: Vec<ObjId>,
    /// zwp_relative_pointer_v1 objects: raw mouse motion goes to them.
    pub rel_pointers: Vec<ObjId>,
    pub keyboards: Vec<ObjId>,
    pub outputs: Vec<(u32, ObjId)>,
    pub xdg_outputs: Vec<(u32, ObjId)>,
    pub data_devices: Vec<ObjId>,
    pub primary_devices: Vec<ObjId>,
    pub registries: Vec<ObjId>,
}

impl Client {
    pub fn new(conn: Conn) -> Self {
        Self {
            conn,
            objs: Vec::new(),
            sobjs: HashMap::new(),
            next_sid: SERVER_ID_BASE,
            dead: None,
            pointers: vec![],
            rel_pointers: vec![],
            keyboards: vec![],
            outputs: vec![],
            xdg_outputs: vec![],
            data_devices: vec![],
            primary_devices: vec![],
            registries: vec![],
        }
    }
    pub fn obj(&self, id: ObjId) -> Option<&Obj> {
        if id >= SERVER_ID_BASE {
            self.sobjs.get(&id)
        } else {
            self.objs.get(id as usize).and_then(|o| o.as_ref())
        }
    }
    pub fn obj_mut(&mut self, id: ObjId) -> Option<&mut Obj> {
        if id >= SERVER_ID_BASE {
            self.sobjs.get_mut(&id)
        } else {
            self.objs.get_mut(id as usize).and_then(|o| o.as_mut())
        }
    }
    /// Registers a client-allocated id. Fails on reuse of a live id.
    pub fn insert(&mut self, id: ObjId, iface: Iface, version: u32, data: Data) -> bool {
        if id == 0 || id >= SERVER_ID_BASE || id > 4_000_000 {
            self.kill(format!("invalid new id {id}"));
            return false;
        }
        let i = id as usize;
        if self.objs.len() <= i {
            self.objs.resize_with(i + 1, || None);
        }
        if self.objs[i].is_some() {
            self.kill(format!("id {id} already in use"));
            return false;
        }
        self.objs[i] = Some(Obj { iface, version, data });
        true
    }
    pub fn insert_server(&mut self, iface: Iface, version: u32, data: Data) -> ObjId {
        let id = self.next_sid;
        self.next_sid += 1;
        self.sobjs.insert(id, Obj { iface, version, data });
        id
    }
    pub fn remove(&mut self, id: ObjId) -> Option<Obj> {
        if id >= SERVER_ID_BASE {
            self.sobjs.remove(&id)
        } else {
            self.objs.get_mut(id as usize).and_then(|o| o.take())
        }
    }
    pub fn version(&self, id: ObjId) -> u32 {
        self.obj(id).map_or(0, |o| o.version)
    }
    pub fn kill(&mut self, why: String) {
        if self.dead.is_none() {
            self.dead = Some(why);
        }
    }
}

#[derive(Default, Clone)]
pub struct Positioner {
    pub size: (i32, i32),
    pub anchor_rect: Rect,
    pub anchor: u32,
    pub gravity: u32,
    pub constraint: u32,
    pub offset: (i32, i32),
    pub reactive: bool,
}

// ------------------------------------------------------------------ surfaces

pub enum Attach {
    Keep,
    Detach,
    Buffer(Rc<Buffer>, ObjId),
}

impl Default for Attach {
    fn default() -> Self {
        Attach::Keep
    }
}

#[derive(Default)]
pub struct Pending {
    pub attach: Attach,
    pub dx: i32,
    pub dy: i32,
    pub damage_surface: Vec<Rect>,
    pub damage_buffer: Vec<Rect>,
    pub frame_cbs: Vec<ObjId>,
    pub input: Option<Option<Region>>,
    pub opaque: Option<Option<Region>>,
    pub scale: Option<i32>,
    pub geometry: Option<Rect>,
    pub vp_src: Option<Option<(f64, f64, f64, f64)>>,
    pub vp_dst: Option<Option<(i32, i32)>>,
    pub glass: Option<Vec<GlassPanel>>,
}

/// A panel of glass a client asked for under its surface (resc_glass_v1), surface-local.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GlassPanel {
    /// x, y, w, h.
    pub rect: [f32; 4],
    /// Top left, top right, bottom right, bottom left.
    pub radii: [f32; 4],
    /// Shown only inside it (x, y, w, h).
    pub clip: Option<[f32; 4]>,
    /// Sigma, surface px; negative: the desk's own.
    pub blur: f32,
    /// 0xAARRGGBB straight; 0: the desk's window tint.
    pub tint: u32,
    /// Surface px; negative: the desk's own.
    pub bezel: f32,
    pub flags: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    None,
    Toplevel(WKey),
    Popup,
    Subsurface,
    Cursor,
    Layer,
    /// The picture that follows the pointer during a drag.
    DndIcon,
}

pub struct PopupInfo {
    pub parent: SKey,
    /// Position of the popup's geometry origin relative to the parent's geometry origin.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub obj: ObjId,
    pub grab: bool,
    pub positioner: Positioner,
}

pub struct Surface {
    pub client: CKey,
    pub id: ObjId,
    pub pending: Pending,
    pub buffer: Option<(Rc<Buffer>, ObjId)>,
    pub scale: i32,
    /// Logical size (after buffer scale / viewport).
    pub w: i32,
    pub h: i32,
    pub input: Option<Region>,
    pub opaque: Option<Region>,
    pub frame_cbs: Vec<ObjId>,
    pub role: Role,
    pub xdg_obj: ObjId,
    pub xdg_configured: bool,
    pub xdg_initial_sent: bool,
    pub acked: u32,
    pub geometry: Option<Rect>,
    pub popup: Option<PopupInfo>,
    pub popups: Vec<SKey>,
    /// Subsurface children in stacking order; `SKey::default()` marks the parent itself.
    pub subs: Vec<SKey>,
    pub parent: Option<SKey>,
    pub sub_pos: (i32, i32),
    pub sub_pos_pending: Option<(i32, i32)>,
    pub sub_sync: bool,
    pub cached: Option<Pending>,
    pub outputs: u64,
    pub vp_src: Option<(f64, f64, f64, f64)>,
    pub vp_dst: Option<(i32, i32)>,
    pub frac_obj: ObjId,
    pub sent_scale: i32,
    pub sent_frac: u32,
    pub mapped: bool,
    pub destroyed: bool,
    /// Buffer pixels changed since the GPU last copied them (gpu.rs), or all of them.
    pub tex_dmg: Vec<Rect>,
    pub tex_full: bool,
    /// Bumped whenever its pixels change (the glass over it knows its frost is still good).
    pub gen: u64,
    /// The X window this surface was given to (Xwayland's surfaces, xwm.rs); 0 for none.
    /// A surface is given to an X window only once.
    pub xwin: u32,
    /// Where an X menu or tooltip sits (global logical coords): X places those itself.
    pub x_pos: Option<(i32, i32)>,
    /// Glass under parts of it, bottom to top (resc_glass_v1), and its object (0: none).
    pub glass: Vec<GlassPanel>,
    pub glass_obj: ObjId,
}

impl Surface {
    pub fn new(client: CKey, id: ObjId) -> Self {
        Self {
            client,
            id,
            pending: Pending::default(),
            buffer: None,
            scale: 1,
            w: 0,
            h: 0,
            input: None,
            opaque: None,
            frame_cbs: vec![],
            role: Role::None,
            xdg_obj: 0,
            xdg_configured: false,
            xdg_initial_sent: false,
            acked: 0,
            geometry: None,
            popup: None,
            popups: vec![],
            subs: vec![SKey::default()],
            parent: None,
            sub_pos: (0, 0),
            sub_pos_pending: None,
            sub_sync: true,
            cached: None,
            outputs: 0,
            vp_src: None,
            vp_dst: None,
            frac_obj: 0,
            sent_scale: 0,
            sent_frac: 0,
            mapped: false,
            destroyed: false,
            tex_dmg: Vec::new(),
            tex_full: true,
            gen: 0,
            xwin: 0,
            x_pos: None,
            glass: Vec::new(),
            glass_obj: 0,
        }
    }
    /// Window geometry in surface-local coordinates.
    pub fn geom(&self) -> Rect {
        let full = Rect::new(0, 0, self.w, self.h);
        match self.geometry {
            Some(g) if !g.is_empty() => {
                let i = g.intersect(&full);
                if i.is_empty() { g } else { i }
            }
            _ => full,
        }
    }
    pub fn accepts_input(&self, lx: f64, ly: f64) -> bool {
        if !Rect::new(0, 0, self.w, self.h).contains(lx, ly) {
            return false;
        }
        match &self.input {
            None => true,
            Some(r) => r.contains(lx, ly),
        }
    }
    pub fn is_opaque(&self) -> bool {
        if let Some((b, _)) = &self.buffer {
            if b.opaque_format() {
                return true;
            }
        }
        match &self.opaque {
            Some(r) => r.covers(&Rect::new(0, 0, self.w, self.h)),
            None => false,
        }
    }
}

// ------------------------------------------------------------------ windows

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Snap {
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tile {
    Floating,
    Maximized,
    Snapped(Snap),
    Fullscreen,
}

pub struct Window {
    pub surf: SKey,
    pub client: CKey,
    pub toplevel: ObjId,
    pub title: String,
    pub app_id: String,
    /// Global logical position of the window geometry (content) origin.
    pub x: i32,
    pub y: i32,
    pub tile: Tile,
    pub minimized: bool,
    /// Growing out of its taskbar button, shrinking into it or spreading out of the preview
    /// card (winanim.rs).
    pub morph: Option<crate::winanim::Morph>,
    /// How it looked when it last drew: what it goes away as when it closes (life.rs).
    pub snap: Option<Box<crate::life::Snap>>,
    pub activated: bool,
    /// Floating geometry to go back to.
    pub restore: Rect,
    pub ssd: bool,
    pub deco_obj: ObjId,
    pub min_size: (i32, i32),
    pub max_size: (i32, i32),
    pub workspace: u32,
    pub placed: bool,
    /// Position to apply once the client commits a configure >= serial.
    pub pending_pos: Option<(u32, i32, i32)>,
    /// Interactive resize anchor: keep this edge fixed (right/bottom in global coords).
    pub anchor: Option<(u32, i32, i32)>,
    /// The resize is over: the anchor holds until the client has acked this configure (the
    /// last size), then goes. Kept longer, every later frame (a hover redraw) would pull the
    /// window back to where the edge was.
    pub anchor_until: Option<u32>,
    pub last_cfg: (i32, i32, Vec<u32>),
    pub cfg_serial: u32,
    pub parent: Option<WKey>,
    pub output: usize,
    pub fullscreen_output: Option<usize>,
    /// The X window behind it (an X program through Xwayland, xwm.rs); 0 for Wayland ones.
    pub x11: u32,
    /// A program's notification (toast.rs): bottom right, over the windows, no focus.
    pub toast: bool,
}

impl Window {
    pub fn frame_insets(&self) -> (i32, i32) {
        if self.desktop() {
            return (0, 0);
        }
        if self.ssd && self.tile != Tile::Fullscreen { (crate::shell::BORDER, if self.merged() { 0 } else { crate::shell::TITLE_H }) } else { (0, 0) }
    }

    /// Windows 11's "content in the title bar" (Explorer's tabs): our frame and glass stay,
    /// but no title strip — the program draws its own row with the caption buttons there
    /// and asks us to move / maximize / minimize the window.
    pub fn merged(&self) -> bool {
        self.app_id == "resc.files"
    }

    /// The desktop's icons («Проводник» in desktop mode): under every window, the whole
    /// work area, on every virtual desktop, never in the taskbar or Alt+Tab.
    pub fn desktop(&self) -> bool {
        self.app_id == "resc.desktop"
    }
}

// ------------------------------------------------------------------ outputs

pub struct Output {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub pw: i32,
    pub ph: i32,
    pub scale: f64,
    /// Physical rectangle on the monitor plane, millimetres (see layout.rs).
    pub plane: [f64; 4],
    /// How far (mm) "glue" moved this monitor left to close bezel gaps.
    pub shift_mm: f64,
    pub refresh_mhz: i32,
    pub global: u32,
    pub damage: Damage,
    pub frame_pending: bool,
    pub wants_frame: bool,
    pub last_frame_ns: u64,
    pub frames: u64,
}

impl Output {
    pub fn lw(&self) -> i32 {
        (self.pw as f64 / self.scale).round() as i32
    }
    pub fn lh(&self) -> i32 {
        (self.ph as f64 / self.scale).round() as i32
    }
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.lw(), self.lh())
    }
    pub fn int_scale(&self) -> i32 {
        self.scale.ceil() as i32
    }
}

// ------------------------------------------------------------------ data device

pub struct DataSource {
    pub client: CKey,
    pub obj: ObjId,
    pub mimes: Vec<String>,
    /// zwp_primary_selection_source_v1 (middle-click paste) rather than wl_data_source.
    pub primary: bool,
    /// Served by the compositor itself (clipboard history), no client behind it.
    pub own: Option<std::sync::Arc<Vec<u8>>>,
    /// What an X program copied: read through the X window manager (xwm/sel.rs).
    pub x11: bool,
    /// What a drag allows (dnd::COPY | MOVE | ASK).
    pub actions: u32,
}

// ------------------------------------------------------------------ globals

#[derive(Clone, Copy)]
pub struct Global {
    pub name: u32,
    pub iface: Iface,
    pub version: u32,
    pub output: Option<u32>,
    pub alive: bool,
}

pub struct Keymap {
    pub fd: OwnedFd,
    pub size: u32,
}

pub type Surfaces = Slab<Surface>;
