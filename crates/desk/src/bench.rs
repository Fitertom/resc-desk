//! `ctl bench start SECONDS [move|idle|still]`: how fast the desk can draw, without the host
//! in the way. For SECONDS every output gets a new frame as soon as its last one is out:
//! «move» carries the active window round a circle (the window, its shadow and the frost
//! under it redrawn each frame, the usual load of dragging), «idle» repaints the whole
//! screen with nothing moved (the floor: what a frame costs at all), «still» repaints only
//! a tiny corner (the overhead with almost no pixels). `ctl bench` then tells the frames
//! per second and how long the frames took. On the headless backend with a fast virtual
//! monitor (`--backend headless --outputs 1920x1080@1000hz`) nothing but the desk limits it.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::state::Tile;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Move,
    Idle,
    Still,
}

pub struct Bench {
    pub mode: Mode,
    start: u64,
    until: u64,
    /// The window carried round, and where it started.
    win: Option<(crate::state::WKey, i32, i32)>,
    /// Per output: how long each present took (ns), and when the frames went out.
    times: Vec<Vec<u64>>,
    stamps: Vec<Vec<u64>>,
    glass0: (u64, u64),
}

#[derive(Default)]
pub struct State {
    pub run: Option<Bench>,
    pub report: Option<String>,
}

pub fn start(d: &mut Desk, secs: f64, mode: Mode) {
    let now = sys::now_ns();
    let win = d.mru.first().copied().filter(|&wk| d.windows.get(wk).map_or(false, |w| w.tile == Tile::Floating && !w.minimized && !w.desktop())).map(|wk| (wk, d.windows[wk].x, d.windows[wk].y));
    use std::sync::atomic::Ordering::Relaxed;
    d.bench.run = Some(Bench {
        mode,
        start: now,
        until: now + (secs * 1e9) as u64,
        win,
        times: vec![Vec::new(); d.outputs.len()],
        stamps: vec![Vec::new(); d.outputs.len()],
        glass0: (crate::gpu::BLURS.load(Relaxed), crate::gpu::KEPT.load(Relaxed)),
    });
    d.bench.report = None;
}

/// Before drawing: this frame's change (every output ready for one gets it).
pub fn step(d: &mut Desk) {
    let Some(b) = &d.bench.run else { return };
    let now = sys::now_ns();
    if now >= b.until {
        finish(d);
        return;
    }
    let mode = b.mode;
    let t = (now - b.start) as f64 / 1e9;
    match mode {
        Mode::Move => {
            if let Some((wk, x0, y0)) = b.win {
                if d.windows.get(wk).is_none() {
                    return;
                }
                if let Some(r) = crate::wm::visual_rect(d, wk) {
                    d.damage(r);
                }
                // One turn a second, 200 px round.
                let a = t * std::f64::consts::TAU;
                let w = &mut d.windows[wk];
                w.x = x0 + (200.0 * a.cos()) as i32 - 200;
                w.y = y0 + (120.0 * a.sin()) as i32;
                if let Some(r) = crate::wm::visual_rect(d, wk) {
                    d.damage(r);
                }
                let sk = d.windows[wk].surf;
                d.damage_surface_tree(sk);
            } else {
                d.damage_all();
            }
        }
        Mode::Idle => d.damage_all(),
        Mode::Still => {
            for o in d.outputs.iter_mut() {
                let r = o.rect();
                o.damage.add(Rect::new(r.x, r.y, 4, 4));
            }
        }
    }
}

/// Around one output's present: how long it took.
pub fn presented(d: &mut Desk, oi: usize, t0: u64) {
    let now = sys::now_ns();
    if let Some(b) = d.bench.run.as_mut() {
        if let (Some(t), Some(s)) = (b.times.get_mut(oi), b.stamps.get_mut(oi)) {
            t.push(now - t0);
            s.push(now);
        }
    }
}

fn finish(d: &mut Desk) {
    let Some(b) = d.bench.run.take() else { return };
    if let Some((wk, x0, y0)) = b.win {
        if d.windows.get(wk).is_some() {
            if let Some(r) = crate::wm::visual_rect(d, wk) {
                d.damage(r);
            }
            d.windows[wk].x = x0;
            d.windows[wk].y = y0;
            if let Some(r) = crate::wm::visual_rect(d, wk) {
                d.damage(r);
            }
        }
    }
    use std::sync::atomic::Ordering::Relaxed;
    let secs = (b.until - b.start) as f64 / 1e9;
    let mut s = format!("bench {:?} {:.1} s, gpu {}\n", b.mode, secs, if d.gpu.is_some() { "vulkan" } else { "cpu" });
    for (oi, (times, stamps)) in b.times.iter().zip(&b.stamps).enumerate() {
        let Some(o) = d.outputs.get(oi) else { continue };
        s.push_str(&format!("{} {}x{} {:.0} Hz: {}\n", o.name, o.pw, o.ph, o.refresh_mhz as f64 / 1000.0, summary(times, stamps, secs)));
    }
    s.push_str(&format!(
        "glass: blurred {} kept {}\n",
        crate::gpu::BLURS.load(Relaxed) - b.glass0.0,
        crate::gpu::KEPT.load(Relaxed) - b.glass0.1
    ));
    eprint!("{s}");
    d.bench.report = Some(s);
}

/// «N fps, frame avg/p50/p99/max ms, gaps p99/max ms».
pub fn summary(times: &[u64], stamps: &[u64], secs: f64) -> String {
    if times.is_empty() {
        return "no frames".into();
    }
    let ms = |ns: u64| ns as f64 / 1e6;
    let pct = |v: &[u64], p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    let mut t = times.to_vec();
    t.sort_unstable();
    let avg = t.iter().sum::<u64>() as f64 / t.len() as f64;
    let mut gaps: Vec<u64> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    let g = if gaps.is_empty() { String::new() } else { format!(", gaps p99 {:.2} max {:.2} ms", ms(pct(&gaps, 0.99)), ms(*gaps.last().unwrap())) };
    format!(
        "{:.0} fps, frame avg {:.2} p50 {:.2} p99 {:.2} max {:.2} ms{g}",
        times.len() as f64 / secs,
        avg / 1e6,
        ms(pct(&t, 0.5)),
        ms(pct(&t, 0.99)),
        ms(*t.last().unwrap())
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn summary_reads() {
        let times = vec![1_000_000, 2_000_000, 3_000_000, 10_000_000];
        let stamps = vec![0, 5_000_000, 10_000_000, 30_000_000];
        let s = super::summary(&times, &stamps, 2.0);
        assert!(s.starts_with("2 fps, frame avg 4.00 p50 3.00"), "{s}");
        assert!(s.contains("max 10.00 ms, gaps p99 20.00 max 20.00 ms"), "{s}");
        assert_eq!(super::summary(&[], &[], 1.0), "no frames");
    }
}
