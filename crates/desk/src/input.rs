//! Seat: pointer & keyboard routing, interactive grabs (move/resize/snap),
//! global shortcuts (Windows-style), cursor state.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::shell;
use crate::state::*;
use crate::wm::{self, Hit};
use wl::protocol::*;
use wl::{Fixed, ObjId};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

pub mod key {
    pub const ESC: u32 = 1;
    pub const N1: u32 = 2;
    pub const N9: u32 = 10;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const Q: u32 = 16;
    pub const E: u32 = 18;
    pub const R: u32 = 19;
    pub const T: u32 = 20;
    pub const I: u32 = 23;
    pub const P: u32 = 25;
    pub const V: u32 = 47;
    pub const SPACE: u32 = 57;
    pub const DELETE: u32 = 111;
    pub const ENTER: u32 = 28;
    pub const LCTRL: u32 = 29;
    pub const S: u32 = 31;
    pub const D: u32 = 32;
    pub const L: u32 = 38;
    pub const LSHIFT: u32 = 42;
    pub const N: u32 = 49;
    pub const M: u32 = 50;
    pub const RSHIFT: u32 = 54;
    pub const LALT: u32 = 56;
    pub const CAPS: u32 = 58;
    pub const F4: u32 = 62;
    pub const NUMLOCK: u32 = 69;
    pub const SYSRQ: u32 = 99;
    pub const RCTRL: u32 = 97;
    pub const RALT: u32 = 100;
    pub const UP: u32 = 103;
    pub const PAGEUP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const DOWN: u32 = 108;
    pub const MUTE: u32 = 113;
    pub const VOLUMEDOWN: u32 = 114;
    pub const VOLUMEUP: u32 = 115;
    pub const PAGEDOWN: u32 = 109;
    pub const LMETA: u32 = 125;
    pub const RMETA: u32 = 126;
}

pub const M_SHIFT: u32 = 1;
pub const M_CTRL: u32 = 2;
pub const M_ALT: u32 = 4;
pub const M_SUPER: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grab {
    None,
    Implicit(SKey),
    Move { wk: WKey, dx: f64, dy: f64, sx: f64, sy: f64 },
    Resize { wk: WKey, edges: u32, start: Rect, px: f64, py: f64 },
    Deco { wk: WKey, hit: Hit },
    Shell,
    /// Click that dismissed popups: swallow until release.
    Swallow,
    /// A drag and drop between programs (dnd.rs).
    Dnd,
}

pub struct Seat {
    pub x: f64,
    pub y: f64,
    pub buttons: Vec<u32>,
    pub ptr_focus: Option<SKey>,
    pub kb_focus: Option<SKey>,
    pub keys: Vec<u32>,
    pub swallowed: Vec<u32>,
    pub held: u32,
    pub caps: bool,
    pub num: bool,
    pub group: u32,
    pub host_mods: Option<[u32; 4]>,
    pub sent_mods: [u32; 4],
    pub grab: Grab,
    pub resizing: Option<WKey>,
    pub popup_grabs: Vec<SKey>,
    pub cursor_surface: Option<SKey>,
    pub cursor_hotspot: (i32, i32),
    pub cursor_shape: u32,
    pub cursor_hidden: bool,
    pub last_click: (u32, f64, f64, Option<WKey>),
    pub super_alone: bool,
    /// Alt+Shift held with nothing else pressed yet: switch the layout on release.
    pub layout_chord: bool,
    pub hover: Option<(WKey, Hit)>,
}

impl Default for Seat {
    fn default() -> Self {
        Self {
            x: 100.0,
            y: 100.0,
            buttons: vec![],
            ptr_focus: None,
            kb_focus: None,
            keys: vec![],
            swallowed: vec![],
            held: 0,
            caps: false,
            num: true,
            group: 0,
            host_mods: None,
            sent_mods: [u32::MAX; 4],
            grab: Grab::None,
            resizing: None,
            popup_grabs: vec![],
            cursor_surface: None,
            cursor_hotspot: (0, 0),
            cursor_shape: 1,
            cursor_hidden: false,
            last_click: (0, 0.0, 0.0, None),
            super_alone: false,
            layout_chord: false,
            hover: None,
        }
    }
}

// ------------------------------------------------------------------ pointer events to clients

fn client_of(d: &Desk, sk: SKey) -> Option<(CKey, ObjId)> {
    d.surfaces.get(sk).map(|s| (s.client, s.id))
}

fn ptr_frame(c: &mut Client, p: ObjId) {
    if c.version(p) >= 5 {
        wl_pointer::evt::frame(&mut c.conn, p);
    }
}

fn send_enter(d: &mut Desk, sk: SKey, lx: f64, ly: f64) {
    let Some((ck, sid)) = client_of(d, sk) else { return };
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    for p in c.pointers.clone() {
        wl_pointer::evt::enter(&mut c.conn, p, serial, sid, Fixed::from_f64(lx), Fixed::from_f64(ly));
        ptr_frame(c, p);
    }
}

fn send_leave(d: &mut Desk, sk: SKey) {
    let Some((ck, sid)) = client_of(d, sk) else { return };
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    for p in c.pointers.clone() {
        wl_pointer::evt::leave(&mut c.conn, p, serial, sid);
        ptr_frame(c, p);
    }
}

fn send_motion(d: &mut Desk, sk: SKey, time: u32, lx: f64, ly: f64) {
    let Some((ck, _)) = client_of(d, sk) else { return };
    let Some(c) = d.clients.get_mut(ck) else { return };
    for p in c.pointers.clone() {
        wl_pointer::evt::motion(&mut c.conn, p, time, Fixed::from_f64(lx), Fixed::from_f64(ly));
        ptr_frame(c, p);
    }
}

