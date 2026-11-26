//! The start-up animation, after the user's prototype `Start_anim.html` ("Gravity Burst"): a
//! black desktop, 2000 particles held in a tight ball at its middle by gravity, their white
//! trails fading behind them; then the burst: gravity off, every particle thrown out from the
//! middle, and the desktop shows through a polygon whose corners follow the particles a fixed
//! way behind; the trails die out, and the rest of the desktop fades in.
//!
//! The physics is the prototype's (`step`, `burst`, `lagPoint`) at its fixed 60 steps a
//! second, each split into `SUB` substeps (240 a second: smooth on a 240 Hz monitor), in desktop (logical) px, scaled from the prototype's 3440 px wide main monitor
//! to ours. Drawn over everything (the CPU path's last item; on the GPU path the
//! overlays' layer): black where the desktop is not shown yet, the trails on it.
//!
//! The ball holds while the desktop loads (the wallpaper, the `--spawn` programs' windows).
//! `RESC_BOOT=0` skips it; `ctl boot` plays it again; a click during the ball bursts it now,
//! during the burst shows the desktop at once.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::{blend, Canvas, Xf};

const N: usize = 2000;
const G: f32 = 1.0;
const CLICK_G: f32 = -5.0;
const DAMP: f32 = 0.99;
/// The prototype's speed scale and corner lag, relative to the width of its main (middle)
/// monitor.
const PROTO_W: f32 = 3440.0;
const PROTO_SCALE: f32 = 3.4;
const PROTO_LAG: f32 = 600.0;
/// Each of the prototype's 60 steps a second is split into this many substeps (the same
/// motion, 240 frames a second on a 240 Hz monitor).
const SUB: usize = 4;
/// Substeps of the ball before it is first shown ("the mouse ten seconds at the centre").
const PREROLL: usize = 600 * SUB;
/// The ball holds while the desktop loads: at least `BALL_MIN`, until the wallpaper is
/// decoded and the start-up programs have shown their windows, then `SETTLE` without a new
/// one; never longer than `BALL_MAX` (s).
const BALL_MIN: f32 = 0.8;
const SETTLE: f32 = 0.4;
const BALL_MAX: f32 = 15.0;
/// The last steps of the preroll drawn into the trails, so the first frame has the ball.
const PRE_TRAIL: usize = 12 * SUB;
/// After the burst: the trails fade over this long (s); the desktop fades in after `SHOW_AT`
/// over `SHOW_FOR`.
const TRAIL_FOR: f32 = 2.5;
const SHOW_AT: f32 = 2.6;
const SHOW_FOR: f32 = 0.6;
/// A trail pixel is gone after this many steps (×0.8 each, truncated): only the area drawn
/// in that many last steps is faded and blended, not the whole screen.
const FADE_SUB: usize = 112;
/// Trail points kept per corner particle.
const HIST: usize = 60 * SUB;
const STEP: f32 = 1.0 / (60 * SUB) as f32;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Ball,
    Burst,
}

pub struct Boot {
    phase: Phase,
    /// Time in the phase; the desktop's fade-in (0..1); physics time not yet stepped.
    t: f32,
    reveal: f32,
    acc: f32,
    g: f32,
    /// The particles (desktop px).
    px: Vec<f32>,
    py: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    /// The centre; the speed scale; the corners' lag (desktop px).
    cx: f32,
    cy: f32,
    scale: f32,
    lag: f32,
    /// After the burst: every particle a corner of the polygon, in the order of the way it
    /// flies, with its recent path.
    lead: Vec<(usize, std::collections::VecDeque<(f32, f32)>)>,
    /// The trails, one coverage byte per physical pixel of each output.
    trails: Vec<(Rect, Vec<u8>)>,
    /// Per output: what each of the last updates drew (output px) and its substeps, back to
    /// `FADE_SUB` substeps ago, so where the trails can be non-zero is their union.
    drawn: Vec<std::collections::VecDeque<(Rect, usize)>>,
    rng: u64,
    /// Windows still awaited from the start-up programs; when the last one appeared.
    wait: usize,
    last_map: f32,
}

impl Boot {
    fn rand(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
    }

