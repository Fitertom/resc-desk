//! Liquify's glass, line for line: the glass card of `shaders/liquify/ui.frag` (kind 2:
//! refraction by the bezel, frosted backdrop with saturate(1.6) brightness(1.02), tint,
//! inner shadow, screen-blended specular rim, inset highlight, hairline border) with the
//! soft shadow of `shaders/liquify/glass.frag`, and the continuous corners of
//! `squircle.glsl`. This is the CPU path; the GPU path (Vulkan, RADV on AMD) runs the
//! shader files themselves. The backdrop blur stands in for the shader's mip sampling.
//!
//! Refraction model from archisvaze/liquid-glass (webgl.html):
//! MIT License, Copyright (c) 2026 Archis Vaze. Permission is hereby granted, free of
//! charge, to any person obtaining a copy of this software and associated documentation
//! files (the "Software"), to deal in the Software without restriction, including without
//! limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
//! and/or sell copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions: The above copyright notice and
//! this permission notice shall be included in all copies or substantial portions of the
//! Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
//! HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//! CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR
//! THE USE OR OTHER DEALINGS IN THE SOFTWARE.
use crate::geom::Rect;
use crate::render::Canvas;

// ui/mod.rs: GLASS_THICKNESS, GLASS_BEZEL (×1.5 in desktop mode, as here).
pub const THICKNESS: f32 = 30.0;
/// The default bezel (the band along the edge where the glass rounds over and refracts),
/// CSS px; settings change it (bezel()).
pub const BEZEL: f32 = 24.0 * 1.5;
/// Window glass's default bezel, CSS px (the title bar is thin).
pub const WIN_BEZEL: f32 = 8.0;
/// Settings' bezel slider range and step, CSS px. ui.frag keeps a bezel to the larger of
/// the corners' radius and half of it, and to half the panel's height: past 48 px nothing
/// of ours would change.
pub const MAX_BEZEL: f32 = 48.0;
pub const BEZEL_STEP: f32 = 2.0;
// ui.frag
pub const IOR: f32 = 3.0;
pub const SPEC: f32 = 0.55;
// kit.rs glass_part: inset highlight 0.12, border 1px white(0.1)
pub const HIGHLIGHT: f32 = 0.12;
pub const BORDER_W: f32 = 1.0;
pub const BORDER_A: f32 = 0.1;
// glass.frag / common.glsl: GLASS_MARGIN, SHADOW_DROP; BAR_LOOK.shadow
const GLASS_MARGIN: f32 = 56.0;
const SHADOW_DROP: f32 = 6.0;
pub const SHADOW: f32 = 0.5;
/// See-through frost: gaussian sigma, CSS px (Liquify's DEFAULT_BLUR on the desktop).
pub const FROST: f32 = 70.0;
/// Settings' blur slider range (Liquify's MAX_BLUR) and step.
pub const MAX_BLUR: f32 = 150.0;
pub const BLUR_STEP: f32 = 5.0;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
static FROSTED: AtomicBool = AtomicBool::new(true);
static BLUR_BITS: AtomicU32 = AtomicU32::new(0x428c_0000); // 70.0
static BEZEL_BITS: AtomicU32 = AtomicU32::new(0x4210_0000); // 36.0
static WIN_BEZEL_BITS: AtomicU32 = AtomicU32::new(0x4100_0000); // 8.0

/// Settings: the bezel's width on the panels and on the windows, CSS px.
pub fn set_bezel(panels: f32, windows: f32) {
    BEZEL_BITS.store(panels.clamp(0.0, MAX_BEZEL).to_bits(), Ordering::Relaxed);
    WIN_BEZEL_BITS.store(windows.clamp(0.0, MAX_BEZEL).to_bits(), Ordering::Relaxed);
}

/// The panels' bezel now, CSS px.
pub fn bezel() -> f32 {
    f32::from_bits(BEZEL_BITS.load(Ordering::Relaxed))
}

/// The windows' bezel now, CSS px.
pub fn win_bezel() -> f32 {
    f32::from_bits(WIN_BEZEL_BITS.load(Ordering::Relaxed))
}

/// Settings: frosted glass (the card of ui.frag) or clear glass (glass.frag: only the
/// refraction along the edge, no blur, no tint), and the frost's sigma in CSS px.
pub fn set(frosted: bool, blur: f32) {
    FROSTED.store(frosted, Ordering::Relaxed);
    BLUR_BITS.store(blur.clamp(0.0, MAX_BLUR).to_bits(), Ordering::Relaxed);
}

pub fn frosted() -> bool {
    FROSTED.load(Ordering::Relaxed)
}

/// The frost's sigma, CSS px (the slider's value, also kept while the glass is clear).
pub fn blur() -> f32 {
    f32::from_bits(BLUR_BITS.load(Ordering::Relaxed))
}

