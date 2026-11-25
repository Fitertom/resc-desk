//! Window management with Windows-like behaviour: cascade placement, click to
//! focus/raise, maximize, Aero snap (halves/quarters), minimize, show desktop,
//! move between monitors, virtual desktops.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::shell;
use crate::state::*;
use wl::protocol::*;
use wl::ObjId;

pub fn create_window(d: &mut Desk, ck: CKey, sk: SKey, toplevel: ObjId) {
    let out = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
    let wk = d.windows.insert(Window {
        surf: sk,
        client: ck,
        toplevel,
        title: String::new(),
        app_id: String::new(),
        x: 0,
        y: 0,
        tile: Tile::Floating,
        minimized: false,
        morph: None,
        snap: None,
        activated: false,
        restore: Rect::default(),
        ssd: false,
        deco_obj: 0,
        min_size: (0, 0),
        max_size: (0, 0),
        workspace: d.workspace,
        placed: false,
        pending_pos: None,
        anchor: None,
        last_cfg: (-1, -1, vec![]),
        cfg_serial: 0,
        parent: None,
        output: out,
        fullscreen_output: None,
        x11: 0,
    });
    d.surfaces[sk].role = Role::Toplevel(wk);
}

/// Work area of an output (minus the taskbar).
pub fn workarea(d: &Desk, oi: usize) -> Rect {
    let Some(o) = d.outputs.get(oi) else { return Rect::new(0, 0, 800, 600) };
    let r = o.rect();
    Rect::new(r.x, r.y, r.w, r.h - shell::TASKBAR_H)
}

/// Outer frame (decorations included) in global coords.
pub fn frame_rect(d: &Desk, wk: WKey) -> Option<Rect> {
    let w = d.windows.get(wk)?;
    let s = d.surfaces.get(w.surf)?;
    let g = s.geom();
    let (b, t) = w.frame_insets();
    Some(Rect::new(w.x - b, w.y - t - b, g.w + 2 * b, g.h + t + 2 * b))
}

/// Everything the window paints (frame + full surface incl. CSD shadows + popups).
pub fn visual_rect(d: &Desk, wk: WKey) -> Option<Rect> {
    let w = d.windows.get(wk)?;
    let mut r = frame_rect(d, wk)?;
    if let Some((ox, oy)) = d.surface_origin(w.surf) {
        let s = &d.surfaces[w.surf];
        r = r.union(&Rect::new(ox, oy, s.w, s.h));
    }
    Some(r.inflate(shell::SHADOW))
}

pub fn damage_window(d: &mut Desk, wk: WKey) {
    if let Some(r) = visual_rect(d, wk) {
        d.damage(r);
    }
    if let Some(w) = d.windows.get(wk) {
        let sk = w.surf;
        d.damage_surface_tree(sk);
    }
    shell::damage_taskbars(d);
}

/// Content rect (window geometry) that produces `frame` for this window.
fn content_for_frame(d: &Desk, wk: WKey, frame: Rect) -> Rect {
    let (b, t) = d.windows[wk].frame_insets();
    Rect::new(frame.x + b, frame.y + t + b, frame.w - 2 * b, frame.h - t - 2 * b)
}

// ------------------------------------------------------------------ configure

pub fn send_configure(d: &mut Desk, wk: WKey, w: i32, h: i32) {
    let Some(win) = d.windows.get(wk) else { return };
    if win.x11 != 0 {
        // X windows are told by xwm::sync (size and place together, each loop).
        let serial = d.next_serial();
        let win = &mut d.windows[wk];
        win.last_cfg = (w, h, vec![]);
        win.cfg_serial = serial;
        return;
    }
    let mut states = Vec::new();
    match win.tile {
        Tile::Maximized => states.push(xdg_toplevel::state::MAXIMIZED),
        Tile::Fullscreen => states.push(xdg_toplevel::state::FULLSCREEN),
        _ => {}
    }
    if win.activated {
        states.push(xdg_toplevel::state::ACTIVATED);
    }
    if d.seat.resizing == Some(wk) {
        states.push(xdg_toplevel::state::RESIZING);
    }
    let (ck, top, sk) = (win.client, win.toplevel, win.surf);
    let ver = d.clients.get(ck).map_or(1, |c| c.version(top));
    if ver >= 2 {
        let tiled: &[u32] = match win.tile {
            Tile::Maximized => &[5, 6, 7, 8],
            Tile::Snapped(Snap::Left) => &[5, 7, 8],
            Tile::Snapped(Snap::Right) => &[6, 7, 8],
            Tile::Snapped(Snap::TopLeft) => &[5, 7],
            Tile::Snapped(Snap::TopRight) => &[6, 7],
            Tile::Snapped(Snap::BottomLeft) => &[5, 8],
            Tile::Snapped(Snap::BottomRight) => &[6, 8],
            _ => &[],
        };
        states.extend_from_slice(tiled);
    }
    let bytes: Vec<u8> = states.iter().flat_map(|s| s.to_ne_bytes()).collect();
    let xdg = d.surfaces.get(sk).map_or(0, |s| s.xdg_obj);
    let wa = workarea(d, win.output);
    let serial = d.next_serial();
    let Some(c) = d.clients.get_mut(ck) else { return };
    if ver >= 4 {
        xdg_toplevel::evt::configure_bounds(&mut c.conn, top, wa.w, wa.h - shell::TITLE_H);
    }
    xdg_toplevel::evt::configure(&mut c.conn, top, w.max(0), h.max(0), &bytes);
    if xdg != 0 {
        xdg_surface::evt::configure(&mut c.conn, xdg, serial);
    }
    let win = &mut d.windows[wk];
    win.last_cfg = (w, h, states);
    win.cfg_serial = serial;
}

