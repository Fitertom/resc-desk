//! «Пуск» opening and closing as liquid glass, after the approved prototype (`start`): the
//! menu's glass spreads out of the start button (smooth-unioned with the taskbar's hover drop
//! under it, a bridge that swells only while it lets go or lands), and the logo's four squares
//! fly apart on their own arcs: the top two out and up to the search's ends, then flowing
//! toward each other into the search pill (bulging where they meet); the bottom left one
//! sideways along the footer into the avatar; the bottom right one sideways into the corner,
//! where it divides into the gear and the power button. They start as the logo (solid, in its
//! colours) and become glass. The menu's content comes in as a wave from the bottom middle up
//! both sides (each thing sliding in toward the middle, growing), lies in the glass under the
//! drops and melts away toward the glass's rim. Closing runs it all back into the logo.
//!
//! Time, not springs: every parameter rides its own cubic bezier over its own part of the
//! 600 ms (winanim.rs `Curve`). Turned around mid-way, each parameter starts back from where
//! it is.
//!
//! Layers (the GPU path; the CPU path does not animate, start.rs draws the menu whole): the
//! menu's glass under CPU layer 0 (`drops`, layer 1), its content in CPU layer 0
//! (`draw_layer0`), the drops on it under CPU layer 1 (`drops`, layer 2), what is on the drops
//! (search, glyphs) in CPU layer 1 (`draw_layer1`), each shown only inside its drops.
use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{blend, fade, Canvas, Xf};
use crate::shell::{self, col};
use crate::start;
use crate::winanim::{c, Curve};

const DUR: f32 = 0.6;
/// The bridge to the hover drop, the search's merge, the buttons' division: widest k (px).
const KMAX: f32 = 50.0;
const KS: f32 = 30.0;
const KP: f32 = 24.0;
/// How far the top squares swing out past the search's ends (px).
const ARC: f32 = 70.0;
/// The content's wave: the share of the time it takes from the first thing to the last, how
/// far each slides in, the size it grows from; the wave's arc (its width, its centre's height
/// from the menu's top).
const SPREAD: f32 = 0.3;
const SLIDE: f32 = 36.0;
const SCALE0: f32 = 0.85;
const ASPECT: f32 = 0.6;
const WAVE_CY: f32 = 300.0;
/// The content melts away over this much of the glass's edge (px, past its first 2).
const FEATHER: f32 = 10.0;
/// What is on the drops fades in over this much of their edge (px).
const GFEATHER: f32 = 8.0;
/// The search's text: soft edge of the merging drops' overlap, its gradient's width and how
/// far ahead of the overlap's ends it starts (px).
const FRONT: f32 = 1.5;
const GRAD_W: f32 = 24.0;
const GRAD_LEAD: f32 = 12.0;

// The parameters: each 0 in the button, 1 in the menu.
const EDGES: usize = 0;
const RADIUS: usize = 1;
const NECK: usize = 2;
const S_FLY: usize = 3;
const S_GROW: usize = 4;
const U_FLY: usize = 5;
const P_FLY: usize = 6;
const P_SPLIT: usize = 7;
const LOOK: usize = 8;
const GLYPHS: usize = 9;
const NP: usize = 10;

