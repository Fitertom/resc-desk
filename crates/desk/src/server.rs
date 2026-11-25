//! Protocol dispatch: every request from every client lands here.
use crate::desk::Desk;
use crate::geom::{Rect, Region};
use crate::shm::{Buffer, Pool, FMT_ARGB8888, FMT_XRGB8888};
use crate::state::*;
use std::cell::Cell;
use std::os::fd::AsFd;
use std::rc::Rc;
use wl::protocol::*;
use wl::{Iface, ObjId};

macro_rules! parse {
    ($d:expr, $ck:expr, $h:expr, $m:ident) => {{
        let c = &mut $d.clients[$ck];
        let mut a = c.conn.args(&$h);
        match $m::Request::parse($h.op, &mut a) {
            Ok(r) => r,
            Err(e) => {
                let name = stringify!($m);
                c.kill(format!("bad {name} request op {}: {e:?}", $h.op));
                return;
            }
        }
    }};
}

/// Reads everything a client sent and dispatches it.
pub fn client_readable(d: &mut Desk, ck: CKey) {
    loop {
        let Some(c) = d.clients.get_mut(ck) else { return };
        match c.conn.read() {
            Ok(0) => {
                c.kill("eof".into());
                return;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                c.kill(format!("read: {e}"));
                return;
            }
        }
        loop {
            let Some(c) = d.clients.get_mut(ck) else { return };
            if c.dead.is_some() {
                return;
            }
            let h = match c.conn.next_msg() {
                Ok(Some(h)) => h,
                Ok(None) => break,
                Err(e) => {
                    c.kill(format!("framing: {e:?}"));
                    return;
                }
            };
            let Some(o) = c.obj(h.obj) else {
                // Request to an object we already destroyed (race) - drop it.
                continue;
            };
            let (iface, ver) = (o.iface, o.version);
            dispatch(d, ck, h, iface, ver);
        }
    }
}

fn new_obj(d: &mut Desk, ck: CKey, id: ObjId, iface: Iface, ver: u32, data: Data) -> bool {
    d.clients[ck].insert(id, iface, ver, data)
}

/// Destroys a client-created object and tells the client the id is free.
fn destroy_obj(d: &mut Desk, ck: CKey, id: ObjId) {
    let c = &mut d.clients[ck];
    c.remove(id);
    if id < SERVER_ID_BASE {
        wl_display::evt::delete_id(&mut c.conn, 1, id);
    }
}

fn data_of(d: &Desk, ck: CKey, id: ObjId) -> Option<&Data> {
    d.clients.get(ck)?.obj(id).map(|o| &o.data)
}

/// Surface behind a surface-role object; None if that surface is already gone.
fn surface_of(d: &Desk, ck: CKey, id: ObjId) -> Option<SKey> {
    surface_key(d, ck, id).filter(|k| d.surfaces.contains(*k))
}

fn surface_key(d: &Desk, ck: CKey, id: ObjId) -> Option<SKey> {
    match data_of(d, ck, id)? {
        Data::Surface(s)
        | Data::XdgSurface(s)
        | Data::Toplevel(s)
        | Data::Popup(s)
        | Data::Subsurface(s)
        | Data::Decoration(s)
        | Data::Viewport(s)
        | Data::FracScale(s)
        | Data::LayerSurface(s) => Some(*s),
        _ => None,
    }
}

fn region_of(d: &Desk, ck: CKey, id: ObjId) -> Option<Region> {
    match data_of(d, ck, id)? {
        Data::Region(r) => Some(r.clone()),
        _ => None,
    }
}

