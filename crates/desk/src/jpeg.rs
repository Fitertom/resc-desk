//! JPEG decoder (no libjpeg): baseline and progressive Huffman-coded 8-bit images, grayscale
//! or YCbCr with any chroma subsampling, restart intervals, the EXIF orientation. Not
//! supported: arithmetic coding, lossless and 12-bit JPEG, CMYK.
//!
//! All scans decode into per-component coefficient blocks (so progressive refinement is
//! the same code path), then each block is dequantized and inverse-DCT'd, and the planes
//! are upsampled and converted to RGB.

use crate::wallpaper::Image;

const ZZ: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37,
    44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct Huff {
    /// 9-bit lookup: (length, symbol); length 0: longer code.
    fast: Vec<(u8, u8)>,
    maxcode: [i32; 17],
    mincode: [i32; 17],
    valptr: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(counts: &[u8; 16], vals: Vec<u8>) -> Huff {
        let mut h = Huff { fast: vec![(0, 0); 512], vals, ..Default::default() };
        let (mut code, mut k) = (0i32, 0i32);
        for len in 1..=16usize {
            let n = counts[len - 1] as i32;
            h.valptr[len] = k;
            h.mincode[len] = code;
            for _ in 0..n {
                if len <= 9 && (k as usize) < h.vals.len() {
                    let first = (code << (9 - len)) as usize;
                    for e in &mut h.fast[first..first + (1 << (9 - len))] {
                        *e = (len as u8, h.vals[k as usize]);
                    }
                }
                code += 1;
                k += 1;
            }
            h.maxcode[len] = if n > 0 { code - 1 } else { -1 };
            code <<= 1;
        }
        h
    }
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
    /// A marker was reached: zeros from here on.
    marker: bool,
}

impl<'a> Bits<'a> {
    fn fill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if !self.marker && self.pos < self.d.len() {
                b = self.d[self.pos];
                if b == 0xff {
                    match self.d.get(self.pos + 1) {
                        Some(0) => self.pos += 2,
                        _ => {
                            self.marker = true;
                            b = 0;
                        }
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acc |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }
    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        self.fill();
        let v = (self.acc >> (64 - k)) as u32;
        self.acc <<= k;
        self.n -= k;
        v
    }
    fn bit(&mut self) -> bool {
        self.bits(1) != 0
    }
    /// `s` bits as a signed value (JPEG's EXTEND).
    fn receive(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < 1 << (s - 1) {
            v - (1 << s) + 1
        } else {
            v
        }
    }
    fn decode(&mut self, h: &Huff) -> u8 {
        self.fill();
        let (len, sym) = h.fast[(self.acc >> 55) as usize];
        if len > 0 {
            self.acc <<= len;
            self.n -= len as u32;
            return sym;
        }
        for len in 10..=16usize {
            let code = (self.acc >> (64 - len)) as i32;
            if code <= h.maxcode[len] {
                self.acc <<= len;
                self.n -= len as u32;
                let i = (h.valptr[len] + code - h.mincode[len]) as usize;
                return h.vals.get(i).copied().unwrap_or(0);
            }
        }
        // Not a code: skip a bit and go on (corrupt data).
        self.bits(1);
        0
    }
    /// After a restart interval: the RSTn marker, then fresh bits.
    fn restart(&mut self) {
        self.acc = 0;
        self.n = 0;
        self.marker = false;
        while self.pos + 1 < self.d.len() {
            if self.d[self.pos] == 0xff && (0xd0..=0xd7).contains(&self.d[self.pos + 1]) {
                self.pos += 2;
                return;
            }
            if self.d[self.pos] == 0xff && self.d[self.pos + 1] != 0 && self.d[self.pos + 1] != 0xff {
                return; // another marker: the scan is over
            }
            self.pos += 1;
        }
    }
}

#[derive(Clone, Default)]
struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per row and column (padded to whole MCUs).
    bw: usize,
    bh: usize,
    coef: Vec<i32>,
    pred: i32,
    td: usize,
    ta: usize,
}

