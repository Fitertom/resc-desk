//! Drag and drop between programs (wl_data_device): a client starts a drag while a button
//! is held; the pointer then belongs to the drag. The surface under it hears enter /
//! motion / leave with an offer of the source's types, says what it would take (accept,
//! set_actions), and on release gets the drop; the data itself goes through the same
//! receive → send pipe as the clipboard. The drag icon follows the pointer.
use crate::desk::Desk;
use crate::input::{self, Grab};
use crate::state::*;
use crate::wm::{self, Hit};
use wl::protocol::*;
use wl::{Fixed, Iface, ObjId};

pub const COPY: u32 = 1;
pub const MOVE: u32 = 2;
pub const ASK: u32 = 4;

pub struct Target {
    pub sk: SKey,
    pub ck: CKey,
    pub dev: ObjId,
    /// 0 when the drag has no source (inside one client).
    pub offer: ObjId,
    pub accepted: bool,
    pub dest: u32,
    pub pref: u32,
    pub action: u32,
}

pub struct Dnd {
    pub src: Option<DKey>,
    /// The client that started it (a drag without a source stays inside it).
    pub client: CKey,
    pub icon: Option<SKey>,
    /// Where the icon's top-left is from the pointer.
    pub icon_off: (i32, i32),
    pub target: Option<Target>,
    /// Dropped; waiting for the target to finish reading.
    pub dropped: bool,
}

/// wl_data_device.start_drag.
pub fn start(d: &mut Desk, ck: CKey, src: Option<DKey>, origin: Option<SKey>, icon: Option<SKey>) {
    // Only with a button held on that client's surface (the implicit grab).
    let ok = !d.seat.buttons.is_empty()
        && match d.seat.grab {
            Grab::Implicit(sk) => d.surfaces.get(sk).map(|s| s.client) == Some(ck),
            _ => false,
        }
        && origin.is_some()
        && d.dnd.is_none();
    if !ok {
        if let Some(dk) = src {
            source_evt(d, dk, |c, obj, _| wl_data_source::evt::cancelled(c, obj));
        }
        return;
    }
    if let Some(ik) = icon {
        let s = &mut d.surfaces[ik];
        if s.role == Role::None || s.role == Role::DndIcon {
            s.role = Role::DndIcon;
        }
    }
    d.dnd = Some(Dnd { src, client: ck, icon: icon.filter(|k| d.surfaces[*k].role == Role::DndIcon), icon_off: (0, 0), target: None, dropped: false });
    d.seat.grab = Grab::Dnd;
    // The pointer leaves whoever had it; the drag's enter / motion take over.
    input::set_ptr_focus(d, None);
    input::damage_cursor(d);
    motion(d, d.now_ms());
}

fn source_evt(d: &mut Desk, dk: DKey, f: impl FnOnce(&mut wl::Conn, ObjId, u32)) {
    let Some(s) = d.sources.get(dk) else { return };
    if s.own.is_some() || s.x11 || s.primary {
        return;
    }
    let (ck, obj) = (s.client, s.obj);
    if let Some(c) = d.clients.get_mut(ck) {
        let v = c.version(obj);
        f(&mut c.conn, obj, v);
    }
}

/// The pointer moved during a drag.
pub fn motion(d: &mut Desk, time: u32) {
    let Some(dnd) = d.dnd.as_ref() else { return };
    if dnd.dropped {
        return;
    }
    let (x, y) = (d.seat.x, d.seat.y);
    let under = if crate::shell::taskbar_contains(d, x, y) {
        None
    } else {
        match wm::hit_test(d, x, y) {
            Some((_, Hit::Surface(sk, lx, ly))) => Some((sk, lx, ly)),
            _ => None,
        }
    };
    let cur = dnd.target.as_ref().map(|t| t.sk);
    match under {
        Some((sk, lx, ly)) if cur == Some(sk) => {
            let t = dnd.target.as_ref().unwrap();
            let (ck, dev) = (t.ck, t.dev);
            if let Some(c) = d.clients.get_mut(ck) {
                wl_data_device::evt::motion(&mut c.conn, dev, time, Fixed::from_f64(lx), Fixed::from_f64(ly));
            }
        }
        _ => {
            leave(d);
            if let Some((sk, lx, ly)) = under {
                enter(d, sk, lx, ly);
            }
        }
    }
}