/// Opening (the prototype's defaults).
const OPEN: [Curve; NP] = [
    c(0.25, 0.1, 0.25, 1.04, 0.0, 0.85),
    c(0.3, 0.0, 0.3, 1.0, 0.0, 0.7),
    c(0.25, 0.0, 0.4, 1.0, 0.0, 0.55),
    c(0.3, 0.0, 0.25, 1.04, 0.03, 0.62),
    c(0.35, 0.0, 0.25, 1.0, 0.42, 0.9),
    c(0.3, 0.0, 0.25, 1.06, 0.03, 0.6),
    c(0.3, 0.0, 0.25, 1.0, 0.03, 0.55),
    c(0.3, 0.0, 0.3, 1.15, 0.42, 0.86),
    c(0.4, 0.0, 0.4, 1.0, 0.15, 0.65),
    c(0.3, 0.0, 0.3, 1.0, 0.4, 0.8),
];
/// Closing (progress toward the button): the splits first, everything back into the logo.
const CLOSE: [Curve; NP] = [
    c(0.45, 0.0, 0.7, 1.0, 0.1, 1.0),
    c(0.4, 0.0, 0.6, 1.0, 0.3, 0.9),
    c(0.4, 0.0, 0.6, 1.0, 0.5, 1.0),
    c(0.4, 0.0, 0.6, 1.0, 0.3, 0.8),
    c(0.4, 0.0, 0.6, 1.0, 0.05, 0.45),
    c(0.4, 0.0, 0.6, 1.0, 0.2, 0.8),
    c(0.4, 0.0, 0.6, 1.0, 0.3, 0.8),
    c(0.4, 0.0, 0.6, 1.0, 0.05, 0.45),
    c(0.4, 0.0, 0.6, 1.0, 0.5, 0.95),
    c(0.3, 0.0, 0.6, 1.0, 0.0, 0.25),
];
/// Each thing of the content (shifted along the wave).
const OPEN_CONTENT: Curve = c(0.3, 0.0, 0.2, 1.06, 0.3, 0.8);
const CLOSE_CONTENT: Curve = c(0.3, 0.0, 0.6, 1.0, 0.0, 0.35);

/// The drops' glass (premultiplied): lighter than the menu's.
const CHIP_TINT: [f32; 4] = [0.07, 0.07, 0.075, 0.07];
const CHIP_BEZEL: f32 = 10.0;
/// Shadows: strength, sigma, drop, reach (logical px).
const PANEL_SH: [f32; 4] = [crate::glass::SHADOW * 0.6, 20.0, 6.0, 54.0];
const CHIP_SH: [f32; 4] = [0.22, 5.0, 2.0, 14.0];
const WHITE: u32 = 0xfff4_f0ff;

#[derive(Clone, Debug)]
pub struct Anim {
    pub open: bool,
    pub oi: usize,
    /// Seconds so far (the shell's clock: slow motion slows it).
    pub t: f32,
    /// Where each parameter and each thing of the content started (turned around mid-way).
    from: [f32; NP],
    from_items: Vec<f32>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| lerp(a[i], b[i], t))
}

fn bx(cx: f32, cy: f32, w: f32, h: f32) -> [f32; 4] {
    [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0]
}

/// 0 at both ends, 1 half way: a bridge or a bulge that is there only while things move.
fn bump(n: f32) -> f32 {
    let n = n.clamp(0.0, 1.0);
    4.0 * n * (1.0 - n)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    lerp(b, a, h) - k * h * (1.0 - h)
}

/// Signed distance to a rounded box (x0, y0, x1, y1), negative inside.
fn sd_rr(px: f32, py: f32, r: [f32; 4], rad: f32) -> f32 {
    let (hw, hh) = (((r[2] - r[0]) * 0.5).max(0.0), ((r[3] - r[1]) * 0.5).max(0.0));
    let rad = rad.min(hw).min(hh).max(0.0);
    let qx = (px - (r[0] + hw)).abs() - hw + rad;
    let qy = (py - (r[1] + hh)).abs() - hh + rad;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad
}

/// Along a cubic from `q`'s centre to `end` (control points `c1`, `c2`), growing from `q`'s size
/// to `w` × `h`; past the end (overshoot) on along the curve's last direction.
fn fly(q: [f32; 4], c1: [f32; 2], c2: [f32; 2], end: [f32; 2], w: f32, h: f32, t: f32) -> [f32; 4] {
    let a = [(q[0] + q[2]) / 2.0, (q[1] + q[3]) / 2.0];
    let tt = t.clamp(0.0, 1.0);
    let s = 1.0 - tt;
    let at = |i: usize| s * s * s * a[i] + 3.0 * s * s * tt * c1[i] + 3.0 * s * tt * tt * c2[i] + tt * tt * tt * end[i];
    let o = t - tt;
    bx(at(0) + (end[0] - c2[0]) * 3.0 * o, at(1) + (end[1] - c2[1]) * 3.0 * o, lerp(q[2] - q[0], w, tt), lerp(q[3] - q[1], h, tt))
}

