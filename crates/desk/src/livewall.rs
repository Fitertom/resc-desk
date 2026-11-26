//! Live wallpaper «Частицы», after a Wallpaper Engine one (`main.js`: gravity particles):
//! 2000 particles pulled toward the pointer, each leaving a 1 px trail that fades by a fifth
//! every frame; a click on the desktop throws them away from it for a moment (gravity ×−5
//! for 50 ms). The physics is the script's, at its 60 steps a second, on the CPU (2000
//! particles are nothing); the trails live on the GPU (gpu.rs: a trail image per output,
//! lw_solid.frag fades it, lw_seg.vert/.frag draw this step's moves into it, the background
//! is that image). Across every monitor at once, in desktop (logical) px.
//!
//! Only where the trails are is redrawn (the moves of the last `FADE` steps), and not where
//! a window covers it or a game has the monitor. Needs the GPU renderer.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::Xf;
use std::collections::VecDeque;

pub const N: usize = 2000;
const G: f32 = 1.0;
const CLICK_G: f32 = -5.0;
const CLICK_S: f32 = 0.05;
const DAMP: f32 = 0.99;
const STEP: f32 = 1.0 / 60.0;
/// A trail is gone after this many steps (×0.8 each, and 1/255 off).
const FADE: usize = 26;

pub struct Live {
    px: Vec<f32>,
    py: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    acc: f32,
    /// Seconds of the click's thrust left.
    pulse: f32,
    /// Per output: steps not drawn into its trail image yet (each its moves, physical px),
    /// steps dropped from that (their fade still owed), and what the last `FADE` steps
    /// drew (logical).
    pend: Vec<VecDeque<Vec<[f32; 4]>>>,
    owed: Vec<u32>,
    hist: Vec<VecDeque<Rect>>,
    /// Fade toward the wallpaper's picture instead of black.
    pub on_picture: bool,
    rng: u64,
    /// Steps taken (glass over the trails is blurred again when it changes, gpu.rs).
    pub gen: u64,
}

/// What an output's trail image gets this frame (gpu.rs).
pub struct Frame {
    /// Steps in order, each its segments (physical px of the output).
    pub steps: Vec<Vec<[f32; 4]>>,
    /// Fades owed besides one per step.
    pub owed: u32,
    pub on_picture: bool,
    pub width: f32,
}

impl Live {
    fn new(d: &Desk, on_picture: bool) -> Live {
        let (mx, my) = (d.seat.x as f32, d.seat.y as f32);
        let mut l = Live {
            px: vec![mx; N],
            py: vec![my; N],
            vx: vec![0.0; N],
            vy: vec![0.0; N],
            acc: 0.0,
            pulse: 0.0,
            pend: Vec::new(),
            owed: Vec::new(),
            hist: Vec::new(),
            on_picture,
            rng: sys::now_ns() | 1,
            gen: 0,
        };
        for i in 0..N {
            let v = l.rand().sqrt() * 32.0;
            let a = l.rand() * std::f32::consts::TAU;
            l.vx[i] = a.cos() * v;
            l.vy[i] = a.sin() * v;
        }
        l
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
    }
}

pub fn active(d: &Desk) -> bool {
    d.live.is_some()
}

/// On or off (`ctl livewall`, the settings); the frame is drawn whole again.
pub fn set(d: &mut Desk, on: bool, on_picture: bool) {
    if on && d.gpu.is_none() {
        eprintln!("livewall: needs the GPU renderer");
        return;
    }
    d.live = on.then(|| Box::new(Live::new(d, on_picture)));
    d.display_cfg.wallpaper_live = on;
    d.display_cfg.wallpaper_live_picture = on_picture;
    d.overlay.animating = true;
    d.damage_all();
}

/// A click on the desktop: the particles are thrown away from the pointer for a moment.
pub fn click(d: &mut Desk) {
    if let Some(l) = d.live.as_mut() {
        l.pulse = CLICK_S;
    }
}