struct Dec {
    w: usize,
    h: usize,
    comps: Vec<Comp>,
    qt: [[u16; 64]; 4],
    dc: [Huff; 4],
    ac: [Huff; 4],
    progressive: bool,
    restart: usize,
    hmax: usize,
    vmax: usize,
    adobe_rgb: bool,
    orientation: u16,
}

fn be16(d: &[u8], i: usize) -> Option<usize> {
    Some(((*d.get(i)? as usize) << 8) | *d.get(i + 1)? as usize)
}

pub fn decode(d: &[u8]) -> Option<Image> {
    if d.get(0..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut dec = Dec {
        w: 0,
        h: 0,
        comps: Vec::new(),
        qt: [[1; 64]; 4],
        dc: Default::default(),
        ac: Default::default(),
        progressive: false,
        restart: 0,
        hmax: 1,
        vmax: 1,
        adobe_rgb: false,
        orientation: 1,
    };
    let mut i = 2;
    let mut framed = false;
    loop {
        // Next marker (fill bytes allowed).
        while *d.get(i)? != 0xff {
            i += 1;
        }
        while *d.get(i)? == 0xff {
            i += 1;
        }
        let m = d[i];
        i += 1;
        match m {
            0xd8 | 0x01 | 0xd0..=0xd7 => continue,
            0xd9 => break,
            _ => {}
        }
        let len = be16(d, i)?;
        let seg = d.get(i + 2..i + len)?;
        let after = i + len;
        match m {
            0xdb => {
                let mut p = 0;
                while p < seg.len() {
                    let (pq, tq) = ((seg[p] >> 4) as usize, (seg[p] & 15) as usize);
                    p += 1;
                    let t = dec.qt.get_mut(tq)?;
                    for k in 0..64 {
                        let v = if pq == 0 { *seg.get(p + k)? as u16 } else { be16(seg, p + 2 * k)? as u16 };
                        t[ZZ[k]] = v;
                    }
                    p += if pq == 0 { 64 } else { 128 };
                }
            }
            0xc4 => {
                let mut p = 0;
                while p + 17 <= seg.len() {
                    let (tc, th) = (seg[p] >> 4, (seg[p] & 15) as usize);
                    let counts: [u8; 16] = seg[p + 1..p + 17].try_into().ok()?;
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let vals = seg.get(p + 17..p + 17 + n)?.to_vec();
                    let h = Huff::new(&counts, vals);
                    if tc == 0 {
                        *dec.dc.get_mut(th)? = h;
                    } else {
                        *dec.ac.get_mut(th)? = h;
                    }
                    p += 17 + n;
                }
            }
            0xc0 | 0xc1 | 0xc2 => {
                if *seg.first()? != 8 {
                    return None;
                }
                dec.progressive = m == 0xc2;
                dec.h = be16(seg, 1)?;
                dec.w = be16(seg, 3)?;
                let nf = *seg.get(5)? as usize;
                if dec.w == 0 || dec.h == 0 || !(nf == 1 || nf == 3) || dec.w * dec.h > 1 << 28 {
                    return None;
                }
                for c in 0..nf {
                    let b = seg.get(6 + 3 * c..9 + 3 * c)?;
                    let (h, v) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                        return None;
                    }
                    dec.comps.push(Comp { id: b[0], h, v, tq: (b[2] & 3) as usize, ..Default::default() });
                }
                dec.hmax = dec.comps.iter().map(|c| c.h).max()?;
                dec.vmax = dec.comps.iter().map(|c| c.v).max()?;
                let mcux = dec.w.div_ceil(8 * dec.hmax);
                let mcuy = dec.h.div_ceil(8 * dec.vmax);
                for c in &mut dec.comps {
                    c.bw = mcux * c.h;
                    c.bh = mcuy * c.v;
                    c.coef = vec![0; c.bw * c.bh * 64];
                }
                framed = true;
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => return None,
            0xdd => dec.restart = be16(seg, 0)?,
            0xee if seg.starts_with(b"Adobe") && seg.len() >= 12 => dec.adobe_rgb = seg[11] == 0,
            0xe1 if seg.starts_with(b"Exif\0\0") => dec.orientation = exif_orientation(&seg[6..]).unwrap_or(1),
            0xda => {
                if !framed {
                    return None;
                }
                let end = scan(&mut dec, seg, d, after)?;
                i = end;
                continue;
            }
            _ => {}
        }
        i = after;
    }
    if !framed {
        return None;
    }
    Some(finish(&dec))
}

/// One scan: its header `seg`, entropy-coded data from `d[start..]`. Returns where it ends.
fn scan(dec: &mut Dec, seg: &[u8], d: &[u8], start: usize) -> Option<usize> {
    let ns = *seg.first()? as usize;
    let mut ids = Vec::new();
    for k in 0..ns {
        let cs = *seg.get(1 + 2 * k)?;
        let t = *seg.get(2 + 2 * k)?;
        let ci = dec.comps.iter().position(|c| c.id == cs)?;
        dec.comps[ci].td = (t >> 4) as usize & 3;
        dec.comps[ci].ta = (t & 15) as usize & 3;
        ids.push(ci);
    }
    let p = 1 + 2 * ns;
    let (ss, se) = (*seg.get(p)? as usize, *seg.get(p + 1)? as usize);
    let (ah, al) = ((seg.get(p + 2)? >> 4) as u32, (seg.get(p + 2)? & 15) as u32);
    let (ss, se) = if dec.progressive { (ss, se.min(63)) } else { (0, 63) };
    for c in &mut dec.comps {
        c.pred = 0;
    }
    let mut b = Bits { d, pos: start, acc: 0, n: 0, marker: false };
    let mut eobrun = 0u32;
    let mut todo = dec.restart;
    let mcux = dec.w.div_ceil(8 * dec.hmax);
    let mcuy = dec.h.div_ceil(8 * dec.vmax);
    let mut restart_check = |b: &mut Bits, dec: &mut Dec, eobrun: &mut u32| {
        if dec.restart == 0 {
            return;
        }
        if todo == 0 {
            b.restart();
            for c in &mut dec.comps {
                c.pred = 0;
            }
            *eobrun = 0;
            todo = dec.restart;
        }
        todo -= 1;
    };
    if ns == 1 {
        // One component: its own blocks, in raster order (each block an MCU).
        let ci = ids[0];
        let c = &dec.comps[ci];
        let cw = (dec.w * c.h).div_ceil(dec.hmax).div_ceil(8);
        let ch = (dec.h * c.v).div_ceil(dec.vmax).div_ceil(8);
        for by in 0..ch {
            for bx in 0..cw {
                restart_check(&mut b, dec, &mut eobrun);
                block(dec, &mut b, ci, by * dec.comps[ci].bw + bx, ss, se, ah, al, &mut eobrun);
            }
        }
    } else {
        for my in 0..mcuy {
            for mx in 0..mcux {
                restart_check(&mut b, dec, &mut eobrun);
                for &ci in &ids {
                    let (h, v, bw) = (dec.comps[ci].h, dec.comps[ci].v, dec.comps[ci].bw);
                    for y in 0..v {
                        for x in 0..h {
                            let idx = (my * v + y) * bw + mx * h + x;
                            block(dec, &mut b, ci, idx, ss, se, ah, al, &mut eobrun);
                        }
                    }
                }
            }
        }
    }
    // The next marker after the data.
    let mut i = b.pos;
    while i + 1 < d.len() {
        if d[i] == 0xff && d[i + 1] != 0 && !(0xd0..=0xd7).contains(&d[i + 1]) {
            return Some(i);
        }
        i += 1;
    }
    Some(d.len())
}

#[allow(clippy::too_many_arguments)]
fn block(dec: &mut Dec, b: &mut Bits, ci: usize, idx: usize, ss: usize, se: usize, ah: u32, al: u32, eobrun: &mut u32) {
    let (td, ta) = (dec.comps[ci].td, dec.comps[ci].ta);
    let at = idx * 64;
    if at + 64 > dec.comps[ci].coef.len() {
        return;
    }
    if !dec.progressive {
        let t = b.decode(&dec.dc[td]) as u32;
        let diff = b.receive(t);
        let c = &mut dec.comps[ci];
        c.pred += diff;
        let blk = &mut c.coef[at..at + 64];
        blk[0] = c.pred;
        let mut k = 1;
        while k < 64 {
            let rs = b.decode(&dec.ac[ta]);
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if rs != 0xf0 {
                    break;
                }
                k += 16;
                continue;
            }
            k += r;
            if k > 63 {
                break;
            }
            blk[ZZ[k]] = b.receive(s);
            k += 1;
        }
        return;
    }
    if ss == 0 {
        // DC: first pass or a refinement bit.
        if ah == 0 {
            let t = b.decode(&dec.dc[td]) as u32;
            let diff = b.receive(t);
            let c = &mut dec.comps[ci];
            c.pred += diff;
            c.coef[at] = c.pred * (1 << al);
        } else if b.bit() {
            dec.comps[ci].coef[at] |= 1 << al;
        }
        return;
    }
    let hac = &dec.ac[ta];
    let blk = &mut dec.comps[ci].coef[at..at + 64];
    if ah == 0 {
        // AC first pass.
        if *eobrun > 0 {
            *eobrun -= 1;
            return;
        }
        let mut k = ss;
        while k <= se {
            let rs = b.decode(hac);
            let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            if s == 0 {
                if r < 15 {
                    *eobrun = (1 << r) - 1;
                    if r > 0 {
                        *eobrun += b.bits(r);
                    }
                    break;
                }
                k += 16;
            } else {
                k += r as usize;
                if k > 63 {
                    break;
                }
                blk[ZZ[k]] = b.receive(s) * (1 << al);
                k += 1;
            }
        }
        return;
    }
    // AC refinement (stb_image's order of reading the correction bits).
    let bit = 1i32 << al;
    let refine = |b: &mut Bits, p: &mut i32| {
        if b.bit() && *p & bit == 0 {
            if *p > 0 {
                *p += bit;
            } else {
                *p -= bit;
            }
        }
    };
    if *eobrun > 0 {
        *eobrun -= 1;
        for k in ss..=se {
            let p = &mut blk[ZZ[k]];
            if *p != 0 {
                refine(b, p);
            }
        }
        return;
    }
    let mut k = ss;
    while k <= se {
        let rs = b.decode(hac);
        let (mut r, mut s) = ((rs >> 4) as i32, (rs & 15) as i32);
        if s == 0 {
            if r < 15 {
                *eobrun = (1u32 << r) - 1;
                if r > 0 {
                    *eobrun += b.bits(r as u32);
                }
                r = 64; // the rest of the block: refinements only
            }
        } else {
            s = if b.bit() { bit } else { -bit };
        }
        while k <= se {
            let p = &mut blk[ZZ[k]];
            k += 1;
            if *p != 0 {
                refine(b, p);
            } else {
                if r == 0 {
                    *p = s;
                    break;
                }
                r -= 1;
            }
        }
    }
}

