//! CPU compositor: flattens the scene per output, repaints only damaged rects,
//! starts each rect at the topmost fully-opaque element (occlusion culling).
use crate::desk::Desk;
use crate::geom::{Damage, Rect};
use crate::shell;
use crate::state::*;
use crate::wm;
use wl::protocol::*;

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
    pub stride: usize,
}

/// Logical → physical transform of one output: `(logical - o) * scale - p`. `p` moves the
/// result by whole physical pixels (a canvas that is a piece of the output: the GPU's
/// per-window frame textures), so rounding stays exactly as on the output itself.
#[derive(Clone, Copy)]
pub struct Xf {
    pub ox: i32,
    pub oy: i32,
    pub scale: f64,
    pub px: i32,
    pub py: i32,
}

impl Xf {
    pub fn new(ox: i32, oy: i32, scale: f64) -> Xf {
        Xf { ox, oy, scale, px: 0, py: 0 }
    }
    /// The same transform drawn into a canvas whose (0, 0) is physical (dx, dy) here.
    pub fn shifted(self, dx: i32, dy: i32) -> Xf {
        Xf { px: self.px + dx, py: self.py + dy, ..self }
    }
    pub fn r(&self, r: Rect) -> Rect {
        if self.scale == 1.0 {
            return r.translate(-self.ox - self.px, -self.oy - self.py);
        }
        let x0 = ((r.x - self.ox) as f64 * self.scale).round() as i32 - self.px;
        let y0 = ((r.y - self.oy) as f64 * self.scale).round() as i32 - self.py;
        let x1 = ((r.x1() - self.ox) as f64 * self.scale).round() as i32 - self.px;
        let y1 = ((r.y1() - self.oy) as f64 * self.scale).round() as i32 - self.py;
        Rect::from_edges(x0, y0, x1, y1)
    }
    pub fn len(&self, v: i32) -> i32 {
        ((v as f64) * self.scale).round() as i32
    }
    /// A logical x / y as a physical coordinate (not rounded).
    pub fn fx(&self, x: f64) -> f64 {
        (x - self.ox as f64) * self.scale - self.px as f64
    }
    pub fn fy(&self, y: f64) -> f64 {
        (y - self.oy as f64) * self.scale - self.py as f64
    }
}

#[inline(always)]
pub fn blend(dst: u32, src: u32) -> u32 {
    let a = src >> 24;
    if a == 255 {
        return src;
    }
    if a == 0 {
        return dst;
    }
    let ia = 255 - a;
    let mut rb = (dst & 0x00ff_00ff) * ia + 0x0080_0080;
    rb = ((rb + ((rb >> 8) & 0x00ff_00ff)) >> 8) & 0x00ff_00ff;
    let mut ag = ((dst >> 8) & 0x00ff_00ff) * ia + 0x0080_0080;
    ag = (ag + ((ag >> 8) & 0x00ff_00ff)) & 0xff00_ff00;
    src.wrapping_add(rb | ag)
}

/// Multiplies a premultiplied pixel by alpha (0..255).
#[inline(always)]
pub fn fade(px: u32, a: u32) -> u32 {
    let mut rb = (px & 0x00ff_00ff) * a + 0x0080_0080;
    rb = ((rb + ((rb >> 8) & 0x00ff_00ff)) >> 8) & 0x00ff_00ff;
    let mut ag = ((px >> 8) & 0x00ff_00ff) * a + 0x0080_0080;
    ag = (ag + ((ag >> 8) & 0x00ff_00ff)) & 0xff00_ff00;
    rb | ag
}

/// Packed per-channel lerp of two ARGB pixels, t in 0..=255.
#[inline(always)]
pub fn lerp(a: u32, b: u32, t: u32) -> u32 {
    if t == 0 || a == b {
        return a;
    }
    let it = 256 - t;
    let rb = (((a & 0x00ff_00ff) * it + (b & 0x00ff_00ff) * t) >> 8) & 0x00ff_00ff;
    let ag = (((a >> 8) & 0x00ff_00ff) * it + ((b >> 8) & 0x00ff_00ff) * t) & 0xff00_ff00;
    rb | ag
}

/// Builds a premultiplied ARGB color.
pub const fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)
}

impl<'a> Canvas<'a> {
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    /// Makes `r` fully transparent (a layer about to be redrawn there).
    pub fn clear(&mut self, r: Rect) {
        let r = r.intersect(&self.bounds());
        for y in r.y..r.y1() {
            let o = y as usize * self.stride;
            self.px[o + r.x as usize..o + r.x1() as usize].fill(0);
        }
    }

    pub fn fill(&mut self, clip: Rect, r: Rect, color: u32) {
        let r = r.intersect(&clip).intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        let a = color >> 24;
        for y in r.y..r.y1() {
            let row = &mut self.px[y as usize * self.stride + r.x as usize..y as usize * self.stride + r.x1() as usize];
            if a == 255 {
                row.fill(color);
            } else {
                for p in row.iter_mut() {
                    *p = blend(*p, color);
                }
            }
        }
    }

    /// Vertical two-stop gradient over `r` (only `clip` is touched).
    pub fn vgradient(&mut self, clip: Rect, r: Rect, top: u32, bottom: u32) {
        let c = r.intersect(&clip).intersect(&self.bounds());
        if c.is_empty() {
            return;
        }
        let h = r.h.max(1) as u32;
        for y in c.y..c.y1() {
            let t = (((y - r.y).max(0) as u32) * 256 / h).min(256);
            let lerp = |s: u32| -> u32 {
                let a = (top >> s) & 0xff;
                let b = (bottom >> s) & 0xff;
                ((a * (256 - t) + b * t) >> 8) & 0xff
            };
            let col = 0xff00_0000 | (lerp(16) << 16) | (lerp(8) << 8) | lerp(0);
            let o = y as usize * self.stride;
            self.px[o + c.x as usize..o + c.x1() as usize].fill(col);
        }
    }

