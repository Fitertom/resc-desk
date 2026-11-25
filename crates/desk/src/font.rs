//! Minimal TrueType: cmap (4/12), glyf (simple + composite), hmtx; scanline
//! area-coverage rasterizer (same idea as font-rs) and a glyph cache.
use crate::geom::Rect;
use crate::render::{Canvas, Xf};
use std::cell::RefCell;
use std::collections::HashMap;

struct Font {
    d: Vec<u8>,
    glyf: usize,
    loca: usize,
    hmtx: usize,
    cmap: usize,
    cmap_fmt: u16,
    upem: f32,
    num_hmetrics: u16,
    num_glyphs: u16,
    loca_long: bool,
}

struct Glyph {
    w: i32,
    h: i32,
    left: i32,
    top: i32,
    adv: f32,
    mask: Vec<u8>,
}

/// Text weight. Manrope (built in) for Latin and Cyrillic, the system sans for the rest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum W {
    /// 500: body text
    Text = 0,
    /// 600: names, titles
    Semi = 1,
    /// 800: headings
    Bold = 2,
}

struct State {
    /// Per weight: a fallback chain, first font that has the glyph wins.
    faces: [Vec<std::rc::Rc<Font>>; 3],
    cache: HashMap<(u8, char, u32), Glyph>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State { faces: [vec![], vec![], vec![]], cache: HashMap::new() });
}

const MANROPE: [[&[u8]; 2]; 3] = [
    [include_bytes!("../assets/fonts/manrope-latin-500.ttf"), include_bytes!("../assets/fonts/manrope-cyrillic-500.ttf")],
    [include_bytes!("../assets/fonts/manrope-latin-600.ttf"), include_bytes!("../assets/fonts/manrope-cyrillic-600.ttf")],
    [include_bytes!("../assets/fonts/manrope-latin-800.ttf"), include_bytes!("../assets/fonts/manrope-cyrillic-800.ttf")],
];