fn dispatch(d: &mut Desk, ck: CKey, h: wl::wire::MsgHdr, iface: Iface, ver: u32) {
    let id = h.obj;
    match iface {
        Iface::WlDisplay => match parse!(d, ck, h, wl_display) {
            wl_display::Request::Sync { callback } => {
                let s = d.serial;
                let c = &mut d.clients[ck];
                wl_callback::evt::done(&mut c.conn, callback, s);
                wl_display::evt::delete_id(&mut c.conn, 1, callback);
            }
            wl_display::Request::GetRegistry { registry } => {
                if !new_obj(d, ck, registry, Iface::WlRegistry, 1, Data::None) {
                    return;
                }
                d.clients[ck].registries.push(registry);
                let globals: Vec<Global> = d.globals.iter().filter(|g| g.alive).copied().collect();
                let c = &mut d.clients[ck];
                for g in globals {
                    wl_registry::evt::global(&mut c.conn, registry, g.name, g.iface.name(), g.version);
                }
            }
        },
        Iface::WlRegistry => match parse!(d, ck, h, wl_registry) {
            wl_registry::Request::Bind { name, id: nid } => bind(d, ck, name, nid),
        },
        Iface::WlCompositor => match parse!(d, ck, h, wl_compositor) {
            wl_compositor::Request::CreateSurface { id: sid } => {
                let sk = d.surfaces.insert(Surface::new(ck, sid));
                if !new_obj(d, ck, sid, Iface::WlSurface, ver, Data::Surface(sk)) {
                    d.surfaces.remove(sk);
                } else if d.xwm.as_ref().map_or(false, |x| x.client == Some(ck)) {
                    crate::xwm::surface_created(d, sk);
                }
            }
            wl_compositor::Request::CreateRegion { id: rid } => {
                new_obj(d, ck, rid, Iface::WlRegion, 1, Data::Region(Region::default()));
            }
        },
        Iface::WlRegion => {
            let r = parse!(d, ck, h, wl_region);
            if let wl_region::Request::Destroy = r {
                destroy_obj(d, ck, id);
                return;
            }
            if let Some(Obj { data: Data::Region(reg), .. }) = d.clients[ck].obj_mut(id) {
                match r {
                    wl_region::Request::Add { x, y, width, height } => reg.ops.push((true, Rect::new(x, y, width, height))),
                    wl_region::Request::Subtract { x, y, width, height } => {
                        reg.ops.push((false, Rect::new(x, y, width, height)))
                    }
                    _ => {}
                }
            }
        }
        Iface::WlSurface => {
            let r = parse!(d, ck, h, wl_surface);
            surface_request(d, ck, id, ver, r)
        }
        Iface::WlCallback => {}
        Iface::WlShm => match parse!(d, ck, h, wl_shm) {
            wl_shm::Request::CreatePool { id: pid, fd, size } => {
                if size <= 0 {
                    d.post_error(ck, id, wl_shm::error::INVALID_STRIDE, "invalid pool size");
                    return;
                }
                match Pool::new(fd, size as usize) {
                    Ok(p) => {
                        new_obj(d, ck, pid, Iface::WlShmPool, 1, Data::Pool(Rc::new(p)));
                    }
                    Err(e) => d.post_error(ck, id, wl_shm::error::INVALID_FD, &format!("mmap failed: {e}")),
                }
            }
            wl_shm::Request::Release => destroy_obj(d, ck, id),
        },
        Iface::WlShmPool => {
            let r = parse!(d, ck, h, wl_shm_pool);
            let Some(Data::Pool(pool)) = data_of(d, ck, id) else { return };
            let pool = pool.clone();
            match r {
                wl_shm_pool::Request::CreateBuffer { id: bid, offset, width, height, stride, format } => {
                    if format != FMT_ARGB8888 && format != FMT_XRGB8888 {
                        d.post_error(ck, id, wl_shm::error::INVALID_FORMAT, "unsupported format");
                        return;
                    }
                    let b = Buffer {
                        pool: Some(pool),
                        offset: offset.max(0) as usize,
                        w: width,
                        h: height,
                        stride,
                        format,
                        broken: Cell::new(false),
                        solid: None,
                        dmabuf: None,
                    };
                    if offset < 0 || !b.valid_for_pool() {
                        d.post_error(ck, id, wl_shm::error::INVALID_STRIDE, "buffer outside pool");
                        return;
                    }
                    new_obj(d, ck, bid, Iface::WlBuffer, 1, Data::Buffer(Rc::new(b)));
                }
                wl_shm_pool::Request::Destroy => destroy_obj(d, ck, id),
                wl_shm_pool::Request::Resize { size } => {
                    if let Err(e) = pool.resize(size.max(0) as usize) {
                        d.post_error(ck, id, wl_shm::error::INVALID_FD, &format!("remap failed: {e}"));
                    }
                }
            }
        }
        Iface::WlBuffer => {
            if let wl_buffer::Request::Destroy = parse!(d, ck, h, wl_buffer) {
                destroy_obj(d, ck, id);
            }
        }
        Iface::WlSubcompositor => match parse!(d, ck, h, wl_subcompositor) {
            wl_subcompositor::Request::Destroy => destroy_obj(d, ck, id),
            wl_subcompositor::Request::GetSubsurface { id: nid, surface, parent } => {
                let (Some(sk), Some(pk)) = (surface_of(d, ck, surface), surface_of(d, ck, parent)) else {
                    d.post_error(ck, id, wl_subcompositor::error::BAD_SURFACE, "bad surface");
                    return;
                };
                if sk == pk || d.surfaces[sk].role != Role::None {
                    d.post_error(ck, id, wl_subcompositor::error::BAD_SURFACE, "surface already has a role");
                    return;
                }
                if !new_obj(d, ck, nid, Iface::WlSubsurface, 1, Data::Subsurface(sk)) {
                    return;
                }
                let s = &mut d.surfaces[sk];
                s.role = Role::Subsurface;
                s.parent = Some(pk);
                s.sub_sync = true;
                d.surfaces[pk].subs.push(sk);
            }
        },
        Iface::WlSubsurface => {
            let r = parse!(d, ck, h, wl_subsurface);
            let Some(sk) = surface_of(d, ck, id) else { return };
            match r {
                wl_subsurface::Request::Destroy => {
                    d.damage_surface_tree(sk);
                    detach_subsurface(d, sk);
                    if let Some(s) = d.surfaces.get_mut(sk) {
                        s.role = Role::None;
                    }
                    destroy_obj(d, ck, id);
                }
                wl_subsurface::Request::SetPosition { x, y } => d.surfaces[sk].sub_pos_pending = Some((x, y)),
                wl_subsurface::Request::PlaceAbove { sibling } | wl_subsurface::Request::PlaceBelow { sibling } => {
                    let above = matches!(r, wl_subsurface::Request::PlaceAbove { .. });
                    let Some(pk) = d.surfaces[sk].parent else { return };
                    let Some(sib) = surface_of(d, ck, sibling) else { return };
                    let marker = if sib == pk { SKey::default() } else { sib };
                    let subs = &mut d.surfaces[pk].subs;
                    subs.retain(|k| *k != sk);
                    if let Some(i) = subs.iter().position(|k| *k == marker) {
                        subs.insert(if above { i + 1 } else { i }, sk);
                    } else {
                        subs.push(sk);
                    }
                    d.damage_surface_tree(pk);
                }
                wl_subsurface::Request::SetSync => d.surfaces[sk].sub_sync = true,
                wl_subsurface::Request::SetDesync => {
                    d.surfaces[sk].sub_sync = false;
                    if !effectively_sync(d, sk) {
                        if let Some(p) = d.surfaces[sk].cached.take() {
                            apply_state(d, sk, p);
                        }
                    }
                }
            }
        }
        Iface::WlOutput => {
            if let wl_output::Request::Release = parse!(d, ck, h, wl_output) {
                d.clients[ck].outputs.retain(|(_, o)| *o != id);
                destroy_obj(d, ck, id);
            }
        }
        Iface::WlSeat => match parse!(d, ck, h, wl_seat) {
            wl_seat::Request::GetPointer { id: pid } => {
                if new_obj(d, ck, pid, Iface::WlPointer, ver, Data::None) {
                    d.clients[ck].pointers.push(pid);
                    crate::input::init_pointer(d, ck);
                }
            }
            wl_seat::Request::GetKeyboard { id: kid } => {
                if new_obj(d, ck, kid, Iface::WlKeyboard, ver, Data::None) {
                    d.clients[ck].keyboards.push(kid);
                    crate::input::init_keyboard(d, ck, kid);
                }
            }
            wl_seat::Request::GetTouch { id: tid } => {
                new_obj(d, ck, tid, Iface::WlTouch, ver, Data::None);
            }
            wl_seat::Request::Release => destroy_obj(d, ck, id),
        },
        Iface::WlPointer => match parse!(d, ck, h, wl_pointer) {
            wl_pointer::Request::SetCursor { serial: _, surface, hotspot_x, hotspot_y } => {
                crate::input::set_client_cursor(d, ck, surface_of(d, ck, surface), hotspot_x, hotspot_y);
            }
            wl_pointer::Request::Release => {
                d.clients[ck].pointers.retain(|p| *p != id);
                destroy_obj(d, ck, id);
            }
        },
        Iface::WlKeyboard => {
            if let wl_keyboard::Request::Release = parse!(d, ck, h, wl_keyboard) {
                d.clients[ck].keyboards.retain(|p| *p != id);
                destroy_obj(d, ck, id);
            }
        }
        Iface::WlTouch => {
            if let wl_touch::Request::Release = parse!(d, ck, h, wl_touch) {
                destroy_obj(d, ck, id);
            }
        }
        Iface::WlDataDeviceManager => match parse!(d, ck, h, wl_data_device_manager) {
            wl_data_device_manager::Request::CreateDataSource { id: sid } => {
                let dk = d.sources.insert(DataSource { client: ck, obj: sid, mimes: vec![], primary: false, own: None, x11: false, actions: 0 });
                if !new_obj(d, ck, sid, Iface::WlDataSource, ver, Data::DataSource(dk)) {
                    d.sources.remove(dk);
                }
            }
            wl_data_device_manager::Request::GetDataDevice { id: did, seat: _ } => {
                if new_obj(d, ck, did, Iface::WlDataDevice, ver, Data::None) {
                    d.clients[ck].data_devices.push(did);
                    let focused = d.seat.kb_focus.and_then(|s| d.surfaces.get(s)).map(|s| s.client);
                    if focused == Some(ck) {
                        send_selection(d, ck);
                    }
                }
            }
        },
        Iface::WlDataSource => {
            let r = parse!(d, ck, h, wl_data_source);
            let Some(Data::DataSource(dk)) = data_of(d, ck, id) else { return };
            let dk = *dk;
            match r {
                wl_data_source::Request::Offer { mime_type } => {
                    if let Some(s) = d.sources.get_mut(dk) {
                        s.mimes.push(mime_type);
                    }
                }
                wl_data_source::Request::Destroy => {
                    destroy_source(d, dk);
                    destroy_obj(d, ck, id);
                }
                wl_data_source::Request::SetActions { dnd_actions } => {
                    if let Some(s) = d.sources.get_mut(dk) {
                        s.actions = dnd_actions & 7;
                    }
                    crate::dnd::choose(d);
                }
            }
        }
        Iface::WlDataDevice => match parse!(d, ck, h, wl_data_device) {
            wl_data_device::Request::StartDrag { source, origin, icon, serial: _ } => {
                let src = match data_of(d, ck, source) {
                    Some(Data::DataSource(k)) => Some(*k),
                    _ => None,
                };
                let origin = surface_of(d, ck, origin);
                let icon = if icon == 0 { None } else { surface_of(d, ck, icon) };
                crate::dnd::start(d, ck, src, origin, icon);
            }
            wl_data_device::Request::SetSelection { source, serial: _ } => {
                let new = match data_of(d, ck, source) {
                    Some(Data::DataSource(k)) => Some(*k),
                    _ => None,
                };
                set_selection(d, new);
            }
            wl_data_device::Request::Release => {
                d.clients[ck].data_devices.retain(|x| *x != id);
                destroy_obj(d, ck, id);
            }
        },
        Iface::WlDataOffer => {
            let r = parse!(d, ck, h, wl_data_offer);
            match r {
                wl_data_offer::Request::Receive { mime_type, fd } => {
                    let Some(Data::DataOffer(dk)) = data_of(d, ck, id) else { return };
                    let dk = *dk;
                    source_send(d, dk, &mime_type, fd);
                }
                wl_data_offer::Request::Destroy => {
                    crate::dnd::offer_gone(d, ck, id);
                    destroy_obj(d, ck, id);
                }
                wl_data_offer::Request::Accept { serial: _, mime_type } => crate::dnd::accept(d, ck, id, mime_type),
                wl_data_offer::Request::SetActions { dnd_actions, preferred_action } => crate::dnd::set_actions(d, ck, id, dnd_actions & 7, preferred_action & 7),
                wl_data_offer::Request::Finish => crate::dnd::finish(d, ck, id),
            }
        }
        // Primary selection: what is selected now, pasted with the middle button (separate from
        // the clipboard, not kept in its history).
        Iface::ZwpPrimarySelectionDeviceManagerV1 => match parse!(d, ck, h, zwp_primary_selection_device_manager_v1) {
            zwp_primary_selection_device_manager_v1::Request::CreateSource { id: sid } => {
                let dk = d.sources.insert(DataSource { client: ck, obj: sid, mimes: vec![], primary: true, own: None, x11: false, actions: 0 });
                if !new_obj(d, ck, sid, Iface::ZwpPrimarySelectionSourceV1, ver, Data::DataSource(dk)) {
                    d.sources.remove(dk);
                }
            }
            zwp_primary_selection_device_manager_v1::Request::GetDevice { id: did, seat: _ } => {
                if new_obj(d, ck, did, Iface::ZwpPrimarySelectionDeviceV1, ver, Data::None) {
                    d.clients[ck].primary_devices.push(did);
                    let focused = d.seat.kb_focus.and_then(|s| d.surfaces.get(s)).map(|s| s.client);
                    if focused == Some(ck) {
                        send_primary(d, ck);
                    }
                }
            }
            zwp_primary_selection_device_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
        },
        Iface::ZwpPrimarySelectionSourceV1 => {
            let r = parse!(d, ck, h, zwp_primary_selection_source_v1);
            let Some(Data::DataSource(dk)) = data_of(d, ck, id) else { return };
            let dk = *dk;
            match r {
                zwp_primary_selection_source_v1::Request::Offer { mime_type } => {
                    if let Some(s) = d.sources.get_mut(dk) {
                        s.mimes.push(mime_type);
                    }
                }
                zwp_primary_selection_source_v1::Request::Destroy => {
                    destroy_source(d, dk);
                    destroy_obj(d, ck, id);
                }
            }
        }
        Iface::ZwpPrimarySelectionDeviceV1 => match parse!(d, ck, h, zwp_primary_selection_device_v1) {
            zwp_primary_selection_device_v1::Request::SetSelection { source, serial: _ } => {
                let new = match data_of(d, ck, source) {
                    Some(Data::DataSource(k)) => Some(*k),
                    _ => None,
                };
                set_primary(d, new);
            }
            zwp_primary_selection_device_v1::Request::Destroy => {
                d.clients[ck].primary_devices.retain(|x| *x != id);
                destroy_obj(d, ck, id);
            }
        },
        Iface::ZwpPrimarySelectionOfferV1 => match parse!(d, ck, h, zwp_primary_selection_offer_v1) {
            zwp_primary_selection_offer_v1::Request::Receive { mime_type, fd } => {
                let Some(Data::DataOffer(dk)) = data_of(d, ck, id) else { return };
                let dk = *dk;
                source_send(d, dk, &mime_type, fd);
            }
            zwp_primary_selection_offer_v1::Request::Destroy => destroy_obj(d, ck, id),
        },
        Iface::XdgWmBase => match parse!(d, ck, h, xdg_wm_base) {
            xdg_wm_base::Request::Destroy => destroy_obj(d, ck, id),
            xdg_wm_base::Request::CreatePositioner { id: pid } => {
                new_obj(d, ck, pid, Iface::XdgPositioner, ver, Data::Positioner(Box::default()));
            }
            xdg_wm_base::Request::GetXdgSurface { id: xid, surface } => {
                let Some(sk) = surface_of(d, ck, surface) else { return };
                if d.surfaces[sk].buffer.is_some() || d.surfaces[sk].xdg_obj != 0 {
                    d.post_error(ck, id, xdg_wm_base::error::INVALID_SURFACE_STATE, "surface has buffer or role");
                    return;
                }
                if new_obj(d, ck, xid, Iface::XdgSurface, ver, Data::XdgSurface(sk)) {
                    d.surfaces[sk].xdg_obj = xid;
                }
            }
            xdg_wm_base::Request::Pong { .. } => {}
        },
        Iface::XdgPositioner => {
            let r = parse!(d, ck, h, xdg_positioner);
            if let xdg_positioner::Request::Destroy = r {
                destroy_obj(d, ck, id);
                return;
            }
            let Some(Obj { data: Data::Positioner(p), .. }) = d.clients[ck].obj_mut(id) else { return };
            match r {
                xdg_positioner::Request::SetSize { width, height } => p.size = (width, height),
                xdg_positioner::Request::SetAnchorRect { x, y, width, height } => {
                    p.anchor_rect = Rect::new(x, y, width, height)
                }
                xdg_positioner::Request::SetAnchor { anchor } => p.anchor = anchor,
                xdg_positioner::Request::SetGravity { gravity } => p.gravity = gravity,
                xdg_positioner::Request::SetConstraintAdjustment { constraint_adjustment } => {
                    p.constraint = constraint_adjustment
                }
                xdg_positioner::Request::SetOffset { x, y } => p.offset = (x, y),
                xdg_positioner::Request::SetReactive => p.reactive = true,
                _ => {}
            }
        }
        Iface::XdgSurface => {
            let r = parse!(d, ck, h, xdg_surface);
            xdg_surface_request(d, ck, id, ver, r)
        }
        Iface::XdgToplevel => {
            let r = parse!(d, ck, h, xdg_toplevel);
            crate::wm::toplevel_request(d, ck, id, r);
        }
        Iface::XdgPopup => {
            let r = parse!(d, ck, h, xdg_popup);
            let Some(sk) = surface_of(d, ck, id) else { return };
            match r {
                xdg_popup::Request::Destroy => {
                    unmap_popup(d, sk);
                    d.surfaces[sk].role = Role::None;
                    d.surfaces[sk].popup = None;
                    destroy_obj(d, ck, id);
                }
                xdg_popup::Request::Grab { .. } => {
                    if let Some(p) = d.surfaces[sk].popup.as_mut() {
                        p.grab = true;
                    }
                    d.seat.popup_grabs.retain(|k| *k != sk);
                    d.seat.popup_grabs.push(sk);
                    crate::input::set_kb_focus(d, Some(sk));
                }
                xdg_popup::Request::Reposition { positioner, token } => {
                    let Some(Data::Positioner(p)) = data_of(d, ck, positioner) else { return };
                    let p = (**p).clone();
                    d.damage_surface_tree(sk);
                    let xdg = d.surfaces[sk].xdg_obj;
                    if let Some(info) = d.surfaces[sk].popup.as_mut() {
                        info.positioner = p;
                    }
                    place_popup(d, sk);
                    let c = &mut d.clients[ck].conn;
                    xdg_popup::evt::repositioned(c, id, token);
                    send_popup_configure(d, sk, xdg);
                }
            }
        }
        Iface::ZxdgDecorationManagerV1 => match parse!(d, ck, h, zxdg_decoration_manager_v1) {
            zxdg_decoration_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id: did, toplevel } => {
                let Some(sk) = surface_of(d, ck, toplevel) else { return };
                if !new_obj(d, ck, did, Iface::ZxdgToplevelDecorationV1, 1, Data::Decoration(sk)) {
                    return;
                }
                if let Role::Toplevel(wk) = d.surfaces[sk].role {
                    d.windows[wk].deco_obj = did;
                    crate::wm::set_decoration(d, wk, true);
                }
            }
        },
        Iface::ZxdgToplevelDecorationV1 => {
            let r = parse!(d, ck, h, zxdg_toplevel_decoration_v1);
            let Some(sk) = surface_of(d, ck, id) else { return };
            let wk = match d.surfaces.get(sk).map(|s| s.role) {
                Some(Role::Toplevel(wk)) => Some(wk),
                _ => None,
            };
            match r {
                zxdg_toplevel_decoration_v1::Request::Destroy => {
                    if let Some(wk) = wk {
                        d.windows[wk].deco_obj = 0;
                        crate::wm::set_decoration(d, wk, false);
                    }
                    destroy_obj(d, ck, id);
                }
                zxdg_toplevel_decoration_v1::Request::SetMode { mode } => {
                    if let Some(wk) = wk {
                        crate::wm::set_decoration(d, wk, mode != zxdg_toplevel_decoration_v1::mode::CLIENT_SIDE);
                    }
                }
                zxdg_toplevel_decoration_v1::Request::UnsetMode => {
                    if let Some(wk) = wk {
                        crate::wm::set_decoration(d, wk, true);
                    }
                }
            }
        }
        Iface::WpViewporter => match parse!(d, ck, h, wp_viewporter) {
            wp_viewporter::Request::Destroy => destroy_obj(d, ck, id),
            wp_viewporter::Request::GetViewport { id: vid, surface } => {
                if let Some(sk) = surface_of(d, ck, surface) {
                    new_obj(d, ck, vid, Iface::WpViewport, 1, Data::Viewport(sk));
                }
            }
        },
        Iface::WpViewport => {
            let r = parse!(d, ck, h, wp_viewport);
            let Some(sk) = surface_of(d, ck, id) else { return };
            let Some(s) = d.surfaces.get_mut(sk) else { return };
            match r {
                wp_viewport::Request::Destroy => {
                    s.pending.vp_src = Some(None);
                    s.pending.vp_dst = Some(None);
                    destroy_obj(d, ck, id);
                }
                wp_viewport::Request::SetSource { x, y, width, height } => {
                    s.pending.vp_src = Some(if width.0 == -256 && height.0 == -256 {
                        None
                    } else {
                        Some((x.to_f64(), y.to_f64(), width.to_f64(), height.to_f64()))
                    });
                }
                wp_viewport::Request::SetDestination { width, height } => {
                    s.pending.vp_dst = Some(if width == -1 && height == -1 { None } else { Some((width, height)) });
                }
            }
        }
        Iface::WpFractionalScaleManagerV1 => match parse!(d, ck, h, wp_fractional_scale_manager_v1) {
            wp_fractional_scale_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            wp_fractional_scale_manager_v1::Request::GetFractionalScale { id: fid, surface } => {
                if let Some(sk) = surface_of(d, ck, surface) {
                    if new_obj(d, ck, fid, Iface::WpFractionalScaleV1, 1, Data::FracScale(sk)) {
                        d.surfaces[sk].frac_obj = fid;
                        d.surfaces[sk].sent_frac = 0;
                        crate::wm::update_surface_scale(d, sk);
                    }
                }
            }
        },
        Iface::WpFractionalScaleV1 => {
            if let wp_fractional_scale_v1::Request::Destroy = parse!(d, ck, h, wp_fractional_scale_v1) {
                if let Some(sk) = surface_of(d, ck, id) {
                    d.surfaces[sk].frac_obj = 0;
                }
                destroy_obj(d, ck, id);
            }
        }
        Iface::ZxdgOutputManagerV1 => match parse!(d, ck, h, zxdg_output_manager_v1) {
            zxdg_output_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            zxdg_output_manager_v1::Request::GetXdgOutput { id: xid, output } => {
                let Some(Data::Output(oid)) = data_of(d, ck, output) else { return };
                let oid = *oid;
                if new_obj(d, ck, xid, Iface::ZxdgOutputV1, ver, Data::XdgOutput(oid)) {
                    d.clients[ck].xdg_outputs.push((oid, xid));
                    send_xdg_output(d, ck, xid, oid);
                    if let Some(idx) = d.output_index(oid) {
                        if d.clients[ck].version(output) >= 2 {
                            let _ = idx;
                            wl_output::evt::done(&mut d.clients[ck].conn, output);
                        }
                    }
                }
            }
        },
        Iface::ZxdgOutputV1 => {
            if let zxdg_output_v1::Request::Destroy = parse!(d, ck, h, zxdg_output_v1) {
                d.clients[ck].xdg_outputs.retain(|(_, o)| *o != id);
                destroy_obj(d, ck, id);
            }
        }
        Iface::WpCursorShapeManagerV1 => match parse!(d, ck, h, wp_cursor_shape_manager_v1) {
            wp_cursor_shape_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            wp_cursor_shape_manager_v1::Request::GetPointer { cursor_shape_device, pointer: _ } => {
                new_obj(d, ck, cursor_shape_device, Iface::WpCursorShapeDeviceV1, 1, Data::None);
            }
            wp_cursor_shape_manager_v1::Request::GetTabletToolV2 { cursor_shape_device, .. } => {
                new_obj(d, ck, cursor_shape_device, Iface::WpCursorShapeDeviceV1, 1, Data::None);
            }
        },
        Iface::WpCursorShapeDeviceV1 => match parse!(d, ck, h, wp_cursor_shape_device_v1) {
            wp_cursor_shape_device_v1::Request::Destroy => destroy_obj(d, ck, id),
            wp_cursor_shape_device_v1::Request::SetShape { serial: _, shape } => {
                crate::input::set_shape_cursor(d, ck, shape);
            }
        },
        Iface::WpSinglePixelBufferManagerV1 => match parse!(d, ck, h, wp_single_pixel_buffer_manager_v1) {
            wp_single_pixel_buffer_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            wp_single_pixel_buffer_manager_v1::Request::CreateU32RgbaBuffer { id: bid, r, g, b, a } => {
                let c = |v: u32| v >> 24;
                // Values are premultiplied already per protocol.
                let px = (c(a) << 24) | (c(r) << 16) | (c(g) << 8) | c(b);
                let buf = Buffer {
                    pool: None,
                    offset: 0,
                    w: 1,
                    h: 1,
                    stride: 4,
                    format: FMT_ARGB8888,
                    broken: Cell::new(false),
                    solid: Some(px),
                    dmabuf: None,
                };
                new_obj(d, ck, bid, Iface::WlBuffer, 1, Data::Buffer(Rc::new(buf)));
            }
        },
        // wlr-screencopy (screencopy.rs): screen sharing and screenshots.
        Iface::ZwlrScreencopyManagerV1 => match parse!(d, ck, h, zwlr_screencopy_manager_v1) {
            zwlr_screencopy_manager_v1::Request::Destroy => destroy_obj(d, ck, id),
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, overlay_cursor: _, output } => {
                crate::screencopy::capture(d, ck, frame, ver, output, None);
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion { frame, overlay_cursor: _, output, x, y, width, height } => {
                crate::screencopy::capture(d, ck, frame, ver, output, Some((x, y, width, height)));
            }
        },
        Iface::ZwlrScreencopyFrameV1 => match parse!(d, ck, h, zwlr_screencopy_frame_v1) {
            zwlr_screencopy_frame_v1::Request::Destroy => {
                crate::screencopy::forget(d, ck, id);
                destroy_obj(d, ck, id);
            }
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => crate::screencopy::copy(d, ck, id, buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => crate::screencopy::copy(d, ck, id, buffer, true),
        },
        // linux-dmabuf (dmabuf.rs): GPU buffers.
        Iface::ZwpLinuxDmabufV1 => match parse!(d, ck, h, zwp_linux_dmabuf_v1) {
            zwp_linux_dmabuf_v1::Request::Destroy => destroy_obj(d, ck, id),
            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                new_obj(d, ck, params_id, Iface::ZwpLinuxBufferParamsV1, ver, Data::Params(Box::default()));
            }
            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id: fid } | zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id: fid, .. } => {
                if new_obj(d, ck, fid, Iface::ZwpLinuxDmabufFeedbackV1, ver, Data::None) {
                    send_dmabuf_feedback(d, ck, fid);
                }
            }
        },
        Iface::ZwpLinuxDmabufFeedbackV1 => {
            let zwp_linux_dmabuf_feedback_v1::Request::Destroy = parse!(d, ck, h, zwp_linux_dmabuf_feedback_v1);
            destroy_obj(d, ck, id);
        }
        Iface::ZwpLinuxBufferParamsV1 => {
            use zwp_linux_buffer_params_v1::error as perr;
            let r = parse!(d, ck, h, zwp_linux_buffer_params_v1);
            if let zwp_linux_buffer_params_v1::Request::Destroy = r {
                destroy_obj(d, ck, id);
                return;
            }
            let Some(Obj { data: Data::Params(p), .. }) = d.clients[ck].obj_mut(id) else { return };
            match r {
                zwp_linux_buffer_params_v1::Request::Add { fd, plane_idx, offset, stride, modifier_hi, modifier_lo } => {
                    let why = if p.used {
                        Some((perr::ALREADY_USED, "params already used"))
                    } else if plane_idx >= 4 {
                        Some((perr::PLANE_IDX, "plane index out of range"))
                    } else if p.planes[plane_idx as usize].is_some() {
                        Some((perr::PLANE_SET, "plane already set"))
                    } else {
                        p.planes[plane_idx as usize] = Some((fd, offset, stride, ((modifier_hi as u64) << 32) | modifier_lo as u64));
                        None
                    };
                    if let Some((code, msg)) = why {
                        d.post_error(ck, id, code, msg);
                    }
                }
                zwp_linux_buffer_params_v1::Request::Create { width, height, format, flags } => {
                    let mut p = std::mem::take(p);
                    let made = match &d.dmabuf {
                        Some(sup) => crate::dmabuf::create(sup, &mut p, width, height, format, flags),
                        None => Err((u32::MAX, String::new())),
                    };
                    if let Some(Obj { data: Data::Params(q), .. }) = d.clients[ck].obj_mut(id) {
                        q.used = true;
                    }
                    match made {
                        Ok(dm) => {
                            let c = &mut d.clients[ck];
                            let bid = c.insert_server(Iface::WlBuffer, 1, Data::Buffer(Rc::new(dmabuf_buffer(dm))));
                            zwp_linux_buffer_params_v1::evt::created(&mut c.conn, id, bid);
                        }
                        Err((u32::MAX, _)) => zwp_linux_buffer_params_v1::evt::failed(&mut d.clients[ck].conn, id),
                        Err((code, msg)) => d.post_error(ck, id, code, &msg),
                    }
                }
                zwp_linux_buffer_params_v1::Request::CreateImmed { buffer_id, width, height, format, flags } => {
                    let mut p = std::mem::take(p);
                    let made = match &d.dmabuf {
                        Some(sup) => crate::dmabuf::create(sup, &mut p, width, height, format, flags),
                        None => Err((u32::MAX, String::new())),
                    };
                    if let Some(Obj { data: Data::Params(q), .. }) = d.clients[ck].obj_mut(id) {
                        q.used = true;
                    }
                    match made {
                        Ok(dm) => {
                            new_obj(d, ck, buffer_id, Iface::WlBuffer, 1, Data::Buffer(Rc::new(dmabuf_buffer(dm))));
                        }
                        Err((u32::MAX, _)) => d.post_error(ck, id, perr::INVALID_WL_BUFFER, "format or modifier not supported"),
                        Err((code, msg)) => d.post_error(ck, id, code, &msg),
                    }
                }
                zwp_linux_buffer_params_v1::Request::Destroy => {}
            }
        }
        other => {
            let name = other.name();
            d.clients[ck].kill(format!("request on unsupported interface {name}"));
        }
    }
}

