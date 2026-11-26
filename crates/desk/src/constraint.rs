//! Games taking the mouse: pointer locks and confinement (pointer-constraints-unstable-v1) and
//! raw motion (relative-pointer-unstable-v1). A game (GLFW, SDL) locks the pointer to its
//! surface and turns the camera by the relative motion it is sent.
//!
//! A constraint is in force only while its window has the keyboard, the pointer is on its
//! surface and no shell panel is open: Alt+Tab, Win or a click elsewhere let the mouse go, and
//! a persistent constraint takes it back when the window has it all again.

use crate::desk::Desk;
use crate::geom::Rect;
use crate::input::Grab;
use crate::state::*;
use wl::protocol::*;
use wl::{Fixed, ObjId};

#[derive(Clone, Copy)]
pub struct Constraint {
    pub ck: CKey,
    pub id: ObjId,
    pub surf: SKey,
    /// A lock (the pointer stays put), or a confinement (it stays inside the surface).
    pub lock: bool,
    /// Taken again after it is let go; a one-shot one is spent.
    pub persistent: bool,
    pub active: bool,
    pub spent: bool,
    /// Where the client says the pointer is in the game's eyes: it is left there on unlocking.
    pub hint: Option<(f64, f64)>,
}

/// zwp_pointer_constraints_v1 error: already_constrained.
const ALREADY_CONSTRAINED: u32 = 1;

/// lock_pointer / confine_pointer. False: the surface already has one (an error was posted).
pub fn create(d: &mut Desk, ck: CKey, mgr: ObjId, id: ObjId, surf: SKey, lock: bool, lifetime: u32) -> bool {
    if d.constraints.iter().any(|c| c.surf == surf && !c.spent) {
        d.post_error(ck, mgr, ALREADY_CONSTRAINED, "the surface already has a pointer constraint");
        return false;
    }
    d.constraints.push(Constraint { ck, id, surf, lock, persistent: lifetime == 2, active: false, spent: false, hint: None });
    update(d);
    true
}

/// The client destroyed its constraint object.
pub fn destroy(d: &mut Desk, ck: CKey, id: ObjId) {
    let Some(i) = d.constraints.iter().position(|c| c.ck == ck && c.id == id) else { return };
    let c = d.constraints.remove(i);
    if c.active && c.lock {
        leave_at_hint(d, &c);
    }
}

pub fn set_hint(d: &mut Desk, ck: CKey, id: ObjId, x: f64, y: f64) {
    if let Some(c) = d.constraints.iter_mut().find(|c| c.ck == ck && c.id == id) {
        c.hint = Some((x, y));
    }
}

/// A surface died: its constraints with it (their objects stay, inert).
pub fn surface_gone(d: &mut Desk, sk: SKey) {
    d.constraints.retain(|c| c.surf != sk);
}

/// A shell panel or menu is open over the windows: the mouse is the shell's.
fn shell_busy(d: &Desk) -> bool {
    d.overlay.start_menu.is_some() || d.overlay.switcher.is_some() || crate::taskview::is_open(d) || d.tray.fly.is_some() || d.clip.open.is_some() || d.mons.panel.is_some()
}

/// Each constraint in force or not, as things are now; the clients told when that changes.
pub fn update(d: &mut Desk) {
    if d.constraints.is_empty() {
        return;
    }
    let busy = shell_busy(d);
    let grab_ok = |sk: SKey| matches!(d.seat.grab, Grab::None) || d.seat.grab == Grab::Implicit(sk);
    let kb_win = d.seat.kb_focus.and_then(|k| d.window_of(k));
    let mut changes = Vec::new();
    for (i, c) in d.constraints.iter().enumerate() {
        let want = !c.spent && !busy && d.surfaces.contains(c.surf) && d.seat.ptr_focus == Some(c.surf) && grab_ok(c.surf) && kb_win.is_some() && kb_win == d.window_of(c.surf);
        if want != c.active {
            changes.push((i, want));
        }
    }
    for (i, on) in changes {
        let c = &mut d.constraints[i];
        c.active = on;
        if !on && !c.persistent {
            c.spent = true;
        }
        let (ck, id, lock) = (c.ck, c.id, c.lock);
        if let Some(cl) = d.clients.get_mut(ck) {
            match (lock, on) {
                (true, true) => zwp_locked_pointer_v1::evt::locked(&mut cl.conn, id),
                (true, false) => zwp_locked_pointer_v1::evt::unlocked(&mut cl.conn, id),
                (false, true) => zwp_confined_pointer_v1::evt::confined(&mut cl.conn, id),
                (false, false) => zwp_confined_pointer_v1::evt::unconfined(&mut cl.conn, id),
            }
        }
        if lock && !on {
            let c = d.constraints[i];
            leave_at_hint(d, &c);
        }
    }
}

/// Unlocked: the pointer shows where the game last said it was, if it said.
fn leave_at_hint(d: &mut Desk, c: &Constraint) {
    let Some((hx, hy)) = c.hint else { return };
    let Some((ox, oy)) = d.surface_origin(c.surf) else { return };
    let (x, y) = (ox as f64 + hx, oy as f64 + hy);
    if d.output_at(x, y).is_some() {
        let t = d.now_ms();
        crate::input::pointer_motion(d, x, y, t);
    }
}

/// The pointer is locked: motion moves nothing, the game gets it as relative motion only.
pub fn locked(d: &Desk) -> bool {
    d.constraints.iter().any(|c| c.active && c.lock)
}

/// The pointer is confined: the surface it must stay on, in global coordinates.
pub fn confine_rect(d: &Desk) -> Option<Rect> {
    let c = d.constraints.iter().find(|c| c.active && !c.lock)?;
    let (x, y) = d.surface_origin(c.surf)?;
    let s = d.surfaces.get(c.surf)?;
    Some(Rect::new(x, y, s.w.max(1), s.h.max(1)))
}

/// Raw motion to the client the pointer is on: `dx, dy` as the pointer moves (accelerated, in
/// surface coordinates), `raw` as the device moved.
pub fn relative(d: &mut Desk, dx: f64, dy: f64, raw: (f64, f64), ms: u64) {
    let Some(sk) = d.seat.ptr_focus else { return };
    let Some(ck) = d.surfaces.get(sk).map(|s| s.client) else { return };
    let Some(c) = d.clients.get_mut(ck) else { return };
    if c.rel_pointers.is_empty() {
        return;
    }
    let us = ms.wrapping_mul(1000);
    for r in c.rel_pointers.clone() {
        zwp_relative_pointer_v1::evt::relative_motion(&mut c.conn, r, (us >> 32) as u32, us as u32, Fixed::from_f64(dx), Fixed::from_f64(dy), Fixed::from_f64(raw.0), Fixed::from_f64(raw.1));
    }
}