fn e(a: &Anim) -> f32 {
    (a.t / DUR).clamp(0.0, 1.0)
}

fn params(a: Option<&Anim>) -> [f32; NP] {
    let Some(a) = a else { return [1.0; NP] };
    let e = e(a);
    std::array::from_fn(|i| if a.open { lerp(a.from[i], 1.0, OPEN[i].at(e)) } else { lerp(a.from[i], 0.0, CLOSE[i].at(e)) })
}

/// How far thing `i` of the content is in (0..1, may overshoot), `d` its place on the wave.
fn item_in(a: Option<&Anim>, i: usize, d: f32) -> f32 {
    let Some(a) = a else { return 1.0 };
    let (e, span) = (e(a), 1.0 - SPREAD);
    let from = a.from_items.get(i).copied().unwrap_or(if a.open { 0.0 } else { 1.0 });
    if a.open {
        lerp(from, 1.0, OPEN_CONTENT.at((e - d * SPREAD) / span))
    } else {
        // Back the way it came: what came last goes first.
        lerp(from, 0.0, CLOSE_CONTENT.at((e - (1.0 - d) * SPREAD) / span))
    }
}

/// A thing's place on the wave (0..1): the angle round the content's centre, 0 below, ½ at
/// the sides, 1 above.
fn order(f: Rect, r: Rect) -> f32 {
    let dx = (r.x as f32 + r.w as f32 / 2.0 - (f.x as f32 + f.w as f32 / 2.0)) * ASPECT;
    let dy = r.y as f32 + r.h as f32 / 2.0 - (f.y as f32 + WAVE_CY);
    dx.abs().atan2(dy) / std::f32::consts::PI
}

fn anim_on(d: &Desk, oi: usize) -> Option<&Anim> {
    d.start.anim.as_ref().filter(|a| a.oi == oi)
}

/// The output the menu is drawn on: open, or still closing.
pub fn shown(d: &Desk) -> Option<usize> {
    d.overlay.start_menu.or(d.start.anim.as_ref().map(|a| a.oi))
}

/// All the menu may cover on output `oi` while shown (logical): itself, the button, their glass.
pub fn content_rect(d: &Desk, oi: usize) -> Option<Rect> {
    (shown(d) == Some(oi)).then(|| bounds(d, oi))
}

fn bounds(d: &Desk, oi: usize) -> Rect {
    let r = start::rect(d, oi);
    let r = shell::start_button(d, oi).map_or(r, |b| r.union(&b));
    r.inflate(shell::GLASS_REACH)
}

/// The menu starts opening (`open`) or closing on output `oi` (the GPU path; the CPU path
/// shows it at once). Turning around mid-way, it starts back from where it is.
pub fn begin(d: &mut Desk, oi: usize, open: bool) {
    if d.gpu.is_none() || oi >= d.outputs.len() {
        d.start.anim = None;
        return;
    }
    let prev = d.start.anim.take().filter(|a| a.oi == oi);
    let (from, from_items) = match &prev {
        Some(a) => {
            let f = start::rect(d, oi);
            let items = start::wave_items(d, oi).into_iter().enumerate().map(|(i, r)| item_in(Some(a), i, order(f, r))).collect();
            (params(Some(a)), items)
        }
        None => ([if open { 0.0 } else { 1.0 }; NP], Vec::new()),
    };
    d.start.anim = Some(Anim { open, oi, t: 0.0, from, from_items });
    d.overlay.animating = true;
    d.damage(bounds(d, oi));
    shell::damage_taskbars(d);
}