/// Re-sends the current size with updated states (activation etc.).
fn reconfigure(d: &mut Desk, wk: WKey) {
    let Some(w) = d.windows.get(wk) else { return };
    let s = &d.surfaces[w.surf];
    if !s.xdg_initial_sent {
        return;
    }
    let (cw, ch) = if w.tile == Tile::Floating && w.last_cfg.0 <= 0 {
        (0, 0)
    } else {
        let g = s.geom();
        (g.w, g.h)
    };
    send_configure(d, wk, cw, ch);
}

/// Ask the client to take `content` geometry (global coords).
fn configure_to(d: &mut Desk, wk: WKey, content: Rect) {
    send_configure(d, wk, content.w.max(1), content.h.max(1));
    let serial = d.windows[wk].cfg_serial;
    d.windows[wk].pending_pos = Some((serial, content.x, content.y));
    d.windows[wk].anchor = None;
}

pub fn set_decoration(d: &mut Desk, wk: WKey, ssd: bool) {
    let deco = d.windows[wk].deco_obj;
    d.windows[wk].ssd = ssd && deco != 0;
    let ck = d.windows[wk].client;
    if deco != 0 {
        let mode = if ssd { zxdg_toplevel_decoration_v1::mode::SERVER_SIDE } else { zxdg_toplevel_decoration_v1::mode::CLIENT_SIDE };
        if let Some(c) = d.clients.get_mut(ck) {
            zxdg_toplevel_decoration_v1::evt::configure(&mut c.conn, deco, mode);
        }
    }
    reconfigure(d, wk);
    damage_window(d, wk);
}

// ------------------------------------------------------------------ requests

pub fn toplevel_request(d: &mut Desk, ck: CKey, id: ObjId, r: xdg_toplevel::Request) {
    let Some(Obj { data: Data::Toplevel(sk), .. }) = d.clients[ck].obj(id) else { return };
    let sk = *sk;
    let Some(Role::Toplevel(wk)) = d.surfaces.get(sk).map(|s| s.role) else {
        if let xdg_toplevel::Request::Destroy = r {
            d.clients[ck].remove(id);
            wl_display::evt::delete_id(&mut d.clients[ck].conn, 1, id);
        }
        return;
    };
    use xdg_toplevel::Request as R;
    match r {
        R::Destroy => {
            destroy_window(d, wk);
            if let Some(s) = d.surfaces.get_mut(sk) {
                s.role = Role::None;
                s.xdg_initial_sent = false;
                s.mapped = false;
            }
            d.clients[ck].remove(id);
            wl_display::evt::delete_id(&mut d.clients[ck].conn, 1, id);
        }
        R::SetParent { parent } => {
            let p = match d.clients[ck].obj(parent) {
                Some(Obj { data: Data::Toplevel(ps), .. }) => match d.surfaces.get(*ps).map(|s| s.role) {
                    Some(Role::Toplevel(pw)) => Some(pw),
                    _ => None,
                },
                _ => None,
            };
            d.windows[wk].parent = p;
        }
        R::SetTitle { title } => {
            d.windows[wk].title = title;
            if let Some(f) = frame_rect(d, wk) {
                d.damage(Rect::new(f.x, f.y, f.w, shell::TITLE_H + shell::BORDER));
            }
            shell::damage_taskbars(d);
        }
        R::SetAppId { app_id } => {
            d.windows[wk].app_id = app_id;
            shell::damage_taskbars(d);
        }
        R::ShowWindowMenu { .. } => {}
        R::Move { .. } => crate::input::begin_move(d, wk),
        R::Resize { edges, .. } => crate::input::begin_resize(d, wk, edges),
        R::SetMaxSize { width, height } => d.windows[wk].max_size = (width, height),
        R::SetMinSize { width, height } => d.windows[wk].min_size = (width, height),
        R::SetMaximized => maximize(d, wk),
        R::UnsetMaximized => {
            if d.windows[wk].tile == Tile::Maximized {
                restore(d, wk)
            }
        }
        R::SetFullscreen { output } => {
            let oi = match d.clients[ck].obj(output) {
                Some(Obj { data: Data::Output(oid), .. }) => d.output_index(*oid),
                _ => None,
            };
            fullscreen(d, wk, true, oi);
        }
        R::UnsetFullscreen => fullscreen(d, wk, false, None),
        R::SetMinimized => minimize(d, wk),
    }
}

// ------------------------------------------------------------------ commit