    /// Scaled (nearest) blit of a source sub-rect into `dst`, clipped to `clip`.
    pub fn blit(&mut self, clip: Rect, dst: Rect, src: &[u32], sstride: usize, srect: Rect, opaque: bool, alpha: u32) {
        let c = dst.intersect(&clip).intersect(&self.bounds());
        if c.is_empty() || srect.is_empty() || dst.is_empty() {
            return;
        }
        let same = srect.w == dst.w && srect.h == dst.h;
        // 16.16 fixed-point source stepping.
        let sxs = ((srect.w as i64) << 16) / dst.w as i64;
        let sys = ((srect.h as i64) << 16) / dst.h as i64;
        let max_x = srect.x1() - 1;
        let max_y = srect.y1() - 1;
        for y in c.y..c.y1() {
            let sy = if same {
                srect.y + (y - dst.y)
            } else {
                (srect.y + ((((y - dst.y) as i64 * sys) + (sys >> 1)) >> 16) as i32).min(max_y)
            };
            let srow = sy as usize * sstride;
            let drow = y as usize * self.stride;
            if same {
                let sx0 = srow + (srect.x + (c.x - dst.x)) as usize;
                let s = &src[sx0..sx0 + c.w as usize];
                let d = &mut self.px[drow + c.x as usize..drow + c.x1() as usize];
                if opaque && alpha == 255 {
                    for (dp, sp) in d.iter_mut().zip(s) {
                        *dp = *sp | 0xff00_0000;
                    }
                } else if alpha == 255 {
                    for (dp, sp) in d.iter_mut().zip(s) {
                        *dp = blend(*dp, *sp);
                    }
                } else {
                    for (dp, sp) in d.iter_mut().zip(s) {
                        let sp = if opaque { *sp | 0xff00_0000 } else { *sp };
                        *dp = blend(*dp, fade(sp, alpha));
                    }
                }
            } else {
                // Bilinear: sample at the destination pixel centre (16.16 fixed point).
                let fy = (((y - dst.y) as i64 * sys) + (sys >> 1) - (1 << 15)).max(0);
                let sy0 = (srect.y + (fy >> 16) as i32).min(max_y);
                let sy1 = (sy0 + 1).min(max_y);
                let wy = ((fy >> 8) & 0xff) as u32;
                let (r0, r1) = (sy0 as usize * sstride, sy1 as usize * sstride);
                let _ = srow;
                let mut fx = ((c.x - dst.x) as i64 * sxs) + (sxs >> 1) - (1 << 15);
                for x in c.x..c.x1() {
                    let fxc = fx.max(0);
                    fx += sxs;
                    let sx0 = (srect.x + (fxc >> 16) as i32).min(max_x) as usize;
                    let sx1 = ((sx0 as i32 + 1).min(max_x)) as usize;
                    let wx = ((fxc >> 8) & 0xff) as u32;
                    let top = lerp(src[r0 + sx0], src[r0 + sx1], wx);
                    let bot = lerp(src[r1 + sx0], src[r1 + sx1], wx);
                    let mut sp = lerp(top, bot, wy);
                    if opaque {
                        sp |= 0xff00_0000;
                    }
                    if alpha != 255 {
                        sp = fade(sp, alpha);
                    }
                    let dp = &mut self.px[drow + x as usize];
                    *dp = blend(*dp, sp);
                }
            }
        }
    }

    /// Alpha mask blit (glyphs, icons): `mask` is w*h coverage 0..255, tinted by `color`.
    pub fn mask(&mut self, clip: Rect, x: i32, y: i32, mw: i32, mh: i32, mask: &[u8], color: u32) {
        let dst = Rect::new(x, y, mw, mh);
        let c = dst.intersect(&clip).intersect(&self.bounds());
        if c.is_empty() {
            return;
        }
        for yy in c.y..c.y1() {
            let mrow = ((yy - y) * mw) as usize;
            let drow = yy as usize * self.stride;
            for xx in c.x..c.x1() {
                let a = mask[mrow + (xx - x) as usize] as u32;
                if a == 0 {
                    continue;
                }
                let p = &mut self.px[drow + xx as usize];
                *p = blend(*p, fade(color, a));
            }
        }
    }

    /// Filled rounded rectangle with anti-aliased corners.
    pub fn rrect(&mut self, clip: Rect, r: Rect, radius: i32, color: u32) {
        let c = r.intersect(&clip).intersect(&self.bounds());
        if c.is_empty() {
            return;
        }
        let rad = radius.min(r.w / 2).min(r.h / 2).max(0) as f32;
        if rad < 0.5 {
            self.fill(clip, r, color);
            return;
        }
        // The continuous corner's curve starts up to 1.6 radii from the corner.
        let zone = rad * 1.6 + 1.0;
        for y in c.y..c.y1() {
            let py = y as f32 + 0.5;
            let drow = y as usize * self.stride;
            let in_y = py < r.y as f32 + zone || py > r.y1() as f32 - zone;
            for x in c.x..c.x1() {
                let px = x as f32 + 0.5;
                let mut cov = 1.0f32;
                if in_y && (px < r.x as f32 + zone || px > r.x1() as f32 - zone) {
                    cov = (0.5 - rrect_sdf(px, py, r, rad)).clamp(0.0, 1.0);
                }
                if cov <= 0.0 {
                    continue;
                }
                let a = (cov * 255.0) as u32;
                let p = &mut self.px[drow + x as usize];
                *p = blend(*p, if a >= 255 { color } else { fade(color, a) });
            }
        }
    }

