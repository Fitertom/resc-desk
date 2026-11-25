//! Output/input backends. `headless` renders to memory (tests, CI);
//! `nested` runs inside another Wayland compositor, one host window per output;
//! `kms` is the session itself on a Linux console (DRM/KMS, evdev, the VT).
mod drm;
mod evdev;
pub mod headless;
pub mod kms;
pub mod nested;
mod vt;

use crate::desk::Desk;
use crate::geom::Damage;
use crate::render::Xf;

pub trait Backend {
    fn dispatch(&mut self, d: &mut Desk, token: u64);
    fn present(&mut self, d: &mut Desk, oi: usize);
    /// Last presented pixels of an output (XRGB, tightly packed).
    fn read_pixels(&mut self, d: &mut Desk, oi: usize) -> Option<(Vec<u32>, i32, i32)>;
    fn flush(&mut self) {}
    /// Once a second (the main loop's housekeeping timer).
    fn housekeeping(&mut self, _d: &mut Desk) {}
    fn name(&self) -> &'static str;
    /// The monitors connected, with what they can do and do now.
    fn monitors(&self, d: &Desk) -> Vec<crate::monitors::MonInfo> {
        d.outputs
            .iter()
            .map(|o| crate::monitors::MonInfo { name: o.name.clone(), modes: vec![(o.pw, o.ph, o.refresh_mhz)], cur: (o.pw, o.ph, o.refresh_mhz), on: true, ..Default::default() })
            .collect()
    }
    /// Applies the monitor settings (d.display_cfg: each monitor's, the main one, Win+P).
    fn reconfigure(&mut self, _d: &mut Desk) {}
}

/// One emulated monitor: pixels, optional physical size and plane position (mm),
/// and a scale hint used only when the physical size is unknown (96 dpi × hint).
#[derive(Clone, Debug)]
pub struct OutSpec {
    pub w: i32,
    pub h: i32,
    pub scale: f64,
    pub mm: Option<(f64, f64)>,
    pub pos_mm: Option<(f64, f64)>,
}

fn pair<T: std::str::FromStr>(s: &str, sep: char) -> Option<(T, T)> {
    let (a, b) = s.split_once(sep)?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Output spec, comma separated: `WxH[/WMMxHMM][@scale][+Xmm+Ymm]`, e.g.
/// `3440x1440/1048x441,2560x1080/572x241,1080x1920/296x532`.
/// Unplaced outputs go left→right, top-aligned.
pub fn parse_outputs(spec: &str) -> Vec<OutSpec> {
    let mut v = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (main, pos) = match part.find('+') {
            Some(i) => (&part[..i], Some(&part[i + 1..])),
            None => (part, None),
        };
        let (main, scale) = match main.split_once('@') {
            Some((s, sc)) => (s, sc.parse::<f64>().unwrap_or(1.0)),
            None => (main, 1.0),
        };
        let (px, mm) = match main.split_once('/') {
            Some((p, m)) => (p, pair::<f64>(m, 'x')),
            None => (main, None),
        };
        let Some((w, h)) = pair::<i32>(px, 'x') else { continue };
        let pos_mm = pos.and_then(|p| pair::<f64>(p, '+'));
        v.push(OutSpec { w: w.max(64), h: h.max(64), scale: scale.clamp(0.5, 4.0), mm, pos_mm });
    }
    if v.is_empty() {
        v.push(OutSpec { w: 1920, h: 1080, scale: 1.0, mm: None, pos_mm: None });
    }
    v
}

/// Logical-damage → physical rects for output `oi`; clears the output's damage.
/// The rects are what changed; the renderers grow them around glass themselves (and say
/// which rects they repainted), so the GPU path redraws its CPU layers only where needed.
pub fn take_damage(d: &mut Desk, oi: usize) -> Damage {
    let o = &mut d.outputs[oi];
    let xf = Xf::new(o.x, o.y, o.scale);
    let mut out = Damage::default();
    for r in o.damage.rects.drain(..) {
        // Round outwards so fractional scales never leave seams.
        let p = r.to_physical(xf.ox, xf.oy, xf.scale);
        out.add(p.inflate(if xf.scale.fract() != 0.0 { 1 } else { 0 }));
    }
    o.wants_frame = false;
    out
}
