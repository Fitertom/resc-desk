//! wlr-screencopy (v3): monitor pictures copied into client shm buffers. This is what
//! xdg-desktop-portal-wlr reads to stream the screen through PipeWire (screen sharing in
//! browsers, Discord, OBS), and what grim uses for screenshots.
//!
//! `copy` is answered after the next pass of the main loop; `copy_with_damage` waits until
//! the monitor has drawn a new frame since the request, so an idle screen costs nothing.
use crate::backend::Backend;
use crate::desk::Desk;
use crate::geom::Rect;
use crate::shm::{Buffer, FMT_ARGB8888, FMT_XRGB8888};
use crate::state::*;
use std::rc::Rc;
use wl::protocol::*;
use wl::{Iface, ObjId};

/// What a frame object captures: an output (by id) and the part of it, in its pixels.
pub struct FrameInfo {
    pub out: u32,
    pub rect: Rect,
    pub used: bool,
}

/// A copy the client asked for, waiting for pixels.
pub struct Pending {
    client: CKey,
    obj: ObjId,
    out: u32,
    rect: Rect,
    buffer: Rc<Buffer>,
    /// copy_with_damage: the output's frame count at the request; wait for a newer frame.
    after_frame: Option<u64>,
}

/// capture_output / capture_output_region. `region` is in the output's logical coordinates.
pub fn capture(d: &mut Desk, ck: CKey, frame: ObjId, ver: u32, output: ObjId, region: Option<(i32, i32, i32, i32)>) {
    let oid = match d.clients.get(ck).and_then(|c| c.obj(output)).map(|o| &o.data) {
        Some(Data::Output(o)) => Some(*o),
        _ => None,
    };
    let Some((oid, idx)) = oid.and_then(|o| d.output_index(o).map(|i| (o, i))) else {
        // The output is gone: an inert frame that fails at once.
        if d.clients[ck].insert(frame, Iface::ZwlrScreencopyFrameV1, ver, Data::None) {
            zwlr_screencopy_frame_v1::evt::failed(&mut d.clients[ck].conn, frame);
        }
        return;
    };
    let o = &d.outputs[idx];
    let full = Rect::new(0, 0, o.pw, o.ph);
    let rect = match region {
        None => full,
        Some((x, y, w, h)) => {
            let s = o.scale;
            let (x0, y0) = ((x as f64 * s).floor() as i32, (y as f64 * s).floor() as i32);
            let (x1, y1) = (((x + w) as f64 * s).ceil() as i32, ((y + h) as f64 * s).ceil() as i32);
            Rect::from_edges(x0, y0, x1, y1).intersect(&full)
        }
    };
    let info = FrameInfo { out: oid, rect, used: false };
    if !d.clients[ck].insert(frame, Iface::ZwlrScreencopyFrameV1, ver, Data::ScreencopyFrame(Box::new(info))) {
        return;
    }
    let c = &mut d.clients[ck];
    if rect.is_empty() {
        zwlr_screencopy_frame_v1::evt::failed(&mut c.conn, frame);
        return;
    }
    zwlr_screencopy_frame_v1::evt::buffer(&mut c.conn, frame, FMT_XRGB8888, rect.w as u32, rect.h as u32, rect.w as u32 * 4);
    if ver >= 3 {
        zwlr_screencopy_frame_v1::evt::buffer_done(&mut c.conn, frame);
    }
}

/// copy / copy_with_damage on frame `id`.
pub fn copy(d: &mut Desk, ck: CKey, id: ObjId, buffer: ObjId, with_damage: bool) {
    use zwlr_screencopy_frame_v1::error as err;
    let buf = match d.clients.get(ck).and_then(|c| c.obj(buffer)).map(|o| &o.data) {
        Some(Data::Buffer(b)) => Some(b.clone()),
        _ => None,
    };
    let Some(Obj { data: Data::ScreencopyFrame(f), .. }) = d.clients[ck].obj_mut(id) else { return };
    if f.used {
        d.post_error(ck, id, err::ALREADY_USED, "frame already copied");
        return;
    }
    f.used = true;
    let (out, rect) = (f.out, f.rect);
    let ok = buf.as_ref().map_or(false, |b| {
        b.dmabuf.is_none()
            && b.solid.is_none()
            && (b.format == FMT_XRGB8888 || b.format == FMT_ARGB8888)
            && b.w == rect.w
            && b.h == rect.h
            && b.stride >= rect.w * 4
            && b.valid_for_pool()
    });
    let Some(buffer) = buf.filter(|_| ok) else {
        d.post_error(ck, id, err::INVALID_BUFFER, "buffer does not match the advertised one");
        return;
    };
    let after_frame = if with_damage { d.output_index(out).map(|i| d.outputs[i].frames) } else { None };
    d.screencopy.push(Pending { client: ck, obj: id, out, rect, buffer, after_frame });
}