/// A wl_buffer for a client's dma-buf (the format code says whether alpha counts).
fn dmabuf_buffer(dm: crate::dmabuf::Dmabuf) -> Buffer {
    Buffer {
        pool: None,
        offset: 0,
        w: dm.w,
        h: dm.h,
        stride: dm.planes[0].stride as i32,
        format: if crate::dmabuf::has_alpha(dm.format) { FMT_ARGB8888 } else { FMT_XRGB8888 },
        broken: Cell::new(false),
        solid: None,
        dmabuf: Some(dm),
    }
}

/// zwp_linux_dmabuf_feedback_v1: one tranche, the renderer's GPU, everything we take.
fn send_dmabuf_feedback(d: &mut Desk, ck: CKey, id: ObjId) {
    let Some(sup) = &d.dmabuf else { return };
    let dev = sup.dev.to_ne_bytes();
    let indices: Vec<u8> = (0..sup.formats.len() as u16).flat_map(|i| i.to_ne_bytes()).collect();
    let c = &mut d.clients[ck];
    zwp_linux_dmabuf_feedback_v1::evt::format_table(&mut c.conn, id, sup.table.as_fd(), sup.table_size);
    zwp_linux_dmabuf_feedback_v1::evt::main_device(&mut c.conn, id, &dev);
    zwp_linux_dmabuf_feedback_v1::evt::tranche_target_device(&mut c.conn, id, &dev);
    zwp_linux_dmabuf_feedback_v1::evt::tranche_formats(&mut c.conn, id, &indices);
    zwp_linux_dmabuf_feedback_v1::evt::tranche_flags(&mut c.conn, id, 0);
    zwp_linux_dmabuf_feedback_v1::evt::tranche_done(&mut c.conn, id);
    zwp_linux_dmabuf_feedback_v1::evt::done(&mut c.conn, id);
}