/// The sigma the glass blurs with now: 0 for clear glass.
pub fn frost() -> f32 {
    if frosted() {
        blur()
    } else {
        0.0
    }
}

/// kit.rs glass_deep: rgba(18, 18, 22, 0.55), premultiplied.
/// The taskbar's hover drop (shell.rs `Lens`), the only glass on the taskbar: a faint light
/// tint, premultiplied, so it reads as a clear drop over the wallpaper.
pub const TINT_LENS: [f32; 4] = [0.09, 0.09, 0.10, 0.09];
pub const TINT_DEEP: [f32; 4] = [18.0 / 255.0 * 0.55, 18.0 / 255.0 * 0.55, 22.0 / 255.0 * 0.55, 0.55];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// squircle.glsl `sdSquircle`: signed distance to a box of half size `b` with continuous
/// corners of radius `r` (< 0 inside), `p` from the centre.
pub fn sd_squircle(p: (f32, f32), b: (f32, f32), r: f32) -> f32 {
    let (ax, ay) = (p.0.abs(), p.1.abs());
    let r = r.min(b.0.min(b.1));
    if r <= 0.5 {
        let (dx, dy) = (ax - b.0, ay - b.1);
        return dx.max(dy).min(0.0) + (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    }
    let (mn, mx) = (b.0.min(b.1), b.0.max(b.1));
    let cap = smoothstep(0.8, 1.0, r / mn);
    let round = cap * smoothstep(0.9, 1.0, mn / mx);
    let n = mix(mix(2.8, 2.25, cap), 2.0, round);
    let rr = r * mix(1.6, 1.12, cap);
    let (rx, ry) = (rr.min(b.0), rr.min(b.1));
    let (qx, qy) = (ax - (b.0 - rx), ay - (b.1 - ry));
    if qx <= 0.0 || qy <= 0.0 {
        return (ax - b.0).max(ay - b.1);
    }
    let (ux, uy) = (qx / rx, qy / ry);
    let s = (ux.powf(n) + uy.powf(n)).powf(1.0 / n);
    let gx = (ux / s).powf(n - 1.0) / rx;
    let gy = (uy / s).powf(n - 1.0) / ry;
    (s - 1.0) / (gx * gx + gy * gy).sqrt().max(1e-6)
}

/// ui.frag / glass.frag `surfaceHeight`: 0 at the edge, 1 where the flat top starts.
fn surface_height(t: f32) -> f32 {
    let s = 1.0 - t;
    (1.0 - s * s * s * s).powf(0.25)
}

/// What is under the glass, blurred: a gaussian of sigma `sigma` px, the way the shader
/// samples a mip level (average down by f, two box passes, bilinear back up).
struct Backdrop {
    low: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    x0: i32,
    y0: i32,
    f: i32,
    /// The frost's saturate(1.6) brightness(1.02): not for clear glass.
    look: bool,
}

/// Low-res box radius; two passes of it ≈ a gaussian of sigma 2·f.
const K: usize = 2;

impl Backdrop {
    fn capture(cv: &Canvas, need: Rect, sigma: f32, look: bool) -> Backdrop {
        let f = ((sigma / 2.0).round() as i32).max(1);
        let s = need.inflate((2 * K as i32 + 2) * f).intersect(&cv.bounds());
        // Grid on absolute multiples of f: the same picture whatever the damage shape.
        let x0 = s.x.div_euclid(f) * f;
        let y0 = s.y.div_euclid(f) * f;
        let w = ((s.x1() - x0 + f - 1) / f).max(1) as usize;
        let h = ((s.y1() - y0 + f - 1) / f).max(1) as usize;
        let mut low = vec![[0f32; 3]; w * h];
        for ly in 0..h {
            let ya = (y0 + ly as i32 * f).max(s.y);
            let yb = (y0 + (ly as i32 + 1) * f).min(s.y1());
            for lx in 0..w {
                let xa = (x0 + lx as i32 * f).max(s.x);
                let xb = (x0 + (lx as i32 + 1) * f).min(s.x1());
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for y in ya..yb {
                    let o = y as usize * cv.stride;
                    for p in &cv.px[o + xa as usize..o + xb.max(xa) as usize] {
                        r += (p >> 16) & 0xff;
                        g += (p >> 8) & 0xff;
                        b += p & 0xff;
                        n += 1;
                    }
                }
                let n = n.max(1) as f32 * 255.0;
                low[ly * w + lx] = [r as f32 / n, g as f32 / n, b as f32 / n];
            }
        }
        if f > 1 {
            let mut tmp = low.clone();
            for _ in 0..2 {
                box_pass(&low, &mut tmp, w, h, true);
                box_pass(&tmp, &mut low, w, h, false);
            }
        }
        Backdrop { low, w, h, x0, y0, f, look }
    }

    /// ui.frag `wallFrost`: blurred, then saturate(1.6) brightness(1.02).
    fn frost(&self, px: f32, py: f32) -> [f32; 3] {
        let u = (px - self.x0 as f32) / self.f as f32 - 0.5;
        let v = (py - self.y0 as f32) / self.f as f32 - 0.5;
        let xi = u.floor().clamp(0.0, (self.w - 1) as f32) as usize;
        let yi = v.floor().clamp(0.0, (self.h - 1) as f32) as usize;
        let xj = (xi + 1).min(self.w - 1);
        let yj = (yi + 1).min(self.h - 1);
        let fx = (u - u.floor()).clamp(0.0, 1.0);
        let fy = (v - v.floor()).clamp(0.0, 1.0);
        let (a, b, c, d) = (self.low[yi * self.w + xi], self.low[yi * self.w + xj], self.low[yj * self.w + xi], self.low[yj * self.w + xj]);
        let mut o = [0f32; 3];
        for ch in 0..3 {
            o[ch] = mix(mix(a[ch], b[ch], fx), mix(c[ch], d[ch], fx), fy);
        }
        if !self.look {
            return o;
        }
        let lum = 0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2];
        o.map(|c| ((lum + (c - lum) * 1.6) * 1.02).clamp(0.0, 1.0))
    }
}