fn u16_at(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}
fn i16_at(d: &[u8], o: usize) -> i16 {
    u16_at(d, o) as i16
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

impl Font {
    fn parse(d: Vec<u8>) -> Option<Font> {
        let n = u16_at(&d, 4) as usize;
        let mut t = HashMap::new();
        for i in 0..n {
            let r = 12 + i * 16;
            let tag = d.get(r..r + 4)?.to_vec();
            t.insert(tag, u32_at(&d, r + 8) as usize);
        }
        let head = *t.get(&b"head".to_vec())?;
        let hhea = *t.get(&b"hhea".to_vec())?;
        let maxp = *t.get(&b"maxp".to_vec())?;
        let cmap = *t.get(&b"cmap".to_vec())?;
        let glyf = *t.get(&b"glyf".to_vec())?;
        let loca = *t.get(&b"loca".to_vec())?;
        let hmtx = *t.get(&b"hmtx".to_vec())?;
        // Pick the best Unicode cmap subtable.
        let nsub = u16_at(&d, cmap + 2) as usize;
        let mut best: Option<(u32, usize, u16)> = None;
        for i in 0..nsub {
            let r = cmap + 4 + i * 8;
            let (pid, eid) = (u16_at(&d, r), u16_at(&d, r + 2));
            let off = cmap + u32_at(&d, r + 4) as usize;
            let fmt = u16_at(&d, off);
            let score = match (pid, eid, fmt) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
                (3, 1, 4) | (0, 3, 4) => 2,
                (0, _, 4) => 1,
                _ => 0,
            };
            if score > 0 && best.map_or(true, |b| score > b.0) {
                best = Some((score, off, fmt));
            }
        }
        let (_, coff, cfmt) = best?;
        Some(Font {
            upem: u16_at(&d, head + 18) as f32,
            loca_long: i16_at(&d, head + 50) != 0,
            num_hmetrics: u16_at(&d, hhea + 34),
            num_glyphs: u16_at(&d, maxp + 4),
            glyf,
            loca,
            hmtx,
            cmap: coff,
            cmap_fmt: cfmt,
            d,
        })
    }

    fn glyph_index(&self, c: char) -> u16 {
        let c = c as u32;
        let d = &self.d;
        let o = self.cmap;
        if self.cmap_fmt == 12 {
            let n = u32_at(d, o + 12) as usize;
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = o + 16 + mid * 12;
                let (s, e) = (u32_at(d, g), u32_at(d, g + 4));
                if c < s {
                    hi = mid;
                } else if c > e {
                    lo = mid + 1;
                } else {
                    return (u32_at(d, g + 8) + c - s) as u16;
                }
            }
            return 0;
        }
        if c > 0xffff {
            return 0;
        }
        let segx2 = u16_at(d, o + 6) as usize;
        let ends = o + 14;
        let starts = ends + segx2 + 2;
        let deltas = starts + segx2;
        let ranges = deltas + segx2;
        for i in 0..segx2 / 2 {
            let end = u16_at(d, ends + i * 2) as u32;
            if end < c {
                continue;
            }
            let start = u16_at(d, starts + i * 2) as u32;
            if start > c {
                return 0;
            }
            let delta = u16_at(d, deltas + i * 2) as u32;
            let ro = u16_at(d, ranges + i * 2) as usize;
            if ro == 0 {
                return ((c + delta) & 0xffff) as u16;
            }
            let addr = ranges + i * 2 + ro + 2 * (c - start) as usize;
            let g = u16_at(d, addr) as u32;
            return if g == 0 { 0 } else { ((g + delta) & 0xffff) as u16 };
        }
        0
    }

    fn advance(&self, g: u16) -> f32 {
        let i = g.min(self.num_hmetrics.saturating_sub(1)) as usize;
        u16_at(&self.d, self.hmtx + i * 4) as f32
    }

    fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
        if g >= self.num_glyphs {
            return None;
        }
        let (a, b) = if self.loca_long {
            (u32_at(&self.d, self.loca + g as usize * 4) as usize, u32_at(&self.d, self.loca + g as usize * 4 + 4) as usize)
        } else {
            (u16_at(&self.d, self.loca + g as usize * 2) as usize * 2, u16_at(&self.d, self.loca + g as usize * 2 + 2) as usize * 2)
        };
        if b <= a { None } else { Some((self.glyf + a, self.glyf + b)) }
    }

    /// Appends contours (as closed polylines of quadratic segments) in font units.
    fn outline(&self, g: u16, xf: [f32; 6], out: &mut Vec<Vec<(f32, f32, bool)>>, depth: u32) {
        let Some((o, _end)) = self.glyph_range(g) else { return };
        if depth > 8 {
            return;
        }
        let d = &self.d;
        let nc = i16_at(d, o);
        let tr = |x: f32, y: f32| (xf[0] * x + xf[2] * y + xf[4], xf[1] * x + xf[3] * y + xf[5]);
        if nc >= 0 {
            let nc = nc as usize;
            let ends: Vec<usize> = (0..nc).map(|i| u16_at(d, o + 10 + i * 2) as usize).collect();
            let npts = ends.last().map_or(0, |e| e + 1);
            let ilen = u16_at(d, o + 10 + nc * 2) as usize;
            let mut p = o + 12 + nc * 2 + ilen;
            let mut flags = Vec::with_capacity(npts);
            while flags.len() < npts {
                let f = *d.get(p).unwrap_or(&0);
                p += 1;
                flags.push(f);
                if f & 8 != 0 {
                    let r = *d.get(p).unwrap_or(&0);
                    p += 1;
                    for _ in 0..r {
                        flags.push(f);
                    }
                }
            }
            flags.truncate(npts);
            let mut xs = Vec::with_capacity(npts);
            let mut v = 0i32;
            for f in &flags {
                if f & 2 != 0 {
                    let dx = *d.get(p).unwrap_or(&0) as i32;
                    p += 1;
                    v += if f & 16 != 0 { dx } else { -dx };
                } else if f & 16 == 0 {
                    v += i16_at(d, p) as i32;
                    p += 2;
                }
                xs.push(v);
            }
            let mut ys = Vec::with_capacity(npts);
            v = 0;
            for f in &flags {
                if f & 4 != 0 {
                    let dy = *d.get(p).unwrap_or(&0) as i32;
                    p += 1;
                    v += if f & 32 != 0 { dy } else { -dy };
                } else if f & 32 == 0 {
                    v += i16_at(d, p) as i32;
                    p += 2;
                }
                ys.push(v);
            }
            let mut s = 0;
            for e in ends {
                let mut c = Vec::new();
                for i in s..=e.min(npts.saturating_sub(1)) {
                    let (x, y) = tr(xs[i] as f32, ys[i] as f32);
                    c.push((x, y, flags[i] & 1 != 0));
                }
                s = e + 1;
                if !c.is_empty() {
                    out.push(c);
                }
            }
        } else {
            let mut p = o + 10;
            loop {
                let fl = u16_at(d, p);
                let gi = u16_at(d, p + 2);
                p += 4;
                let (dx, dy) = if fl & 1 != 0 {
                    let r = (i16_at(d, p) as f32, i16_at(d, p + 2) as f32);
                    p += 4;
                    r
                } else {
                    let r = (d.get(p).map_or(0, |b| *b as i8) as f32, d.get(p + 1).map_or(0, |b| *b as i8) as f32);
                    p += 2;
                    r
                };
                let f2 = |v: i16| v as f32 / 16384.0;
                let (mut a, mut b, mut c, mut dd) = (1.0, 0.0, 0.0, 1.0);
                if fl & 8 != 0 {
                    a = f2(i16_at(d, p));
                    dd = a;
                    p += 2;
                } else if fl & 0x40 != 0 {
                    a = f2(i16_at(d, p));
                    dd = f2(i16_at(d, p + 2));
                    p += 4;
                } else if fl & 0x80 != 0 {
                    a = f2(i16_at(d, p));
                    b = f2(i16_at(d, p + 2));
                    c = f2(i16_at(d, p + 4));
                    dd = f2(i16_at(d, p + 6));
                    p += 8;
                }
                let (dx, dy) = if fl & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
                // Compose: parent ∘ component.
                let m = [
                    xf[0] * a + xf[2] * b,
                    xf[1] * a + xf[3] * b,
                    xf[0] * c + xf[2] * dd,
                    xf[1] * c + xf[3] * dd,
                    xf[0] * dx + xf[2] * dy + xf[4],
                    xf[1] * dx + xf[3] * dy + xf[5],
                ];
                self.outline(gi, m, out, depth + 1);
                if fl & 0x20 == 0 {
                    break;
                }
            }
        }
    }
}