/// Advances the animation; true while it runs.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let Some(a) = d.start.anim.as_mut() else { return false };
    a.t += dt;
    let (oi, done) = (a.oi, a.t >= DUR);
    if done {
        d.start.anim = None;
    }
    if oi < d.outputs.len() {
        d.damage(bounds(d, oi));
    }
    // The logo comes back to the button (or leaves it).
    shell::damage_taskbars(d);
    !done
}

/// The menu and its drops as they are now (logical; boxes are x0, y0, x1, y1).
pub struct Geom {
    pub f: Rect,
    pub btn: Rect,
    pub p: [f32; NP],
    /// The menu's glass and its corners; the bridge to the hover drop.
    pub body: [f32; 4],
    pub radius: f32,
    pub k: f32,
    sx: f32,
    sy: f32,
    /// The search's two drops, their corners, their merge's k; the pill they become.
    pub s: [[f32; 4]; 2],
    pub sr: [f32; 2],
    pub sk: f32,
    pub pill: [f32; 4],
    /// The avatar, its corners, its colour.
    pub u: [f32; 4],
    pub ur: f32,
    pub ucol: u32,
    /// Gear and power, their corners, their division's k.
    pub pw: [[f32; 4]; 2],
    pub pr: f32,
    pub pk: f32,
    /// How much the drops are still the logo's solid squares.
    pub solid: f32,
}

impl Geom {
    /// A point of the menu (as at rest) where it is inside the glass now (stretched with it).
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.body[0] + (x - self.f.x as f32) * self.sx, self.body[1] + (y - self.f.y as f32) * self.sy)
    }
}