pub fn toplevel_commit(d: &mut Desk, wk: WKey, old_frame: Option<Rect>) {
    let Some(w) = d.windows.get(wk) else { return };
    let sk = w.surf;
    let s = &d.surfaces[sk];
    if !s.xdg_initial_sent {
        d.surfaces[sk].xdg_initial_sent = true;
        let ck = w.client;
        let top = w.toplevel;
        if d.clients.get(ck).map_or(0, |c| c.version(top)) >= 5 {
            let caps: Vec<u8> = [2u32, 3, 4].iter().flat_map(|v| v.to_ne_bytes()).collect();
            xdg_toplevel::evt::wm_capabilities(&mut d.clients[ck].conn, top, &caps);
        }
        if d.windows[wk].deco_obj != 0 {
            let deco = d.windows[wk].deco_obj;
            let mode = if d.windows[wk].ssd { 2 } else { 1 };
            zxdg_toplevel_decoration_v1::evt::configure(&mut d.clients[ck].conn, deco, mode);
        }
        if d.windows[wk].desktop() {
            d.windows[wk].ssd = false;
            d.windows[wk].tile = Tile::Maximized;
        }
        match d.windows[wk].tile {
            Tile::Maximized => {
                d.windows[wk].tile = Tile::Floating;
                maximize(d, wk);
            }
            Tile::Fullscreen => {
                d.windows[wk].tile = Tile::Floating;
                let o = d.windows[wk].fullscreen_output;
                fullscreen(d, wk, true, o);
            }
            _ => send_configure(d, wk, 0, 0),
        }
        return;
    }
    let has_buf = s.buffer.is_some();
    if has_buf && !s.mapped {
        if !s.xdg_configured {
            return; // buffer before ack: ignore until configured
        }
        d.surfaces[sk].mapped = true;
        map_window(d, wk);
        return;
    }
    if !has_buf && s.mapped {
        unmap_window(d, wk);
        return;
    }
    if !s.mapped {
        return;
    }
    // Apply a pending move once the client acked the matching configure.
    let acked = s.acked;
    let g = s.geom();
    let win = &mut d.windows[wk];
    if let Some((serial, x, y)) = win.pending_pos {
        if serial_reached(acked, serial) {
            win.x = x;
            win.y = y;
            win.pending_pos = None;
        }
    }
    if let Some((edges, ax, ay)) = win.anchor {
        if edges & 4 != 0 {
            win.x = ax - g.w;
        }
        if edges & 1 != 0 {
            win.y = ay - g.h;
        }
    }
    let new_frame = frame_rect(d, wk);
    if new_frame != old_frame {
        if let Some(r) = old_frame {
            d.damage(r.inflate(shell::SHADOW));
        }
        damage_window(d, wk);
        update_window_outputs(d, wk);
    }
    crate::life::snapshot(d, wk);
}

fn serial_reached(acked: u32, want: u32) -> bool {
    acked.wrapping_sub(want) < 0x8000_0000
}

fn map_window(d: &mut Desk, wk: WKey) {
    if d.windows[wk].desktop() {
        d.windows[wk].placed = true;
        d.stack.retain(|k| *k != wk);
        d.stack.insert(0, wk);
        d.windows[wk].minimized = false;
        // Focus stays where it is unless nothing else is open.
        if focused(d).is_none() {
            focus_window(d, Some(wk));
        }
        damage_window(d, wk);
        update_window_outputs(d, wk);
        return;
    }
    if !d.windows[wk].placed {
        place_window(d, wk);
    }
    d.stack.retain(|k| *k != wk);
    d.stack.push(wk);
    d.windows[wk].minimized = false;
    crate::boot::mapped(d);
    focus_window(d, Some(wk));
    damage_window(d, wk);
    update_window_outputs(d, wk);
    // A window of its own (not a dialog) grows out of its new taskbar button, or out of the
    // icon it was started from (life.rs).
    if d.windows[wk].parent.is_none() && !crate::life::mapped(d, wk) {
        crate::winanim::open(d, wk);
    }
    if d.windows[wk].x11 != 0 {
        crate::xwm::window_mapped(d, wk);
    }
}

/// Windows-style placement: transient → centered on parent; otherwise centered
/// on the output under the pointer with a cascade offset.
fn place_window(d: &mut Desk, wk: WKey) {
    let sk = d.windows[wk].surf;
    let g = d.surfaces[sk].geom();
    let (b, t) = d.windows[wk].frame_insets();
    let fw = g.w + 2 * b;
    let fh = g.h + t + 2 * b;
    let oi = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
    let wa = workarea(d, oi);
    let now = d.now_ms();
    let hint = d.place_next.take().filter(|h| h.0 == d.windows[wk].app_id && now < h.3);
    let (mut fx, mut fy) = if let Some(p) = d.windows[wk].parent.and_then(|p| frame_rect(d, p)) {
        (p.x + (p.w - fw) / 2, p.y + (p.h - fh) / 2)
    } else if let Some((_, ox, oy, _)) = &hint {
        (d.seat.x as i32 - ox - b, d.seat.y as i32 - oy - t - b)
    } else {
        (wa.x + (wa.w - fw) / 2, wa.y + (wa.h - fh) / 2)
    };
    // Cascade away from windows already sitting at the same spot.
    for _ in 0..if hint.is_some() { 0 } else { 10 } {
        let taken = d.stack.iter().any(|k| {
            frame_rect(d, *k).map_or(false, |r| (r.x - fx).abs() < 8 && (r.y - fy).abs() < 8)
        });
        if !taken {
            break;
        }
        fx += 32;
        fy += 32;
    }
    fx = fx.min(wa.x1() - fw).max(wa.x);
    fy = fy.min(wa.y1() - fh).max(wa.y);
    let w = &mut d.windows[wk];
    w.x = fx + b;
    w.y = fy + t + b;
    w.placed = true;
    w.output = oi;
    w.restore = Rect::new(w.x, w.y, g.w, g.h);
}