fn send_button(d: &mut Desk, sk: SKey, time: u32, button: u32, pressed: bool) {
    let Some((ck, _)) = client_of(d, sk) else { return };
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    for p in c.pointers.clone() {
        wl_pointer::evt::button(&mut c.conn, p, serial, time, button, pressed as u32);
        ptr_frame(c, p);
    }
}

pub fn set_ptr_focus(d: &mut Desk, new: Option<(SKey, f64, f64)>) {
    let old = d.seat.ptr_focus;
    if old == new.map(|n| n.0) {
        return;
    }
    if let Some(o) = old {
        send_leave(d, o);
    }
    d.seat.ptr_focus = new.map(|n| n.0);
    // Our own cursor until the client sets one.
    d.seat.cursor_surface = None;
    d.seat.cursor_hidden = false;
    d.seat.cursor_shape = 1;
    damage_cursor(d);
    if let Some((sk, lx, ly)) = new {
        send_enter(d, sk, lx, ly);
    }
}

pub fn init_pointer(d: &mut Desk, ck: CKey) {
    if let Some(sk) = d.seat.ptr_focus {
        if d.surfaces.get(sk).map(|s| s.client) == Some(ck) {
            d.seat.ptr_focus = None;
            let (x, y) = (d.seat.x, d.seat.y);
            pointer_motion(d, x, y, d.now_ms());
        }
    }
}

// ------------------------------------------------------------------ pointer input

/// Absolute pointer position in global logical coordinates.
pub fn pointer_motion(d: &mut Desk, x: f64, y: f64, time: u32) {
    let (x, y) = clamp_to_layout(d, x, y);
    damage_cursor(d);
    d.seat.x = x;
    d.seat.y = y;
    damage_cursor(d);
    match d.seat.grab {
        Grab::Move { wk, dx, dy, sx, sy } => move_grab(d, wk, dx, dy, sx, sy),
        Grab::Resize { wk, edges, start, px, py } => resize_grab(d, wk, edges, start, px, py),
        Grab::Implicit(sk) => {
            if let Some((ox, oy)) = d.surface_origin(sk) {
                send_motion(d, sk, time, x - ox as f64, y - oy as f64);
            }
        }
        Grab::Shell => {
            crate::snip::motion(d, x, y);
            crate::tray::drag(d, x);
            crate::taskview::drag(d, x, y);
        }
        Grab::Deco { .. } | Grab::Swallow => {}
        Grab::Dnd => crate::dnd::motion(d, time),
        Grab::None => hover(d, time),
    }
    shell::pointer_hover(d);
}

fn clamp_to_layout(d: &Desk, x: f64, y: f64) -> (f64, f64) {
    if d.output_at(x, y).is_some() || d.outputs.is_empty() {
        return (x, y);
    }
    // Into a bezel gap: carry across to the monitor on the other side.
    if let Some(p) = crate::layout::cross_gap(d, (d.seat.x, d.seat.y), (x, y)) {
        return p;
    }
    // Clamp into the nearest output.
    let mut best = (x, y, f64::MAX);
    for o in &d.outputs {
        let r = o.rect();
        let cx = x.clamp(r.x as f64, (r.x1() - 1) as f64);
        let cy = y.clamp(r.y as f64, (r.y1() - 1) as f64);
        let dist = (cx - x).powi(2) + (cy - y).powi(2);
        if dist < best.2 {
            best = (cx, cy, dist);
        }
    }
    (best.0, best.1)
}

fn hover(d: &mut Desk, time: u32) {
    let (x, y) = (d.seat.x, d.seat.y);
    if shell::taskbar_contains(d, x, y) {
        set_ptr_focus(d, None);
        set_hover(d, None);
        return;
    }
    match wm::hit_test(d, x, y) {
        Some((_, Hit::Surface(sk, lx, ly))) => {
            let entering = d.seat.ptr_focus != Some(sk);
            set_ptr_focus(d, Some((sk, lx, ly)));
            if !entering {
                send_motion(d, sk, time, lx, ly);
            }
            set_hover(d, None);
        }
        Some((wk, h)) => {
            set_ptr_focus(d, None);
            d.seat.cursor_shape = match h {
                Hit::Edge(e) => edge_cursor(e),
                _ => 1,
            };
            set_hover(d, Some((wk, h)));
        }
        None => {
            set_ptr_focus(d, None);
            set_hover(d, None);
        }
    }
}

fn set_hover(d: &mut Desk, h: Option<(WKey, Hit)>) {
    if d.seat.hover != h {
        for (wk, _) in [d.seat.hover, h].into_iter().flatten() {
            if let Some(f) = wm::frame_rect(d, wk) {
                d.damage(Rect::new(f.x, f.y, f.w, shell::TITLE_H + shell::BORDER));
            }
        }
        d.seat.hover = h;
    }
}

/// wp_cursor_shape values for resize edges.
fn edge_cursor(e: u32) -> u32 {
    match e {
        1 => 18,  // n_resize
        2 => 22,  // s_resize
        4 => 24,  // w_resize
        8 => 20,  // e_resize
        5 => 25,  // nw_resize
        9 => 19,  // ne_resize
        6 => 23,  // sw_resize
        10 => 21, // se_resize
        _ => 1,
    }
}