    /// Rounded-rect outline `width` px thick. The alpha goes from `top` at the upper
    /// edge to `bottom` at the lower one: the rim light of Liquify's glass.
    pub fn rrect_stroke(&mut self, clip: Rect, r: Rect, radius: i32, width: f32, rgb: u32, top: u32, bottom: u32) {
        let c = r.intersect(&clip).intersect(&self.bounds());
        if c.is_empty() {
            return;
        }
        let rad = (radius.min(r.w / 2).min(r.h / 2).max(0)) as f32;
        let h = r.h.max(1) as f32;
        for y in c.y..c.y1() {
            let py = y as f32 + 0.5;
            let t = ((py - r.y as f32) / h).clamp(0.0, 1.0);
            let alpha = top as f32 + (bottom as f32 - top as f32) * t.sqrt();
            let row = y as usize * self.stride;
            for x in c.x..c.x1() {
                let px = x as f32 + 0.5;
                let d = rrect_sdf(px, py, r, rad);
                // Inside the band [-width, 0] of the signed distance.
                let cov = ((d + width + 0.5).clamp(0.0, 1.0)) * ((0.5 - d).clamp(0.0, 1.0));
                if cov <= 0.0 {
                    continue;
                }
                let a = (cov * alpha) as u32;
                if a == 0 {
                    continue;
                }
                let p = &mut self.px[row + x as usize];
                *p = blend(*p, argb(a, (rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff));
            }
        }
    }

    /// Anti-aliased line segment in physical pixels (icons: search, power, chevrons).
    pub fn line(&mut self, clip: Rect, p0: (f32, f32), p1: (f32, f32), width: f32, color: u32) {
        let pad = width.ceil() as i32 + 1;
        let bb = Rect::from_edges(
            p0.0.min(p1.0) as i32 - pad,
            p0.1.min(p1.1) as i32 - pad,
            p0.0.max(p1.0) as i32 + pad + 1,
            p0.1.max(p1.1) as i32 + pad + 1,
        );
        let c = bb.intersect(&clip).intersect(&self.bounds());
        let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for y in c.y..c.y1() {
            for x in c.x..c.x1() {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = (((px - p0.0) * dx + (py - p0.1) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (p0.0 + t * dx - px, p0.1 + t * dy - py);
                let dist = (qx * qx + qy * qy).sqrt();
                let cov = (width / 2.0 - dist + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let p = &mut self.px[y as usize * self.stride + x as usize];
                    *p = blend(*p, fade(color, (cov * 255.0) as u32));
                }
            }
        }
    }

    /// Ring (circle outline) centred at `c` in physical pixels.
    /// An arc of a ring from angle `a0` to `a1` (radians, clockwise on screen, 0 = right),
    /// with round ends.
    pub fn arc(&mut self, clip: Rect, c: (f32, f32), radius: f32, width: f32, a0: f32, a1: f32, color: u32) {
        let pad = (radius + width) as i32 + 2;
        let bb = Rect::new(c.0 as i32 - pad, c.1 as i32 - pad, 2 * pad, 2 * pad).intersect(&clip).intersect(&self.bounds());
        let tau = std::f32::consts::TAU;
        let ends = [a0, a1].map(|a| (c.0 + radius * a.cos(), c.1 + radius * a.sin()));
        for y in bb.y..bb.y1() {
            for x in bb.x..bb.x1() {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = ((px - c.0).powi(2) + (py - c.1).powi(2)).sqrt();
                let a = (py - c.1).atan2(px - c.0);
                let rel = (a - a0).rem_euclid(tau);
                let mut cov = if rel <= a1 - a0 { (width / 2.0 - (d - radius).abs() + 0.5).clamp(0.0, 1.0) } else { 0.0 };
                for e in ends {
                    let de = ((px - e.0).powi(2) + (py - e.1).powi(2)).sqrt();
                    cov = cov.max((width / 2.0 - de + 0.5).clamp(0.0, 1.0));
                }
                if cov > 0.0 {
                    let p = &mut self.px[y as usize * self.stride + x as usize];
                    *p = blend(*p, fade(color, (cov * 255.0) as u32));
                }
            }
        }
    }

    pub fn ring(&mut self, clip: Rect, c: (f32, f32), radius: f32, width: f32, color: u32) {
        let pad = (radius + width) as i32 + 2;
        let bb = Rect::new(c.0 as i32 - pad, c.1 as i32 - pad, 2 * pad, 2 * pad).intersect(&clip).intersect(&self.bounds());
        for y in bb.y..bb.y1() {
            for x in bb.x..bb.x1() {
                let d = ((x as f32 + 0.5 - c.0).powi(2) + (y as f32 + 0.5 - c.1).powi(2)).sqrt();
                let cov = (width / 2.0 - (d - radius).abs() + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let p = &mut self.px[y as usize * self.stride + x as usize];
                    *p = blend(*p, fade(color, (cov * 255.0) as u32));
                }
            }
        }
    }

}

/// Signed distance from a point to a rounded rect (negative inside).
/// Signed distance to `r` rounded by `rad`, with the continuous (squircle) corners the
/// glass has (glass.rs `sd_squircle`, liquify's squircle.glsl): frames, outlines and the
/// glass under them meet exactly.
pub fn rrect_sdf(px: f32, py: f32, r: Rect, rad: f32) -> f32 {
    let (hw, hh) = (r.w as f32 / 2.0, r.h as f32 / 2.0);
    let (cx, cy) = (r.x as f32 + hw, r.y as f32 + hh);
    crate::glass::sd_squircle((px - cx, py - cy), (hw, hh), rad)
}

// ------------------------------------------------------------------ scene

#[derive(Clone, Copy)]
enum Elem {
    Background,
    Shadow(WKey),
    Surface(SKey),
    Deco(WKey),
    Taskbar(usize),
    /// «Пуск»'s content, in its glass under its drops (the GPU path, startanim.rs; the CPU
    /// path draws the whole menu with the overlays).
    StartContent,
    Overlays,
    Cursor,
}

impl Elem {
    /// The CPU layer this goes into on the GPU path (render_layers); None: the GPU draws it.
    fn gpu_layer(&self) -> Option<usize> {
        match self {
            Elem::Taskbar(_) | Elem::StartContent => Some(0),
            Elem::Overlays | Elem::Cursor => Some(1),
            _ => None,
        }
    }
}

struct Item {
    rect: Rect,
    opaque: Option<Rect>,
    elem: Elem,
    /// A window's surfaces: clipped to its rounded corners (logical rect, radius).
    round: Option<(Rect, i32)>,
    /// The window it belongs to (its frame, shadow, surfaces and popups).
    win: Option<WKey>,
}

impl Item {
    fn new(rect: Rect, opaque: Option<Rect>, elem: Elem) -> Item {
        Item { rect, opaque, elem, round: None, win: None }
    }
}

fn push_tree(d: &Desk, sk: SKey, out: &mut Vec<Item>, round: Option<(Rect, i32)>) {
    push_tree_p(d, sk, out, round, true);
}

fn push_tree_p(d: &Desk, sk: SKey, out: &mut Vec<Item>, round: Option<(Rect, i32)>, popups: bool) {
    let Some(s) = d.surfaces.get(sk) else { return };
    for c in &s.subs {
        if *c == SKey::default() {
            if s.buffer.is_some() {
                if let Some((x, y)) = d.surface_origin(sk) {
                    let rect = Rect::new(x, y, s.w, s.h);
                    // Rounded bottom corners are not opaque.
                    let opaque = s.is_opaque().then(|| match round {
                        Some((_, r)) => Rect::new(rect.x, rect.y, rect.w, (rect.h - r).max(0)),
                        None => rect,
                    });
                    out.push(Item { rect, opaque, elem: Elem::Surface(sk), round, win: None });
                }
            }
        } else {
            push_tree(d, *c, out, round);
        }
    }
    // Popups (menus) may stick out of the window: never clipped.
    for p in s.popups.iter().filter(|_| popups) {
        if d.surfaces.get(*p).map_or(false, |p| p.mapped) {
            push_tree(d, *p, out, None);
        }
    }
}

/// The windows drawn, bottom → top: the shown ones, and with `morphs` (the GPU path, which
/// draws them moving) the minimized ones still shrinking into their buttons.
fn drawn_stack(d: &Desk, morphs: bool) -> Vec<WKey> {
    let mut v: Vec<WKey> = d
        .stack
        .iter()
        .copied()
        .filter(|k| {
            let w = &d.windows[*k];
            w.workspace == d.workspace
                && d.surfaces.get(w.surf).map_or(false, |s| s.mapped)
                && (!w.minimized || (morphs && crate::winanim::shown(d, *k)))
        })
        .collect();
    // A window on its way into its button stays on top until it gets there (the next one,
    // focused and raised, would otherwise hide it).
    if morphs {
        v.sort_by_key(|k| crate::winanim::shown(d, *k));
    }
    v
}

fn build_scene(d: &Desk, oi: usize) -> Vec<Item> {
    build_scene_m(d, oi, false)
}

fn build_scene_m(d: &Desk, oi: usize, morphs: bool) -> Vec<Item> {
    let o = &d.outputs[oi];
    let orect = o.rect();
    let mut items = vec![Item::new(orect, Some(orect), Elem::Background)];
    let vis = drawn_stack(d, morphs);
    let fullscreen_top = vis.iter().rev().find(|k| d.windows[**k].tile == Tile::Fullscreen).copied();
    for wk in vis {
        let first = items.len();
        let w = &d.windows[wk];
        let Some(frame) = wm::frame_rect(d, wk) else { continue };
        if w.tile != Tile::Fullscreen {
            if w.ssd && w.tile == Tile::Floating {
                items.push(Item::new(frame.inflate(shell::SHADOW), None, Elem::Shadow(wk)));
            }
            if w.ssd {
                // Below the rounded top corners only.
                let r = shell::window_radius(d, wk);
                let title = Rect::new(frame.x, frame.y + r, frame.w, (shell::TITLE_H + shell::BORDER - r).max(0));
                items.push(Item::new(frame, Some(title), Elem::Deco(wk)));
            }
        }
        let r = shell::window_radius(d, wk);
        let round = (r > 0).then(|| (frame.inflate(-shell::BORDER), r - shell::BORDER));
        // The desktop's menus show over every window (below).
        push_tree_p(d, w.surf, &mut items, round, !w.desktop());
        for it in &mut items[first..] {
            it.win = Some(wk);
        }
    }
    for wk in drawn_stack(d, morphs).into_iter().filter(|k| d.windows[*k].desktop()) {
        let first = items.len();
        for p in d.surfaces.get(d.windows[wk].surf).map(|s| s.popups.clone()).unwrap_or_default() {
            if d.surfaces.get(p).map_or(false, |p| p.mapped) {
                push_tree(d, p, &mut items, None);
            }
        }
        for it in &mut items[first..] {
            it.win = Some(wk);
        }
    }
    // The taskbar hides under a fullscreen window on this output.
    let tb_hidden = fullscreen_top.map_or(false, |k| d.windows[k].fullscreen_output == Some(oi) && !shell::taskbar_wanted(d, oi)) || !shell::has_taskbar(d, oi) || d.overlay.tb_anim.get(oi).is_some_and(|a| a.away.value >= 0.999) || (shell::output_covered(d, oi) && !shell::taskbar_wanted(d, oi));
    if !tb_hidden {
        // The islands' glass shadow reaches above the strip: the item covers it too, or the
        // CPU path would cut the shadow off at the strip's top edge.
        let tb = shell::taskbar_rect(d, oi);
        let up = (crate::glass::margin(o.scale as f32) as f64 / o.scale).ceil() as i32;
        let rect = Rect::from_edges(tb.x, tb.y - up, tb.x1(), tb.y1());
        items.push(Item::new(rect, None, Elem::Taskbar(oi)));
    }
    if let Some(r) = crate::startanim::content_rect(d, oi) {
        items.push(Item::new(r, None, Elem::StartContent));
    }
    items.push(Item::new(orect, None, Elem::Overlays));
    let cr = cursor_rect(d);
    if !cr.is_empty() {
        items.push(Item::new(cr, None, Elem::Cursor));
    }
    // A window flying into its button (or out of it) is drawn where it is now, which may be
    // another monitor than the one it stands on: kept wherever its flight covers.
    let flying: Vec<(WKey, Rect)> = if morphs {
        d.windows.iter().filter(|(_, w)| w.morph.is_some()).filter_map(|(k, _)| crate::winanim::pose(d, k).map(|p| (k, p.bounds()))).collect()
    } else {
        Vec::new()
    };
    items.retain(|i| i.rect.overlaps(&orect) || i.win.map_or(false, |w| flying.iter().any(|(k, b)| *k == w && b.overlaps(&orect))));
    items
}

/// Repaints `damage` (physical rects) of output `oi` into `cv`.
/// Afterwards sends frame callbacks for surfaces shown on this output.
pub fn render_output(d: &mut Desk, oi: usize, cv: &mut Canvas, damage: &Damage) -> Vec<Rect> {
    let rects = match d.gpu.take() {
        Some(mut g) => {
            let r = g.render(d, oi, cv, damage);
            match r {
                Ok(r) => {
                    d.gpu = Some(g);
                    r
                }
                Err(e) => {
                    // A lost device and the like: the CPU takes over for good, from a full frame.
                    eprintln!("resc-desk: render: Vulkan failed ({e}), CPU from now on");
                    drop(g);
                    crate::gpu::renderer_lost();
                    let mut full = Damage::default();
                    full.add(cv.bounds());
                    render_cpu(d, oi, cv, &full)
                }
            }
        }
        None => render_cpu(d, oi, cv, damage),
    };
    d.cursor_drawn = cursor_rect(d);
    frames_done(d, oi);
    rects
}

/// Frame callbacks for every surface on output `oi` (a frame of it went out).
pub fn frames_done(d: &mut Desk, oi: usize) {
    let now = d.now_ms();
    for item in &build_scene_m(d, oi, true) {
        if let Elem::Surface(sk) = item.elem {
            send_frame_done(d, sk, now);
        }
    }
    if let Some(sk) = d.seat.cursor_surface {
        send_frame_done(d, sk, now);
    }
    if let Some((ik, _)) = crate::dnd::icon_rect(d) {
        send_frame_done(d, ik, now);
    }
}

/// Output `oi` shows nothing but one fullscreen window whose picture is a GPU buffer of the
/// output's own size (a game): that buffer can go to the monitor as it is (drm.rs, direct
/// scanout), no frame of ours drawn. Nothing of ours may be over it but the pointer, which
/// then goes on the monitor's cursor plane (no bigger than `cursor_max` px).
/// Direct scanout allowed (`RESC_SCANOUT=0` at start, `ctl scanout off|on` meanwhile).
pub static SCANOUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn scanout_candidate(d: &Desk, oi: usize, cursor_max: (i32, i32)) -> Option<(SKey, std::rc::Rc<crate::shm::Buffer>)> {
    scanout_check(d, oi, cursor_max).ok()
}

/// `scanout_candidate`, or why not (ctl scanout).
pub fn scanout_check(d: &Desk, oi: usize, cursor_max: (i32, i32)) -> Result<(SKey, std::rc::Rc<crate::shm::Buffer>), &'static str> {
    let o = d.outputs.get(oi).ok_or("no output")?;
    let orect = o.rect();
    let items = build_scene_m(d, oi, true);
    // The topmost fullscreen window's own surface here: what is under it does not show.
    let main = items.iter().rposition(|it| match (it.elem, it.win) {
        (Elem::Surface(sk), Some(wk)) => d.windows.get(wk).is_some_and(|w| w.surf == sk && w.tile == Tile::Fullscreen && w.fullscreen_output == Some(oi)),
        _ => false,
    });
    let i = main.ok_or("no fullscreen window here")?;
    for it in &items[i + 1..] {
        match it.elem {
            Elem::Overlays | Elem::Cursor => {}
            Elem::Taskbar(_) => return Err("the taskbar shows"),
            Elem::Surface(o) => {
                return Err(match d.surfaces.get(o) {
                    Some(s) if s.role == Role::Popup => "a menu or popup shows over it",
                    Some(s) if s.parent.is_some() => "a subsurface shows over it",
                    _ => "another window shows over it",
                })
            }
            _ => return Err("something shows over it"),
        }
    }
    let (Elem::Surface(sk), Some(wk), rect) = (items[i].elem, items[i].win, items[i].rect) else { return Err("no window") };
    let w = d.windows.get(wk).ok_or("not a window")?;
    if w.morph.is_some() {
        return Err("the window is animating");
    }
    if rect != orect {
        return Err("the window does not cover the monitor exactly");
    }
    let s = d.surfaces.get(sk).ok_or("no surface")?;
    if s.vp_src.is_some() || s.vp_dst.is_some() {
        return Err("the window's picture is scaled (viewport)");
    }
    let (b, _) = s.buffer.as_ref().ok_or("no buffer")?;
    let dm = b.dmabuf.as_ref().ok_or("its buffer is shared memory, not a GPU buffer")?;
    if b.w != o.pw || b.h != o.ph {
        return Err("its buffer is not the monitor's size");
    }
    if dm.y_invert {
        return Err("its buffer is upside down");
    }
    // Ours over it: only the pointer, small enough for the cursor plane.
    let xf = Xf::new(o.x, o.y, o.scale);
    let cur = xf.r(cursor_rect(d));
    if cur.w > cursor_max.0 || cur.h > cursor_max.1 {
        return Err("the pointer is bigger than the cursor plane");
    }
    let ext = layer_extents(d, oi);
    if !ext[0].is_empty() || ext[1].iter().any(|r| !r.intersect(&Rect::new(0, 0, o.pw, o.ph)).is_empty() && !cur.contains_rect(r)) {
        return Err("something of ours shows over it");
    }
    if !shell::glass_panels(d, oi).is_empty() {
        return Err("glass of ours shows over it");
    }
    Ok((sk, b.clone()))
}

/// The pointer (and a dragged icon) as on output `oi`, into `cv` whose (0, 0) is the cursor's
/// top left: for the monitor's cursor plane. Returns its physical rect on the output.
pub fn draw_cursor_plane(d: &Desk, oi: usize, cv: &mut Canvas) -> Rect {
    let o = &d.outputs[oi];
    let xf = Xf::new(o.x, o.y, o.scale);
    let cur = xf.r(cursor_rect(d));
    let b = cv.bounds();
    cv.clear(b);
    if !cur.is_empty() {
        draw_cursor(d, xf.shifted(cur.x, cur.y), cv, b);
    }
    cur
}

/// The CPU path: everything, glass included, into `cv`.
fn render_cpu(d: &Desk, oi: usize, cv: &mut Canvas, damage: &Damage) -> Vec<Rect> {
    let o = &d.outputs[oi];
    let xf = Xf::new(o.x, o.y, o.scale);
    let items = build_scene(d, oi);
    let phys: Vec<(Rect, Option<Rect>)> = items.iter().map(|i| (xf.r(i.rect), i.opaque.map(|r| xf.r(r)))).collect();
    let rects = widen_for_glass(d, oi, xf, damage, cv.bounds(), &[]);
    for dr in &rects {
        let dr = dr.intersect(&cv.bounds());
        if dr.is_empty() {
            continue;
        }
        let start = phys.iter().rposition(|(_, op)| op.map_or(false, |op| op.contains_rect(&dr))).unwrap_or(0);
        for (i, item) in items.iter().enumerate().skip(start) {
            let (pr, _) = phys[i];
            let clip = pr.intersect(&dr);
            if clip.is_empty() {
                continue;
            }
            draw_item(d, oi, xf, cv, clip, pr, item);
        }
    }
    rects
}

/// The GPU path's CPU layers: what the shell draws over the windows, transparent where
/// nothing is. 0: the taskbar's content (on its glass); 1: overlays (start menu, clipboard,
/// Alt+Tab content, snap preview, calibration) and the cursor. Each is redrawn only in its
/// own rects of `damage[layer]`; glass is the GPU's.
pub fn render_layers(d: &Desk, oi: usize, layers: &mut [Canvas; 2], damage: &[Vec<Rect>; 2]) {
    let o = &d.outputs[oi];
    let xf = Xf::new(o.x, o.y, o.scale);
    let items = build_scene(d, oi);
    shell::without_glass(|| {
        for (li, cv) in layers.iter_mut().enumerate() {
            for dr in &damage[li] {
                let dr = dr.intersect(&cv.bounds());
                if dr.is_empty() {
                    continue;
                }
                cv.clear(dr);
                for item in &items {
                    if item.elem.gpu_layer() != Some(li) {
                        continue;
                    }
                    let pr = xf.r(item.rect);
                    let clip = pr.intersect(&dr);
                    if !clip.is_empty() {
                        draw_item(d, oi, xf, cv, clip, pr, item);
                    }
                }
            }
        }
    });
}

/// Where each GPU layer has content now (physical): only there does it need clearing and
/// uploading. The taskbar strip; the overlays' panels, snap preview, calibration screen and
/// cursor.
pub fn layer_extents(d: &Desk, oi: usize) -> [Vec<Rect>; 2] {
    let o = &d.outputs[oi];
    let xf = Xf::new(o.x, o.y, o.scale);
    let mut ext: [Vec<Rect>; 2] = [Vec::new(), Vec::new()];
    for item in build_scene(d, oi) {
        match item.elem {
            Elem::Taskbar(_) => ext[0].push(xf.r(item.rect)),
            Elem::Cursor => ext[1].push(xf.r(item.rect)),
            _ => {}
        }
    }
    if d.calib.is_some() || crate::snip::active(d) {
        ext[1].push(xf.r(o.rect()));
    }
    if let Some(r) = crate::osd::rect(d).filter(|r| r.overlaps(&o.rect())) {
        ext[1].push(xf.r(r));
    }
    if let Some(r) = d.overlay.snap_preview {
        ext[1].push(xf.r(r).inflate(2));
    }
    // The drops' heads and icons (windows to and from the taskbar, the preview card).
    for r in crate::winanim::layer_rects(d, oi).into_iter().chain(crate::life::layer_rects(d, oi)).chain(crate::alttab::bounds(d, oi)) {
        ext[1].push(xf.r(r));
    }
    for (r, _, layer) in shell::glass_panels(d, oi) {
        if layer == 2 {
            ext[1].push(xf.r(r).inflate(2));
        }
    }
    // The start-up animation covers the whole output.
    if crate::boot::active(d) {
        ext[1].push(xf.r(o.rect()));
    }
    // «Пуск»: its content in layer 0, what is on its drops in layer 1 (startanim.rs).
    if let Some(r) = crate::startanim::content_rect(d, oi) {
        ext[0].push(xf.r(r));
        ext[1].push(xf.r(r));
    }
    ext
}

/// What the GPU draws for a window, bottom to top (scene order).
pub enum GpuItem {
    /// Server-side frame and shadow of a window (one texture, the CPU draws it).
    Frame(WKey),
    /// A client surface: its rect on the output (logical), the window's rounded clip, and
    /// the window (moving with it when it grows or shrinks, winanim.rs).
    Surface(SKey, Rect, Option<(Rect, i32)>, Option<WKey>),
}

/// The windows part of the scene for the GPU: frames and surfaces, bottom to top.
pub fn gpu_items(d: &Desk, oi: usize) -> Vec<GpuItem> {
    let mut v = Vec::new();
    for item in build_scene_m(d, oi, true) {
        match item.elem {
            // Shadow comes first when there is one; Deco alone otherwise: one frame texture.
            Elem::Shadow(wk) => v.push(GpuItem::Frame(wk)),
            Elem::Deco(wk) => {
                if !matches!(v.last(), Some(GpuItem::Frame(k)) if *k == wk) {
                    v.push(GpuItem::Frame(wk));
                }
            }
            Elem::Surface(sk) => v.push(GpuItem::Surface(sk, item.rect, item.round, item.win)),
            _ => {}
        }
    }
    v
}

/// A window's surfaces with their rects (logical, as at rest), bottom to top: to draw it
/// somewhere else too (the preview card's content, gpu/mod.rs).
pub fn window_surfaces(d: &Desk, wk: WKey) -> Vec<(SKey, Rect)> {
    let mut items = Vec::new();
    if let Some(w) = d.windows.get(wk) {
        push_tree(d, w.surf, &mut items, None);
    }
    items
        .into_iter()
        .filter_map(|i| match i.elem {
            Elem::Surface(sk) => Some((sk, i.rect)),
            _ => None,
        })
        .collect()
}

/// A window's frame and shadow into `cv`, whose (0, 0) is physical (`dx`, `dy`) on the
/// output `xf` belongs to (the GPU's frame texture).
pub fn draw_frame(d: &Desk, wk: WKey, xf: Xf, cv: &mut Canvas) {
    let all = cv.bounds();
    cv.clear(all);
    let w = &d.windows[wk];
    if w.ssd && w.tile == Tile::Floating {
        shell::draw_shadow(d, wk, xf, cv, all);
    }
    if w.ssd {
        shell::draw_deco(d, wk, xf, cv, all, crate::glass::frosted());
    }
}

/// The logical rect a window's frame texture covers: the frame, plus the shadow when it
/// has one.
pub fn frame_extent(d: &Desk, wk: WKey) -> Option<Rect> {
    let w = d.windows.get(wk)?;
    let f = wm::frame_rect(d, wk)?;
    Some(if w.ssd && w.tile == Tile::Floating { f.inflate(shell::SHADOW) } else { f })
}

/// Glass blurs its surroundings: any damage touching a glass panel grows to the
/// panel plus the blur margin, and overlapping rects merge, so every rect is drawn
/// bottom-to-top before the glass in it reads the pixels below. `extra`: more glass
/// (physical), each with up to two rects its own window's opaque content covers: damage
/// inside one of those (the window's own drawing) shows none of that glass and leaves it be.
pub fn widen_for_glass(d: &Desk, oi: usize, xf: Xf, damage: &Damage, bounds: Rect, extra: &[(Rect, [Rect; 2])]) -> Vec<Rect> {
    let mut rects: Vec<Rect> = damage.rects.iter().map(|r| r.intersect(&bounds)).filter(|r| !r.is_empty()).collect();
    let m = crate::glass::margin(xf.scale as f32);
    let zones: Vec<(Rect, [Rect; 2])> = shell::glass_rects(d, oi)
        .iter()
        .map(|r| (xf.r(*r), [Rect::default(); 2]))
        .chain(extra.iter().copied())
        .map(|(r, cover)| (r.inflate(m).intersect(&bounds), cover))
        .collect();
    if zones.is_empty() {
        return rects;
    }
    loop {
        let mut changed = false;
        for r in rects.iter_mut() {
            for (z, cover) in &zones {
                if r.overlaps(z) && !r.contains_rect(z) && !cover.iter().any(|c| c.contains_rect(r)) {
                    *r = r.union(z);
                    changed = true;
                }
            }
        }
        'merge: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if rects[i].overlaps(&rects[j]) {
                    rects[i] = rects[i].union(&rects[j]);
                    rects.swap_remove(j);
                    changed = true;
                    break 'merge;
                }
            }
        }
        if !changed {
            return rects;
        }
    }
}