pub fn unmap_window(d: &mut Desk, wk: WKey) {
    crate::life::gone(d, wk);
    damage_window(d, wk);
    let sk = d.windows[wk].surf;
    let x11 = d.windows[wk].x11 != 0;
    if let Some(s) = d.surfaces.get_mut(sk) {
        s.mapped = false;
        // X windows have no xdg handshake to start again: the next picture shows them.
        if !x11 {
            s.xdg_initial_sent = false;
            s.xdg_configured = false;
        }
    }
    let w = &mut d.windows[wk];
    w.placed = false;
    w.tile = Tile::Floating;
    w.last_cfg = (-1, -1, vec![]);
    d.stack.retain(|k| *k != wk);
    d.mru.retain(|k| *k != wk);
    crate::input::window_gone(d, wk);
    focus_top(d);
}

pub fn destroy_window(d: &mut Desk, wk: WKey) {
    if !d.windows.contains(wk) {
        return;
    }
    crate::life::gone(d, wk);
    damage_window(d, wk);
    d.stack.retain(|k| *k != wk);
    d.mru.retain(|k| *k != wk);
    d.show_desktop.retain(|k| *k != wk);
    if let Some((list, _)) = d.overlay.switcher.as_mut() {
        list.retain(|k| *k != wk);
    }
    for (_, w) in d.windows.iter_mut() {
        if w.parent == Some(wk) {
            w.parent = None;
        }
    }
    crate::input::window_gone(d, wk);
    d.windows.remove(wk);
    focus_top(d);
}

// ------------------------------------------------------------------ focus & stacking

/// The windows of the current workspace that are shown, bottom → top.
pub fn visible_stack(d: &Desk) -> Vec<WKey> {
    d.stack
        .iter()
        .copied()
        .filter(|k| {
            let w = &d.windows[*k];
            (w.workspace == d.workspace || w.desktop()) && !w.minimized && d.surfaces.get(w.surf).map_or(false, |s| s.mapped)
        })
        .collect()
}

pub fn focus_top(d: &mut Desk) {
    let top = d.mru.iter().copied().find(|k| {
        let w = &d.windows[*k];
        (w.workspace == d.workspace || w.desktop()) && !w.minimized && d.surfaces.get(w.surf).map_or(false, |s| s.mapped)
    });
    focus_window(d, top);
}

pub fn raise(d: &mut Desk, wk: WKey) {
    if d.windows[wk].desktop() {
        // Always at the bottom.
        d.stack.retain(|k| *k != wk);
        d.stack.insert(0, wk);
        return;
    }
    d.stack.retain(|k| *k != wk);
    d.stack.push(wk);
    // Transient children stay above their parent.
    let kids: Vec<WKey> = d.stack.iter().copied().filter(|k| d.windows[*k].parent == Some(wk)).collect();
    for k in kids {
        d.stack.retain(|x| *x != k);
        d.stack.push(k);
    }
    damage_window(d, wk);
}

pub fn focus_window(d: &mut Desk, wk: Option<WKey>) {
    let prev = d.mru.first().copied().filter(|k| d.windows.get(*k).map_or(false, |w| w.activated));
    if let Some(wk) = wk {
        d.mru.retain(|k| *k != wk);
        d.mru.insert(0, wk);
        raise(d, wk);
    }
    if prev != wk {
        if let Some(p) = prev {
            if let Some(w) = d.windows.get_mut(p) {
                w.activated = false;
                reconfigure(d, p);
                damage_window(d, p);
            }
        }
    }
    if let Some(wk) = wk {
        if !d.windows[wk].activated {
            d.windows[wk].activated = true;
            reconfigure(d, wk);
            damage_window(d, wk);
        }
    }
    let surf = wk.map(|k| d.windows[k].surf);
    crate::input::set_kb_focus(d, surf);
    shell::damage_taskbars(d);
}

pub fn focused(d: &Desk) -> Option<WKey> {
    d.mru.first().copied().filter(|k| d.windows.get(*k).map_or(false, |w| w.activated && !w.minimized))
}

pub fn close(d: &mut Desk, wk: WKey) {
    let Some(w) = d.windows.get(wk) else { return };
    if w.x11 != 0 {
        crate::xwm::close(d, w.x11);
        return;
    }
    let (ck, top) = (w.client, w.toplevel);
    if let Some(c) = d.clients.get_mut(ck) {
        xdg_toplevel::evt::close(&mut c.conn, top);
    }
}

// ------------------------------------------------------------------ states