pub fn pointer_button(d: &mut Desk, button: u32, pressed: bool, time: u32) {
    if pressed {
        // A click during the start-up animation hurries it on.
        crate::boot::poke(d);
        if d.seat.buttons.contains(&button) {
            return;
        }
        d.seat.buttons.push(button);
        if d.seat.buttons.len() == 1 {
            first_press(d, button, time);
        } else if let Grab::Implicit(sk) = d.seat.grab {
            send_button(d, sk, time, button, true);
        }
        return;
    }
    if !d.seat.buttons.contains(&button) {
        return;
    }
    d.seat.buttons.retain(|b| *b != button);
    let last = d.seat.buttons.is_empty();
    match d.seat.grab {
        Grab::Implicit(sk) => {
            send_button(d, sk, time, button, false);
            if last {
                d.seat.grab = Grab::None;
                hover(d, time);
            }
        }
        Grab::Move { wk, .. } if last => {
            d.seat.grab = Grab::None;
            // Dropped on another monitor: that is its monitor now (minimize, maximize, monitor changes).
            if d.windows.contains(wk) {
                d.windows[wk].output = wm::window_output(d, wk);
            }
            if let Some(r) = d.overlay.snap_preview.take() {
                d.damage(r.inflate(4));
                finish_snap(d, wk);
            }
            hover(d, time);
        }
        Grab::Resize { wk, .. } if last => {
            d.seat.grab = Grab::None;
            d.seat.resizing = None;
            if let Some(w) = d.windows.get(wk) {
                let g = d.surfaces[w.surf].geom();
                wm::send_configure(d, wk, g.w, g.h);
            }
            hover(d, time);
        }
        Grab::Deco { wk, hit } if last => {
            d.seat.grab = Grab::None;
            let now = wm::hit_test(d, d.seat.x, d.seat.y);
            if now == Some((wk, hit)) {
                match hit {
                    Hit::Minimize => wm::minimize(d, wk),
                    Hit::Maximize => wm::toggle_maximize(d, wk),
                    Hit::Close => wm::close(d, wk),
                    _ => {}
                }
            }
            hover(d, time);
        }
        Grab::Dnd if last => {
            crate::dnd::release(d);
            hover(d, time);
        }
        Grab::Shell | Grab::Swallow if last => {
            if matches!(d.seat.grab, Grab::Shell) {
                crate::snip::release(d);
                crate::tray::release(d);
                crate::taskview::release(d, d.seat.x, d.seat.y);
            }
            d.seat.grab = Grab::None;
            hover(d, time);
        }
        _ => {}
    }
}

fn first_press(d: &mut Desk, button: u32, time: u32) {
    let (x, y) = (d.seat.x, d.seat.y);
    // Click outside an open popup chain closes it (and is swallowed).
    if !d.seat.popup_grabs.is_empty() {
        let on_popup = match wm::hit_test(d, x, y) {
            Some((_, Hit::Surface(sk, _, _))) => is_in_popup_chain(d, sk),
            _ => false,
        };
        if !on_popup {
            crate::server::dismiss_popups(d);
            d.seat.grab = Grab::Swallow;
            return;
        }
    }
    if crate::snip::press(d, button) || shell::overlay_click(d, x, y, button) || shell::taskbar_click(d, x, y, button) {
        d.seat.grab = Grab::Shell;
        set_ptr_focus(d, None);
        return;
    }
    match wm::hit_test(d, x, y) {
        Some((wk, Hit::Surface(sk, lx, ly))) => {
            if !d.seat.popup_grabs.is_empty() && is_in_popup_chain(d, sk) {
                // Keep focus on the popup.
            } else if wm::focused(d) != Some(wk) {
                wm::focus_window(d, Some(wk));
            } else {
                wm::raise(d, wk);
            }
            set_ptr_focus(d, Some((sk, lx, ly)));
            d.seat.grab = Grab::Implicit(sk);
            send_button(d, sk, time, button, true);
        }
        Some((wk, Hit::Title)) => {
            wm::focus_window(d, Some(wk));
            if button == BTN_LEFT {
                let (t, lx, ly, lw) = d.seat.last_click;
                let dbl = lw == Some(wk) && time.wrapping_sub(t) < 450 && (lx - x).abs() < 6.0 && (ly - y).abs() < 6.0;
                d.seat.last_click = (time, x, y, Some(wk));
                if dbl {
                    wm::toggle_maximize(d, wk);
                    d.seat.grab = Grab::Shell;
                    d.seat.last_click.3 = None;
                } else {
                    begin_move(d, wk);
                }
            } else if button == BTN_MIDDLE {
                wm::minimize(d, wk);
                d.seat.grab = Grab::Shell;
            } else {
                d.seat.grab = Grab::Shell;
            }
        }
        Some((wk, h @ (Hit::Minimize | Hit::Maximize | Hit::Close))) => {
            wm::focus_window(d, Some(wk));
            d.seat.grab = Grab::Deco { wk, hit: h };
        }
        Some((wk, Hit::Edge(e))) => {
            wm::focus_window(d, Some(wk));
            begin_resize(d, wk, e);
        }
        None => {
            // Desktop click: deactivate windows like Explorer does.
            shell::close_menus(d);
            wm::focus_window(d, None);
            d.seat.grab = Grab::Shell;
        }
    }
}

fn is_in_popup_chain(d: &Desk, mut sk: SKey) -> bool {
    for _ in 0..32 {
        if d.seat.popup_grabs.contains(&sk) {
            return true;
        }
        match d.surfaces.get(sk) {
            Some(s) if s.role == Role::Subsurface => match s.parent {
                Some(p) => sk = p,
                None => return false,
            },
            _ => return false,
        }
    }
    false
}