/// The frame object was destroyed: forget its pending copy.
pub fn forget(d: &mut Desk, ck: CKey, id: ObjId) {
    d.screencopy.retain(|p| !(p.client == ck && p.obj == id));
}

/// After drawing (main loop): answers every copy whose picture is ready.
pub fn process(d: &mut Desk, b: &mut dyn Backend) {
    if d.screencopy.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut d.screencopy);
    let mut shots: Vec<(u32, Option<(Vec<u32>, i32, i32)>)> = Vec::new();
    for p in pending {
        if d.clients.get(p.client).map_or(true, |c| c.dead.is_some() || c.obj(p.obj).is_none()) {
            continue;
        }
        let Some(idx) = d.output_index(p.out) else {
            zwlr_screencopy_frame_v1::evt::failed(&mut d.clients[p.client].conn, p.obj);
            continue;
        };
        if p.after_frame.map_or(false, |f| d.outputs[idx].frames <= f) {
            d.screencopy.push(p);
            continue;
        }
        if !shots.iter().any(|(o, _)| *o == p.out) {
            let px = b.read_pixels(d, idx).or_else(|| Some(crate::render::render_offscreen(d, idx)));
            shots.push((p.out, px));
        }
        let shot = shots.iter().find(|(o, _)| *o == p.out).and_then(|(_, s)| s.as_ref());
        let done = shot.map_or(false, |(px, w, h)| {
            let r = p.rect;
            if r.x1() > *w || r.y1() > *h || px.len() < (*w * *h) as usize {
                return false;
            }
            p.buffer.write_pixels(|dst, stride| {
                for row in 0..r.h as usize {
                    let s = (r.y as usize + row) * *w as usize + r.x as usize;
                    let t = row * stride;
                    let line = &px[s..s + r.w as usize];
                    for (o, &v) in dst[t..t + r.w as usize].iter_mut().zip(line) {
                        *o = v | 0xff00_0000;
                    }
                }
            })
        });
        let c = &mut d.clients[p.client];
        if !done {
            zwlr_screencopy_frame_v1::evt::failed(&mut c.conn, p.obj);
            continue;
        }
        zwlr_screencopy_frame_v1::evt::flags(&mut c.conn, p.obj, 0);
        if p.after_frame.is_some() {
            zwlr_screencopy_frame_v1::evt::damage(&mut c.conn, p.obj, 0, 0, p.rect.w as u32, p.rect.h as u32);
        }
        let ns = sys::now_ns();
        let sec = ns / 1_000_000_000;
        zwlr_screencopy_frame_v1::evt::ready(&mut c.conn, p.obj, (sec >> 32) as u32, sec as u32, (ns % 1_000_000_000) as u32);
    }
}

/// XDG_CURRENT_DESKTOP for programs started here.
pub const DESKTOP: &str = "resc";

const PORTALS_CONF: &str = "\
# Written by resc-desk: which portal backend serves what on this desktop.
[preferred]
default=gtk
org.freedesktop.impl.portal.ScreenCast=wlr
org.freedesktop.impl.portal.Screenshot=wlr
";

/// Screen sharing through xdg-desktop-portal-wlr: tells it which portal backend to use
/// (unless the user already did), and hands this session's environment to D-Bus and
/// systemd, so the portal started on demand talks to us. A portal left over from another
/// session (X11, another compositor) is stopped; D-Bus starts it again when asked.
pub fn setup_portal(d: &mut Desk) {
    if let Ok(home) = std::env::var("HOME") {
        let cfg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
        let dir = format!("{cfg}/xdg-desktop-portal");
        let path = format!("{dir}/{DESKTOP}-portals.conf");
        if !std::path::Path::new(&path).exists() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, PORTALS_CONF);
        }
        // No output picker yet: the portal shares the main monitor without asking
        // (change output_name there to share another one).
        let wlr = format!("{cfg}/xdg-desktop-portal-wlr");
        let path = format!("{wlr}/config");
        if !std::path::Path::new(&path).exists() && !d.outputs.is_empty() {
            let main = d.outputs[crate::monitors::primary_index(d).min(d.outputs.len() - 1)].name.clone();
            let _ = std::fs::create_dir_all(&wlr);
            let _ = std::fs::write(&path, format!("# Written by resc-desk.\n[screencast]\nchooser_type=none\noutput_name={main}\nmax_fps=60\n"));
        }
    }
    d.spawn(
        "command -v dbus-update-activation-environment >/dev/null && \
         dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE ${DISPLAY:+DISPLAY}; \
         systemctl --user stop xdg-desktop-portal-wlr.service xdg-desktop-portal.service 2>/dev/null; true",
    );
}