fn enter(d: &mut Desk, sk: SKey, lx: f64, ly: f64) {
    let Some(dnd) = d.dnd.as_ref() else { return };
    let Some(s) = d.surfaces.get(sk) else { return };
    let (ck, sid) = (s.client, s.id);
    if dnd.src.is_none() && ck != dnd.client {
        return;
    }
    let src = dnd.src.and_then(|k| d.sources.get(k).map(|s| (k, s.mimes.clone(), s.actions)));
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    let Some(&dev) = c.data_devices.first() else { return };
    let ver = c.version(dev);
    let mut offer = 0;
    if let Some((dk, mimes, actions)) = src {
        offer = c.insert_server(Iface::WlDataOffer, ver, Data::DataOffer(dk));
        wl_data_device::evt::data_offer(&mut c.conn, dev, offer);
        for m in &mimes {
            wl_data_offer::evt::offer(&mut c.conn, offer, m);
        }
        if ver >= 3 {
            wl_data_offer::evt::source_actions(&mut c.conn, offer, actions);
        }
    }
    wl_data_device::evt::enter(&mut c.conn, dev, serial, sid, Fixed::from_f64(lx), Fixed::from_f64(ly), offer);
    if let Some(dnd) = d.dnd.as_mut() {
        dnd.target = Some(Target { sk, ck, dev, offer, accepted: false, dest: 0, pref: 0, action: 0 });
    }
}

fn leave(d: &mut Desk) {
    let Some(t) = d.dnd.as_mut().and_then(|n| n.target.take()) else { return };
    if let Some(c) = d.clients.get_mut(t.ck) {
        wl_data_device::evt::leave(&mut c.conn, t.dev);
    }
    if let Some(dk) = d.dnd.as_ref().and_then(|n| n.src) {
        source_evt(d, dk, |c, obj, _| wl_data_source::evt::target(c, obj, None));
    }
}

/// The last button went up: drop, or cancel when nobody takes it.
pub fn release(d: &mut Desk) {
    d.seat.grab = Grab::None;
    let Some(dnd) = d.dnd.as_ref() else { return };
    let src = dnd.src;
    let take = match &dnd.target {
        Some(_) if src.is_none() => true,
        Some(t) => t.accepted && (t.offer == 0 || t.action != 0 || version_of(d, t) < 3),
        None => false,
    };
    let Some(dnd) = d.dnd.as_mut() else { return };
    if take {
        let t = dnd.target.as_ref().unwrap();
        let (ck, dev) = (t.ck, t.dev);
        dnd.dropped = true;
        if let Some(c) = d.clients.get_mut(ck) {
            wl_data_device::evt::drop(&mut c.conn, dev);
        }
        if let Some(dk) = src {
            source_evt(d, dk, |c, obj, v| {
                if v >= 3 {
                    wl_data_source::evt::dnd_drop_performed(c, obj)
                }
            });
        } else {
            end(d);
        }
    } else {
        cancel(d);
    }
    input::damage_cursor(d);
}

fn version_of(d: &Desk, t: &Target) -> u32 {
    d.clients.get(t.ck).map_or(0, |c| c.version(t.offer))
}

/// Esc, a vanished source, or nobody took it.
pub fn cancel(d: &mut Desk) {
    if d.dnd.is_none() {
        return;
    }
    leave(d);
    if let Some(dk) = d.dnd.as_ref().and_then(|n| n.src) {
        source_evt(d, dk, |c, obj, _| wl_data_source::evt::cancelled(c, obj));
    }
    end(d);
}

fn end(d: &mut Desk) {
    if let Some(ik) = d.dnd.as_ref().and_then(|n| n.icon) {
        d.damage_surface_tree(ik);
    }
    input::damage_cursor(d);
    d.dnd = None;
    if d.seat.grab == Grab::Dnd {
        d.seat.grab = Grab::None;
    }
    input::damage_cursor(d);
}

/// Whether `offer` of client `ck` is the current drag's.
fn is_ours(d: &Desk, ck: CKey, offer: ObjId) -> bool {
    d.dnd.as_ref().and_then(|n| n.target.as_ref()).map_or(false, |t| t.ck == ck && t.offer == offer && offer != 0)
}

pub fn accept(d: &mut Desk, ck: CKey, offer: ObjId, mime: Option<String>) {
    if !is_ours(d, ck, offer) {
        return;
    }
    let Some(dnd) = d.dnd.as_mut() else { return };
    if let Some(t) = dnd.target.as_mut() {
        t.accepted = mime.is_some();
    }
    if let Some(dk) = dnd.src {
        source_evt(d, dk, |c, obj, _| wl_data_source::evt::target(c, obj, mime.as_deref()));
    }
}