pub fn pointer_axis(d: &mut Desk, time: u32, axis: u32, value: f64, v120: Option<i32>, source: Option<u32>) {
    let target = match d.seat.grab {
        Grab::Implicit(sk) => Some(sk),
        Grab::None => d.seat.ptr_focus,
        _ => None,
    };
    if crate::clipboard::scroll(d, value) {
        return;
    }
    if axis == 0 && crate::start::scroll(d, d.seat.x, d.seat.y, value) {
        return;
    }
    if axis == 0 && crate::tray::scroll(d, d.seat.x, d.seat.y, value) {
        return;
    }
    let Some(sk) = target else {
        return;
    };
    let Some((ck, _)) = client_of(d, sk) else { return };
    let Some(c) = d.clients.get_mut(ck) else { return };
    for p in c.pointers.clone() {
        let ver = c.version(p);
        if ver >= 5 {
            if let Some(s) = source {
                wl_pointer::evt::axis_source(&mut c.conn, p, s);
            }
        }
        if ver >= 8 {
            if let Some(v) = v120 {
                wl_pointer::evt::axis_value120(&mut c.conn, p, axis, v);
            }
        } else if ver >= 5 {
            if let Some(v) = v120 {
                wl_pointer::evt::axis_discrete(&mut c.conn, p, axis, v / 120);
            }
        }
        if value == 0.0 && ver >= 5 {
            wl_pointer::evt::axis_stop(&mut c.conn, p, time, axis);
        } else {
            wl_pointer::evt::axis(&mut c.conn, p, time, axis, Fixed::from_f64(value));
        }
        ptr_frame(c, p);
    }
}

// ------------------------------------------------------------------ move / resize grabs

pub fn begin_move(d: &mut Desk, wk: WKey) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    if d.seat.buttons.is_empty() || !d.windows.contains(wk) {
        return;
    }
    let w = &d.windows[wk];
    let (dx, dy) = (d.seat.x - w.x as f64, d.seat.y - w.y as f64);
    d.seat.grab = Grab::Move { wk, dx, dy, sx: d.seat.x, sy: d.seat.y };
    set_ptr_focus(d, None);
}

pub fn begin_resize(d: &mut Desk, wk: WKey, edges: u32) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    if d.seat.buttons.is_empty() || !d.windows.contains(wk) || edges == 0 {
        return;
    }
    let w = &d.windows[wk];
    if w.tile != Tile::Floating {
        return;
    }
    let g = d.surfaces[w.surf].geom();
    let start = Rect::new(w.x, w.y, g.w, g.h);
    d.seat.grab = Grab::Resize { wk, edges, start, px: d.seat.x, py: d.seat.y };
    d.seat.resizing = Some(wk);
    d.seat.cursor_shape = edge_cursor(edges);
    set_ptr_focus(d, None);
}

fn move_grab(d: &mut Desk, wk: WKey, dx: f64, dy: f64, sx: f64, sy: f64) {
    let (x, y) = (d.seat.x, d.seat.y);
    let Some(w) = d.windows.get(wk) else { return };
    if w.tile != Tile::Floating {
        // Drag a maximized/snapped window out: restore, keeping the grab point proportional.
        if (x - sx).abs() < 6.0 && (y - sy).abs() < 6.0 {
            return;
        }
        let g = d.surfaces[w.surf].geom();
        let frac = (dx / g.w.max(1) as f64).clamp(0.0, 1.0);
        let r = w.restore;
        let rw = if r.is_empty() { g.w } else { r.w };
        let rh = if r.is_empty() { g.h } else { r.h };
        let nx = (x - frac * rw as f64) as i32;
        let ny = (y - dy.min(shell::TITLE_H as f64)) as i32;
        let w = &mut d.windows[wk];
        w.tile = Tile::Floating;
        w.restore = Rect::new(nx, ny, rw, rh);
        wm::restore(d, wk);
        d.seat.grab = Grab::Move { wk, dx: x - nx as f64, dy: y - ny as f64, sx, sy };
        return;
    }
    let old = wm::frame_rect(d, wk);
    let w = &mut d.windows[wk];
    w.x = (x - dx).round() as i32;
    w.y = (y - dy).round() as i32;
    // Keep the title bar below the top edge of the layout.
    if let Some(oi) = d.output_at(x, y) {
        let top = d.outputs[oi].y + shell::TITLE_H + shell::BORDER;
        let w = &mut d.windows[wk];
        if w.ssd && w.y < top {
            w.y = top;
        }
    }
    if let Some(r) = old {
        d.damage(r.inflate(shell::SHADOW));
    }
    if let Some(r) = wm::visual_rect(d, wk) {
        d.damage(r);
    }
    let sk = d.windows[wk].surf;
    d.damage_surface_tree(sk);
    wm::update_window_outputs(d, wk);
    // Aero snap preview when the pointer hits an outer edge.
    let preview = snap_target(d, x, y).map(|(r, _)| r);
    if preview != d.overlay.snap_preview {
        if let Some(r) = d.overlay.snap_preview {
            d.damage(r.inflate(4));
        }
        if let Some(r) = preview {
            d.damage(r.inflate(4));
        }
        d.overlay.snap_preview = preview;
    }
}

enum SnapTo {
    Max(usize),
    Side(Snap, usize),
}