struct Raster {
    w: usize,
    h: usize,
    a: Vec<f32>,
}

impl Raster {
    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 { (1.0, p0, p1) } else { (-1.0, p1, p0) };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        let y0 = p0.1.max(0.0) as usize;
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let yend = self.h.min(p1.1.ceil().max(0.0) as usize);
        for y in y0..yend {
            let ls = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let xnext = x + dxdy * dy;
            let dd = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0f = x0.floor();
            let x0i = x0f as i32;
            let x1c = x1.ceil();
            let x1i = x1c as i32;
            let base = ls as isize + x0i as isize;
            if base < 0 || x1i < 0 {
                x = xnext;
                continue;
            }
            let base = base as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0f;
                self.a[base] += dd - dd * xmf;
                self.a[base + 1] += dd * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0r = x0 - x0f;
                let a0 = 0.5 * s * (1.0 - x0r) * (1.0 - x0r);
                let x1r = x1 - x1c + 1.0;
                let am = 0.5 * s * x1r * x1r;
                self.a[base] += dd * a0;
                if x1i == x0i + 2 {
                    self.a[base + 1] += dd * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0r);
                    self.a[base + 1] += dd * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.a[ls + xi as usize] += dd * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.a[ls + (x1i - 1) as usize] += dd * (1.0 - a2 - am);
                }
                self.a[ls + x1i as usize] += dd * am;
            }
            x = xnext;
        }
    }

    fn quad(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) {
        let ddx = p0.0 - 2.0 * p1.0 + p2.0;
        let ddy = p0.1 - 2.0 * p1.1 + p2.1;
        let devsq = ddx * ddx + ddy * ddy;
        if devsq < 0.333 {
            self.line(p0, p2);
            return;
        }
        let n = 1 + (3.0 * devsq).sqrt().sqrt().floor() as usize;
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let p = (
                mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0,
                mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1,
            );
            self.line(prev, p);
            prev = p;
        }
    }
}