/// Inverse DCT of one dequantized block into 8×8 samples.
fn idct(c: &[f32; 64], out: &mut [u8], stride: usize, cos: &[[f32; 8]; 8]) {
    // Flat blocks (only DC) are common: no transform needed.
    if c[1..].iter().all(|&v| v == 0.0) {
        let v = (c[0] / 8.0 + 128.0).round().clamp(0.0, 255.0) as u8;
        for y in 0..8 {
            out[y * stride..y * stride + 8].fill(v);
        }
        return;
    }
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += cos[x][u] * c[y * 8 + u];
            }
            tmp[y * 8 + x] = s;
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += cos[y][v] * tmp[v * 8 + x];
            }
            out[y * stride + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn finish(dec: &Dec) -> Image {
    let mut cos = [[0f32; 8]; 8];
    for (x, row) in cos.iter_mut().enumerate() {
        for (u, v) in row.iter_mut().enumerate() {
            let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            *v = cu * 0.5 * (((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0).cos();
        }
    }
    // Sample planes.
    let planes: Vec<(Vec<u8>, usize)> = dec
        .comps
        .iter()
        .map(|c| {
            let stride = c.bw * 8;
            let mut plane = vec![0u8; stride * c.bh * 8];
            let q = &dec.qt[c.tq];
            let mut f = [0f32; 64];
            for by in 0..c.bh {
                for bx in 0..c.bw {
                    let blk = &c.coef[(by * c.bw + bx) * 64..][..64];
                    for k in 0..64 {
                        f[k] = (blk[k] * q[k] as i32) as f32;
                    }
                    idct(&f, &mut plane[by * 8 * stride + bx * 8..], stride, &cos);
                }
            }
            (plane, stride)
        })
        .collect();
    let (w, h) = (dec.w, dec.h);
    let mut px = vec![0u32; w * h];
    let gray = dec.comps.len() == 1;
    // Per component and output column / row: the two source samples around it and the
    // weight of the second (centred, like libjpeg's "fancy" upsampling; full-size planes
    // land exactly on their samples).
    let taps = |n: usize, f: usize, fmax: usize, real: usize| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|i| {
                let s = ((i as f32 + 0.5) * f as f32 / fmax as f32 - 0.5).max(0.0);
                let i0 = (s as usize).min(real - 1);
                ((i0), (i0 + 1).min(real - 1), s - i0 as f32)
            })
            .collect()
    };
    let cols: Vec<Vec<(usize, usize, f32)>> = dec.comps.iter().map(|c| taps(w, c.h, dec.hmax, (w * c.h).div_ceil(dec.hmax))).collect();
    let rows_t: Vec<Vec<(usize, usize, f32)>> = dec.comps.iter().map(|c| taps(h, c.v, dec.vmax, (h * c.v).div_ceil(dec.vmax))).collect();
    for y in 0..h {
        for x in 0..w {
            let s = |k: usize| {
                let (p, stride) = (&planes[k].0, planes[k].1);
                let (y0, y1, fy) = rows_t[k][y];
                let (x0, x1, fx) = cols[k][x];
                if fx == 0.0 && fy == 0.0 {
                    return p[y0 * stride + x0] as f32;
                }
                let at = |yy: usize, xx: usize| p[yy * stride + xx] as f32;
                let top = at(y0, x0) + (at(y0, x1) - at(y0, x0)) * fx;
                let bot = at(y1, x0) + (at(y1, x1) - at(y1, x0)) * fx;
                top + (bot - top) * fy
            };
            let (r, g, b) = if gray {
                let v = s(0);
                (v, v, v)
            } else if dec.adobe_rgb {
                (s(0), s(1), s(2))
            } else {
                let (yy, cb, cr) = (s(0), s(1) - 128.0, s(2) - 128.0);
                (yy + 1.402 * cr, yy - 0.344_136 * cb - 0.714_136 * cr, yy + 1.772 * cb)
            };
            let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
            px[y * w + x] = 0xff00_0000 | q(r) << 16 | q(g) << 8 | q(b);
        }
    }
    orient(Image { w, h, px }, dec.orientation)
}

/// EXIF IFD0's Orientation (tag 0x0112) from a TIFF block.
fn exif_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = t.get(i..i + 2)?;
        Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = t.get(i..i + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    };
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    for e in 0..n {
        let at = ifd + 2 + e * 12;
        if u16_at(at)? == 0x0112 {
            return u16_at(at + 8).filter(|o| (1..=8).contains(o));
        }
    }
    None
}