    /// One physics step; with `segs`, each particle's move this step is recorded.
    fn step(&mut self, mut segs: Option<&mut Vec<[f32; 4]>>) {
        // A 1/SUB part of the prototype's step: moves, damping and gravity in proportion.
        let h = 1.0 / SUB as f32;
        let gg = self.g * self.scale * h;
        let damp = DAMP.powf(h);
        for i in 0..N {
            let (x0, y0) = (self.px[i], self.py[i]);
            self.px[i] += self.vx[i] * h;
            self.py[i] += self.vy[i] * h;
            if let Some(s) = segs.as_deref_mut() {
                s.push([x0, y0, self.px[i], self.py[i]]);
            }
            self.vx[i] *= damp;
            self.vy[i] *= damp;
            let a = (self.cy - self.py[i]).atan2(self.cx - self.px[i]);
            self.vx[i] += a.cos() * gg;
            self.vy[i] += a.sin() * gg;
        }
        for (i, h) in &mut self.lead {
            h.push_back((self.px[*i], self.py[*i]));
            if h.len() > HIST {
                h.pop_front();
            }
        }
    }

    fn burst(&mut self) {
        if self.phase != Phase::Ball {
            return;
        }
        self.phase = Phase::Burst;
        self.t = 0.0;
        self.g = 0.0;
        // The prototype's click: gravity times click_gravity for ~3 frames, an impulse out from
        // the middle.
        for i in 0..N {
            let a = (self.py[i] - self.cy).atan2(self.px[i] - self.cx) + (self.rand() - 0.5) * 0.6;
            let k = -CLICK_G * G * 3.0 * self.scale * (0.5 + self.rand().powf(0.7) * 0.9);
            self.vx[i] += a.cos() * k * 1.25;
            self.vy[i] += a.sin() * k;
        }
        let mut ord: Vec<usize> = (0..N).collect();
        ord.sort_by(|a, b| self.vy[*a].atan2(self.vx[*a]).total_cmp(&self.vy[*b].atan2(self.vx[*b])));
        self.lead = ord.into_iter().map(|i| (i, std::collections::VecDeque::from([(self.px[i], self.py[i])]))).collect();
    }

    /// `n` substeps drawn into the trails: the old ones dimmer (20 % per prototype step, in
    /// one pass), these substeps' moves on top at `alpha`.
    fn step_drawn(&mut self, d: &Desk, alpha: f32, n: usize) {
        if n == 0 {
            return;
        }
        let mut segs = Vec::with_capacity(N * n);
        for _ in 0..n {
            self.step(Some(&mut segs));
        }
        let m = (256.0 * 0.8f32.powf(n as f32 / SUB as f32)).round() as u32;
        for (oi, (p, buf)) in self.trails.iter_mut().enumerate() {
            if oi >= d.outputs.len() {
                break;
            }
            let Some(drawn) = self.drawn.get_mut(oi) else { break };
            let live = live_rect(drawn, *p);
            for y in live.y..live.y1() {
                let row = (y * p.w) as usize;
                for ch in buf[row + live.x as usize..row + live.x1() as usize].chunks_mut(8) {
                    if ch.len() == 8 && u64::from_ne_bytes((&*ch).try_into().unwrap()) == 0 {
                        continue;
                    }
                    for v in ch {
                        if *v != 0 {
                            *v = ((*v as u32 * m) >> 8) as u8;
                        }
                    }
                }
            }
            let mut now = Rect::default();
            if alpha > 0.0 {
                let (xf, _) = out_xf(d, oi);
                let a = (alpha * 255.0) as u32;
                let (mut x0m, mut y0m, mut x1m, mut y1m) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for s in &segs {
                    let (x0, y0) = (xf.fx(s[0] as f64) as f32 - p.x as f32, xf.fy(s[1] as f64) as f32 - p.y as f32);
                    let (x1, y1) = (xf.fx(s[2] as f64) as f32 - p.x as f32, xf.fy(s[3] as f64) as f32 - p.y as f32);
                    if line(buf, p.w, p.h, x0, y0, x1, y1, a) {
                        x0m = x0m.min(x0.min(x1));
                        y0m = y0m.min(y0.min(y1));
                        x1m = x1m.max(x0.max(x1));
                        y1m = y1m.max(y0.max(y1));
                    }
                }
                if x0m <= x1m {
                    let r = Rect::new(x0m.floor() as i32 - 2, y0m.floor() as i32 - 2, (x1m - x0m).ceil() as i32 + 5, (y1m - y0m).ceil() as i32 + 5);
                    now = r.intersect(&Rect::new(0, 0, p.w, p.h));
                }
            }
            drawn.push_back((now, n));
            // Gone once FADE_SUB substeps have passed after it.
            while drawn.len() > 1 && drawn.iter().skip(1).map(|e| e.1).sum::<usize>() >= FADE_SUB {
                drawn.pop_front();
            }
        }
    }