fn snap_target(d: &Desk, x: f64, y: f64) -> Option<(Rect, SnapTo)> {
    let oi = d.output_at(x, y)?;
    let o = d.outputs[oi].rect();
    let outside = |px: f64, py: f64| d.output_at(px, py).is_none();
    let left = x <= o.x as f64 + 1.0 && outside(x - 2.0, y);
    let right = x >= o.x1() as f64 - 2.0 && outside(x + 2.0, y);
    let top = y <= o.y as f64 + 1.0 && outside(x, y - 2.0);
    let corner = 48.0;
    let near_top = y < o.y as f64 + corner;
    let near_bottom = y > (wm::workarea(d, oi).y1()) as f64 - corner;
    let wa = wm::workarea(d, oi);
    let (hw, hh) = (wa.w / 2, wa.h / 2);
    let t = if left {
        if near_top {
            SnapTo::Side(Snap::TopLeft, oi)
        } else if near_bottom {
            SnapTo::Side(Snap::BottomLeft, oi)
        } else {
            SnapTo::Side(Snap::Left, oi)
        }
    } else if right {
        if near_top {
            SnapTo::Side(Snap::TopRight, oi)
        } else if near_bottom {
            SnapTo::Side(Snap::BottomRight, oi)
        } else {
            SnapTo::Side(Snap::Right, oi)
        }
    } else if top {
        SnapTo::Max(oi)
    } else {
        return None;
    };
    let r = match t {
        SnapTo::Max(_) => wa,
        SnapTo::Side(s, _) => match s {
            Snap::Left => Rect::new(wa.x, wa.y, hw, wa.h),
            Snap::Right => Rect::new(wa.x + hw, wa.y, wa.w - hw, wa.h),
            Snap::TopLeft => Rect::new(wa.x, wa.y, hw, hh),
            Snap::TopRight => Rect::new(wa.x + hw, wa.y, wa.w - hw, hh),
            Snap::BottomLeft => Rect::new(wa.x, wa.y + hh, hw, wa.h - hh),
            Snap::BottomRight => Rect::new(wa.x + hw, wa.y + hh, wa.w - hw, wa.h - hh),
        },
    };
    Some((r.inflate(-6), t))
}

fn finish_snap(d: &mut Desk, wk: WKey) {
    if !d.windows.contains(wk) {
        return;
    }
    match snap_target(d, d.seat.x, d.seat.y) {
        Some((_, SnapTo::Max(_))) => wm::maximize(d, wk),
        Some((_, SnapTo::Side(s, oi))) => wm::snap(d, wk, s, oi),
        None => {}
    }
}

fn resize_grab(d: &mut Desk, wk: WKey, edges: u32, start: Rect, px: f64, py: f64) {
    let Some(w) = d.windows.get(wk) else { return };
    let ddx = (d.seat.x - px).round() as i32;
    let ddy = (d.seat.y - py).round() as i32;
    let (mut nw, mut nh) = (start.w, start.h);
    if edges & 8 != 0 {
        nw += ddx;
    }
    if edges & 4 != 0 {
        nw -= ddx;
    }
    if edges & 2 != 0 {
        nh += ddy;
    }
    if edges & 1 != 0 {
        nh -= ddy;
    }
    let (minw, minh) = (w.min_size.0.max(120), w.min_size.1.max(60));
    nw = nw.max(minw);
    nh = nh.max(minh);
    if w.max_size.0 > 0 {
        nw = nw.min(w.max_size.0);
    }
    if w.max_size.1 > 0 {
        nh = nh.min(w.max_size.1);
    }
    let anchor = (edges, start.x1(), start.y1());
    let w = &mut d.windows[wk];
    w.anchor = if edges & 5 != 0 { Some(anchor) } else { None };
    if (nw, nh) != (w.last_cfg.0, w.last_cfg.1) {
        wm::send_configure(d, wk, nw, nh);
    }
}

// ------------------------------------------------------------------ keyboard

pub fn init_keyboard(d: &mut Desk, ck: CKey, kid: ObjId) {
    let ver = d.clients[ck].version(kid);
    if let Some(km) = &d.keymap {
        let c = &mut d.clients[ck];
        use std::os::fd::AsFd;
        wl_keyboard::evt::keymap(&mut c.conn, kid, wl_keyboard::keymap_format::XKB_V1, km.fd.as_fd(), km.size);
    }
    if ver >= 4 {
        let (rate, delay) = d.repeat;
        wl_keyboard::evt::repeat_info(&mut d.clients[ck].conn, kid, rate, delay);
    }
    if let Some(sk) = d.seat.kb_focus {
        if let Some(s) = d.surfaces.get(sk) {
            if s.client == ck {
                let sid = s.id;
                let serial = d.next_serial();
                let keys = keys_array(&d.seat.keys);
                let m = current_mods(d);
                let c = &mut d.clients[ck];
                wl_keyboard::evt::enter(&mut c.conn, kid, serial, sid, &keys);
                wl_keyboard::evt::modifiers(&mut c.conn, kid, serial, m[0], m[1], m[2], m[3]);
            }
        }
    }
    init_pointer(d, ck);
}

/// Re-sends the keymap to every keyboard (after the host keymap changed).
pub fn broadcast_keymap(d: &mut Desk) {
    for ck in d.clients.keys() {
        let kbs = d.clients[ck].keyboards.clone();
        if let Some(km) = &d.keymap {
            use std::os::fd::AsFd;
            for k in kbs {
                wl_keyboard::evt::keymap(&mut d.clients[ck].conn, k, 1, km.fd.as_fd(), km.size);
            }
        }
    }
}

fn keys_array(keys: &[u32]) -> Vec<u8> {
    keys.iter().flat_map(|k| k.to_ne_bytes()).collect()
}

/// Shift / Ctrl / Alt held now (M_* bits), from the host or our own keys.
pub fn held_mods(d: &Desk) -> u32 {
    let m = current_mods(d)[0];
    (if m & 1 != 0 { M_SHIFT } else { 0 }) | (if m & 4 != 0 { M_CTRL } else { 0 }) | (if m & 8 != 0 { M_ALT } else { 0 })
}