pub fn send_frame_done(d: &mut Desk, sk: SKey, now: u32) {
    let Some(s) = d.surfaces.get_mut(sk) else { return };
    if s.frame_cbs.is_empty() {
        return;
    }
    let cbs = std::mem::take(&mut s.frame_cbs);
    let ck = s.client;
    let Some(c) = d.clients.get_mut(ck) else { return };
    for cb in cbs {
        if c.obj(cb).is_some() {
            wl_callback::evt::done(&mut c.conn, cb, now);
            c.remove(cb);
            wl_display::evt::delete_id(&mut c.conn, 1, cb);
        }
    }
}

fn draw_item(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect, pr: Rect, item: &Item) {
    match item.elem {
        Elem::Background => shell::draw_background(d, oi, xf, cv, clip),
        Elem::Shadow(wk) => shell::draw_shadow(d, wk, xf, cv, clip),
        Elem::Surface(sk) => match item.round {
            Some((r, rad)) => rounded(cv, clip, xf.r(r), rad as f32 * xf.scale as f32, |cv| draw_surface(d, sk, cv, clip, pr)),
            None => draw_surface(d, sk, cv, clip, pr),
        },
        Elem::Deco(wk) => shell::draw_deco(d, wk, xf, cv, clip, false),
        Elem::Taskbar(o) => shell::draw_taskbar(d, o, xf, cv, clip),
        Elem::StartContent => crate::startanim::draw_layer0(d, oi, xf, cv, clip),
        Elem::Overlays => shell::draw_overlays(d, oi, xf, cv, clip),
        Elem::Cursor => draw_cursor(d, xf, cv, clip),
    }
}