    /// A corner: the point on its particle's path `lag` behind it.
    fn corner(&self, h: &std::collections::VecDeque<(f32, f32)>) -> (f32, f32) {
        let mut need = self.lag;
        for k in (1..h.len()).rev() {
            let (x1, y1) = h[k];
            let (x0, y0) = h[k - 1];
            let d = (x1 - x0).hypot(y1 - y0);
            if d >= need {
                let f = need / d;
                return (x1 - (x1 - x0) * f, y1 - (y1 - y0) * f);
            }
            need -= d;
        }
        h[0]
    }
}

/// Where an output's trails can be non-zero (output px, within `p`'s size).
fn live_rect(drawn: &std::collections::VecDeque<(Rect, usize)>, p: Rect) -> Rect {
    let r = drawn.iter().map(|e| &e.0).filter(|r| !r.is_empty()).fold(None, |a: Option<Rect>, r| Some(a.map_or(*r, |a| a.union(r))));
    r.map_or(Rect::default(), |r| r.intersect(&Rect::new(0, 0, p.w, p.h)))
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn out_xf(d: &Desk, oi: usize) -> (Xf, Rect) {
    let o = &d.outputs[oi];
    let r = o.rect();
    let xf = Xf::new(r.x, r.y, o.scale);
    (xf, xf.r(r))
}

/// Starts (or restarts) the animation (`main` skips it at start-up when `RESC_BOOT=0`); the
/// ball holds until `wait` windows have appeared (the start-up programs'), see `BALL_MIN`.
pub fn start(d: &mut Desk, wait: usize) {
    if d.outputs.is_empty() {
        return;
    }
    let bb = d.outputs.iter().map(|o| o.rect()).reduce(|a, b| a.union(&b)).unwrap_or_default();
    // The middle of the output in the layout's middle (the main one, between the others).
    let (bx, by) = bb.center();
    let main = d.outputs.iter().map(|o| o.rect()).find(|r| r.contains(bx as f64, by as f64)).unwrap_or(d.outputs[0].rect());
    let (cx, cy) = main.center();
    let k = main.w.max(1) as f32 / PROTO_W;
    let seed = sys::now_ns();
    let mut b = Boot {
        phase: Phase::Ball,
        t: 0.0,
        reveal: 0.0,
        acc: 0.0,
        g: G,
        px: vec![cx as f32; N],
        py: vec![cy as f32; N],
        vx: vec![0.0; N],
        vy: vec![0.0; N],
        cx: cx as f32,
        cy: cy as f32,
        scale: PROTO_SCALE * k,
        lag: PROTO_LAG * k,
        lead: Vec::new(),
        trails: Vec::new(),
        drawn: Vec::new(),
        rng: seed | 1,
        wait,
        last_map: 0.0,
    };
    for i in 0..N {
        let v = b.rand().sqrt() * 32.0 * b.scale;
        let a = b.rand() * std::f32::consts::TAU;
        b.vx[i] = a.cos() * v;
        b.vy[i] = a.sin() * v;
    }
    for _ in 0..PREROLL - PRE_TRAIL {
        b.step(None);
    }
    b.trails = (0..d.outputs.len())
        .map(|oi| {
            let (_, p) = out_xf(d, oi);
            (p, vec![0u8; (p.w.max(0) * p.h.max(0)) as usize])
        })
        .collect();
    b.drawn = vec![Default::default(); d.outputs.len()];
    for _ in 0..PRE_TRAIL {
        b.step_drawn(d, 1.0, 1);
    }
    d.overlay.boot = Some(Box::new(b));
    d.overlay.animating = true;
    d.damage_all();
}

/// A click: the ball bursts now; the burst shows the desktop at once.
pub fn poke(d: &mut Desk) {
    if let Some(b) = d.overlay.boot.as_mut() {
        match b.phase {
            Phase::Ball => b.burst(),
            Phase::Burst => b.t = b.t.max(SHOW_AT),
        }
    }
}

/// A window appeared (`wm::map_window`): one fewer awaited; the ball settles after it.
pub fn mapped(d: &mut Desk) {
    if let Some(b) = d.overlay.boot.as_mut() {
        b.wait = b.wait.saturating_sub(1);
        b.last_map = b.t;
    }
}

pub fn active(d: &Desk) -> bool {
    d.overlay.boot.is_some()
}

/// Steps the physics and the trails; true while it runs.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let Some(mut b) = d.overlay.boot.take() else { return false };
    b.acc += dt;
    b.t += dt;
    if b.phase == Phase::Ball {
        // The wallpaper is otherwise picked up by the once-a-second housekeeping.
        crate::wallpaper::poll(d);
        let loaded = !d.wall.is_loading() && b.wait == 0 && b.t - b.last_map >= SETTLE;
        if (b.t >= BALL_MIN && loaded) || b.t >= BALL_MAX {
            b.burst();
        }
    }
    let reveal0 = b.reveal;
    if b.phase == Phase::Burst && b.t > SHOW_AT {
        b.reveal = (b.reveal + dt / SHOW_FOR).min(1.0);
    }
    let alpha = if b.phase == Phase::Burst { (1.0 - b.t / TRAIL_FOR).max(0.0) } else { 1.0 };
    // Substeps due by now (a frame a little early still takes its substep: frame times
    // jitter around 1/240 s, and 0, 2, 0, 2 would judder), at most a prototype step's 4.
    let n = ((b.acc / STEP + 0.3).floor().max(0.0) as usize).min(4 * SUB);
    b.step_drawn(d, alpha, n);
    b.acc -= n as f32 * STEP;
    if b.acc > 0.1 {
        b.acc = 0.0;
    }
    // Redrawn only when it moved: a monitor faster than the substeps would otherwise redraw
    // (and upload) every output for nothing.
    if n > 0 || b.reveal != reveal0 {
        d.damage_all();
    }
    if b.reveal >= 1.0 {
        d.damage_all();
        return false;
    }
    d.overlay.boot = Some(b);
    true
}