fn current_mods(d: &Desk) -> [u32; 4] {
    let h = d.seat.held;
    if let Some(mut m) = d.seat.host_mods {
        // The host's state, plus keys we were told about otherwise (the control socket).
        m[0] |= (if h & M_SHIFT != 0 { 1 } else { 0 }) | (if h & M_CTRL != 0 { 4 } else { 0 }) | (if h & M_ALT != 0 { 8 } else { 0 });
        return m;
    }
    let mut dep = 0;
    if h & M_SHIFT != 0 {
        dep |= 1;
    }
    if h & M_CTRL != 0 {
        dep |= 4;
    }
    if h & M_ALT != 0 {
        dep |= 8;
    }
    if h & M_SUPER != 0 {
        dep |= 64;
    }
    let mut lock = 0;
    if d.seat.caps {
        lock |= 2;
    }
    if d.seat.num {
        lock |= 16;
    }
    [dep, 0, lock, d.seat.group]
}

fn send_mods(d: &mut Desk, force: bool) {
    let m = current_mods(d);
    if m == d.seat.sent_mods && !force {
        return;
    }
    d.seat.sent_mods = m;
    let Some(sk) = d.seat.kb_focus else { return };
    let Some((ck, _)) = client_of(d, sk) else { return };
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    for k in c.keyboards.clone() {
        wl_keyboard::evt::modifiers(&mut c.conn, k, serial, m[0], m[1], m[2], m[3]);
    }
}

pub fn host_modifiers(d: &mut Desk, dep: u32, lat: u32, lock: u32, group: u32) {
    d.seat.host_mods = Some([dep, lat, lock, group]);
    send_mods(d, false);
}

pub fn set_kb_focus(d: &mut Desk, sk: Option<SKey>) {
    // While a popup grab is active, the topmost popup keeps the keyboard.
    let sk = match (d.seat.popup_grabs.last().copied(), sk) {
        (Some(p), Some(s)) if d.surfaces.contains(p) && d.surfaces[p].client == d.surfaces.get(s).map_or(p, |x| x.client) && d.window_of(p) == d.window_of(s) => Some(p),
        _ => sk,
    };
    if d.seat.kb_focus == sk {
        return;
    }
    if let Some(old) = d.seat.kb_focus {
        if let Some((ck, sid)) = client_of(d, old) {
            let serial = d.next_serial();
            if let Some(c) = d.clients.get_mut(ck) {
                for k in c.keyboards.clone() {
                    wl_keyboard::evt::leave(&mut c.conn, k, serial, sid);
                }
            }
        }
    }
    d.seat.kb_focus = sk;
    let Some(sk) = sk else { return };
    let Some((ck, sid)) = client_of(d, sk) else { return };
    crate::server::send_selection(d, ck);
    crate::server::send_primary(d, ck);
    let serial = d.next_serial();
    let keys = keys_array(&d.seat.keys);
    let m = current_mods(d);
    d.seat.sent_mods = m;
    if let Some(c) = d.clients.get_mut(ck) {
        for k in c.keyboards.clone() {
            wl_keyboard::evt::enter(&mut c.conn, k, serial, sid, &keys);
            wl_keyboard::evt::modifiers(&mut c.conn, k, serial, m[0], m[1], m[2], m[3]);
        }
    }
}

fn mod_bit(code: u32) -> u32 {
    match code {
        key::LSHIFT | key::RSHIFT => M_SHIFT,
        key::LCTRL | key::RCTRL => M_CTRL,
        key::LALT | key::RALT => M_ALT,
        key::LMETA | key::RMETA => M_SUPER,
        _ => 0,
    }
}

/// A key event from the backend (evdev code).
pub fn key(d: &mut Desk, code: u32, pressed: bool, time: u32) {
    let bit = mod_bit(code);
    if pressed {
        d.seat.held |= bit;
    } else {
        d.seat.held &= !bit;
    }
    // During a drag: Esc cancels it; Shift / Ctrl change copy ↔ move.
    if d.seat.grab == Grab::Dnd {
        if code == 1 && pressed {
            crate::dnd::cancel(d);
            return;
        }
        if bit != 0 {
            crate::dnd::choose(d);
        }
    }
    if pressed && d.seat.host_mods.is_none() {
        match code {
            key::CAPS => d.seat.caps = !d.seat.caps,
            key::NUMLOCK => d.seat.num = !d.seat.num,
            _ => {}
        }
    }
    // Alt+Shift switches the layout (us <-> ru) when released with nothing pressed in
    // between, so Alt+Shift+arrows stay editing keys (Windows switches on press).
    if pressed {
        let both = d.seat.held & M_ALT != 0 && d.seat.held & M_SHIFT != 0;
        d.seat.layout_chord = both && (bit == M_ALT || bit == M_SHIFT);
    } else if d.seat.layout_chord && (bit == M_ALT || bit == M_SHIFT) {
        d.seat.layout_chord = false;
        switch_layout(d);
    }
    // Super tap opens the start menu.
    if code == key::LMETA || code == key::RMETA {
        if pressed {
            d.seat.super_alone = true;
        } else if d.seat.super_alone {
            d.seat.super_alone = false;
            shell::toggle_start(d);
        }
    } else if pressed {
        d.seat.super_alone = false;
    }
    // Releasing Alt commits Alt+Tab.
    if !pressed && bit == M_ALT && d.overlay.switcher.is_some() && d.seat.held & M_ALT == 0 {
        shell::switcher_commit(d);
    }
    if !pressed {
        if let Some(i) = d.seat.swallowed.iter().position(|k| *k == code) {
            d.seat.swallowed.swap_remove(i);
            send_mods(d, false);
            return;
        }
    }
    if pressed && shortcut(d, code) {
        d.seat.swallowed.push(code);
        return;
    }
    if pressed {
        if !d.seat.keys.contains(&code) {
            d.seat.keys.push(code);
        }
    } else {
        d.seat.keys.retain(|k| *k != code);
    }
    if crate::snip::key(d, code, pressed) {
        return;
    }
    if shell::menu_key(d, code, pressed) {
        return;
    }
    if let Some(sk) = d.seat.kb_focus {
        if let Some((ck, _)) = client_of(d, sk) {
            let serial = d.next_serial();
            if let Some(c) = d.clients.get_mut(ck) {
                for k in c.keyboards.clone() {
                    wl_keyboard::evt::key(&mut c.conn, k, serial, time, code, pressed as u32);
                }
            }
        }
    }
    send_mods(d, false);
}