/// Runs `f` (which draws into `cv`) clipped to the rounded rect `r` (physical) at its
/// corners: the corner pixels are kept before and mixed back by coverage after.
fn rounded(cv: &mut Canvas, clip: Rect, r: Rect, rad: f32, f: impl FnOnce(&mut Canvas)) {
    let n = rad.ceil() as i32 + 1;
    let corners = [
        Rect::new(r.x, r.y, n, n),
        Rect::new(r.x1() - n, r.y, n, n),
        Rect::new(r.x, r.y1() - n, n, n),
        Rect::new(r.x1() - n, r.y1() - n, n, n),
    ]
    .map(|c| c.intersect(&clip).intersect(&cv.bounds()));
    let mut saved: Vec<u32> = Vec::new();
    for c in &corners {
        for y in c.y..c.y1() {
            let o = y as usize * cv.stride;
            saved.extend_from_slice(&cv.px[o + c.x as usize..o + c.x1() as usize]);
        }
    }
    f(cv);
    let mut i = 0;
    for c in &corners {
        for y in c.y..c.y1() {
            for x in c.x..c.x1() {
                let cov = (0.5 - rrect_sdf(x as f32 + 0.5, y as f32 + 0.5, r, rad)).clamp(0.0, 1.0);
                let old = saved[i];
                i += 1;
                if cov >= 1.0 {
                    continue;
                }
                let p = &mut cv.px[y as usize * cv.stride + x as usize];
                *p = lerp(old, *p, (cov * 255.0) as u32);
            }
        }
    }
}