pub fn geom(d: &Desk, oi: usize) -> Option<Geom> {
    if oi >= d.outputs.len() {
        return None;
    }
    let f = start::rect(d, oi);
    let btn = shell::start_button(d, oi).unwrap_or(Rect::new(f.x + f.w / 2 - 21, f.y1() + 8, 42, 42));
    let p = params(anim_on(d, oi));
    let (cx, cy) = btn.center();
    let (cx, cy) = (cx as f32, cy as f32);
    let (fx0, fy0, fx1, fy1) = (f.x as f32, f.y as f32, f.x1() as f32, f.y1() as f32);
    // The glass grows out of the logo's middle.
    let pe = p[EDGES];
    let body = [lerp(cx, fx0, pe), lerp(cy, fy0, pe), lerp(cx, fx1, pe), lerp(cy, fy1, pe)];
    let (bw, bh) = ((body[2] - body[0]).max(0.0), (body[3] - body[1]).max(0.0));
    let radius = lerp(bw.min(bh) / 2.0, start::RADIUS as f32, p[RADIUS].clamp(0.0, 1.0));
    let (sx, sy) = (bw / f.w.max(1) as f32, bh / f.h.max(1) as f32);
    let map = |x: f32, y: f32| [body[0] + (x - fx0) * sx, body[1] + (y - fy0) * sy];
    // The logo's squares (shell.rs `draw_logo`).
    let sq = |dx: f32, dy: f32| [cx + dx, cy + dy, cx + dx + 9.0, cy + dy + 9.0];
    let (tl, tr, bl, br) = (sq(-10.0, -10.0), sq(1.0, -10.0), sq(-10.0, 1.0), sq(1.0, 1.0));
    let fcx = (fx0 + fx1) / 2.0;

    // Search: the top squares swing out and up to its ends (drops as tall as it), then flow
    // toward each other and merge into the pill.
    let sr0 = start::search_rect(f);
    let sh = sr0.h as f32;
    let (srx0, srx1, sry) = (sr0.x as f32, sr0.x1() as f32, sr0.y as f32 + sh / 2.0);
    let (sx0, sx1, scy) = (map(srx0, 0.0)[0], map(srx1, 0.0)[0], map(0.0, sry)[1]);
    let mid = (sx0 + sx1) / 2.0;
    let (el, er) = ([(sx0 + sh / 2.0).min(mid), scy], [(sx1 - sh / 2.0).max(mid), scy]);
    let mut s0 = fly(tl, map(fcx - 30.0, fy1 - 140.0), map(srx0 + sh / 2.0 - ARC, fy0 + 300.0), el, sh, sh, p[S_FLY]);
    let mut s1 = fly(tr, map(fcx + 30.0, fy1 - 140.0), map(srx1 - sh / 2.0 + ARC, fy0 + 300.0), er, sh, sh, p[S_FLY]);
    s0[2] = lerp(s0[2], s0[2].max(sx1), p[S_GROW]);
    s1[0] = lerp(s1[0], s1[0].min(sx0), p[S_GROW]);
    let fl = p[S_FLY].clamp(0.0, 1.0);
    let sr = [lerp(3.0, sh / 2.0, fl), lerp(4.0, sh / 2.0, fl)];

    // Avatar: the bottom left square up to the footer and along it.
    let av = start::avatar_rect(f);
    let ay0 = av.y as f32 + av.h as f32 / 2.0;
    let [ax, ay] = map(av.x as f32 + av.w as f32 / 2.0, ay0);
    let u = fly(bl, map(fcx - 40.0, ay0), map(lerp(fcx, av.x as f32, 0.55), ay0), [ax, ay], av.w as f32, av.h as f32, p[U_FLY]);
    let ul = p[U_FLY].clamp(0.0, 1.0);

    // Gear and power: the bottom right square to the corner as one drop, then dividing.
    let (gr, pwr) = (start::gear_btn(f), start::power_btn(f));
    let gy0 = gr.y as f32 + gr.h as f32 / 2.0;
    let [gx, gy] = map(gr.x as f32 + gr.w as f32 / 2.0, gy0);
    let px = map(pwr.x as f32 + pwr.w as f32 / 2.0, gy0)[0];
    let pmx = (gr.x as f32 + pwr.x1() as f32) / 2.0;
    let bs = gr.w as f32;
    let pm = fly(br, map(fcx + 40.0, gy0), map(lerp(fcx, pmx, 0.55), gy0), [(gx + px) / 2.0, gy], bs, bs, p[P_FLY]);
    let ps = p[P_SPLIT];

    let bl_col = crate::render::lerp(WHITE, col::accent(), 110);
    Some(Geom {
        f,
        btn,
        p,
        body,
        radius,
        k: KMAX * bump(p[NECK]),
        sx,
        sy,
        s: [s0, s1],
        sr,
        sk: KS * bump(p[S_GROW]),
        pill: [sx0, scy - sh / 2.0, sx1, scy + sh / 2.0],
        u,
        ur: lerp(3.0, av.w as f32 / 2.0, ul),
        ucol: crate::render::lerp(bl_col, col::accent(), (ul * 255.0) as u32),
        pw: [lerp4(pm, bx(gx, gy, bs, bs), ps), lerp4(pm, bx(px, gy, bs, bs), ps)],
        pr: lerp(3.0, bs / 2.0, p[P_FLY].clamp(0.0, 1.0)),
        pk: KP * bump(ps),
        solid: 1.0 - p[LOOK].clamp(0.0, 1.0),
    })
}

/// One drop of the menu for the GPU (drop.frag, gpu/mod.rs `start_op`), logical px.
pub struct DropSpec {
    pub a: [f32; 4],
    pub ra: f32,
    /// A second shape smooth-unioned with it by `k` (the search's other half, the power button).
    pub b: Option<([f32; 4], f32)>,
    pub k: f32,
    /// Premultiplied tints of A and B.
    pub tint: [[f32; 4]; 2],
    pub bezel: f32,
    /// Shadow: strength, sigma, drop, reach.
    pub sh: [f32; 4],
    /// 1: under CPU layer 0 (the menu's glass); 2: under CPU layer 1 (the drops on it).
    pub layer: usize,
    /// The start button: it bridges (by `k`) to the taskbar's hover drop there, if any.
    pub bridge: Option<Rect>,
}