// ------------------------------------------------------------------ registry

pub fn advertised_globals(d: &mut Desk) {
    use Iface::*;
    for (i, v) in [
        (WlCompositor, 6),
        (WlSubcompositor, 1),
        (WlShm, 1),
        (WlSeat, 8),
        (WlDataDeviceManager, 3),
        (ZwpPrimarySelectionDeviceManagerV1, 1),
        (XdgWmBase, 6),
        (ZxdgDecorationManagerV1, 1),
        (WpViewporter, 1),
        (WpFractionalScaleManagerV1, 1),
        (ZxdgOutputManagerV1, 3),
        (WpCursorShapeManagerV1, 1),
        (WpSinglePixelBufferManagerV1, 1),
        (ZwlrScreencopyManagerV1, 3),
    ] {
        d.add_global(i, v, None);
    }
}

fn bind(d: &mut Desk, ck: CKey, name: u32, nid: wl::NewIdDyn) {
    let Some(g) = d.globals.get(name.wrapping_sub(1) as usize).copied().filter(|g| g.alive) else {
        // Binding a just-removed global is legal; give it an inert object.
        if let Some(i) = Iface::from_name(&nid.iface) {
            new_obj(d, ck, nid.id, i, nid.version.max(1), Data::None);
        }
        return;
    };
    if g.iface.name() != nid.iface || nid.version == 0 || nid.version > g.version {
        d.post_error(ck, 1, wl_display::error::INVALID_OBJECT, "bad bind");
        return;
    }
    let ver = nid.version;
    let id = nid.id;
    let data = match g.output {
        Some(o) => Data::Output(o),
        None => Data::None,
    };
    if !new_obj(d, ck, id, g.iface, ver, data) {
        return;
    }
    let c = &mut d.clients[ck];
    match g.iface {
        Iface::WlShm => {
            wl_shm::evt::format(&mut c.conn, id, FMT_ARGB8888);
            wl_shm::evt::format(&mut c.conn, id, FMT_XRGB8888);
        }
        Iface::ZwpLinuxDmabufV1 if ver < 4 => {
            // Before feedback: the formats (and from version 3 the modifiers) as events.
            if let Some(sup) = &d.dmabuf {
                let mut seen = Vec::new();
                for &(f, m) in &sup.formats {
                    if ver >= 3 {
                        zwp_linux_dmabuf_v1::evt::modifier(&mut c.conn, id, f, (m >> 32) as u32, m as u32);
                    } else if !seen.contains(&f) {
                        seen.push(f);
                        zwp_linux_dmabuf_v1::evt::format(&mut c.conn, id, f);
                    }
                }
            }
        }
        Iface::WlSeat => {
            wl_seat::evt::capabilities(&mut c.conn, id, wl_seat::capability::POINTER | wl_seat::capability::KEYBOARD);
            if ver >= 2 {
                wl_seat::evt::name(&mut c.conn, id, "seat0");
            }
        }
        Iface::WlOutput => {
            let oid = g.output.unwrap();
            c.outputs.push((oid, id));
            if let Some(idx) = d.output_index(oid) {
                send_output_info(d, ck, id, idx, true);
                // A client binding late must still learn about surfaces already on it.
                crate::wm::resend_enters(d, ck, oid, id);
            }
        }
        _ => {}
    }
}