/// The part of the buffer a surface shows (buffer pixels): the viewport's source, or all.
pub fn surface_src(s: &Surface, b: &crate::shm::Buffer) -> Rect {
    let sc = s.scale.max(1);
    match s.vp_src {
        Some((x, y, w, h)) => Rect::new((x * sc as f64) as i32, (y * sc as f64) as i32, (w * sc as f64) as i32, (h * sc as f64) as i32)
            .intersect(&Rect::new(0, 0, b.w, b.h)),
        None => Rect::new(0, 0, b.w, b.h),
    }
}

fn draw_surface(d: &Desk, sk: SKey, cv: &mut Canvas, clip: Rect, pr: Rect) {
    let s = &d.surfaces[sk];
    let Some((b, _)) = &s.buffer else { return };
    if let Some(c) = b.solid {
        cv.fill(clip, pr, c);
        return;
    }
    let srect = surface_src(s, b);
    let opaque = s.is_opaque();
    b.with_pixels(|px, stride| cv.blit(clip, pr, px, stride, srect, opaque, 255));
}

// ------------------------------------------------------------------ cursor

const ARROW: [&str; 21] = [
    "X           ",
    "XX          ",
    "X.X         ",
    "X..X        ",
    "X...X       ",
    "X....X      ",
    "X.....X     ",
    "X......X    ",
    "X.......X   ",
    "X........X  ",
    "X.........X ",
    "X..........X",
    "X......XXXXX",
    "X...X..X    ",
    "X..XX..X    ",
    "X.X  X..X   ",
    "XX   X..X   ",
    "X     X..X  ",
    "      X..X  ",
    "       XX   ",
    "            ",
];