fn premul(c: u32) -> [f32; 4] {
    [(c >> 16) & 255, (c >> 8) & 255, c & 255, c >> 24].map(|v| v as f32 / 255.0)
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    lerp4(a, b, t)
}

/// The menu's drops on output `oi` (the GPU path).
pub fn drops(d: &Desk, oi: usize) -> Vec<DropSpec> {
    let mut v = Vec::new();
    if shown(d) != Some(oi) {
        return v;
    }
    let Some(g) = geom(d, oi) else { return v };
    if g.body[2] - g.body[0] >= 1.0 && g.body[3] - g.body[1] >= 1.0 {
        v.push(DropSpec {
            a: g.body,
            ra: g.radius,
            b: None,
            k: g.k,
            tint: [crate::glass::TINT_DEEP; 2],
            bezel: crate::glass::bezel(),
            sh: PANEL_SH,
            layer: 1,
            bridge: Some(g.btn),
        });
    }
    let chip = |c: u32| mix4(CHIP_TINT, premul(c), g.solid);
    let tr = crate::render::lerp(WHITE, col::accent(), 110);
    let br = crate::render::lerp(WHITE, col::accent(), 200);
    v.push(DropSpec { a: g.s[0], ra: g.sr[0], b: Some((g.s[1], g.sr[1])), k: g.sk, tint: [chip(WHITE), chip(tr)], bezel: CHIP_BEZEL, sh: CHIP_SH, layer: 2, bridge: None });
    v.push(DropSpec { a: g.pw[0], ra: g.pr, b: Some((g.pw[1], g.pr)), k: g.pk, tint: [chip(br); 2], bezel: CHIP_BEZEL, sh: CHIP_SH, layer: 2, bridge: None });
    // The avatar stays solid: the logo's colour going to the accent.
    v.push(DropSpec { a: g.u, ra: g.ur, b: None, k: 0.0, tint: [premul(g.ucol); 2], bezel: CHIP_BEZEL * 0.8, sh: CHIP_SH, layer: 2, bridge: None });
    v
}

fn phys4(xf: Xf, r: [f32; 4]) -> [f32; 4] {
    [xf.fx(r[0] as f64) as f32, xf.fy(r[1] as f64) as f32, xf.fx(r[2] as f64) as f32, xf.fy(r[3] as f64) as f32]
}

/// A box (logical) to whole pixels.
fn rect_of(r: [f32; 4]) -> Rect {
    Rect::from_edges(r[0].round() as i32, r[1].round() as i32, r[2].round() as i32, r[3].round() as i32)
}

/// Draws something aside, on a canvas of its own covering `area` (physical).
fn aside(area: Rect, xf: Xf, f: impl FnOnce(&mut Canvas, Xf)) -> Vec<u32> {
    let mut buf = vec![0u32; (area.w.max(0) * area.h.max(0)) as usize];
    if !buf.is_empty() {
        let mut off = Canvas { px: &mut buf, w: area.w, h: area.h, stride: area.w as usize };
        f(&mut off, xf.shifted(area.x, area.y));
    }
    buf
}

/// Lays `buf` (drawn aside over `area`) onto `cv` within `clip`, each pixel at `mask` (0..1) of
/// its pixel centre (physical).
fn lay(cv: &mut Canvas, clip: Rect, area: Rect, buf: &[u32], mask: impl Fn(f32, f32) -> f32) {
    let c = area.intersect(&clip).intersect(&cv.bounds());
    for y in c.y..c.y1() {
        let row = ((y - area.y) * area.w) as usize;
        for x in c.x..c.x1() {
            let s = buf[row + (x - area.x) as usize];
            if s == 0 {
                continue;
            }
            let m = mask(x as f32 + 0.5, y as f32 + 0.5);
            if m <= 0.0 {
                continue;
            }
            let a = (m.min(1.0) * 255.0 + 0.5) as u32;
            let i = y as usize * cv.stride + x as usize;
            cv.px[i] = blend(cv.px[i], if a >= 255 { s } else { fade(s, a) });
        }
    }
}