/// Steps the physics; damages where trails are on each output. True while on.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let Some(mut l) = d.live.take() else { return false };
    let n_out = d.outputs.len();
    l.pend.resize_with(n_out, VecDeque::new);
    l.owed.resize(n_out, 0);
    l.hist.resize_with(n_out, VecDeque::new);
    l.acc = (l.acc + dt).min(4.0 * STEP);
    let (mx, my) = (d.seat.x as f32, d.seat.y as f32);
    let rects: Vec<Rect> = d.outputs.iter().map(|o| o.rect()).collect();
    let xfs: Vec<Xf> = d.outputs.iter().map(|o| Xf::new(o.x, o.y, o.scale)).collect();
    let mut stepped = false;
    while l.acc >= STEP {
        l.acc -= STEP;
        stepped = true;
        l.gen += 1;
        let g = if l.pulse > 0.0 { G * CLICK_G } else { G };
        l.pulse -= STEP;
        let mut per: Vec<Vec<[f32; 4]>> = vec![Vec::new(); n_out];
        let mut boxes: Vec<Option<(f32, f32, f32, f32)>> = vec![None; n_out];
        for i in 0..N {
            let (x0, y0) = (l.px[i], l.py[i]);
            l.px[i] += l.vx[i];
            l.py[i] += l.vy[i];
            let (x1, y1) = (l.px[i], l.py[i]);
            l.vx[i] *= DAMP;
            l.vy[i] *= DAMP;
            let a = (my - y1).atan2(mx - x1);
            l.vx[i] += a.cos() * g;
            l.vy[i] += a.sin() * g;
            let (lx, ly, hx, hy) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
            for (oi, r) in rects.iter().enumerate() {
                if hx < r.x as f32 - 2.0 || lx > r.x1() as f32 + 2.0 || hy < r.y as f32 - 2.0 || ly > r.y1() as f32 + 2.0 {
                    continue;
                }
                let xf = xfs[oi];
                per[oi].push([xf.fx(x0 as f64) as f32, xf.fy(y0 as f64) as f32, xf.fx(x1 as f64) as f32, xf.fy(y1 as f64) as f32]);
                let b = boxes[oi].get_or_insert((lx, ly, hx, hy));
                *b = (b.0.min(lx), b.1.min(ly), b.2.max(hx), b.3.max(hy));
            }
        }
        for oi in 0..n_out {
            l.pend[oi].push_back(std::mem::take(&mut per[oi]));
            if l.pend[oi].len() > FADE {
                l.pend[oi].pop_front();
                l.owed[oi] += 1;
            }
            let r = boxes[oi].map_or(Rect::default(), |(a, b, c, e)| Rect::from_edges(a.floor() as i32 - 2, b.floor() as i32 - 2, c.ceil() as i32 + 3, e.ceil() as i32 + 3).intersect(&rects[oi]));
            l.hist[oi].push_back(r);
            if l.hist[oi].len() > FADE {
                l.hist[oi].pop_front();
            }
        }
    }
    if stepped {
        for oi in 0..n_out {
            let live = l.hist[oi].iter().filter(|r| !r.is_empty()).fold(None, |a: Option<Rect>, r| Some(a.map_or(*r, |a| a.union(r))));
            if let Some(r) = live.filter(|r| !hidden(d, oi, *r)) {
                d.damage(r);
            }
        }
    }
    d.live = Some(l);
    true
}

/// `r` on output `oi` does not show: a game has the monitor, or an opaque window covers it.
fn hidden(d: &Desk, oi: usize, r: Rect) -> bool {
    if crate::shell::output_covered(d, oi) {
        return true;
    }
    crate::wm::visible_stack(d).into_iter().any(|wk| {
        let w = &d.windows[wk];
        !w.desktop()
            && d.surfaces.get(w.surf).is_some_and(|s| s.is_opaque())
            // Its content, not its frame: the frame's glass shows the wallpaper through.
            && d.surfaces.get(w.surf).is_some_and(|s| {
                let g = s.geom();
                Rect::new(w.x, w.y, g.w, g.h).contains_rect(&r)
            })
    })
}

/// Where output `oi`'s trails are (logical) and the step they are at: glass over them is
/// blurred again (its kept frost is of trails gone by).
pub fn area(d: &Desk, oi: usize) -> Option<(Rect, u64)> {
    let l = d.live.as_ref()?;
    let r = l.hist.get(oi)?.iter().filter(|r| !r.is_empty()).fold(None, |a: Option<Rect>, r| Some(a.map_or(*r, |a| a.union(r))))?;
    Some((r, l.gen))
}

/// What output `oi`'s trail image gets now (taken: drawn once). None when off.
pub fn take(d: &mut Desk, oi: usize) -> Option<Frame> {
    let l = d.live.as_mut()?;
    let scale = d.outputs.get(oi)?.scale as f32;
    let steps = l.pend.get_mut(oi).map(|p| p.drain(..).collect()).unwrap_or_default();
    let owed = l.owed.get_mut(oi).map(std::mem::take).unwrap_or(0);
    Some(Frame { steps, owed, on_picture: l.on_picture, width: scale.max(1.0) })
}