pub fn cursor_rect(d: &Desk) -> Rect {
    let r = pointer_rect(d);
    match crate::dnd::icon_rect(d) {
        Some((_, ir)) if !r.is_empty() => r.union(&ir),
        Some((_, ir)) => ir,
        None => r,
    }
}

fn pointer_rect(d: &Desk) -> Rect {
    if d.seat.cursor_hidden || d.outputs.is_empty() || crate::snip::hides_cursor(d) {
        return Rect::default();
    }
    let (x, y) = (d.seat.x as i32, d.seat.y as i32);
    if let Some(sk) = d.seat.cursor_surface {
        if let Some(s) = d.surfaces.get(sk) {
            if s.buffer.is_some() {
                let (hx, hy) = d.seat.cursor_hotspot;
                return Rect::new(x - hx, y - hy, s.w, s.h);
            }
        }
        return Rect::default();
    }
    // The theme's cursor at the scale of the output it is on.
    let s = d.output_at(d.seat.x, d.seat.y).and_then(|o| d.outputs.get(o)).map_or(1.0, |o| o.scale);
    if let Some((img, k)) = theme_cursor(d.seat.cursor_shape, s) {
        let (w, h) = ((img.w as f64 * k / s).ceil() as i32, (img.h as f64 * k / s).ceil() as i32);
        let (hx, hy) = ((img.xhot as f64 * k / s).round() as i32, (img.yhot as f64 * k / s).round() as i32);
        return Rect::new(x - hx, y - hy, w + 1, h + 1);
    }
    shell::cursor_shape_rect(d.seat.cursor_shape, x, y)
}

