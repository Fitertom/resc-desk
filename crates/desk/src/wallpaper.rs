//! Wallpaper stretched over the whole desktop plane, not over each monitor.
//!
//! The image is laid out (cover fit) over the logical bounding box of all outputs.
//! Outputs already sit on the physical plane (layout.rs), so:
//! - in physical mode the part behind a bezel is not shown, and a line in the
//!   picture runs straight across the joint;
//! - in "glue" mode the gaps are closed and nothing is lost.
//!
//! Every output keeps a pre-rendered copy at its own pixel size; repainting a
//! damaged rect is then just a row copy.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::Canvas;
use std::cell::RefCell;

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>, // opaque ARGB
}

type Loading = std::sync::Arc<std::sync::Mutex<Option<Result<Image, String>>>>;

#[derive(Default)]
pub struct Wall {
    pub img: Option<Image>,
    /// A picture being decoded on a thread (a 5K JPEG takes seconds on a slow CPU): its
    /// path and where the result lands.
    loading: Option<(String, Loading)>,
    cache: RefCell<Vec<(u32, i32, i32, Vec<u32>)>>,
    /// Bumped on every change: the GPU re-uploads its wallpaper textures.
    pub gen: std::cell::Cell<u64>,
}

impl Wall {
    pub fn invalidate(&self) {
        self.cache.borrow_mut().clear();
        self.gen.set(self.gen.get() + 1);
    }

    /// A picture is being decoded (`set`) and not shown yet.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Loads a JPEG or PNG; `None` means the built-in wallpaper.
    pub fn load(&mut self, path: Option<&str>) -> Result<(), String> {
        self.img = match path {
            None => None,
            Some(p) => Some(decode_file(p)?),
        };
        self.invalidate();
        Ok(())
    }
}

/// Switches to the picture at `path` (None: the built-in one) without stalling the desktop:
/// it is decoded on a thread and shown when ready (`poll`, from the housekeeping timer).
pub fn set(d: &mut Desk, path: Option<String>) {
    match path {
        None => {
            d.wall.loading = None;
            let _ = d.wall.load(None);
            d.display_cfg.wallpaper = None;
            d.damage_all();
            let _ = crate::layout::save_config(d);
        }
        Some(p) => {
            let slot: Loading = Default::default();
            let out = slot.clone();
            let file = p.clone();
            std::thread::spawn(move || {
                let r = decode_file(&file);
                if let Ok(mut g) = out.lock() {
                    *g = Some(r);
                }
            });
            d.wall.loading = Some((p, slot));
        }
    }
}

/// A picture finished decoding: show it, remember it.
pub fn poll(d: &mut Desk) {
    let Some((path, slot)) = &d.wall.loading else { return };
    let Some(res) = slot.lock().ok().and_then(|mut g| g.take()) else { return };
    let path = path.clone();
    d.wall.loading = None;
    match res {
        Ok(img) => {
            d.wall.img = Some(img);
            d.wall.invalidate();
            d.display_cfg.wallpaper = Some(path);
            d.damage_all();
            let _ = crate::layout::save_config(d);
        }
        Err(e) => eprintln!("wallpaper: {e}"),
    }
}

/// A picture file: JPEG or PNG (by its first bytes).
pub fn decode_file(p: &str) -> Result<Image, String> {
    let data = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
    let img = if data.starts_with(&[0xff, 0xd8]) {
        crate::jpeg::decode(&data)
    } else if data.starts_with(b"\x89PNG") {
        decode_png(&data)
    } else {
        None
    };
    img.ok_or_else(|| format!("{p}: не JPEG/PNG или неподдерживаемый вариант формата"))
}