/// CPU layer 0: the menu's content, in its glass (under the drops).
pub fn draw_layer0(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if !shell::glass_on_gpu() || shown(d) != Some(oi) {
        return;
    }
    let Some(g) = geom(d, oi) else { return };
    let Some(a) = anim_on(d, oi) else {
        start::draw_content(d, oi, xf, cv, clip);
        let av = start::avatar_rect(g.f);
        start::draw_user(cv, clip, xf, av.x1() + 12, av.y + 20);
        return;
    };
    let s = xf.scale as f32;
    let bp = phys4(xf, g.body);
    if bp[2] - bp[0] < 2.0 || bp[3] - bp[1] < 2.0 {
        return;
    }
    let rad = g.radius * s;
    let inside = |x: f32, y: f32| {
        let sd = sd_rr(x, y, bp, rad);
        (0.5 - sd).clamp(0.0, 1.0) * smoothstep(2.0 * s, (2.0 + FEATHER) * s, -sd)
    };
    // The content at rest, aside; then each thing where the wave has it now: stretched with the
    // glass, slid toward the middle, grown about its centre, at its opacity.
    let fp = xf.r(g.f);
    let rest = aside(fp, xf, |off, lxf| {
        let all = off.bounds();
        start::draw_content(d, oi, lxf, off, all);
    });
    let area = Rect::from_edges(bp[0].floor() as i32, bp[1].floor() as i32, bp[2].ceil() as i32, bp[3].ceil() as i32);
    let fcx = g.f.x as f32 + g.f.w as f32 / 2.0;
    let moved = aside(area, xf, |off, _| {
        let all = off.bounds();
        for (i, it) in start::wave_items(d, oi).into_iter().enumerate() {
            let t = item_in(Some(a), i, order(g.f, it));
            let al = t.clamp(0.0, 1.0);
            if al <= 0.005 {
                continue;
            }
            let src = xf.r(it).translate(-fp.x, -fp.y).intersect(&Rect::new(0, 0, fp.w, fp.h));
            if src.is_empty() {
                continue;
            }
            let (icx, icy) = (it.x as f32 + it.w as f32 / 2.0, it.y as f32 + it.h as f32 / 2.0);
            let side = if (icx - fcx).abs() < 30.0 { 0.0 } else { (icx - fcx).signum() };
            let k = lerp(SCALE0, 1.0, t);
            let (hw, hh, mx) = (it.w as f32 * k / 2.0, it.h as f32 * k / 2.0, icx + side * (1.0 - t) * SLIDE);
            let (x0, y0) = g.map(mx - hw, icy - hh);
            let (x1, y1) = g.map(mx + hw, icy + hh);
            let px = |x: f32| xf.fx(x as f64).round() as i32 - area.x;
            let py = |y: f32| xf.fy(y as f64).round() as i32 - area.y;
            let dst = Rect::from_edges(px(x0), py(y0), px(x1).max(px(x0) + 1), py(y1).max(py(y0) + 1));
            off.blit(all, dst, &rest, fp.w as usize, src, false, (al * 255.0).round() as u32);
        }
    });
    lay(cv, clip, area, &moved, inside);

    // The user's name comes out from behind the avatar once it has landed (nothing of it left
    // of the avatar's middle).
    let gl = g.p[GLYPHS];
    if gl > 0.01 {
        let name = start::user_name();
        let w = font::text_width_w(&name, 13.0, font::W::Semi) as f32;
        let (ucx, ucy) = ((g.u[0] + g.u[2]) / 2.0, (g.u[1] + g.u[3]) / 2.0);
        let x = lerp(ucx - w, g.u[2] + 12.0, gl);
        let ta = xf.r(Rect::new(x.floor() as i32 - 2, (ucy - 14.0) as i32, w as i32 + 8, 24));
        let buf = aside(ta, xf, |off, lxf| {
            let all = off.bounds();
            start::draw_user(off, all, lxf, x.round() as i32, (ucy + 5.0).round() as i32);
        });
        let (cut, al) = (xf.fx(ucx as f64) as f32, gl.clamp(0.0, 1.0));
        lay(cv, clip, ta, &buf, |x, y| if x < cut { 0.0 } else { al * inside(x, y) });
    }
}