pub fn set_actions(d: &mut Desk, ck: CKey, offer: ObjId, dest: u32, pref: u32) {
    if !is_ours(d, ck, offer) {
        return;
    }
    if let Some(t) = d.dnd.as_mut().and_then(|n| n.target.as_mut()) {
        t.dest = dest;
        t.pref = pref;
    }
    choose(d);
}

/// The action both sides allow: the target's preference, else Shift moves / Ctrl copies.
pub fn choose(d: &mut Desk) {
    let Some(dnd) = d.dnd.as_ref() else { return };
    let Some(t) = dnd.target.as_ref() else { return };
    let src_actions = dnd.src.and_then(|k| d.sources.get(k)).map_or(0, |s| s.actions);
    let both = src_actions & t.dest;
    let mods = input::held_mods(d);
    let a = if both == 0 {
        0
    } else if mods & input::M_SHIFT != 0 && both & MOVE != 0 {
        MOVE
    } else if mods & input::M_CTRL != 0 && both & COPY != 0 {
        COPY
    } else if both & t.pref != 0 {
        t.pref
    } else {
        1 << both.trailing_zeros()
    };
    if a == t.action {
        return;
    }
    let (ck, offer, src) = (t.ck, t.offer, dnd.src);
    if let Some(t) = d.dnd.as_mut().and_then(|n| n.target.as_mut()) {
        t.action = a;
    }
    if let Some(c) = d.clients.get_mut(ck) {
        if c.version(offer) >= 3 {
            wl_data_offer::evt::action(&mut c.conn, offer, a);
        }
    }
    if let Some(dk) = src {
        source_evt(d, dk, |c, obj, v| {
            if v >= 3 {
                wl_data_source::evt::action(c, obj, a)
            }
        });
    }
}

/// wl_data_offer.finish: the target has read what it wanted.
pub fn finish(d: &mut Desk, ck: CKey, offer: ObjId) {
    if !is_ours(d, ck, offer) || !d.dnd.as_ref().map_or(false, |n| n.dropped) {
        return;
    }
    if let Some(dk) = d.dnd.as_ref().and_then(|n| n.src) {
        source_evt(d, dk, |c, obj, v| {
            if v >= 3 {
                wl_data_source::evt::dnd_finished(c, obj)
            }
        });
    }
    end(d);
}

/// wl_data_offer.destroy: after a drop without finish, an old client is done; otherwise
/// nothing (the drag goes on).
pub fn offer_gone(d: &mut Desk, ck: CKey, offer: ObjId) {
    if !is_ours(d, ck, offer) {
        return;
    }
    let dropped = d.dnd.as_ref().map_or(false, |n| n.dropped);
    if !dropped {
        if let Some(t) = d.dnd.as_mut().and_then(|n| n.target.as_mut()) {
            t.offer = 0;
            t.accepted = false;
        }
        return;
    }
    let v = d.clients.get(ck).map_or(0, |c| c.version(offer));
    if let Some(dk) = d.dnd.as_ref().and_then(|n| n.src) {
        source_evt(d, dk, |c, obj, sv| {
            if v >= 3 {
                wl_data_source::evt::cancelled(c, obj)
            } else if sv >= 3 {
                wl_data_source::evt::dnd_finished(c, obj)
            }
        });
    }
    end(d);
}

pub fn source_gone(d: &mut Desk, dk: DKey) {
    if d.dnd.as_ref().map_or(false, |n| n.src == Some(dk)) {
        if let Some(n) = d.dnd.as_mut() {
            n.src = None;
        }
        leave(d);
        end(d);
    }
}

pub fn surface_gone(d: &mut Desk, sk: SKey) {
    let Some(n) = d.dnd.as_mut() else { return };
    if n.icon == Some(sk) {
        n.icon = None;
    }
    if n.target.as_ref().map_or(false, |t| t.sk == sk) {
        n.target = None;
        if n.dropped {
            end(d);
        }
    }
}

/// The icon's place on screen (logical), when there is one with content.
pub fn icon_rect(d: &Desk) -> Option<(SKey, crate::geom::Rect)> {
    let n = d.dnd.as_ref()?;
    if n.dropped {
        return None;
    }
    let ik = n.icon?;
    let s = d.surfaces.get(ik)?;
    s.buffer.as_ref()?;
    Some((ik, crate::geom::Rect::new(d.seat.x as i32 + n.icon_off.0, d.seat.y as i32 + n.icon_off.1, s.w, s.h)))
}