pub fn send_output_info(d: &mut Desk, ck: CKey, obj: ObjId, idx: usize, initial: bool) {
    let o = &d.outputs[idx];
    let (x, y, pw, ph, scale, refresh, name) = (o.x, o.y, o.pw, o.ph, o.int_scale(), o.refresh_mhz, o.name.clone());
    let (lw, lh) = (o.lw(), o.lh());
    let (mw, mh) = (o.plane[2].round() as i32, o.plane[3].round() as i32);
    let oid = o.id;
    let c = &mut d.clients[ck];
    let ver = c.version(obj);
    // Subpixel "none": text in shades of grey, no RGB subpixel smoothing. "unknown" leaves it to
    // each app, and some pick RGB: colored fringes on a WOLED (RWBG) panel, and fringes smeared
    // wherever a window is scaled or sits on another kind of panel.
    wl_output::evt::geometry(&mut c.conn, obj, x, y, mw, mh, wl_output::subpixel::NONE as i32, "resc", &name, 0);
    wl_output::evt::mode(&mut c.conn, obj, wl_output::mode::CURRENT | wl_output::mode::PREFERRED, pw, ph, refresh);
    if ver >= 2 {
        wl_output::evt::scale(&mut c.conn, obj, scale);
    }
    if ver >= 4 && initial {
        wl_output::evt::name(&mut c.conn, obj, &name);
        wl_output::evt::description(&mut c.conn, obj, &format!("resc-desk {name} {lw}x{lh}"));
    }
    let xdg: Vec<ObjId> = c.xdg_outputs.iter().filter(|(i, _)| *i == oid).map(|(_, o)| *o).collect();
    for xo in xdg {
        send_xdg_output(d, ck, xo, oid);
    }
    let c = &mut d.clients[ck];
    if ver >= 2 {
        wl_output::evt::done(&mut c.conn, obj);
    }
}

fn send_xdg_output(d: &mut Desk, ck: CKey, xid: ObjId, oid: u32) {
    let Some(idx) = d.output_index(oid) else { return };
    let o = &d.outputs[idx];
    let (x, y, lw, lh, name) = (o.x, o.y, o.lw(), o.lh(), o.name.clone());
    let c = &mut d.clients[ck];
    let ver = c.version(xid);
    zxdg_output_v1::evt::logical_position(&mut c.conn, xid, x, y);
    zxdg_output_v1::evt::logical_size(&mut c.conn, xid, lw, lh);
    // Since v2, and still in v3 (deprecated for wl_output.name, but "compositors must still
    // support this event"): Qt waits for the name before it counts the screen as ready.
    if ver >= 2 {
        zxdg_output_v1::evt::name(&mut c.conn, xid, &name);
        zxdg_output_v1::evt::description(&mut c.conn, xid, &format!("resc {name}"));
    }
    if ver < 3 {
        zxdg_output_v1::evt::done(&mut c.conn, xid);
    }
}