/// CPU layer 1: what is on the drops (the search, the footer's glyphs), seen only inside
/// them; the power menu.
pub fn draw_layer1(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(g) = geom(d, oi) else { return };
    if anim_on(d, oi).is_none() {
        start::draw_search(d, start::search_rect(g.f), xf, cv, clip);
        start::draw_initial(cv, clip, xf, start::avatar_rect(g.f));
        start::draw_footer_buttons(d, oi, xf, cv, clip, false);
        start::draw_power_menu(d, oi, xf, cv, clip);
        return;
    }
    let s = xf.scale as f32;
    // The search's text, where the pill will be: uncovered by its two drops where they already
    // overlap (the merge's bulging "sausage", its edges), fading in from the overlap's ends; a
    // side that has reached the pill's end has no fade left.
    let sb = rect_of(g.pill);
    let area = xf.r(sb).inflate(2);
    let buf = aside(area, xf, |off, lxf| {
        let all = off.bounds();
        start::draw_search(d, sb, lxf, off, all);
    });
    let (s0, s1, pl) = (phys4(xf, g.s[0]), phys4(xf, g.s[1]), phys4(xf, g.pill));
    let (r0, r1, k) = (g.sr[0] * s, g.sr[1] * s, g.sk * s);
    let (lead, gw) = (GRAD_LEAD * s, GRAD_W * s);
    let ol = s0[0].max(s1[0]) - k * 0.25 - lead;
    let or = s0[2].min(s1[2]) + k * 0.25 + lead;
    let wl = gw.min((ol - pl[0]).max(0.0)) + 0.001;
    let wr = gw.min((pl[2] - or).max(0.0)) + 0.001;
    lay(cv, clip, area, &buf, |x, y| {
        let di = sd_rr(x, y, s0, r0).max(sd_rr(x, y, s1, r1)) - k * 0.25;
        (1.0 - smoothstep(-FRONT * s - 0.5, 0.5, di)) * smoothstep(0.0, 1.0, (x - ol) / wl) * smoothstep(0.0, 1.0, (or - x) / wr)
    });
    // The footer's glyphs, on their drops as they come.
    let ga = g.p[GLYPHS].clamp(0.0, 1.0);
    if ga <= 0.01 {
        return;
    }
    let gf = GFEATHER * s;
    let (q0, q1, pr, pk) = (phys4(xf, g.pw[0]), phys4(xf, g.pw[1]), g.pr * s, g.pk * s);
    let (gb, pb) = (rect_of(g.pw[0]), rect_of(g.pw[1]));
    let area = xf.r(gb.union(&pb)).inflate(12);
    let buf = aside(area, xf, |off, lxf| {
        let all = off.bounds();
        start::draw_gear_glyph(off, all, lxf, gb, col::TEXT);
        start::draw_power_glyph(off, all, lxf, pb, col::TEXT);
    });
    lay(cv, clip, area, &buf, |x, y| ga * smoothstep(0.5, 0.5 + gf, -smin(sd_rr(x, y, q0, pr), sd_rr(x, y, q1, pr), pk)));
    let (uq, ur, ub) = (phys4(xf, g.u), g.ur * s, rect_of(g.u));
    let area = xf.r(ub).inflate(6);
    let buf = aside(area, xf, |off, lxf| {
        let all = off.bounds();
        start::draw_initial(off, all, lxf, ub);
    });
    lay(cv, clip, area, &buf, |x, y| ga * smoothstep(0.5, 0.5 + gf, -sd_rr(x, y, uq, ur)));
}