fn save_restore(d: &mut Desk, wk: WKey) {
    let w = &d.windows[wk];
    if w.tile == Tile::Floating {
        let g = d.surfaces[w.surf].geom();
        let r = Rect::new(w.x, w.y, g.w, g.h);
        d.windows[wk].restore = r;
    }
}

fn window_output(d: &Desk, wk: WKey) -> usize {
    frame_rect(d, wk).map(|r| d.output_for_rect(&r)).unwrap_or(0)
}

pub fn maximize(d: &mut Desk, wk: WKey) {
    if !d.surfaces[d.windows[wk].surf].xdg_initial_sent {
        d.windows[wk].tile = Tile::Maximized;
        return;
    }
    save_restore(d, wk);
    let oi = window_output(d, wk);
    d.windows[wk].output = oi;
    d.windows[wk].tile = Tile::Maximized;
    let c = content_for_frame(d, wk, workarea(d, oi));
    configure_to(d, wk, c);
}

pub fn snap(d: &mut Desk, wk: WKey, s: Snap, oi: usize) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    save_restore(d, wk);
    let wa = workarea(d, oi);
    let (hw, hh) = (wa.w / 2, wa.h / 2);
    let f = match s {
        Snap::Left => Rect::new(wa.x, wa.y, hw, wa.h),
        Snap::Right => Rect::new(wa.x + hw, wa.y, wa.w - hw, wa.h),
        Snap::TopLeft => Rect::new(wa.x, wa.y, hw, hh),
        Snap::TopRight => Rect::new(wa.x + hw, wa.y, wa.w - hw, hh),
        Snap::BottomLeft => Rect::new(wa.x, wa.y + hh, hw, wa.h - hh),
        Snap::BottomRight => Rect::new(wa.x + hw, wa.y + hh, wa.w - hw, wa.h - hh),
    };
    d.windows[wk].tile = Tile::Snapped(s);
    d.windows[wk].output = oi;
    let c = content_for_frame(d, wk, f);
    configure_to(d, wk, c);
}

pub fn restore(d: &mut Desk, wk: WKey) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    let r = d.windows[wk].restore;
    d.windows[wk].tile = Tile::Floating;
    if r.is_empty() {
        send_configure(d, wk, 0, 0);
    } else {
        configure_to(d, wk, r);
    }
}

pub fn fullscreen(d: &mut Desk, wk: WKey, on: bool, oi: Option<usize>) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    if !d.surfaces[d.windows[wk].surf].xdg_initial_sent {
        if on {
            d.windows[wk].tile = Tile::Fullscreen;
            d.windows[wk].fullscreen_output = oi;
        }
        return;
    }
    if on {
        save_restore(d, wk);
        let oi = oi.unwrap_or_else(|| window_output(d, wk));
        d.windows[wk].tile = Tile::Fullscreen;
        d.windows[wk].fullscreen_output = Some(oi);
        let r = d.outputs[oi].rect();
        configure_to(d, wk, r);
        raise(d, wk);
    } else if d.windows[wk].tile == Tile::Fullscreen {
        restore(d, wk);
    }
}

pub fn minimize(d: &mut Desk, wk: WKey) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    damage_window(d, wk);
    d.windows[wk].minimized = true;
    crate::winanim::close(d, wk);
    if focused(d) == Some(wk) || d.mru.first() == Some(&wk) {
        d.mru.retain(|k| *k != wk);
        d.mru.push(wk);
        if let Some(w) = d.windows.get_mut(wk) {
            w.activated = false;
        }
        reconfigure(d, wk);
        focus_top(d);
    }
    shell::damage_taskbars(d);
}

pub fn unminimize(d: &mut Desk, wk: WKey) {
    if d.windows[wk].minimized || crate::winanim::shown(d, wk) {
        crate::winanim::open(d, wk);
    }
    d.windows[wk].minimized = false;
    d.windows[wk].workspace = d.workspace;
    focus_window(d, Some(wk));
}

/// Taskbar click semantics: focused → minimize, otherwise bring to front.
pub fn taskbar_activate(d: &mut Desk, wk: WKey) {
    if focused(d) == Some(wk) {
        minimize(d, wk);
    } else {
        if d.windows[wk].workspace != d.workspace {
            switch_workspace(d, d.windows[wk].workspace);
        }
        unminimize(d, wk);
    }
}

pub fn toggle_maximize(d: &mut Desk, wk: WKey) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    if d.windows[wk].tile == Tile::Maximized {
        restore(d, wk);
    } else {
        maximize(d, wk);
    }
}

/// Win+D: minimize everything / bring it back.
pub fn show_desktop(d: &mut Desk) {
    if !d.show_desktop.is_empty() {
        let list = std::mem::take(&mut d.show_desktop);
        for wk in list.iter().rev() {
            if d.windows.contains(*wk) {
                d.windows[*wk].minimized = false;
                crate::winanim::open(d, *wk);
                damage_window(d, *wk);
            }
        }
        if let Some(top) = list.first().copied().filter(|k| d.windows.contains(*k)) {
            focus_window(d, Some(top));
        }
        return;
    }
    let vis: Vec<WKey> = d.mru.iter().copied().filter(|k| visible_stack(d).contains(k) && !d.windows[*k].desktop()).collect();
    for wk in &vis {
        damage_window(d, *wk);
        d.windows[*wk].minimized = true;
        crate::winanim::close(d, *wk);
    }
    d.show_desktop = vis;
    focus_window(d, None);
}