// ------------------------------------------------------------------ wl_surface

fn surface_request(d: &mut Desk, ck: CKey, id: ObjId, ver: u32, r: wl_surface::Request) {
    let Some(sk) = surface_of(d, ck, id) else { return };
    use wl_surface::Request as R;
    match r {
        R::Destroy => {
            destroy_surface(d, sk);
            destroy_obj(d, ck, id);
        }
        R::Attach { buffer, x, y } => {
            let att = if buffer == 0 {
                Attach::Detach
            } else {
                match data_of(d, ck, buffer) {
                    Some(Data::Buffer(b)) => Attach::Buffer(b.clone(), buffer),
                    _ => {
                        d.post_error(ck, id, wl_display::error::INVALID_OBJECT, "attach: not a buffer");
                        return;
                    }
                }
            };
            let s = &mut d.surfaces[sk];
            s.pending.attach = att;
            if ver < 5 {
                s.pending.dx = x;
                s.pending.dy = y;
            }
        }
        R::Damage { x, y, width, height } => d.surfaces[sk].pending.damage_surface.push(clamp_rect(x, y, width, height)),
        R::DamageBuffer { x, y, width, height } => {
            d.surfaces[sk].pending.damage_buffer.push(clamp_rect(x, y, width, height))
        }
        R::Frame { callback } => {
            if new_obj(d, ck, callback, Iface::WlCallback, 1, Data::None) {
                d.surfaces[sk].pending.frame_cbs.push(callback);
            }
        }
        R::SetOpaqueRegion { region } => {
            let r = if region == 0 { None } else { region_of(d, ck, region) };
            d.surfaces[sk].pending.opaque = Some(r);
        }
        R::SetInputRegion { region } => {
            let r = if region == 0 { None } else { region_of(d, ck, region) };
            d.surfaces[sk].pending.input = Some(r);
        }
        R::Commit => commit(d, sk),
        R::SetBufferTransform { .. } => {}
        R::SetBufferScale { scale } => {
            if scale < 1 {
                d.post_error(ck, id, wl_surface::error::INVALID_SCALE, "scale < 1");
                return;
            }
            d.surfaces[sk].pending.scale = Some(scale);
        }
        R::Offset { x, y } => {
            let s = &mut d.surfaces[sk];
            s.pending.dx = x;
            s.pending.dy = y;
        }
    }
}

fn clamp_rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    let x0 = x.clamp(-(1 << 20), 1 << 20);
    let y0 = y.clamp(-(1 << 20), 1 << 20);
    let x1 = (x as i64 + w.max(0) as i64).clamp(-(1 << 20), 1 << 20) as i32;
    let y1 = (y as i64 + h.max(0) as i64).clamp(-(1 << 20), 1 << 20) as i32;
    Rect::from_edges(x0, y0, x1, y1)
}

fn effectively_sync(d: &Desk, mut sk: SKey) -> bool {
    for _ in 0..32 {
        let Some(s) = d.surfaces.get(sk) else { return false };
        if s.role != Role::Subsurface {
            return false;
        }
        if s.sub_sync {
            return true;
        }
        match s.parent {
            Some(p) => sk = p,
            None => return false,
        }
    }
    false
}

fn merge_pending(into: &mut Pending, p: Pending) {
    if !matches!(p.attach, Attach::Keep) {
        into.attach = p.attach;
    }
    into.dx += p.dx;
    into.dy += p.dy;
    into.damage_surface.extend(p.damage_surface);
    into.damage_buffer.extend(p.damage_buffer);
    into.frame_cbs.extend(p.frame_cbs);
    macro_rules! take {
        ($f:ident) => {
            if p.$f.is_some() {
                into.$f = p.$f;
            }
        };
    }
    take!(input);
    take!(opaque);
    take!(scale);
    take!(geometry);
    take!(vp_src);
    take!(vp_dst);
}

pub fn commit(d: &mut Desk, sk: SKey) {
    let p = std::mem::take(&mut d.surfaces[sk].pending);
    if effectively_sync(d, sk) {
        let s = &mut d.surfaces[sk];
        match s.cached.as_mut() {
            Some(c) => merge_pending(c, p),
            None => s.cached = Some(p),
        }
        return;
    }
    let role = d.surfaces[sk].role;
    let old_frame = match role {
        Role::Toplevel(wk) => crate::wm::frame_rect(d, wk),
        _ => None,
    };
    apply_state(d, sk, p);
    apply_children(d, sk);
    match role {
        Role::Toplevel(wk) => crate::wm::toplevel_commit(d, wk, old_frame),
        Role::Popup => popup_commit(d, sk),
        Role::Cursor | Role::DndIcon => {
            let r = d.cursor_drawn;
            d.damage(r);
            crate::input::damage_cursor(d);
        }
        _ => {}
    }
}

/// On parent commit: apply cached state of synchronized children and pending positions.
fn apply_children(d: &mut Desk, sk: SKey) {
    let subs: Vec<SKey> = d.surfaces[sk].subs.iter().copied().filter(|k| *k != SKey::default()).collect();
    for c in subs {
        let Some(cs) = d.surfaces.get_mut(c) else { continue };
        let moved = cs.sub_pos_pending.take();
        let cached = cs.cached.take();
        if moved.is_some() {
            d.damage_surface_tree(c);
            d.surfaces[c].sub_pos = moved.unwrap();
            d.damage_surface_tree(c);
        }
        if let Some(p) = cached {
            apply_state(d, c, p);
        }
        apply_children(d, c);
    }
}

fn apply_state(d: &mut Desk, sk: SKey, p: Pending) {
    let mut release = Vec::new();
    let s = &mut d.surfaces[sk];
    let (old_w, old_h) = (s.w, s.h);
    let mut new_content = false;
    match p.attach {
        Attach::Keep => {}
        Attach::Detach => {
            if let Some((_, obj)) = s.buffer.take() {
                release.push(obj);
            }
        }
        Attach::Buffer(b, obj) => {
            if let Some((_, old)) = s.buffer.replace((b, obj)) {
                if old != obj {
                    release.push(old);
                }
            }
            new_content = true;
        }
    }
    if s.role == Role::Cursor && (p.dx != 0 || p.dy != 0) {
        d.seat.cursor_hotspot.0 -= p.dx;
        d.seat.cursor_hotspot.1 -= p.dy;
    }
    if s.role == Role::DndIcon && (p.dx != 0 || p.dy != 0) {
        if let Some(n) = d.dnd.as_mut() {
            n.icon_off.0 += p.dx;
            n.icon_off.1 += p.dy;
        }
    }
    let s = &mut d.surfaces[sk];
    if let Some(sc) = p.scale {
        s.scale = sc;
    }
    if let Some(i) = p.input {
        s.input = i;
    }
    if let Some(o) = p.opaque {
        s.opaque = o;
    }
    if let Some(g) = p.geometry {
        s.geometry = Some(g);
    }
    if let Some(v) = p.vp_src {
        s.vp_src = v;
    }
    if let Some(v) = p.vp_dst {
        s.vp_dst = v;
    }
    match &s.buffer {
        Some((b, _)) => {
            let (bw, bh) = (b.w, b.h);
            let (w, h) = if let Some((w, h)) = s.vp_dst {
                (w, h)
            } else if let Some((_, _, w, h)) = s.vp_src {
                (w.round() as i32, h.round() as i32)
            } else {
                (bw / s.scale.max(1), bh / s.scale.max(1))
            };
            s.w = w.max(1);
            s.h = h.max(1);
        }
        None => {
            s.w = 0;
            s.h = 0;
        }
    }
    s.frame_cbs.extend(p.frame_cbs);
    // What changed in the buffer's own pixels, for the GPU's copy of it (gpu.rs uploads only
    // that). A viewport or a new size: all of it.
    if s.buffer.is_some() && (s.tex_full || !p.damage_buffer.is_empty() || !p.damage_surface.is_empty() || s.w != old_w || s.h != old_h) {
        s.gen += 1;
    }
    if s.buffer.is_some() && !s.tex_full {
        if s.w != old_w || s.h != old_h || s.vp_src.is_some() || s.vp_dst.is_some() {
            s.tex_full = true;
        } else {
            let sc = s.scale.max(1);
            s.tex_dmg.extend(p.damage_buffer.iter().copied());
            s.tex_dmg.extend(p.damage_surface.iter().map(|r| Rect::new(r.x * sc, r.y * sc, r.w * sc, r.h * sc)));
            if s.tex_dmg.len() > 32 {
                s.tex_full = true;
            }
        }
        if s.tex_full {
            s.tex_dmg.clear();
        }
    }
    let full = Rect::new(0, 0, s.w.max(old_w), s.h.max(old_h));
    let mut dmg: Vec<Rect> = Vec::new();
    if s.w != old_w || s.h != old_h || (new_content && s.vp_src.is_some()) {
        dmg.push(full);
    } else {
        let sc = s.scale.max(1);
        for r in p.damage_surface {
            dmg.push(r.intersect(&full));
        }
        for r in p.damage_buffer {
            let lr = Rect::from_edges(r.x / sc, r.y / sc, (r.x1() + sc - 1) / sc, (r.y1() + sc - 1) / sc);
            dmg.push(lr.intersect(&full));
        }
    }
    let ck = s.client;
    if let Some(c) = d.clients.get_mut(ck) {
        for obj in release {
            if c.obj(obj).map_or(false, |o| o.iface == Iface::WlBuffer) {
                wl_buffer::evt::release(&mut c.conn, obj);
            }
        }
    }
    if let Some((ox, oy)) = d.surface_origin(sk) {
        for r in dmg {
            d.damage(r.translate(ox, oy));
        }
    }
    if new_content {
        crate::shell::peek_commit(d, sk);
    }
    // A frame callback with no damage still needs a frame tick.
    if !d.surfaces[sk].frame_cbs.is_empty() {
        for o in d.outputs.iter_mut() {
            o.wants_frame = true;
        }
    }
}