fn rasterize(f: &Font, c: char, px: f32) -> Glyph {
    let g = f.glyph_index(c);
    let s = px / f.upem;
    let adv = f.advance(g) * s;
    let mut contours = Vec::new();
    f.outline(g, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut contours, 0);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for c in &contours {
        for p in c {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
    }
    if contours.is_empty() || x1 <= x0 {
        return Glyph { w: 0, h: 0, left: 0, top: 0, adv, mask: vec![] };
    }
    let left = (x0 * s).floor() as i32;
    let top = (y1 * s).ceil() as i32;
    let w = ((x1 * s).ceil() as i32 - left + 1).max(1) as usize;
    let h = (top - (y0 * s).floor() as i32 + 1).max(1) as usize;
    let mut r = Raster { w, h, a: vec![0.0; w * h + w + 4] };
    let map = |p: &(f32, f32, bool)| (p.0 * s - left as f32, top as f32 - p.1 * s);
    for c in &contours {
        let n = c.len();
        // Find a starting on-curve point (or synthesize one).
        let start = match c.iter().position(|p| p.2) {
            Some(i) => map(&c[i]),
            None => {
                let a = map(&c[0]);
                let b = map(&c[1 % n]);
                ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
            }
        };
        let first = c.iter().position(|p| p.2).unwrap_or(0);
        let mut cur = start;
        let mut ctrl: Option<(f32, f32)> = None;
        for k in 1..=n {
            let p = &c[(first + k) % n];
            let q = map(p);
            if p.2 {
                match ctrl.take() {
                    Some(cp) => r.quad(cur, cp, q),
                    None => r.line(cur, q),
                }
                cur = q;
            } else if let Some(cp) = ctrl {
                let mid = ((cp.0 + q.0) / 2.0, (cp.1 + q.1) / 2.0);
                r.quad(cur, cp, mid);
                cur = mid;
                ctrl = Some(q);
            } else {
                ctrl = Some(q);
            }
        }
        match ctrl {
            Some(cp) => r.quad(cur, cp, start),
            None => r.line(cur, start),
        }
    }
    let mut acc = 0.0f32;
    let mut mask = vec![0u8; w * h];
    for (i, m) in mask.iter_mut().enumerate() {
        acc += r.a[i];
        let v = acc.abs().min(1.0);
        // Light gamma lift for light-on-dark UI text.
        *m = (v.powf(0.8) * 255.0) as u8;
    }
    Glyph { w: w as i32, h: h as i32, left, top, adv, mask }
}

/// Loads the first usable sans font from the usual places.
pub fn init() {
    let mut cands: Vec<String> = [
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Ok(p) = std::env::var("RESC_FONT") {
        cands.insert(0, p);
    }
    // Any *.ttf as a last resort.
    let mut stack = vec![std::path::PathBuf::from("/usr/share/fonts")];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map_or(false, |x| x == "ttf") {
                    let s = p.to_string_lossy().to_string();
                    if s.contains("Sans") && s.contains("Regular") || s.ends_with("DejaVuSans.ttf") {
                        cands.push(s);
                    }
                }
            }
        }
    }
    let mut system = None;
    for c in cands {
        if let Ok(data) = std::fs::read(&c) {
            if let Some(f) = Font::parse(data) {
                eprintln!("font: Manrope (built in) + {c}");
                system = Some(std::rc::Rc::new(f));
                break;
            }
        }
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for (w, files) in MANROPE.iter().enumerate() {
            let mut chain: Vec<std::rc::Rc<Font>> = files.iter().filter_map(|b| Font::parse(b.to_vec())).map(std::rc::Rc::new).collect();
            if let Some(f) = &system {
                chain.push(f.clone());
            }
            s.faces[w] = chain;
        }
    });
}