/// Win+Left / Win+Right with Windows semantics (cycles snap → other monitor).
pub fn snap_key(d: &mut Desk, wk: WKey, right: bool) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    let oi = window_output(d, wk);
    let tile = d.windows[wk].tile;
    let next_out = |d: &Desk, right: bool| -> Option<usize> { neighbour_output(d, oi, right) };
    match (tile, right) {
        (Tile::Snapped(Snap::Left), true) | (Tile::Snapped(Snap::Right), false) => restore(d, wk),
        (Tile::Snapped(Snap::Left), false) => {
            if let Some(n) = next_out(d, false) {
                snap(d, wk, Snap::Right, n);
            }
        }
        (Tile::Snapped(Snap::Right), true) => {
            if let Some(n) = next_out(d, true) {
                snap(d, wk, Snap::Left, n);
            }
        }
        (Tile::Snapped(Snap::TopLeft) | Tile::Snapped(Snap::BottomLeft), true) => {
            let s = if tile == Tile::Snapped(Snap::TopLeft) { Snap::TopRight } else { Snap::BottomRight };
            snap(d, wk, s, oi)
        }
        (Tile::Snapped(Snap::TopRight) | Tile::Snapped(Snap::BottomRight), false) => {
            let s = if tile == Tile::Snapped(Snap::TopRight) { Snap::TopLeft } else { Snap::BottomLeft };
            snap(d, wk, s, oi)
        }
        _ => snap(d, wk, if right { Snap::Right } else { Snap::Left }, oi),
    }
}

/// Win+Up / Win+Down.
pub fn vertical_key(d: &mut Desk, wk: WKey, up: bool) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    let oi = window_output(d, wk);
    match (d.windows[wk].tile, up) {
        (Tile::Floating, true) => maximize(d, wk),
        (Tile::Snapped(Snap::Left), true) => snap(d, wk, Snap::TopLeft, oi),
        (Tile::Snapped(Snap::Right), true) => snap(d, wk, Snap::TopRight, oi),
        (Tile::Snapped(Snap::BottomLeft), true) => snap(d, wk, Snap::Left, oi),
        (Tile::Snapped(Snap::BottomRight), true) => snap(d, wk, Snap::Right, oi),
        (Tile::Snapped(Snap::Left), false) => snap(d, wk, Snap::BottomLeft, oi),
        (Tile::Snapped(Snap::Right), false) => snap(d, wk, Snap::BottomRight, oi),
        (Tile::Snapped(Snap::TopLeft), false) => snap(d, wk, Snap::Left, oi),
        (Tile::Snapped(Snap::TopRight), false) => snap(d, wk, Snap::Right, oi),
        (Tile::Maximized, false) | (Tile::Fullscreen, false) => restore(d, wk),
        (Tile::Floating, false) => minimize(d, wk),
        _ => {}
    }
}

/// Nearest output to the left/right of `oi` (by center).
pub fn neighbour_output(d: &Desk, oi: usize, right: bool) -> Option<usize> {
    let c = d.outputs.get(oi)?.rect().center();
    d.outputs
        .iter()
        .enumerate()
        .filter(|(i, o)| {
            let oc = o.rect().center();
            *i != oi && if right { oc.0 > c.0 } else { oc.0 < c.0 }
        })
        .min_by_key(|(_, o)| (o.rect().center().0 - c.0).abs())
        .map(|(i, _)| i)
}

/// Win+Shift+Left/Right: move to the neighbouring monitor keeping relative placement.
pub fn move_to_output(d: &mut Desk, wk: WKey, right: bool) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    let from = window_output(d, wk);
    let to = neighbour_output(d, from, right).or_else(|| {
        // Wrap around like Windows does.
        let n = d.outputs.len();
        if n > 1 { Some(if right { 0 } else { n - 1 }) } else { None }
    });
    let Some(to) = to.filter(|t| *t != from) else { return };
    match d.windows[wk].tile {
        Tile::Maximized => {
            d.windows[wk].output = to;
            let c = content_for_frame(d, wk, workarea(d, to));
            configure_to(d, wk, c);
        }
        Tile::Snapped(s) => snap(d, wk, s, to),
        Tile::Fullscreen => fullscreen(d, wk, true, Some(to)),
        Tile::Floating => {
            let (a, b) = (workarea(d, from), workarea(d, to));
            damage_window(d, wk);
            let w = &mut d.windows[wk];
            let fx = (w.x - a.x) as f64 / a.w.max(1) as f64;
            let fy = (w.y - a.y) as f64 / a.h.max(1) as f64;
            w.x = b.x + (fx * b.w as f64) as i32;
            w.y = b.y + (fy * b.h as f64) as i32;
            w.output = to;
            damage_window(d, wk);
            update_window_outputs(d, wk);
        }
    }
}

// ------------------------------------------------------------------ workspaces