/// The theme's image for `shape` on an output at `scale`, and how much to scale it (its
/// physical pixels per image pixel).
fn theme_cursor(shape: u32, scale: f64) -> Option<(std::rc::Rc<crate::xcursor::Image>, f64)> {
    let want = (crate::xcursor::base_size() as f64 * scale).round() as i32;
    let img = crate::xcursor::get(shape, want).or_else(|| crate::xcursor::get(1, want))?;
    // A theme's images come in steps (24, 32, 48…): the nearest drawn at the size wanted,
    // by the size it is made for (not its pixels: Windows' 32 px images are size 24).
    let nominal = img.nominal.max(1) as f64;
    let k = if (nominal - want as f64).abs() <= want as f64 * 0.2 { 1.0 } else { want as f64 / nominal };
    Some((img, k))
}

fn draw_cursor(d: &Desk, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if let Some((ik, ir)) = crate::dnd::icon_rect(d) {
        draw_surface(d, ik, cv, clip, xf.r(ir));
    }
    if let Some(sk) = d.seat.cursor_surface {
        let r = xf.r(pointer_rect(d));
        draw_surface(d, sk, cv, clip, r);
        return;
    }
    let (x, y) = (d.seat.x as i32, d.seat.y as i32);
    if let Some((img, k)) = theme_cursor(d.seat.cursor_shape, xf.scale) {
        let (px, py) = (xf.fx(d.seat.x) as f64, xf.fy(d.seat.y) as f64);
        let dst = Rect::new(
            (px - img.xhot as f64 * k).round() as i32,
            (py - img.yhot as f64 * k).round() as i32,
            (img.w as f64 * k).round() as i32,
            (img.h as f64 * k).round() as i32,
        );
        cv.blit(clip, dst, &img.px, img.w as usize, Rect::new(0, 0, img.w, img.h), false, 255);
        return;
    }
    if shell::draw_cursor_shape(d.seat.cursor_shape, x, y, xf, cv, clip) {
        return;
    }
    let s = xf.scale;
    let (px, py) = (xf.fx(x as f64) as i32, xf.fy(y as f64) as i32);
    let cell = s.max(1.0);
    for (ry, row) in ARROW.iter().enumerate() {
        for (rx, ch) in row.bytes().enumerate() {
            let col = match ch {
                b'X' => 0xff00_0000,
                b'.' => 0xffff_ffff,
                _ => continue,
            };
            let r = Rect::new(
                px + (rx as f64 * cell) as i32,
                py + (ry as f64 * cell) as i32,
                cell.ceil() as i32,
                cell.ceil() as i32,
            );
            cv.fill(clip, r, col);
        }
    }
}

/// Full-layout or single-output pixel readback helper used by screenshots.
pub fn render_offscreen(d: &mut Desk, oi: usize) -> (Vec<u32>, i32, i32) {
    let (w, h) = (d.outputs[oi].pw, d.outputs[oi].ph);
    let mut px = vec![0u32; (w * h) as usize];
    let mut cv = Canvas { px: &mut px, w, h, stride: w as usize };
    let mut dmg = Damage::default();
    dmg.add(Rect::new(0, 0, w, h));
    let saved = d.cursor_drawn;
    render_output(d, oi, &mut cv, &dmg);
    d.cursor_drawn = saved;
    (px, w, h)
}