/// Applies EXIF orientation `o` (1 = as stored).
fn orient(img: Image, o: u16) -> Image {
    if o <= 1 || o > 8 {
        return img;
    }
    let (w, h) = (img.w, img.h);
    let swap = o >= 5;
    let (nw, nh) = if swap { (h, w) } else { (w, h) };
    let mut px = vec![0u32; w * h];
    for y in 0..nh {
        for x in 0..nw {
            // Where (x, y) of the upright image is in the stored one.
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x), // 8
            };
            px[y * nw + x] = img.px[sy * w + sx];
        }
    }
    Image { w: nw, h: nh, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real baseline 16×8 JPEG (4:2:0, quality 90): the left half red, the right blue.
    #[test]
    fn decodes_baseline() {
        // Built by the encoder below so the test needs no files.
        let jpg = tiny_encoder::encode_two_colors();
        let img = decode(&jpg).expect("decode");
        assert_eq!((img.w, img.h), (16, 8));
        let c = |p: u32| ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        let (r, g, b) = c(img.px[2 * 16 + 2]);
        assert!(r > 200 && g < 60 && b < 60, "left {:?}", (r, g, b));
        let (r, g, b) = c(img.px[2 * 16 + 13]);
        assert!(b > 200 && r < 60 && g < 60, "right {:?}", (r, g, b));
    }

    #[test]
    fn exif_orientation_reads() {
        // "MM", offset 8, one entry: 0x0112 SHORT 1 = 6.
        let t = [b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0];
        assert_eq!(exif_orientation(&t), Some(6));
        let img = Image { w: 2, h: 1, px: vec![1, 2] };
        let r = orient(img, 6);
        assert_eq!((r.w, r.h), (1, 2));
        assert_eq!(r.px, vec![1, 2]);
    }

    /// A minimal baseline encoder (4:4:4, one quant table of ones, the standard Huffman
    /// tables), only for the test above.
    mod tiny_encoder {
        const DC_COUNTS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
        const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        // AC table with just EOB (0x00) and nothing else needed: flat blocks.
        const AC_COUNTS: [u8; 16] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        const AC_VALS: [u8; 1] = [0];

        struct Bw {
            out: Vec<u8>,
            acc: u32,
            n: u32,
        }
        impl Bw {
            fn put(&mut self, v: u32, len: u32) {
                for i in (0..len).rev() {
                    self.acc = (self.acc << 1) | ((v >> i) & 1);
                    self.n += 1;
                    if self.n == 8 {
                        let b = self.acc as u8;
                        self.out.push(b);
                        if b == 0xff {
                            self.out.push(0);
                        }
                        self.acc = 0;
                        self.n = 0;
                    }
                }
            }
            fn flush(&mut self) {
                while self.n != 0 {
                    self.put(1, 1);
                }
            }
        }

        fn dc_code(cat: u32) -> (u32, u32) {
            // Canonical codes of DC_COUNTS.
            let (mut code, mut k) = (0u32, 0u32);
            for (len, &n) in DC_COUNTS.iter().enumerate() {
                for _ in 0..n {
                    if k == cat {
                        return (code, len as u32 + 1);
                    }
                    code += 1;
                    k += 1;
                }
                code <<= 1;
            }
            unreachable!()
        }

        fn put_dc(bw: &mut Bw, diff: i32) {
            let cat = if diff == 0 { 0 } else { 32 - diff.unsigned_abs().leading_zeros() };
            let (c, l) = dc_code(cat);
            bw.put(c, l);
            if cat > 0 {
                let v = if diff < 0 { (diff - 1) as u32 & ((1 << cat) - 1) } else { diff as u32 };
                bw.put(v, cat);
            }
            bw.put(0, 1); // AC: EOB (the one 1-bit code)
        }

        pub fn encode_two_colors() -> Vec<u8> {
            let mut o = vec![0xff, 0xd8];
            // DQT: all ones.
            o.extend([0xff, 0xdb, 0, 67, 0]);
            o.extend([1u8; 64]);
            // SOF0 16×8, three components 1×1 on table 0.
            o.extend([0xff, 0xc0, 0, 17, 8, 0, 8, 0, 16, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
            // DHT: DC 0 and AC 0.
            let mut dht = vec![0x00];
            dht.extend(DC_COUNTS);
            dht.extend(DC_VALS);
            dht.push(0x10);
            dht.extend(AC_COUNTS);
            dht.extend(AC_VALS);
            o.extend([0xff, 0xc4]);
            o.extend(((dht.len() + 2) as u16).to_be_bytes());
            o.extend(dht);
            // SOS: three components, tables 0/0.
            o.extend([0xff, 0xda, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 63, 0]);
            // Two MCUs: red (Y 76, Cb 85, Cr 255) then blue (Y 29, Cb 255, Cr 107). DC of a flat
            // block of value v (level-shifted) is 8 (v - 128) with unit quantization.
            let colors = [[76, 85, 255], [29, 255, 107]];
            let mut bw = Bw { out: Vec::new(), acc: 0, n: 0 };
            let mut pred = [0i32; 3];
            for c in colors {
                for k in 0..3 {
                    let dcv = 8 * (c[k] - 128);
                    put_dc(&mut bw, dcv - pred[k]);
                    pred[k] = dcv;
                }
            }
            bw.flush();
            o.extend(bw.out);
            o.extend([0xff, 0xd9]);
            o
        }
    }
}