pub fn switch_workspace(d: &mut Desk, ws: u32) {
    if ws == d.workspace || ws >= d.workspaces {
        return;
    }
    d.workspace = ws;
    d.overlay.switcher = None;
    d.damage_all();
    focus_top(d);
    // Hidden surfaces leave their outputs.
    for wk in d.windows.keys() {
        update_window_outputs(d, wk);
    }
}

pub fn new_workspace(d: &mut Desk) {
    d.workspaces += 1;
    switch_workspace(d, d.workspaces - 1);
}

pub fn close_workspace(d: &mut Desk) {
    if d.workspaces <= 1 {
        return;
    }
    let cur = d.workspace;
    let target = if cur > 0 { cur - 1 } else { 0 };
    for (_, w) in d.windows.iter_mut() {
        if w.workspace == cur {
            w.workspace = target;
        } else if w.workspace > cur {
            w.workspace -= 1;
        }
    }
    d.workspaces -= 1;
    d.workspace = u32::MAX;
    switch_workspace(d, target);
}

pub fn move_to_workspace(d: &mut Desk, wk: WKey, ws: u32) {
    if d.windows.get(wk).map_or(true, |w| w.desktop()) {
        return;
    }
    if ws >= d.workspaces {
        return;
    }
    damage_window(d, wk);
    d.windows[wk].workspace = ws;
    focus_top(d);
    update_window_outputs(d, wk);
}

// ------------------------------------------------------------------ outputs

/// After output add/remove/resize: pull windows back on screen, refit tiles.
pub fn outputs_changed(d: &mut Desk) {
    d.damage_all();
    if d.outputs.is_empty() {
        return;
    }
    for wk in d.windows.keys() {
        if !d.surfaces.get(d.windows[wk].surf).map_or(false, |s| s.mapped) {
            continue;
        }
        let oi = d.windows[wk].output.min(d.outputs.len() - 1);
        d.windows[wk].output = oi;
        match d.windows[wk].tile {
            Tile::Maximized => {
                let c = content_for_frame(d, wk, workarea(d, oi));
                configure_to(d, wk, c);
            }
            Tile::Snapped(s) => snap(d, wk, s, oi),
            Tile::Fullscreen => {
                let o = d.windows[wk].fullscreen_output.unwrap_or(oi).min(d.outputs.len() - 1);
                fullscreen(d, wk, true, Some(o));
            }
            Tile::Floating => {
                // Keep at least the title bar reachable.
                if let Some(f) = frame_rect(d, wk) {
                    let any = d.outputs.iter().any(|o| o.rect().overlaps(&Rect::new(f.x, f.y, f.w, shell::TITLE_H)));
                    if !any {
                        let wa = workarea(d, oi);
                        let w = &mut d.windows[wk];
                        w.x = wa.x + 40;
                        w.y = wa.y + 40 + shell::TITLE_H;
                    }
                }
            }
        }
        update_window_outputs(d, wk);
    }
}

/// Recomputes wl_surface.enter/leave + preferred scale for a window's surface tree.
pub fn update_window_outputs(d: &mut Desk, wk: WKey) {
    let Some(w) = d.windows.get(wk) else { return };
    let visible = !w.minimized && w.workspace == d.workspace && d.surfaces.get(w.surf).map_or(false, |s| s.mapped);
    let frame = frame_rect(d, wk).unwrap_or_default();
    let mut mask = 0u64;
    if visible {
        for (i, o) in d.outputs.iter().enumerate() {
            if o.rect().overlaps(&frame) {
                mask |= 1 << o.id.min(63);
            }
        }
    }
    let mut stack = vec![w.surf];
    while let Some(sk) = stack.pop() {
        let Some(s) = d.surfaces.get(sk) else { continue };
        stack.extend(s.subs.iter().filter(|k| **k != SKey::default()));
        stack.extend(s.popups.iter());
        set_surface_outputs(d, sk, mask);
        update_surface_scale(d, sk);
    }
}

fn set_surface_outputs(d: &mut Desk, sk: SKey, mask: u64) {
    let s = &mut d.surfaces[sk];
    let old = s.outputs;
    if old == mask {
        return;
    }
    s.outputs = mask;
    let (ck, sid) = (s.client, s.id);
    let Some(c) = d.clients.get_mut(ck) else { return };
    let outs = c.outputs.clone();
    for (oid, obj) in outs {
        let bit = 1u64 << oid.min(63);
        if mask & bit != 0 && old & bit == 0 {
            wl_surface::evt::enter(&mut c.conn, sid, obj);
        } else if mask & bit == 0 && old & bit != 0 {
            wl_surface::evt::leave(&mut c.conn, sid, obj);
        }
    }
}

pub fn resend_enters(d: &mut Desk, ck: CKey, oid: u32, obj: ObjId) {
    let bit = 1u64 << oid.min(63);
    let list: Vec<ObjId> =
        d.surfaces.iter().filter(|(_, s)| s.client == ck && s.outputs & bit != 0).map(|(_, s)| s.id).collect();
    for sid in list {
        wl_surface::evt::enter(&mut d.clients[ck].conn, sid, obj);
    }
}