/// A segment into a coverage buffer: 1 px wide, its ends' coverage split over the two
/// pixels nearest along the minor axis.
fn line(buf: &mut [u8], w: i32, h: i32, x0: f32, y0: f32, x1: f32, y1: f32, a: u32) -> bool {
    let (dx, dy) = (x1 - x0, y1 - y0);
    if !(dx.is_finite() && dy.is_finite()) {
        return false;
    }
    let n = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
    if n > 4000 {
        return false;
    }
    // Wholly off this output: nothing to draw.
    if x0.max(x1) < -1.0 || y0.max(y1) < -1.0 || x0.min(x1) > w as f32 + 1.0 || y0.min(y1) > h as f32 + 1.0 {
        return false;
    }
    let mut put = |x: i32, y: i32, c: f32| {
        if x >= 0 && y >= 0 && x < w && y < h {
            let i = (y * w + x) as usize;
            let v = (a as f32 * c) as u32;
            if v as u8 > buf[i] {
                buf[i] = v.min(255) as u8;
            }
        }
    };
    let steep = dy.abs() > dx.abs();
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let (x, y) = (x0 + dx * t, y0 + dy * t);
        if steep {
            let fx = x - 0.5;
            let xi = fx.floor();
            let f = fx - xi;
            put(xi as i32, y as i32, 1.0 - f);
            put(xi as i32 + 1, y as i32, f);
        } else {
            let fy = y - 0.5;
            let yi = fy.floor();
            let f = fy - yi;
            put(x as i32, yi as i32, 1.0 - f);
            put(x as i32, yi as i32 + 1, f);
        }
    }
    true
}