/// Next layout group (us <-> ru). In nested mode the host owns the layout.
pub fn switch_layout(d: &mut Desk) {
    if d.seat.host_mods.is_some() {
        return;
    }
    d.seat.group = (d.seat.group + 1) % 2;
    shell::damage_clocks(d);
    send_mods(d, false);
}

/// Current layout group: 0 us, 1 ru.
pub fn layout_group(d: &Desk) -> u32 {
    d.seat.host_mods.map_or(d.seat.group, |m| m[3])
}

/// Modifier state for the shell's own text fields.
pub fn text_mods(d: &Desk) -> crate::textedit::Mods {
    let h = d.seat.held;
    let caps = d.seat.host_mods.map_or(d.seat.caps, |m| m[2] & 2 != 0);
    crate::textedit::Mods { shift: h & M_SHIFT != 0, ctrl: h & M_CTRL != 0, alt: h & M_ALT != 0, caps, group: layout_group(d) }
}

/// Ctrl+V into the focused window (after Win+V picked an entry): the app pastes the
/// selection it was just offered, as if the user pressed the keys.
pub fn paste_into_focus(d: &mut Desk) {
    let Some(sk) = d.seat.kb_focus else { return };
    let Some((ck, _)) = client_of(d, sk) else { return };
    // Terminals paste with Ctrl+Shift+V (Ctrl+V is a control character there).
    let app = d.window_of(sk).and_then(|w| d.windows.get(w)).map(|w| w.app_id.to_lowercase()).unwrap_or_default();
    let name = app.rsplit('.').next().unwrap_or(&app).to_string();
    const TERMS: [&str; 9] = ["foot", "kitty", "alacritty", "wezterm", "konsole", "xterm", "ghostty", "ptyxis", "st"];
    let terminal = app.contains("term") || TERMS.contains(&name.as_str());
    let held = if terminal { 4 | 1 } else { 4 };
    let m = current_mods(d);
    let t = d.now_ms();
    let s1 = d.next_serial();
    let s2 = d.next_serial();
    let s3 = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    for k in c.keyboards.clone() {
        // Only Ctrl (+Shift) held, whatever is physically down (Win may still be).
        wl_keyboard::evt::modifiers(&mut c.conn, k, s1, held, 0, m[2], m[3]);
        wl_keyboard::evt::key(&mut c.conn, k, s2, t, key::V, 1);
        wl_keyboard::evt::key(&mut c.conn, k, s3, t, key::V, 0);
    }
    send_mods(d, true);
}

/// Host lost keyboard focus: release everything.
pub fn release_all_keys(d: &mut Desk) {
    let t = d.now_ms();
    for k in d.seat.keys.clone() {
        key(d, k, false, t);
    }
    d.seat.held = 0;
    d.seat.swallowed.clear();
}

fn is_mod(d: &Desk) -> bool {
    let h = d.seat.held;
    h & M_SUPER != 0 || (d.cfg.nested && h & M_CTRL != 0 && h & M_ALT != 0)
}