/// Preferred integer scale (wl_surface v6) and fractional scale for a surface.
pub fn update_surface_scale(d: &mut Desk, sk: SKey) {
    let scale = {
        let wk = d.window_of(sk);
        let oi = match wk.and_then(|w| frame_rect(d, w)) {
            Some(r) => d.output_for_rect(&r),
            None => d.output_at(d.seat.x, d.seat.y).unwrap_or(0),
        };
        d.outputs.get(oi).map_or(1.0, |o| o.scale)
    };
    let s = &mut d.surfaces[sk];
    let int = scale.ceil() as i32;
    let frac = (scale * 120.0).round() as u32;
    let (ck, sid, fobj) = (s.client, s.id, s.frac_obj);
    let send_int = s.sent_scale != int;
    let send_frac = fobj != 0 && s.sent_frac != frac;
    s.sent_scale = int;
    if fobj != 0 {
        s.sent_frac = frac;
    }
    let Some(c) = d.clients.get_mut(ck) else { return };
    if send_int && c.version(sid) >= 6 {
        wl_surface::evt::preferred_buffer_scale(&mut c.conn, sid, int);
    }
    if send_frac {
        wp_fractional_scale_v1::evt::preferred_scale(&mut c.conn, fobj, frac);
    }
}

// ------------------------------------------------------------------ hit testing

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Surface(SKey, f64, f64),
    Title,
    Minimize,
    Maximize,
    Close,
    Edge(u32),
}

/// Topmost window element under (x, y) in global coords.
pub fn hit_test(d: &Desk, x: f64, y: f64) -> Option<(WKey, Hit)> {
    // The desktop's menus are over every window.
    for wk in visible_stack(d).into_iter().filter(|k| d.windows[*k].desktop()) {
        if let Some(h) = hit_surface_tree(d, d.windows[wk].surf, x, y, true) {
            return Some((wk, h));
        }
    }
    for wk in visible_stack(d).into_iter().rev() {
        let w = &d.windows[wk];
        // Popups first (they sit above their window).
        if let Some(h) = hit_surface_tree(d, w.surf, x, y, true) {
            return Some((wk, h));
        }
        let Some(f) = frame_rect(d, wk) else { continue };
        if w.ssd && w.tile != Tile::Fullscreen {
            if let Some(h) = shell::deco_hit_m(f, w.tile, x, y, w.merged()) {
                return Some((wk, h));
            }
        }
        if let Some(h) = hit_surface_tree(d, w.surf, x, y, false) {
            return Some((wk, h));
        }
        // Invisible resize borders around CSD windows too.
        if w.tile == Tile::Floating {
            let g = Rect::new(w.x, w.y, d.surfaces[w.surf].geom().w, d.surfaces[w.surf].geom().h);
            let outer = if w.ssd { f } else { g };
            if let Some(e) = shell::edge_hit(outer, x, y) {
                return Some((wk, Hit::Edge(e)));
            }
        }
    }
    None
}

fn hit_surface_tree(d: &Desk, sk: SKey, x: f64, y: f64, popups_only: bool) -> Option<Hit> {
    let s = d.surfaces.get(sk)?;
    for p in s.popups.iter().rev() {
        if d.surfaces.get(*p).map_or(false, |p| p.mapped) {
            if let Some(h) = hit_surface_tree(d, *p, x, y, false) {
                return Some(h);
            }
        }
    }
    if popups_only {
        // Popups of subsurfaces are rare; ignore.
        return None;
    }
    for c in s.subs.iter().rev() {
        if *c == SKey::default() {
            let (ox, oy) = d.surface_origin(sk)?;
            let (lx, ly) = (x - ox as f64, y - oy as f64);
            if s.buffer.is_some() && s.accepts_input(lx, ly) {
                return Some(Hit::Surface(sk, lx, ly));
            }
        } else if d.surfaces.get(*c).map_or(false, |c| c.buffer.is_some()) {
            if let Some(h) = hit_surface_tree(d, *c, x, y, false) {
                return Some(h);
            }
        }
    }
    None
}

/// Moves the active window so its content point (x, y) sits under the pointer (a single tab
/// of «Проводник» dragged and let go away from any tab strip).
pub fn move_active_to_pointer(d: &mut Desk, x: i32, y: i32) {
    let Some(&wk) = d.mru.first() else { return };
    let Some(w) = d.windows.get(wk) else { return };
    if w.tile != Tile::Floating || w.minimized || w.desktop() {
        return;
    }
    let old = frame_rect(d, wk);
    let oi = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
    let wa = workarea(d, oi);
    let g = d.surfaces[w.surf].geom();
    let nx = (d.seat.x as i32 - x).clamp(wa.x - g.w + 80, wa.x1() - 80);
    let ny = (d.seat.y as i32 - y).clamp(wa.y, wa.y1() - 40);
    let w = &mut d.windows[wk];
    w.x = nx;
    w.y = ny;
    w.restore = Rect::new(nx, ny, g.w, g.h);
    if let Some(r) = old {
        d.damage(r.inflate(crate::shell::SHADOW));
    }
    if let Some(r) = visual_rect(d, wk) {
        d.damage(r);
    }
    let sk = d.windows[wk].surf;
    d.damage_surface_tree(sk);
    update_window_outputs(d, wk);
}