fn box_pass(src: &[[f32; 3]], dst: &mut [[f32; 3]], w: usize, h: usize, horizontal: bool) {
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let span = (2 * K + 1) as f32;
    let cl = |i: isize| i.clamp(0, n as isize - 1) as usize;
    for line in 0..lines {
        let mut acc = [0f32; 3];
        for j in -(K as isize)..=(K as isize) {
            let p = src[at(line, cl(j))];
            for ch in 0..3 {
                acc[ch] += p[ch];
            }
        }
        for i in 0..n {
            dst[at(line, i)] = acc.map(|a| a / span);
            let out = src[at(line, cl(i as isize - K as isize))];
            let inn = src[at(line, cl(i as isize + K as isize + 1))];
            for ch in 0..3 {
                acc[ch] += inn[ch] - out[ch];
            }
        }
    }
}

/// Physical px around a glass rect that must be final before it is drawn (shadow quad
/// of glass.vert, or the reach of the frost, whichever is larger).
pub fn margin(scale: f32) -> i32 {
    let f = ((frost() * scale / 2.0).round() as i32).max(1);
    ((GLASS_MARGIN + SHADOW_DROP) * scale).ceil().max(((2 * K as i32 + 2) * f) as f32) as i32 + 2
}

#[inline]
fn over(top: [f32; 4], bottom: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - top[3];
    [top[0] + bottom[0] * k, top[1] + bottom[1] * k, top[2] + bottom[2] * k, top[3] + bottom[3] * k]
}