pub fn release_buffer(d: &mut Desk, sk: SKey) {
    let Some(s) = d.surfaces.get_mut(sk) else { return };
    if let Some((_, obj)) = s.buffer.take() {
        let ck = s.client;
        if let Some(c) = d.clients.get_mut(ck) {
            if c.obj(obj).map_or(false, |o| o.iface == Iface::WlBuffer) {
                wl_buffer::evt::release(&mut c.conn, obj);
            }
        }
    }
}

fn detach_subsurface(d: &mut Desk, sk: SKey) {
    if let Some(pk) = d.surfaces.get(sk).and_then(|s| s.parent) {
        if let Some(p) = d.surfaces.get_mut(pk) {
            p.subs.retain(|k| *k != sk);
        }
    }
    if let Some(s) = d.surfaces.get_mut(sk) {
        s.parent = None;
    }
}

pub fn destroy_surface(d: &mut Desk, sk: SKey) {
    if !d.surfaces.contains(sk) {
        return;
    }
    d.damage_surface_tree(sk);
    let role = d.surfaces[sk].role;
    match role {
        Role::Toplevel(wk) => crate::wm::destroy_window(d, wk),
        Role::Popup => unmap_popup(d, sk),
        Role::Cursor => {
            if d.seat.cursor_surface == Some(sk) {
                d.seat.cursor_surface = None;
            }
        }
        _ => {}
    }
    detach_subsurface(d, sk);
    let subs: Vec<SKey> = d.surfaces[sk].subs.iter().copied().filter(|k| *k != SKey::default()).collect();
    for c in subs {
        if let Some(cs) = d.surfaces.get_mut(c) {
            cs.parent = None;
        }
    }
    let popups = d.surfaces[sk].popups.clone();
    for p in popups {
        if let Some(ps) = d.surfaces.get_mut(p) {
            if let Some(info) = ps.popup.as_mut() {
                info.parent = SKey::default();
            }
        }
    }
    crate::input::surface_gone(d, sk);
    d.surfaces.remove(sk);
}

// ------------------------------------------------------------------ xdg_surface / popups

fn xdg_surface_request(d: &mut Desk, ck: CKey, id: ObjId, ver: u32, r: xdg_surface::Request) {
    let Some(sk) = surface_of(d, ck, id) else { return };
    use xdg_surface::Request as R;
    match r {
        R::Destroy => {
            if let Some(s) = d.surfaces.get_mut(sk) {
                s.xdg_obj = 0;
            }
            destroy_obj(d, ck, id);
        }
        R::GetToplevel { id: tid } => {
            if d.surfaces[sk].role != Role::None {
                d.post_error(ck, id, xdg_surface::error::ALREADY_CONSTRUCTED, "role already set");
                return;
            }
            if !new_obj(d, ck, tid, Iface::XdgToplevel, ver, Data::Toplevel(sk)) {
                return;
            }
            crate::wm::create_window(d, ck, sk, tid);
        }
        R::GetPopup { id: pid, parent, positioner } => {
            let Some(Data::Positioner(pos)) = data_of(d, ck, positioner) else {
                d.post_error(ck, id, xdg_wm_base::error::INVALID_POSITIONER, "bad positioner");
                return;
            };
            let pos = (**pos).clone();
            let parent_sk = if parent == 0 { None } else { surface_of(d, ck, parent) };
            if !new_obj(d, ck, pid, Iface::XdgPopup, ver, Data::Popup(sk)) {
                return;
            }
            let s = &mut d.surfaces[sk];
            s.role = Role::Popup;
            s.popup = Some(PopupInfo {
                parent: parent_sk.unwrap_or_default(),
                x: 0,
                y: 0,
                w: pos.size.0,
                h: pos.size.1,
                obj: pid,
                grab: false,
                positioner: pos,
            });
            if let Some(pk) = parent_sk {
                d.surfaces[pk].popups.push(sk);
            }
            place_popup(d, sk);
        }
        R::SetWindowGeometry { x, y, width, height } => {
            if width <= 0 || height <= 0 {
                d.post_error(ck, id, xdg_surface::error::INVALID_SIZE, "geometry size <= 0");
                return;
            }
            d.surfaces[sk].pending.geometry = Some(Rect::new(x, y, width, height));
        }
        R::AckConfigure { serial } => {
            let s = &mut d.surfaces[sk];
            s.acked = serial;
            s.xdg_configured = true;
        }
    }
}

/// Computes popup position from its positioner, constrained to the output.
pub fn place_popup(d: &mut Desk, sk: SKey) {
    let Some(info) = d.surfaces[sk].popup.as_ref() else { return };
    let p = info.positioner.clone();
    let parent = info.parent;
    let (w, h) = (p.size.0.max(1), p.size.1.max(1));
    let compute = |anchor: u32, gravity: u32| -> (i32, i32) {
        let ar = p.anchor_rect;
        let ax = match anchor {
            3 | 5 | 6 => ar.x,
            4 | 7 | 8 => ar.x1(),
            _ => ar.x + ar.w / 2,
        };
        let ay = match anchor {
            1 | 5 | 7 => ar.y,
            2 | 6 | 8 => ar.y1(),
            _ => ar.y + ar.h / 2,
        };
        let x = match gravity {
            3 | 5 | 6 => ax - w,
            4 | 7 | 8 => ax,
            _ => ax - w / 2,
        };
        let y = match gravity {
            1 | 5 | 7 => ay - h,
            2 | 6 | 8 => ay,
            _ => ay - h / 2,
        };
        (x + p.offset.0, y + p.offset.1)
    };
    let flip_x = |v: u32| match v {
        3 => 4,
        4 => 3,
        5 => 7,
        7 => 5,
        6 => 8,
        8 => 6,
        o => o,
    };
    let flip_y = |v: u32| match v {
        1 => 2,
        2 => 1,
        5 => 6,
        6 => 5,
        7 => 8,
        8 => 7,
        o => o,
    };
    let (mut x, mut y) = compute(p.anchor, p.gravity);
    // Constrain in global space.
    let origin = d.surface_origin(parent).map(|(ox, oy)| {
        let g = d.surfaces.get(parent).map(|s| s.geom()).unwrap_or_default();
        (ox + g.x, oy + g.y)
    });
    if let Some((gx, gy)) = origin {
        let oi = d.output_for_rect(&Rect::new(gx + x, gy + y, w, h));
        let bounds = d.outputs.get(oi).map(|o| o.rect()).unwrap_or_default();
        let r = Rect::new(gx + x, gy + y, w, h);
        if r.x < bounds.x || r.x1() > bounds.x1() {
            if p.constraint & 4 != 0 {
                let (fx, _) = compute(flip_x(p.anchor), flip_x(p.gravity));
                let fr = Rect::new(gx + fx, gy + y, w, h);
                if fr.x >= bounds.x && fr.x1() <= bounds.x1() {
                    x = fx;
                }
            }
            let r = Rect::new(gx + x, gy + y, w, h);
            if p.constraint & 1 != 0 {
                if r.x1() > bounds.x1() {
                    x -= r.x1() - bounds.x1();
                }
                if gx + x < bounds.x {
                    x = bounds.x - gx;
                }
            }
        }
        let r = Rect::new(gx + x, gy + y, w, h);
        if r.y < bounds.y || r.y1() > bounds.y1() {
            if p.constraint & 8 != 0 {
                let (_, fy) = compute(flip_y(p.anchor), flip_y(p.gravity));
                let fr = Rect::new(gx + x, gy + fy, w, h);
                if fr.y >= bounds.y && fr.y1() <= bounds.y1() {
                    y = fy;
                }
            }
            let r = Rect::new(gx + x, gy + y, w, h);
            if p.constraint & 2 != 0 {
                if r.y1() > bounds.y1() {
                    y -= r.y1() - bounds.y1();
                }
                if gy + y < bounds.y {
                    y = bounds.y - gy;
                }
            }
        }
    }
    let info = d.surfaces[sk].popup.as_mut().unwrap();
    info.x = x;
    info.y = y;
    info.w = w;
    info.h = h;
}