/// Global shortcuts. Returns true if consumed.
fn shortcut(d: &mut Desk, code: u32) -> bool {
    let h = d.seat.held;
    let shift = h & M_SHIFT != 0;
    let alt = h & M_ALT != 0;
    let ctrl = h & M_CTRL != 0;
    let m = is_mod(d);
    let fw = wm::focused(d);
    // Volume keys, with or without modifiers.
    match code {
        key::MUTE => {
            crate::tray::toggle_mute(d, false);
            return true;
        }
        key::VOLUMEDOWN | key::VOLUMEUP => {
            crate::tray::volume_step(d, if code == key::VOLUMEUP { 5 } else { -5 });
            return true;
        }
        _ => {}
    }
    // Win+Tab: task view (the desktops live there). In nested mode Ctrl+Alt+Tab too.
    if code == key::TAB && h & M_SUPER != 0 {
        crate::taskview::toggle(d);
        return true;
    }
    // Alt+Tab.
    if code == key::TAB && alt {
        shell::switcher_step(d, if shift { -1 } else { 1 });
        return true;
    }
    if d.overlay.switcher.is_some() && code == key::ESC {
        d.overlay.switcher = None;
        d.damage_all();
        return true;
    }
    if alt && !m && code == key::F4 {
        if let Some(wk) = fw {
            wm::close(d, wk);
        }
        return true;
    }
    if !m {
        if code == key::SYSRQ {
            let path = format!("{}/screenshot-{}.png", std::env::var("HOME").unwrap_or("/tmp".into()), d.now_ms());
            d.pending_screenshot = Some(path);
            return true;
        }
        return false;
    }
    // Workspace switching works for Super+Ctrl+arrows and Mod+PageUp/Down.
    let ws_mod = h & M_SUPER != 0 && ctrl;
    match code {
        key::LEFT | key::RIGHT if ws_mod => {
            let ws = d.workspace as i64 + if code == key::RIGHT { 1 } else { -1 };
            if ws >= 0 {
                wm::switch_workspace(d, ws as u32);
            }
        }
        key::PAGEUP | key::PAGEDOWN => {
            let ws = d.workspace as i64 + if code == key::PAGEDOWN { 1 } else { -1 };
            if shift {
                if let (Some(wk), true) = (fw, ws >= 0) {
                    wm::move_to_workspace(d, wk, ws as u32);
                }
            } else if ws >= 0 {
                wm::switch_workspace(d, ws as u32);
            }
        }
        key::D if ws_mod => wm::new_workspace(d),
        key::N => wm::new_workspace(d),
        key::F4 if ws_mod || ctrl => wm::close_workspace(d),
        key::LEFT | key::RIGHT => {
            if let Some(wk) = fw {
                if shift {
                    wm::move_to_output(d, wk, code == key::RIGHT);
                } else {
                    wm::snap_key(d, wk, code == key::RIGHT);
                }
            }
        }
        key::UP | key::DOWN => {
            if let Some(wk) = fw {
                wm::vertical_key(d, wk, code == key::UP);
            }
        }
        key::D | key::M => wm::show_desktop(d),
        // Win+S = search: our start menu (the bare Win key is eaten by Windows when nested).
        // Win+Shift+S: a snip of the screen to the clipboard (snip.rs).
        key::S if shift => crate::snip::toggle(d),
        key::S => shell::toggle_start(d),
        // Win+P: how several monitors are used (monitors.rs).
        key::P => crate::monitors::toggle(d),
        // Win+V: clipboard history.
        key::V => crate::clipboard::toggle(d),
        // Win+I: settings, as in Windows.
        key::I => shell::open_settings(d),
        // Win+Space: next keyboard layout, as in Windows 11.
        key::SPACE => switch_layout(d),
        key::E => shell::open_files(d, None),
        key::T | key::ENTER => {
            let c = d.cfg.terminal.clone();
            crate::life::launch_cmd(d, &c);
        }
        key::R => {
            let c = d.cfg.launcher.clone();
            d.spawn(&c);
        }
        key::Q => {
            if let Some(wk) = fw {
                wm::close(d, wk);
            }
        }
        key::BACKSPACE if shift => d.running = false,
        key::N1..=key::N9 => shell::taskbar_nth(d, (code - key::N1) as usize),
        _ => return false,
    }
    d.seat.super_alone = false;
    true
}

// ------------------------------------------------------------------ cursor

pub fn set_client_cursor(d: &mut Desk, ck: CKey, sk: Option<SKey>, hx: i32, hy: i32) {
    let focus_client = d.seat.ptr_focus.and_then(|s| d.surfaces.get(s)).map(|s| s.client);
    if focus_client != Some(ck) {
        return;
    }
    damage_cursor(d);
    match sk {
        None => {
            d.seat.cursor_surface = None;
            d.seat.cursor_hidden = true;
        }
        Some(sk) => {
            let s = &mut d.surfaces[sk];
            if s.role != Role::None && s.role != Role::Cursor {
                return;
            }
            s.role = Role::Cursor;
            d.seat.cursor_surface = Some(sk);
            d.seat.cursor_hotspot = (hx, hy);
            d.seat.cursor_hidden = false;
        }
    }
    damage_cursor(d);
}

pub fn set_shape_cursor(d: &mut Desk, ck: CKey, shape: u32) {
    let focus_client = d.seat.ptr_focus.and_then(|s| d.surfaces.get(s)).map(|s| s.client);
    if focus_client != Some(ck) {
        return;
    }
    damage_cursor(d);
    d.seat.cursor_surface = None;
    d.seat.cursor_hidden = false;
    d.seat.cursor_shape = shape;
    damage_cursor(d);
}

/// Damages where the cursor was drawn and where it will be drawn now.
pub fn damage_cursor(d: &mut Desk) {
    let old = d.cursor_drawn;
    d.damage(old);
    let r = crate::render::cursor_rect(d);
    d.damage(r);
}

/// A surface died: drop every reference the seat holds to it.
pub fn surface_gone(d: &mut Desk, sk: SKey) {
    crate::dnd::surface_gone(d, sk);
    if d.seat.ptr_focus == Some(sk) {
        d.seat.ptr_focus = None;
    }
    if d.seat.cursor_surface == Some(sk) {
        d.seat.cursor_surface = None;
        damage_cursor(d);
    }
    if d.seat.grab == Grab::Implicit(sk) {
        d.seat.grab = Grab::None;
    }
    d.seat.popup_grabs.retain(|k| *k != sk);
    if d.seat.kb_focus == Some(sk) {
        d.seat.kb_focus = None;
        // Hand focus back to the owning window if it still exists.
        let back = d.mru.first().and_then(|w| d.windows.get(*w)).map(|w| w.surf).filter(|s| *s != sk);
        set_kb_focus(d, back);
    }
}

pub fn window_gone(d: &mut Desk, wk: WKey) {
    match d.seat.grab {
        Grab::Move { wk: g, .. } | Grab::Resize { wk: g, .. } | Grab::Deco { wk: g, .. } if g == wk => {
            d.seat.grab = Grab::None;
            if let Some(r) = d.overlay.snap_preview.take() {
                d.damage(r.inflate(4));
            }
        }
        _ => {}
    }
    if d.seat.resizing == Some(wk) {
        d.seat.resizing = None;
    }
    if d.seat.hover.map(|h| h.0) == Some(wk) {
        d.seat.hover = None;
    }
    if d.seat.last_click.3 == Some(wk) {
        d.seat.last_click.3 = None;
    }
}