/// Draws it over output `oi` (all of it covers the output while it runs).
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(b) = d.overlay.boot.as_ref() else { return };
    let Some((p, trail)) = b.trails.get(oi) else { return };
    let shown = smooth(b.reveal);
    // The polygon (physical, relative to the output), its edges for a scanline fill.
    let mut edges: Vec<[f32; 4]> = Vec::new();
    if !b.lead.is_empty() && shown < 1.0 {
        let pts: Vec<(f32, f32)> = b.lead.iter().map(|(_, h)| b.corner(h)).map(|(x, y)| (xf.fx(x as f64) as f32 - p.x as f32, xf.fy(y as f64) as f32 - p.y as f32)).collect();
        for i in 0..pts.len() {
            let (a, c) = (pts[i], pts[(i + 1) % pts.len()]);
            if a.1 != c.1 {
                edges.push([a.0, a.1, c.0, c.1]);
            }
        }
    }
    let c = clip.intersect(&cv.bounds()).intersect(p);
    let black_k = |inside: f32| ((1.0 - inside) * 255.0) as u32;
    let outside = black_k(shown);
    // Black first, by spans of the polygon (each span one constant).
    let span = |px: &mut [u32], k: u32| {
        if k >= 255 {
            px.fill(0xff00_0000);
        } else if k > 0 {
            for v in px {
                *v = blend(*v, k << 24);
            }
        }
    };
    // Where the polygon crosses each row of the clip (nonzero winding, like a canvas clip):
    // every edge visits only its own rows.
    let mut rows: Vec<Vec<(f32, i32)>> = vec![Vec::new(); if edges.is_empty() { 0 } else { c.h.max(0) as usize }];
    for e in &edges {
        let (ya, yb) = (e[1], e[3]);
        let (lo, hi) = (ya.min(yb), ya.max(yb));
        // Rows whose centre ly = y - p.y + 0.5 is in [lo, hi).
        let y0 = ((lo - 0.5).ceil() as i32 + p.y).max(c.y);
        let y1 = ((hi - 0.5).ceil() as i32 + p.y).min(c.y1());
        for y in y0..y1 {
            let ly = (y - p.y) as f32 + 0.5;
            let x = e[0] + (ly - ya) / (yb - ya) * (e[2] - e[0]);
            rows[(y - c.y) as usize].push((x, if yb > ya { 1 } else { -1 }));
        }
    }
    for y in c.y..c.y1() {
        let row = y as usize * cv.stride;
        if edges.is_empty() {
            span(&mut cv.px[row + c.x as usize..row + c.x1() as usize], outside);
            continue;
        }
        let xs = &mut rows[(y - c.y) as usize];
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut wind = 0;
        let mut x = c.x;
        for &(ex, w) in xs.iter().chain(std::iter::once(&(f32::MAX, 0))) {
            // Pixels whose centre is before this crossing keep the current winding.
            let end = if ex == f32::MAX { c.x1() } else { ((ex - 0.5).ceil() as i32 + p.x).clamp(x, c.x1()) };
            if end > x {
                let k = if wind != 0 { 0 } else { outside };
                span(&mut cv.px[row + x as usize..row + end as usize], k);
                x = end;
            }
            wind += w;
        }
    }
    // The trails on top, only where they can be.
    let Some(drawn) = b.drawn.get(oi) else { return };
    let live = live_rect(drawn, *p).translate(p.x, p.y).intersect(&c);
    for y in live.y..live.y1() {
        let row = y as usize * cv.stride;
        let trow = ((y - p.y) * p.w) as usize;
        let t = &trail[trow + (live.x - p.x) as usize..trow + (live.x1() - p.x) as usize];
        let out = &mut cv.px[row + live.x as usize..row + live.x1() as usize];
        // Mostly empty: skipped 8 pixels at a time.
        for (k, ch) in t.chunks(8).enumerate() {
            if ch.len() == 8 && u64::from_ne_bytes(ch.try_into().unwrap()) == 0 {
                continue;
            }
            for (j, &tr) in ch.iter().enumerate() {
                if tr > 0 {
                    let tr = tr as u32;
                    let px = &mut out[k * 8 + j];
                    *px = blend(*px, (tr << 24) | (tr << 16) | (tr << 8) | tr);
                }
            }
        }
    }
}