/// Draws one glass panel: `r` in physical px, corner `radius` px, `s` px per CSS px,
/// `tint` premultiplied (the box's fill). Only pixels inside `clip` change. Clear glass
/// (settings): glass.frag with blur 0 and no tint, as the GPU path draws it: the backdrop
/// as it is, moved by the bezel's refraction, inner shadow and rim; no highlight or border.
pub fn draw(cv: &mut Canvas, clip: Rect, r: Rect, radius: f32, s: f32, tint: [f32; 4]) {
    let clear = !frosted();
    let tint = if clear { [0.0; 4] } else { tint };
    let m = ((GLASS_MARGIN - 2.0) * s).ceil() as i32;
    let area = Rect::from_edges(r.x - m, r.y - m, r.x1() + m, r.y1() + m + (SHADOW_DROP * s).ceil() as i32)
        .intersect(&clip)
        .intersect(&cv.bounds());
    if area.is_empty() {
        return;
    }
    let hb = (r.w as f32 * 0.5, r.h as f32 * 0.5);
    let (cx, cy) = (r.x as f32 + hb.0, r.y as f32 + hb.1);
    let inside = r.intersect(&area);
    let bd = if inside.is_empty() { None } else { Some(Backdrop::capture(cv, inside, frost() * s, !clear)) };
    let thick = THICKNESS * s;
    let bz = bezel() * s;
    // ui.frag: at most the corners' radius, or half the bezel asked for when that is more.
    let bezel = (bz.min(radius.max(bz * 0.5).min(hb.0.min(hb.1))) - 1.0).max(1.0);
    let light = {
        let l = (0.5f32 * 0.5 + 0.7 * 0.7).sqrt();
        (0.5 / l, -0.7 / l)
    };
    let border = [BORDER_A, BORDER_A, BORDER_A, BORDER_A];
    let bw = BORDER_W * s;
    for y in area.y..area.y1() {
        let row = y as usize * cv.stride;
        for x in area.x..area.x1() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let p = (px - cx, py - cy);
            let sd = sd_squircle(p, hb, radius);
            let dst = &mut cv.px[row + x as usize];
            // glass.frag: soft shadow under the glass, a little lower, cut to zero inside the quad.
            let ss = sd_squircle((p.0, p.1 - SHADOW_DROP * s), hb, radius).max(0.0);
            let shadow_a = if ss < (GLASS_MARGIN - 2.0) * s { (-ss * ss / (800.0 * s * s)).exp() } else { 0.0 };
            let shadow_k = SHADOW * 0.6 * shadow_a;
            if sd > 0.0 && shadow_k > 0.0 {
                *dst = scale_px(*dst, 1.0 - shadow_k);
            }
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let Some(bd) = &bd else { continue };
            let backdrop = |qx: f32, qy: f32| {
                let c = bd.frost(qx, qy);
                [c[0], c[1], c[2], 1.0]
            };
            // ui.frag glass()
            let d = -sd;
            let mut col = if d >= bezel {
                over(tint, backdrop(px, py))
            } else {
                let t = (d / bezel).clamp(0.0, 1.0);
                let h = surface_height(t);
                let dh = (surface_height((t + 0.001).min(1.0)) - h) / 0.001;
                let slope = (dh * (thick / bezel)).atan();
                let theta_r = (slope.sin() / IOR).clamp(-1.0, 1.0).asin();
                let displacement = h * thick * (slope.tan() - theta_r.tan());
                let gx = sd_squircle((p.0 + 0.5, p.1), hb, radius) - sd + 1e-6;
                let gy = sd_squircle((p.0, p.1 + 0.5), hb, radius) - sd + 1e-6;
                let gl = (gx * gx + gy * gy).sqrt().max(1e-12);
                let (gx, gy) = (gx / gl, gy / gl);
                let mut under = backdrop(px - gx * displacement, py - gy * displacement);
                let inner_shadow = 1.0 - smoothstep(0.0, bezel * 0.6, d);
                let k = mix(1.0, 0.7, inner_shadow * 0.3);
                for c in under.iter_mut().take(3) {
                    *c *= k;
                }
                let mut c = over(tint, under);
                let facing = (gx * light.0 + gy * light.1).abs();
                let line = 1.0 - smoothstep(0.0, 1.6 * s, d);
                let glow = 1.0 - smoothstep(0.0, bezel * 0.25, d);
                let spec = (SPEC * (line * mix(0.35, 1.0, facing * facing) + 0.35 * (facing * glow).powi(2))).clamp(0.0, 1.0);
                for ch in c.iter_mut().take(3) {
                    *ch = 1.0 - (1.0 - *ch) * (1.0 - spec);
                }
                c
            };
            if clear {
                *dst = over_px(*dst, col.map(|v| v * cov));
                continue;
            }
            // CSS `inset 1px 1px 0 white(a)`: the part of the box not covered by itself moved right-down.
            let sd2 = sd_squircle((p.0 - s, p.1 - s), hb, radius);
            let band = 1.0 - (0.5 - sd2).clamp(0.0, 1.0);
            let hl = HIGHLIGHT * band;
            col = over([hl, hl, hl, hl], col);
            // Hairline border.
            let inner = (0.5 - (sd + bw)).clamp(0.0, 1.0);
            let b = border.map(|v| v * (1.0 - inner));
            col = over(b, col);
            let col = col.map(|v| v * cov);
            *dst = over_px(*dst, col);
        }
    }
}

#[inline]
fn scale_px(p: u32, k: f32) -> u32 {
    let f = |v: u32| ((v as f32) * k + 0.5) as u32;
    (f(p >> 24) << 24) | (f((p >> 16) & 0xff) << 16) | (f((p >> 8) & 0xff) << 8) | f(p & 0xff)
}

#[inline]
fn over_px(dst: u32, c: [f32; 4]) -> u32 {
    let k = 1.0 - c[3];
    let ch = |sh: u32, v: f32| ((v * 255.0 + ((dst >> sh) & 0xff) as f32 * k).round().clamp(0.0, 255.0) as u32) << sh;
    let a = ((c[3] * 255.0 + (dst >> 24) as f32 * k).round().clamp(0.0, 255.0) as u32) << 24;
    a | ch(16, c[0]) | ch(8, c[1]) | ch(0, c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squircle_matches_box_on_straight_edges() {
        let b = (100.0, 40.0);
        assert!((sd_squircle((0.0, 40.0), b, 14.0)).abs() < 1e-4);
        assert!((sd_squircle((100.0, 0.0), b, 14.0)).abs() < 1e-4);
        assert!(sd_squircle((0.0, 0.0), b, 14.0) < -39.0);
        // A corner is cut: the box's own corner point lies outside.
        assert!(sd_squircle((100.0, 40.0), b, 14.0) > 1.0);
    }
}