/// `img` covering `w`×`h` (cropped to fill), each output pixel the mean of its source box.
pub fn thumb(img: &Image, w: usize, h: usize) -> Vec<u32> {
    let s = (img.w as f64 / w as f64).min(img.h as f64 / h as f64);
    let (ox, oy) = ((img.w as f64 - w as f64 * s) / 2.0, (img.h as f64 - h as f64 * s) / 2.0);
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        let y0 = (oy + y as f64 * s) as usize;
        let y1 = ((oy + (y + 1) as f64 * s) as usize).clamp(y0 + 1, img.h);
        for x in 0..w {
            let x0 = (ox + x as f64 * s) as usize;
            let x1 = ((ox + (x + 1) as f64 * s) as usize).clamp(x0 + 1, img.w);
            // At most 4×4 samples per box: plenty for a thumbnail.
            let (sx, sy) = (((x1 - x0) / 4).max(1), ((y1 - y0) / 4).max(1));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            let mut yy = y0;
            while yy < y1 {
                let mut xx = x0;
                while xx < x1 {
                    let p = img.px[yy * img.w + xx];
                    r += (p >> 16) & 255;
                    g += (p >> 8) & 255;
                    b += p & 255;
                    n += 1;
                    xx += sx;
                }
                yy += sy;
            }
            let n = n.max(1);
            out[y * w + x] = 0xff00_0000 | (r / n) << 16 | (g / n) << 8 | (b / n);
        }
    }
    out
}

/// The built-in wallpaper at `w`×`h` (a thumbnail of it).
pub fn builtin(w: usize, h: usize) -> Vec<u32> {
    let unit = h.max(1) as f64;
    let bw = w as f64 / unit;
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = liquid((x as f64 + 0.5) / unit, (y as f64 + 0.5) / unit, bw);
        }
    }
    out
}

/// Pictures to offer as wallpapers: the user's pictures folders and the system's
/// wallpaper collections (a few levels deep), JPEG and PNG, at most `max`.
pub fn candidates(max: usize) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![];
    if let Ok(t) = std::fs::read_to_string(format!("{home}/.config/user-dirs.dirs")) {
        for l in t.lines() {
            if let Some(v) = l.strip_prefix("XDG_PICTURES_DIR=") {
                dirs.push(v.trim_matches('"').replace("$HOME", &home));
            }
        }
    }
    for d in ["Pictures", "Изображения", "Pictures/Wallpapers", ".local/share/backgrounds", ".local/share/wallpapers"] {
        dirs.push(format!("{home}/{d}"));
    }
    dirs.push("/usr/share/backgrounds".into());
    dirs.push("/usr/share/wallpapers".into());
    let mut out: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<String>, max: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if out.len() >= max {
                return;
            }
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, out, max);
                }
                continue;
            }
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                continue;
            }
            // KDE packs carry previews and several sizes: one picture per pack is enough,
            // and previews are not wallpapers.
            let s = p.to_string_lossy().to_string();
            if s.contains("/screenshot") || s.contains("/contents/screenshot") {
                continue;
            }
            if let Ok(canon) = std::fs::canonicalize(&p) {
                let c = canon.to_string_lossy().to_string();
                if out.iter().any(|o| std::fs::canonicalize(o).map_or(false, |x| x.to_string_lossy() == c)) {
                    continue;
                }
            }
            out.push(s);
        }
    }
    for d in dirs {
        walk(std::path::Path::new(&d), 4, &mut out, max);
    }
    out
}

/// Logical bounding box of all outputs: the canvas the wallpaper covers.
fn plane_box(d: &Desk) -> Rect {
    let mut it = d.outputs.iter().map(|o| o.rect());
    let first = it.next().unwrap_or(Rect::new(0, 0, 1, 1));
    it.fold(first, |a, r| a.union(&r))
}

pub fn draw(d: &Desk, oi: usize, cv: &mut Canvas, clip: Rect) {
    let o = &d.outputs[oi];
    let mut cache = d.wall.cache.borrow_mut();
    let pos = match cache.iter().position(|c| c.0 == o.id && c.1 == o.pw && c.2 == o.ph) {
        Some(p) => p,
        None => {
            cache.retain(|c| c.0 != o.id);
            cache.push((o.id, o.pw, o.ph, render_output(d, oi)));
            cache.len() - 1
        }
    };
    let src = &cache[pos].3;
    let c = clip.intersect(&cv.bounds()).intersect(&Rect::new(0, 0, o.pw, o.ph));
    if c.w <= 0 || c.h <= 0 {
        return;
    }
    for y in c.y..c.y1() {
        let s = y as usize * o.pw as usize + c.x as usize;
        let t = y as usize * cv.stride + c.x as usize;
        cv.px[t..t + c.w as usize].copy_from_slice(&src[s..s + c.w as usize]);
    }
}