fn with_glyph<R>(st: &mut State, w: W, c: char, px: f32, f: impl FnOnce(&Glyph) -> R) -> Option<R> {
    let key = (w as u8, c, (px * 4.0).round() as u32);
    if !st.cache.contains_key(&key) {
        let chain = &st.faces[w as usize];
        let font = chain.iter().find(|f| f.glyph_index(c) != 0).or(chain.first())?;
        let g = rasterize(font, c, px);
        if st.cache.len() > 4096 {
            st.cache.clear();
        }
        st.cache.insert(key, g);
    }
    st.cache.get(&key).map(f)
}

/// Width in logical px.
pub fn text_width(s: &str, size: f32) -> i32 {
    text_width_w(s, size, W::Text)
}

pub fn text_width_w(s: &str, size: f32, wt: W) -> i32 {
    ST.with(|st| {
        let mut st = st.borrow_mut();
        let mut w = 0.0;
        for c in s.chars() {
            w += with_glyph(&mut st, wt, c, size, |g| g.adv).unwrap_or(0.0);
        }
        w.round() as i32
    })
}

/// Draws `s` with its baseline at logical (x, y); ellipsizes beyond `max_w`.
pub fn draw_text(cv: &mut Canvas, clip: Rect, xf: Xf, x: i32, y: i32, s: &str, size: f32, color: u32, max_w: i32) {
    draw_text_w(cv, clip, xf, x, y, s, size, color, max_w, W::Text)
}

pub fn draw_text_w(cv: &mut Canvas, clip: Rect, xf: Xf, x: i32, y: i32, s: &str, size: f32, color: u32, max_w: i32, wt: W) {
    if max_w <= 0 {
        return;
    }
    let px = size * xf.scale as f32;
    let pmax = max_w as f32 * xf.scale as f32;
    let ox = xf.fx(x as f64) as f32;
    let oy = xf.fy(y as f64).round() as i32;
    ST.with(|st| {
        let mut st = st.borrow_mut();
        if st.faces[wt as usize].is_empty() {
            return;
        }
        let ell = with_glyph(&mut st, wt, '…', px, |g| g.adv).unwrap_or(0.0);
        let total: f32 = s.chars().map(|c| with_glyph(&mut st, wt, c, px, |g| g.adv).unwrap_or(0.0)).sum();
        let limit = if total > pmax { pmax - ell } else { f32::MAX };
        let mut pen = 0.0f32;
        let mut cut = false;
        for c in s.chars() {
            let adv = with_glyph(&mut st, wt, c, px, |g| g.adv).unwrap_or(0.0);
            if pen + adv > limit {
                cut = true;
                break;
            }
            with_glyph(&mut st, wt, c, px, |g| {
                if g.w > 0 {
                    cv.mask(clip, (ox + pen).round() as i32 + g.left, oy - g.top, g.w, g.h, &g.mask, color);
                }
            });
            pen += adv;
        }
        if cut {
            with_glyph(&mut st, wt, '…', px, |g| {
                if g.w > 0 {
                    cv.mask(clip, (ox + pen).round() as i32 + g.left, oy - g.top, g.w, g.h, &g.mask, color);
                }
            });
        }
    });
}
