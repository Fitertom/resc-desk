//! Memory-only outputs paced by a timerfd at the output refresh rate.
use super::{take_damage, Backend};
use crate::desk::{Desk, TOK_BACKEND};
use crate::render::{render_output, Canvas};
use std::collections::HashMap;
use std::os::fd::AsRawFd;

pub struct Headless {
    fbs: HashMap<u32, (Vec<u32>, i32, i32)>,
    timer: sys::TimerFd,
}

impl Headless {
    pub fn new(d: &mut Desk, spec: &str) -> std::io::Result<Self> {
        let timer = sys::TimerFd::new()?;
        d.ep.add(timer.fd.as_raw_fd(), TOK_BACKEND, sys::EPOLLIN)?;
        for (i, s) in super::parse_outputs(spec).into_iter().enumerate() {
            let mhz = (s.hz.unwrap_or(60.0) * 1000.0).round() as i32;
            d.add_output(format!("HEADLESS-{}", i + 1), s.w, s.h, &s, mhz);
        }
        Ok(Self { fbs: HashMap::new(), timer })
    }
}

impl Backend for Headless {
    fn dispatch(&mut self, d: &mut Desk, _token: u64) {
        if self.timer.read() > 0 {
            for o in d.outputs.iter_mut() {
                o.frame_pending = false;
            }
        }
    }

    fn present(&mut self, d: &mut Desk, oi: usize) {
        let (id, pw, ph) = (d.outputs[oi].id, d.outputs[oi].pw, d.outputs[oi].ph);
        let fb = self.fbs.entry(id).or_insert_with(|| (Vec::new(), 0, 0));
        if fb.1 != pw || fb.2 != ph {
            *fb = (vec![0xff00_0000; (pw * ph) as usize], pw, ph);
            let r = d.outputs[oi].rect();
            d.outputs[oi].damage.add(r);
        }
        let dmg = take_damage(d, oi);
        let fb = self.fbs.get_mut(&id).unwrap();
        let mut cv = Canvas { px: &mut fb.0, w: pw, h: ph, stride: pw as usize };
        let t0 = sys::now_ns();
        let painted = render_output(d, oi, &mut cv, &dmg);
        if std::env::var_os("RESC_PROFILE").is_some() {
            let px: i64 = painted.iter().map(|r| r.area()).sum();
            eprintln!("frame out{oi}: {:.2} ms, {} rects, {:.2} Mpx", (sys::now_ns() - t0) as f64 / 1e6, painted.len(), px as f64 / 1e6);
        }
        let o = &mut d.outputs[oi];
        o.frame_pending = true;
        o.frames += 1;
        // Like a vblank: the next one a period after this frame began, not after it ended.
        let period = 1_000_000_000_000u64 / o.refresh_mhz.max(1000) as u64;
        self.timer.arm(period.saturating_sub(sys::now_ns() - t0).max(1000), 0);
    }

    fn name(&self) -> &'static str {
        "headless"
    }

    fn read_pixels(&mut self, d: &mut Desk, oi: usize) -> Option<(Vec<u32>, i32, i32)> {
        let id = d.outputs.get(oi)?.id;
        self.fbs.get(&id).map(|(p, w, h)| (p.clone(), *w, *h))
    }
}