/// The wallpaper of output `oi` scaled into `dst` (physical px of `cv`): task view's
/// desktop tiles.
pub fn draw_scaled(d: &Desk, oi: usize, cv: &mut Canvas, clip: Rect, dst: Rect) {
    let o = &d.outputs[oi];
    let mut cache = d.wall.cache.borrow_mut();
    let pos = match cache.iter().position(|c| c.0 == o.id && c.1 == o.pw && c.2 == o.ph) {
        Some(p) => p,
        None => {
            cache.retain(|c| c.0 != o.id);
            cache.push((o.id, o.pw, o.ph, render_output(d, oi)));
            cache.len() - 1
        }
    };
    let src = &cache[pos].3;
    cv.blit(clip, dst, src, o.pw as usize, Rect::new(0, 0, o.pw, o.ph), true, 255);
}

fn render_output(d: &Desk, oi: usize) -> Vec<u32> {
    let o = &d.outputs[oi];
    let b = plane_box(d);
    let (pw, ph) = (o.pw as usize, o.ph as usize);
    let mut out = vec![0u32; pw * ph];
    // Physical pixel centre → global logical coordinate.
    let lx = |x: usize| o.x as f64 + (x as f64 + 0.5) / o.scale;
    let ly = |y: usize| o.y as f64 + (y as f64 + 0.5) / o.scale;
    match &d.wall.img {
        Some(img) => {
            let s = (b.w as f64 / img.w as f64).max(b.h as f64 / img.h as f64);
            let offx = (b.w as f64 - img.w as f64 * s) / 2.0;
            let offy = (b.h as f64 - img.h as f64 * s) / 2.0;
            let col_u: Vec<f64> = (0..pw).map(|x| (lx(x) - b.x as f64 - offx) / s - 0.5).collect();
            for y in 0..ph {
                let v = (ly(y) - b.y as f64 - offy) / s - 0.5;
                let row = &mut out[y * pw..(y + 1) * pw];
                for (x, p) in row.iter_mut().enumerate() {
                    *p = sample(img, col_u[x], v);
                }
            }
        }
        None => {
            // Built-in: dark violet "liquid" with contour lines that cross the joints.
            let unit = b.h.max(1) as f64;
            let col_u: Vec<f64> = (0..pw).map(|x| (lx(x) - b.x as f64) / unit).collect();
            let bw = b.w as f64 / unit;
            for y in 0..ph {
                let v = (ly(y) - b.y as f64) / unit;
                let row = &mut out[y * pw..(y + 1) * pw];
                for (x, p) in row.iter_mut().enumerate() {
                    *p = liquid(col_u[x], v, bw);
                }
            }
        }
    }
    out
}

fn sample(img: &Image, u: f64, v: f64) -> u32 {
    let x0 = u.floor().clamp(0.0, (img.w - 1) as f64) as usize;
    let y0 = v.floor().clamp(0.0, (img.h - 1) as f64) as usize;
    let x1 = (x0 + 1).min(img.w - 1);
    let y1 = (y0 + 1).min(img.h - 1);
    let fx = ((u - u.floor()).clamp(0.0, 1.0) * 256.0) as u32;
    let fy = ((v - v.floor()).clamp(0.0, 1.0) * 256.0) as u32;
    let a = crate::render::lerp(img.px[y0 * img.w + x0], img.px[y0 * img.w + x1], fx);
    let c = crate::render::lerp(img.px[y1 * img.w + x0], img.px[y1 * img.w + x1], fx);
    crate::render::lerp(a, c, fy)
}

/// u, v in units of the plane height; `bw` is the plane width in the same units.
fn liquid(u: f64, v: f64, bw: f64) -> u32 {
    let blob = |cx: f64, cy: f64, r: f64| {
        let (dx, dy) = (u - cx, v - cy);
        let t = (dx * dx + dy * dy) / (r * r);
        1.0 / (1.0 + t * t)
    };
    let a = blob(bw * 0.30, 0.38, 0.55); // violet
    let b2 = blob(bw * 0.72, 0.78, 0.50); // indigo
    let c = blob(bw * 0.55, 0.12, 0.30); // pink
    let mut r = 11.0 + 120.0 * a + 40.0 * b2 + 90.0 * c;
    let mut g = 10.0 + 85.0 * a + 50.0 * b2 + 30.0 * c;
    let mut bl = 18.0 + 220.0 * a + 170.0 * b2 + 120.0 * c;
    // Flowing contour lines: a field that is the same function of the plane everywhere.
    let f = (u * 5.0 + (v * 3.1 + u * 0.7).sin() * 1.3 + v * 2.0).sin();
    let line = (1.0 - (f.abs() / 0.035)).max(0.0);
    let glow = 0.35 + 0.65 * a.max(c);
    r += 140.0 * line * glow;
    g += 120.0 * line * glow;
    bl += 160.0 * line * glow;
    let cl = |x: f64| x.clamp(0.0, 255.0) as u32;
    0xff00_0000 | cl(r) << 16 | cl(g) << 8 | cl(bl)
}