fn send_popup_configure(d: &mut Desk, sk: SKey, xdg: ObjId) {
    let s = &d.surfaces[sk];
    let Some(info) = s.popup.as_ref() else { return };
    let (obj, x, y, w, h) = (info.obj, info.x, info.y, info.w, info.h);
    let ck = s.client;
    let serial = d.next_serial();
    let c = &mut d.clients[ck];
    xdg_popup::evt::configure(&mut c.conn, obj, x, y, w, h);
    xdg_surface::evt::configure(&mut c.conn, xdg, serial);
}

fn popup_commit(d: &mut Desk, sk: SKey) {
    let s = &d.surfaces[sk];
    if s.xwin != 0 {
        // An X menu or tooltip: no xdg handshake, shown while it has a buffer.
        let (has_buf, mapped) = (s.buffer.is_some(), s.mapped);
        d.damage_surface_tree(sk);
        if has_buf != mapped {
            d.surfaces[sk].mapped = has_buf;
            if let Some(wk) = d.window_of(sk) {
                crate::wm::update_window_outputs(d, wk);
            }
        }
        d.damage_surface_tree(sk);
        return;
    }
    if !s.xdg_initial_sent {
        let xdg = s.xdg_obj;
        d.surfaces[sk].xdg_initial_sent = true;
        send_popup_configure(d, sk, xdg);
        return;
    }
    let has_buf = s.buffer.is_some();
    if has_buf && !s.mapped {
        d.surfaces[sk].mapped = true;
        if let Some(wk) = d.window_of(sk) {
            crate::wm::update_window_outputs(d, wk);
        }
    } else if !has_buf && s.mapped {
        unmap_popup(d, sk);
    }
    d.damage_surface_tree(sk);
}

pub fn unmap_popup(d: &mut Desk, sk: SKey) {
    d.damage_surface_tree(sk);
    let Some(s) = d.surfaces.get_mut(sk) else { return };
    s.mapped = false;
    s.xdg_initial_sent = false;
    let parent = s.popup.as_ref().map(|p| p.parent);
    if let Some(pk) = parent {
        if let Some(p) = d.surfaces.get_mut(pk) {
            p.popups.retain(|k| *k != sk);
        }
    }
    d.seat.popup_grabs.retain(|k| *k != sk);
    release_buffer(d, sk);
    crate::input::surface_gone(d, sk);
}

/// Sends popup_done to every grabbing popup (outside click).
pub fn dismiss_popups(d: &mut Desk) {
    let grabs = std::mem::take(&mut d.seat.popup_grabs);
    for sk in grabs.into_iter().rev() {
        let Some(s) = d.surfaces.get(sk) else { continue };
        let Some(info) = s.popup.as_ref() else { continue };
        let (ck, obj) = (s.client, info.obj);
        if let Some(c) = d.clients.get_mut(ck) {
            xdg_popup::evt::popup_done(&mut c.conn, obj);
        }
    }
    // Keyboard focus goes back to the window.
    if let Some(wk) = d.mru.first().copied() {
        let surf = d.windows.get(wk).map(|w| w.surf);
        crate::input::set_kb_focus(d, surf);
    }
}

// ------------------------------------------------------------------ data device

pub fn destroy_source(d: &mut Desk, dk: DKey) {
    crate::dnd::source_gone(d, dk);
    let focused = d.seat.kb_focus.and_then(|sk| d.surfaces.get(sk)).map(|s| s.client);
    if d.selection == Some(dk) {
        d.selection = None;
        crate::xwm::selection_changed(d, false);
        if let Some(ck) = focused {
            send_selection(d, ck);
        }
    }
    if d.primary == Some(dk) {
        d.primary = None;
        crate::xwm::selection_changed(d, true);
        if let Some(ck) = focused {
            send_primary(d, ck);
        }
    }
    d.sources.remove(dk);
}

/// Asks the source `dk` to write its data as `mime` into `fd`: a client, the compositor's
/// own clipboard history, or an X program (through xwm, on its next turn).
pub fn source_send(d: &mut Desk, dk: DKey, mime: &str, fd: std::os::fd::OwnedFd) {
    let Some(s) = d.sources.get(dk) else { return };
    if let Some(data) = s.own.clone() {
        crate::clipboard::serve(data, fd);
        return;
    }
    if s.x11 {
        d.x_receives.push((dk, mime.to_string(), fd));
        return;
    }
    let (ck, obj, primary) = (s.client, s.obj, s.primary);
    let Some(c) = d.clients.get_mut(ck) else { return };
    if primary {
        zwp_primary_selection_source_v1::evt::send(&mut c.conn, obj, mime, fd.as_fd());
    } else {
        wl_data_source::evt::send(&mut c.conn, obj, mime, fd.as_fd());
    }
    // The connection keeps its own copy of the fd until it is flushed.
}

/// Replaced as the selection: its client hears so; ours and X's simply go.
fn drop_source(d: &mut Desk, old: DKey) {
    let Some(s) = d.sources.get(old) else { return };
    if s.own.is_some() || s.x11 {
        d.sources.remove(old);
        return;
    }
    let (ck, obj, primary) = (s.client, s.obj, s.primary);
    if let Some(c) = d.clients.get_mut(ck) {
        if primary {
            zwp_primary_selection_source_v1::evt::cancelled(&mut c.conn, obj);
        } else {
            wl_data_source::evt::cancelled(&mut c.conn, obj);
        }
    }
}

pub fn set_primary(d: &mut Desk, new: Option<DKey>) {
    if let Some(old) = d.primary.filter(|o| Some(*o) != new) {
        drop_source(d, old);
    }
    d.primary = new;
    crate::xwm::selection_changed(d, true);
    if let Some(ck) = d.seat.kb_focus.and_then(|sk| d.surfaces.get(sk)).map(|s| s.client) {
        send_primary(d, ck);
    }
}

/// Offers the primary selection to all primary-selection devices of a client.
pub fn send_primary(d: &mut Desk, ck: CKey) {
    let src = d.primary.and_then(|k| d.sources.get(k).map(|s| (k, s.mimes.clone())));
    let Some(c) = d.clients.get_mut(ck) else { return };
    for dev in c.primary_devices.clone() {
        let ver = c.version(dev);
        match &src {
            Some((dk, mimes)) => {
                let offer = c.insert_server(Iface::ZwpPrimarySelectionOfferV1, ver, Data::DataOffer(*dk));
                zwp_primary_selection_device_v1::evt::data_offer(&mut c.conn, dev, offer);
                for m in mimes {
                    zwp_primary_selection_offer_v1::evt::offer(&mut c.conn, offer, m);
                }
                zwp_primary_selection_device_v1::evt::selection(&mut c.conn, dev, offer);
            }
            None => zwp_primary_selection_device_v1::evt::selection(&mut c.conn, dev, 0),
        }
    }
}

pub fn set_selection(d: &mut Desk, new: Option<DKey>) {
    if let Some(old) = d.selection.filter(|o| Some(*o) != new) {
        drop_source(d, old);
    }
    d.selection = new;
    crate::xwm::selection_changed(d, false);
    if let Some(k) = new {
        crate::clipboard::capture(d, k);
    }
    if let Some(sk) = d.seat.kb_focus {
        if let Some(ck) = d.surfaces.get(sk).map(|s| s.client) {
            send_selection(d, ck);
        }
    }
}

/// Offers the current selection to all data devices of a client.
pub fn send_selection(d: &mut Desk, ck: CKey) {
    let src = d.selection.and_then(|k| d.sources.get(k).map(|s| (k, s.mimes.clone())));
    let Some(c) = d.clients.get_mut(ck) else { return };
    let devices = c.data_devices.clone();
    for dev in devices {
        let ver = c.version(dev);
        match &src {
            Some((dk, mimes)) => {
                let offer = c.insert_server(Iface::WlDataOffer, ver, Data::DataOffer(*dk));
                wl_data_device::evt::data_offer(&mut c.conn, dev, offer);
                for m in mimes {
                    wl_data_offer::evt::offer(&mut c.conn, offer, m);
                }
                wl_data_device::evt::selection(&mut c.conn, dev, offer);
            }
            None => wl_data_device::evt::selection(&mut c.conn, dev, 0),
        }
    }
}