// ------------------------------------------------------------------ PNG decoder

/// 8-bit non-interlaced PNG: grey, RGB, palette, grey+alpha, RGBA. Alpha is
/// flattened onto black (a wallpaper is opaque).
pub fn decode_png(data: &[u8]) -> Option<Image> {
    decode_png_ex(data, false)
}

/// Same, keeping alpha: premultiplied ARGB (icons).
pub fn decode_png_alpha(data: &[u8]) -> Option<Image> {
    decode_png_ex(data, true)
}

fn decode_png_ex(data: &[u8], keep_alpha: bool) -> Option<Image> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut pos = 8;
    let (mut w, mut h, mut ct, mut depth) = (0usize, 0usize, 0u8, 8usize);
    let mut idat = Vec::new();
    let mut pal: Vec<u32> = Vec::new();
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data.get(pos + 8..pos + 8 + len)?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().ok()?) as usize;
                h = u32::from_be_bytes(body[4..8].try_into().ok()?) as usize;
                depth = body[8] as usize;
                if !matches!(depth, 1 | 2 | 4 | 8 | 16) || body[12] != 0 {
                    return None; // no interlace
                }
                ct = body[9];
            }
            b"PLTE" => pal = body.chunks_exact(3).map(|c| 0xff00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect(),
            b"tRNS" if keep_alpha => {
                // Palette alpha (premultiply the entries).
                for (i, a) in body.iter().enumerate() {
                    if let Some(p) = pal.get_mut(i) {
                        let a = *a as u32;
                        let m = |sh: u32| ((*p >> sh & 0xff) * a / 255) << sh;
                        *p = a << 24 | m(16) | m(8) | m(0);
                    }
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let chans = match ct {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    if (depth < 8 && !matches!(ct, 0 | 3)) || (depth == 16 && ct == 3) {
        return None;
    }
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    // Filters work on whole bytes: at least one per pixel.
    let bpp = (chans * depth / 8).max(1);
    let stride = (w * chans * depth).div_ceil(8);
    let raw = inflate(idat.get(2..)?, (stride + 1) * h)?;
    if raw.len() < (stride + 1) * h {
        return None;
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let ft = line[0];
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let b = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let pred = match ft {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => return None,
            };
            cur[i] = line[1 + i].wrapping_add(pred as u8);
        }
        // Each pixel's channels as 8-bit samples (packed palettes and grays unpacked, 16-bit
        // channels to their high byte).
        let mut samples = [0u8; 4];
        for x in 0..w {
            if depth < 8 {
                let bit = x * depth;
                let mask = (1u32 << depth) - 1;
                let v = (cur[bit / 8] as u32 >> (8 - depth - bit % 8)) & mask;
                samples[0] = if ct == 3 { v as u8 } else { (v * 255 / mask) as u8 };
            } else {
                let step = depth / 8;
                for c in 0..chans {
                    samples[c] = cur[(x * chans + c) * step];
                }
            }
            let p = &samples;
            let rgb = |r: u8, g: u8, b: u8, a: u8| {
                let m = |v: u8| (v as u32 * a as u32 / 255) as u32;
                let top = if keep_alpha { (a as u32) << 24 } else { 0xff00_0000 };
                top | m(r) << 16 | m(g) << 8 | m(b)
            };
            px.push(match ct {
                0 => rgb(p[0], p[0], p[0], 255),
                2 => rgb(p[0], p[1], p[2], 255),
                3 => *pal.get(p[0] as usize).unwrap_or(&0xff00_0000) | if keep_alpha { 0 } else { 0xff00_0000 },
                4 => rgb(p[0], p[0], p[0], p[1]),
                _ => rgb(p[0], p[1], p[2], p[3]),
            });
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Some(Image { w, h, px })
}

struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
    n: u32,
}

impl BitReader<'_> {
    fn need(&mut self, k: u32) -> Option<()> {
        while self.n < k {
            let b = *self.d.get(self.pos)? as u32;
            self.pos += 1;
            self.bit |= b << self.n;
            self.n += 8;
        }
        Some(())
    }
    fn bits(&mut self, k: u32) -> Option<u32> {
        if k == 0 {
            return Some(0);
        }
        self.need(k)?;
        let v = self.bit & ((1u32 << k) - 1);
        self.bit >>= k;
        self.n -= k;
        Some(v)
    }
}

/// Canonical Huffman table: counts per length and symbols sorted by code.
struct Huff {
    count: [u16; 16],
    sym: Vec<u16>,
}

impl Huff {
    fn new(lens: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + count[i - 1];
        }
        let mut sym = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, sym }
    }
    fn decode(&self, br: &mut BitReader) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= br.bits(1)? as i32;
            let c = self.count[len] as i32;
            if code - c < first {
                return self.sym.get((index + code - first) as usize).copied();
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Raw DEFLATE (RFC 1951) decoder.
pub fn inflate(d: &[u8], hint: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(hint);
    let mut br = BitReader { d, pos: 0, bit: 0, n: 0 };
    loop {
        let last = br.bits(1)?;
        match br.bits(2)? {
            0 => {
                br.bit = 0;
                br.n = 0;
                let p = br.pos;
                let len = u16::from_le_bytes([*d.get(p)?, *d.get(p + 1)?]) as usize;
                out.extend_from_slice(d.get(p + 4..p + 4 + len)?);
                br.pos = p + 4 + len;
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                block(&mut br, &mut out, &Huff::new(&l), &Huff::new(&[5u8; 30]))?;
            }
            2 => {
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                const ORD: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for &o in ORD.iter().take(hclen) {
                    cl[o] = br.bits(3)? as u8;
                }
                let ch = Huff::new(&cl);
                let mut lens = Vec::with_capacity(hlit + hdist);
                while lens.len() < hlit + hdist {
                    let s = ch.decode(&mut br)?;
                    match s {
                        0..=15 => lens.push(s as u8),
                        16 => {
                            let p = *lens.last()?;
                            for _ in 0..3 + br.bits(2)? {
                                lens.push(p);
                            }
                        }
                        17 => lens.extend(std::iter::repeat(0).take(3 + br.bits(3)? as usize)),
                        _ => lens.extend(std::iter::repeat(0).take(11 + br.bits(7)? as usize)),
                    }
                }
                if lens.len() > hlit + hdist {
                    return None;
                }
                block(&mut br, &mut out, &Huff::new(&lens[..hlit]), &Huff::new(&lens[hlit..]))?;
            }
            _ => return None,
        }
        if last == 1 {
            return Some(out);
        }
    }
}

fn block(br: &mut BitReader, out: &mut Vec<u8>, lit: &Huff, dist: &Huff) -> Option<()> {
    loop {
        let s = lit.decode(br)? as usize;
        if s < 256 {
            out.push(s as u8);
        } else if s == 256 {
            return Some(());
        } else {
            let i = s - 257;
            let len = *LBASE.get(i)? as usize + br.bits(*LEXT.get(i)? as u32)? as usize;
            let di = dist.decode(br)? as usize;
            let dd = *DBASE.get(di)? as usize + br.bits(*DEXT.get(di)? as u32)? as usize;
            if dd > out.len() {
                return None;
            }
            let start = out.len() - dd;
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_roundtrip_through_own_encoder() {
        let (w, h) = (37, 11);
        let px: Vec<u32> = (0..w * h).map(|i| 0xff00_0000 | (i as u32 * 2654435761) & 0xff_ffff).collect();
        let png = crate::png::encode(&px, w as i32, h as i32, w, 1);
        let img = super::decode_png(&png).expect("decode");
        assert_eq!((img.w, img.h), (w, h));
        assert_eq!(img.px, px);
    }
}
